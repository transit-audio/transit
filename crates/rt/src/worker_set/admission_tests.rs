//! The inline admission rule (issue #168, ADR 0029 deviation 16): no inline render without a
//! cost history, admission by the worst case rather than the EWMA, the overrun bar, and a
//! history that belongs to the instance a slot holds (cleared when it is replaced).
//!
//! Every assertion is about the rule, never about a race: the rule is checked directly with a
//! given time left, or through `finish` in a configuration whose outcome holds by construction
//! (no worker, or the one worker stalled inside another strip; a deadline already passed, or one
//! so far away that only an admitted render can end the call).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{Behavior, Fake, STALL, publish_all, set_of, wait_done, wait_until};
use super::*;
use crate::strip_jobs::JobSlot;

const MS: Duration = Duration::from_millis(1);

/// A strip never rendered is refused whatever the time left, and stays a candidate for the
/// workers until the deadline; with no worker it is taken back unrendered (`Unstarted`). The
/// refusal is counted once per strip per block.
#[test]
fn a_strip_with_no_history_is_never_rendered_inline() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    assert_eq!(set.worst_case(0), Some(Duration::ZERO), "no history after construction");
    assert_eq!(set.admission(0, Duration::from_secs(3600)), Err(Some(InlineRefusal::NoHistory)));
    assert_eq!(set.admission(0, Duration::ZERO), Err(Some(InlineRefusal::NoHistory)), "checked before the time left");
    assert!(set.begin_block(1));
    publish_all(&mut set);
    // Time to spare: under the old rule (estimate 0 fits anything) every strip ran inline here.
    // The deadline only bounds the spin of the refused strips; the outcomes hold for any
    // scheduling.
    let r = set.finish(Instant::now() + 20 * MS, |s| panic!("strip {s} has no history; rendered inline"));
    assert_eq!((r.inline, r.overrun, r.inline_refused), (0, 4, 4), "{r:?}");
    for s in 0..4 {
        assert_eq!(set.outcome(s), Some(Outcome::Overrun(OverrunCause::Unstarted)));
        assert_eq!(fake.strips[s].last_gen.load(Ordering::Acquire), 0, "never executed");
    }
    let st = set.stats();
    assert_eq!((st.inline_refused_no_history, st.inline_refused_worst_case, st.inline_refused_recent_overrun), (4, 0, 0));
    assert_eq!(st.overruns_unstarted, 4);
    assert!(!set.inline_barred(0), "an unstarted overrun does not bar the strip");
    assert!(set.shutdown(STALL).is_empty());
}

/// Once a worker has rendered a strip it has a history, and the callback may render it inline:
/// with the one worker stalled inside another strip, every other strip is rendered inline.
#[test]
fn a_strip_a_worker_has_rendered_may_be_rendered_inline() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut set = set_of(&fake, 1);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_done(&set, 4, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("the worker rendered everything"));
    assert_eq!(r.mixed, 4);
    for s in 0..4 {
        assert!(set.worst_case(s).unwrap() > Duration::ZERO, "strip {s} has a history");
        assert!(set.worst_case(s) >= set.cost_estimate(s), "the worst case is never below the EWMA");
    }
    // The worker is held inside strip 0 (published alone, so it cannot take another first).
    fake.set(0, Behavior::Stall);
    assert!(set.begin_block(2));
    assert_eq!(set.publish(0), Ok(Published::Published));
    set.wake(1);
    wait_until("the worker on strip 0", || fake.stalled.load(Ordering::Acquire) == 1);
    for s in 1..4 {
        assert_eq!(set.publish(s), Ok(Published::Published));
    }
    let mut inline = Vec::with_capacity(4);
    // A far deadline: only the inline renders and the worker can end the call, so the last
    // inline render releases strip 0.
    let r = set.finish(Instant::now() + STALL, |s| {
        fake.execute(s, 2, false);
        inline.push(s);
        if inline.len() == 3 {
            fake.release(0);
        }
    });
    assert_eq!(inline, vec![1, 2, 3], "every strip with a history was rendered inline");
    assert_eq!((r.inline, r.mixed, r.inline_refused), (3, 1, 0), "{r:?}");
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Admission by the worst case: a strip whose EWMA fits in the time left but whose worst case
/// does not is refused, and one whose worst case fits is admitted.
#[test]
fn the_worst_case_not_the_average_decides_admission() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    set.set_cost_estimate(0, 100 * MS);
    assert_eq!((set.cost_estimate(0), set.worst_case(0)), (Some(100 * MS), Some(100 * MS)), "a seed sets both");
    // One spike of 40 ms among renders of 0.1 ms (the gate 6 Super 8 case, scaled).
    set.set_cost_estimate(0, Duration::from_micros(100));
    set.shared.record_cost(0, 40 * MS);
    for _ in 0..8 {
        set.shared.record_cost(0, Duration::from_micros(100));
    }
    let (ewma, worst) = (set.cost_estimate(0).unwrap(), set.worst_case(0).unwrap());
    // timing: value the figures are computed from the durations fed above, not measured
    assert!(ewma < 5 * MS, "the EWMA has mostly forgotten the spike: {ewma:?}");
    assert!(worst > 30 * MS, "the worst case remembers it: {worst:?}");
    let left = 10 * MS;
    assert!(ewma <= left, "the old rule (EWMA) would have admitted it");
    assert_eq!(set.admission(0, left), Err(Some(InlineRefusal::WorstCase)));
    assert_eq!(set.admission(0, worst), Ok(()), "admitted when the worst case fits exactly");
    assert_eq!(set.admission(0, Duration::ZERO), Err(None), "no time left is the deadline, not a refusal");

    // Through `finish`: no worker, so the strip is never claimed; whatever the scheduling, it is
    // not rendered inline (refused, or the deadline passed first) and is taken back unrendered.
    assert!(set.begin_block(1));
    assert_eq!(set.publish(0), Ok(Published::Published));
    let r = set.finish(Instant::now() + left, |s| panic!("strip {s}'s worst case does not fit; rendered inline"));
    assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Unstarted)), "{r:?}");
    assert!(set.stats().inline_refused_worst_case <= 1);
    assert_eq!(set.stats().inline_refused_no_history, 0);
    assert!(set.shutdown(STALL).is_empty());
}

/// The worst case decays toward the EWMA, never below it, and a longer render resets it.
#[test]
fn the_worst_case_decays_toward_the_ewma_and_a_new_spike_resets_it() {
    let fake = Fake::new(1, Duration::ZERO);
    let set = set_of(&fake, 0);
    let small = Duration::from_micros(100);
    set.shared.record_cost(0, small);
    set.shared.record_cost(0, 40 * MS);
    let mut last = set.worst_case(0).unwrap();
    assert_eq!(last, 40 * MS);
    let mut renders = 0u32;
    while last > MS {
        set.shared.record_cost(0, small);
        let w = set.worst_case(0).unwrap();
        assert!(w < last, "it decays on every render while above the EWMA");
        assert!(w >= set.cost_estimate(0).unwrap(), "never below the EWMA");
        last = w;
        renders += 1;
        assert!(renders < 10_000, "it reaches the EWMA's neighborhood");
    }
    // About 240 renders with WORST_DECAY 64 (the module docs' figure).
    assert!((150..=350).contains(&renders), "{renders} renders from 40 ms to 1 ms");
    for _ in 0..2000 {
        set.shared.record_cost(0, small);
    }
    assert_eq!(set.worst_case(0), set.cost_estimate(0), "settled on the EWMA");
    set.shared.record_cost(0, 7 * MS);
    assert_eq!(set.worst_case(0), Some(7 * MS), "a longer render replaces it at once");
}

/// The overrun bar: a strip that overran (`Late` here) is refused inline for the next
/// `OVERRUN_BAR_BLOCKS` blocks, then admitted again. The one worker is held inside another
/// strip throughout, so nothing but the callback can render the barred strip.
#[test]
fn a_strip_that_overran_is_refused_inline_for_the_bar_then_admitted() {
    let fake = Fake::new(3, Duration::ZERO);
    let mut set = set_of(&fake, 1);
    // Block 1: every strip rendered by the worker (a history for each).
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_done(&set, 3, 0);
    let _ = set.finish(Instant::now() + STALL, |_| panic!("the worker rendered everything"));
    // Block 2: strip 0 stalls on the worker past the deadline: `Late`.
    fake.set(0, Behavior::Stall);
    assert!(set.begin_block(2));
    assert_eq!(set.publish(0), Ok(Published::Published));
    set.wake(1);
    wait_until("the worker on strip 0", || fake.stalled.load(Ordering::Acquire) == 1);
    let _ = set.finish(Instant::now(), |_| panic!("nothing is unstarted"));
    assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Late)));
    assert!(set.inline_barred(0));
    fake.release(0);
    wait_until("the late result", || set.jobs().slot(0).map(JobSlot::state) == Some(State::Done));
    // Its history is cheap again (the stall itself was recorded as its cost): only the bar can
    // refuse it below.
    set.set_cost_estimate(0, Duration::from_micros(1));
    // Block 3: the worker is held inside strip 1 for the rest of the test.
    fake.set(1, Behavior::Stall);
    assert!(set.begin_block(3));
    assert!(!set.is_busy(0), "its late result was retired");
    assert_eq!(set.publish(1), Ok(Published::Published));
    set.wake(1);
    wait_until("the worker on strip 1", || fake.stalled.load(Ordering::Acquire) == 1);
    let mut g = 3;
    loop {
        if g > 3 {
            assert!(set.begin_block(g));
            assert_eq!(set.publish(1), Ok(Published::Skipped), "the worker still holds strip 1");
        }
        assert_eq!(set.publish(0), Ok(Published::Published));
        if !set.inline_barred(0) {
            break;
        }
        // Barred: refused (counted) even with the deadline passed, since the bar is checked
        // first; taken back unrendered.
        assert_eq!(set.admission(0, Duration::from_secs(3600)), Err(Some(InlineRefusal::RecentOverrun)));
        let r = set.finish(Instant::now(), |s| panic!("strip {s} is barred; rendered inline"));
        assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Unstarted)), "block {g}: {r:?}");
        assert_eq!(r.inline_refused, 1, "block {g}");
        g += 1;
    }
    // Barred in blocks 3 to 2 + OVERRUN_BAR_BLOCKS (the overrun was in block 2), admitted in
    // the next.
    assert_eq!(g, 3 + OVERRUN_BAR_BLOCKS);
    assert_eq!(set.stats().inline_refused_recent_overrun, OVERRUN_BAR_BLOCKS);
    assert!(!set.inline_barred(0), "an unstarted overrun did not extend the bar");
    let gen_now = g;
    let r = set.finish(Instant::now() + STALL, |s| fake.execute(s, gen_now, false));
    assert_eq!(set.outcome(0), Some(Outcome::Inline), "{r:?}");
    assert_eq!(r.inline_refused, 0);
    fake.release(1);
    wait_until("the stalled worker", || set.jobs().counts().claimed == 0);
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// An inline render that ends past its deadline bars the strip too, and a `Busy` skip does.
#[test]
fn an_inline_render_past_its_deadline_and_a_busy_skip_bar_the_strip() {
    let fake = Fake::new(1, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    set.set_cost_estimate(0, Duration::from_nanos(1));
    assert!(set.begin_block(1));
    assert_eq!(set.publish(0), Ok(Published::Published));
    // Time to spare admits it (as in the other admission tests); the render then sleeps past
    // the deadline, so it ends past it by construction.
    let deadline = Instant::now() + 300 * MS;
    let r = set.finish(deadline, |_| {
        // timing: stall-bound the render outlasts its own deadline by construction, nothing is measured
        while Instant::now() <= deadline + MS {
            std::thread::sleep(MS);
        }
    });
    assert_eq!(set.outcome(0), Some(Outcome::Inline), "{r:?}");
    assert!(set.inline_barred(0), "it ran past its deadline: barred");
    assert!(set.worst_case(0).unwrap() >= 300 * MS, "and its worst case grew to that render");
    assert!(set.shutdown(STALL).is_empty());

    // Busy: the one worker holds strip 0 from block 1 into block 2.
    let fake = Fake::new(1, Duration::ZERO);
    fake.set(0, Behavior::Stall);
    let mut set = set_of(&fake, 1);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_until("the worker on strip 0", || fake.stalled.load(Ordering::Acquire) == 1);
    let _ = set.finish(Instant::now(), |_| panic!("claimed"));
    assert!(set.begin_block(2));
    assert_eq!(set.publish(0), Ok(Published::Skipped));
    assert!(set.inline_barred(0), "skipped busy: barred");
    fake.release(0);
    wait_until("the stalled worker", || set.jobs().counts().claimed == 0);
    assert!(set.shutdown(STALL).is_empty());
}

/// The cost history belongs to the instance a slot holds: `clear_suspect` (the faulted instance
/// was replaced) clears it, so the replacement's first render is never inline; the other strip,
/// published again, keeps its own. Only the history of the replaced strip goes.
#[test]
fn clear_suspect_clears_the_replaced_instances_history() {
    let fake = Fake::new(2, Duration::ZERO);
    fake.set(1, Behavior::Panic);
    let mut set = set_of(&fake, 1);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_done(&set, 2, 0);
    let _ = set.finish(Instant::now() + STALL, |_| panic!("the worker rendered both"));
    assert_eq!(set.outcome(1), Some(Outcome::Faulted));
    assert!(set.worst_case(1).unwrap() > Duration::ZERO, "the faulted run was measured");
    let kept = (set.cost_estimate(0), set.worst_case(0));
    assert!(set.begin_block(2));
    assert!(set.is_suspect(1));
    set.clear_suspect(1);
    assert_eq!((set.cost_estimate(1), set.worst_case(1)), (Some(Duration::ZERO), Some(Duration::ZERO)), "the new instance has no history");
    assert_eq!(set.admission(1, Duration::from_secs(3600)), Err(Some(InlineRefusal::NoHistory)));
    assert_eq!((set.cost_estimate(0), set.worst_case(0)), kept, "the other strip keeps its own");
    // Strip 0 published again (and taken back at a passed deadline, or rendered by the worker):
    // its history is still there.
    assert_eq!(set.publish(0), Ok(Published::Published));
    let _ = set.finish(Instant::now(), |_| {});
    assert!(set.worst_case(0).unwrap() > Duration::ZERO, "a publish keeps the history");
    assert!(set.shutdown(STALL).is_empty());
    let fresh = set_of(&fake, 0);
    assert_eq!(fresh.worst_case(0), Some(Duration::ZERO), "a new set has none");
}

/// `reset_history` (the caller's signal that a slot holds another processing chain) clears the
/// cost figures and the overrun bar of that strip only.
#[test]
fn reset_history_clears_the_figures_and_the_bar() {
    let fake = Fake::new(2, Duration::ZERO);
    fake.set(0, Behavior::Stall);
    let mut set = set_of(&fake, 1);
    set.set_cost_estimate(1, Duration::from_micros(5));
    assert!(set.begin_block(1));
    assert_eq!(set.publish(0), Ok(Published::Published));
    set.wake(1);
    wait_until("the worker on strip 0", || fake.stalled.load(Ordering::Acquire) == 1);
    let _ = set.finish(Instant::now(), |_| panic!("claimed"));
    assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Late)));
    fake.release(0);
    wait_until("the late result", || set.jobs().slot(0).map(JobSlot::state) == Some(State::Done));
    assert!(set.begin_block(2));
    assert!(set.inline_barred(0) && set.worst_case(0).unwrap() > Duration::ZERO);
    set.reset_history(0);
    assert!(!set.inline_barred(0), "the bar is cleared");
    assert_eq!(set.worst_case(0), Some(Duration::ZERO), "and the history");
    assert_eq!(set.admission(0, Duration::from_secs(3600)), Err(Some(InlineRefusal::NoHistory)));
    assert_eq!(set.worst_case(1), Some(Duration::from_micros(5)), "strip 1 untouched");
    assert!(set.shutdown(STALL).is_empty());
}
