//! The diagnostic figures (issue #169): the bucket function and the stamp arithmetic with
//! injected nanoseconds, the per-strip counts, and the lateness and wake histograms through a
//! set with no worker, the test thread claiming and completing a strip by hand with an injected
//! stamp. Nothing here depends on scheduling or on a real clock reading: every stamp a histogram
//! reads is injected or is the publish stamp the set itself took, and every deadline has passed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::figures::{BUCKETS, LIMITS_NS, bucket, past};
use super::tests::{Fake, STALL, set_of};
use super::*;
use crate::testing::measure;

const US: u64 = 1_000;

#[test]
fn buckets_are_closed_at_their_limit_and_open_past_the_last() {
    assert_eq!(BUCKETS, 7);
    assert_eq!(LIMITS_NS, [50 * US, 100 * US, 200 * US, 500 * US, 1_000 * US, 2_000 * US]);
    let cases = [
        (0, 0),
        (50 * US, 0),
        (50 * US + 1, 1),
        (100 * US, 1),
        (150 * US, 2),
        (200 * US, 2),
        (200 * US + 1, 3),
        (500 * US, 3),
        (999 * US, 4),
        (1_000 * US, 4),
        (2_000 * US, 5),
        (2_000 * US + 1, 6),
        (u64::MAX, 6),
    ];
    for (ns, b) in cases {
        assert_eq!(bucket(ns), b, "{ns} ns");
    }
    assert_eq!(past(1_150 * US, 1_000 * US), 150 * US);
    assert_eq!(past(1_000 * US, 1_150 * US), 0, "a stamp before the start is not past it");
}

#[test]
fn a_histogram_counts_each_duration_in_its_bucket() {
    let h = figures::Histogram::default();
    for ns in [0, 40 * US, 60 * US, 3_000 * US, 3_000 * US, 2_000 * US] {
        h.record(ns);
    }
    assert_eq!(h.counts(), [2, 1, 0, 0, 0, 1, 2]);
}

#[test]
fn the_top_strips_are_those_with_the_most_overruns_most_first() {
    let f = figures::Figures::new(5);
    let add = |strip, what, n| {
        for _ in 0..n {
            f.count(strip, what);
        }
    };
    add(0, Count::Busy, 1);
    add(1, Count::Late, 4);
    add(2, Count::Inline, 9);
    add(3, Count::Unstarted, 2);
    add(3, Count::Late, 2);
    add(4, Count::Busy, 1);
    let top = f.top_overruns::<3>();
    let ids: Vec<_> = top.iter().map(|t| t.map(|(i, s)| (i, s.overruns()))).collect();
    assert_eq!(ids, vec![Some((1, 4)), Some((3, 4)), Some((0, 1))], "a tie keeps the lower index first");
    assert_eq!(f.strip(2).unwrap().inline_renders, 9);
    let all = f.top_overruns::<8>();
    assert_eq!(all.iter().flatten().count(), 4, "a strip with no overrun is not listed");
    assert_eq!(f.top_overruns::<0>(), []);
    assert_eq!(f.strip(5), None);
}

/// A late result's lateness, and the wake latency, from injected stamps: the set has no worker;
/// the test thread claims strip 0 by hand (as a worker would), stamps it, and completes it after
/// the deadline. The whole sequence allocates nothing.
#[test]
fn a_late_result_lands_in_the_bucket_of_its_completion_stamp_past_the_deadline() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    let shared = Arc::clone(&set.shared);
    let figures = set.figures();
    let ((), stats) = measure(|| {
        assert!(set.begin_block(1));
        assert_eq!(set.publish(0), Ok(Published::Published));
        assert_eq!(set.publish(1), Ok(Published::Published));
        let published = set.publish_ns;
        assert_ne!(published, 0, "the block's publish stamp");
        // The worker's side: claim, then the claim stamp 75 µs after the publish stamp.
        let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
        shared.figures.claimed(0, published + 75 * US);
        assert_eq!(figures.wake(), [0, 1, 0, 0, 0, 0, 0]);
        // A deadline already passed: strip 0 is `Late` (claimed), strip 1 `Unstarted`.
        let deadline = Instant::now();
        let deadline_ns = figures.nanos(deadline).max(1);
        let _ = set.finish(deadline, |_| panic!("nothing has a history"));
        assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Late)));
        assert_eq!(set.outcome(1), Some(Outcome::Overrun(OverrunCause::Unstarted)));
        // The completion stamp 150 µs past the deadline, before the Release complete.
        shared.figures.done(0, deadline_ns + 150 * US);
        token.complete();
        assert_eq!(figures.lateness(), [0; BUCKETS], "recorded only when the result is retired");
        assert!(set.begin_block(2));
        assert_eq!(figures.lateness(), [0, 0, 1, 0, 0, 0, 0], "150 µs late: the 200 µs bucket");
    });
    assert!(stats.is_zero(), "the stamps and the histograms: {stats:?}");
    let s0 = figures.strip(0).unwrap();
    let s1 = figures.strip(1).unwrap();
    assert_eq!((s0.overruns_late, s0.overruns_busy, s0.overruns_unstarted), (1, 0, 0));
    assert_eq!((s1.overruns_late, s1.overruns_busy, s1.overruns_unstarted), (0, 0, 1));
    assert_eq!(set.stats().late_results, 1);
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// A result held across blocks: late at block 1, the strip `Busy` at block 2 (counted for the
/// strip), retired at block 3 three milliseconds past block 1's deadline (the open bucket). A
/// result of a block `finish` was never called for is retired without a lateness.
#[test]
fn a_result_late_by_several_blocks_is_measured_from_the_deadline_it_missed() {
    let fake = Fake::new(1, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    let shared = Arc::clone(&set.shared);
    let figures = set.figures();
    assert!(set.begin_block(1));
    assert_eq!(set.publish(0), Ok(Published::Published));
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    let deadline = Instant::now();
    let missed = figures.nanos(deadline).max(1);
    let _ = set.finish(deadline, |_| panic!("claimed"));
    assert!(set.begin_block(2));
    assert!(set.is_busy(0));
    assert_eq!(set.publish(0), Ok(Published::Skipped));
    let _ = set.finish(Instant::now(), |_| panic!("skipped"));
    shared.figures.done(0, missed + 3_000 * US);
    token.complete();
    assert!(set.begin_block(3));
    assert_eq!(figures.lateness(), [0, 0, 0, 0, 0, 0, 1]);
    let s = figures.strip(0).unwrap();
    assert_eq!((s.overruns_late, s.overruns_busy), (1, 1));

    // Block 3 is never finished: its worker's result is retired at block 4 with no lateness.
    assert_eq!(set.publish(0), Ok(Published::Published));
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    shared.figures.done(0, u64::MAX);
    token.complete();
    assert!(set.begin_block(4));
    assert_eq!(figures.lateness(), [0, 0, 0, 0, 0, 0, 1], "no deadline, no lateness");
    assert_eq!(set.stats().late_results, 2);
    assert!(set.shutdown(STALL).is_empty());
}

/// The per-strip counts add up to the set's totals: busy, late, unstarted and inline. Every
/// outcome holds by construction: no worker; a far deadline only where an inline render or the
/// closure's completion of the hand-claimed strip ends the call, a passed one elsewhere.
#[test]
fn the_per_strip_counts_add_up_to_the_totals() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    let shared = Arc::clone(&set.shared);
    set.set_cost_estimate(1, Duration::from_nanos(1));
    // Block 1: strip 1 inline; strip 0 claimed by hand and completed from the inline closure.
    assert!(set.begin_block(1));
    for s in 0..2 {
        assert_eq!(set.publish(s), Ok(Published::Published));
    }
    let mut token = shared.jobs.slot(0).unwrap().try_claim();
    let _ = set.finish(Instant::now() + STALL, |s| {
        fake.execute(s, 1, false);
        token.take().unwrap().complete();
    });
    assert_eq!((set.outcome(0), set.outcome(1)), (Some(Outcome::Mixed), Some(Outcome::Inline)));
    // Block 2, deadline passed: strip 0 claimed by hand (`Late`), strip 1 not started.
    assert!(set.begin_block(2));
    for s in 0..2 {
        assert_eq!(set.publish(s), Ok(Published::Published));
    }
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    let _ = set.finish(Instant::now(), |_| panic!("no time left"));
    assert_eq!(set.outcome(0), Some(Outcome::Overrun(OverrunCause::Late)));
    assert_eq!(set.outcome(1), Some(Outcome::Overrun(OverrunCause::Unstarted)));
    // Block 3: strip 0 still held (`Busy`).
    assert!(set.begin_block(3));
    assert_eq!(set.publish(0), Ok(Published::Skipped));
    let _ = set.finish(Instant::now(), |_| panic!("nothing published"));
    token.complete();
    assert!(set.begin_block(4));
    let st = set.stats();
    let f = set.figures();
    let sum = |pick: fn(&figures::StripFigures) -> u64| (0..2).map(|s| pick(&f.strip(s).unwrap())).sum::<u64>();
    assert_eq!(sum(|s| s.overruns_late), st.overruns_late);
    assert_eq!(sum(|s| s.overruns_busy), st.overruns_busy);
    assert_eq!(sum(|s| s.overruns_unstarted), st.overruns_unstarted);
    assert_eq!(sum(|s| s.inline_renders), st.inline_renders);
    assert_eq!((st.overruns_late, st.overruns_busy, st.overruns_unstarted, st.inline_renders), (1, 1, 1, 1));
    let (s0, s1) = (f.strip(0).unwrap(), f.strip(1).unwrap());
    assert_eq!((s0.overruns_late, s0.overruns_busy, s1.overruns_unstarted, s1.inline_renders), (1, 1, 1, 1));
    assert_eq!(Some(Duration::from_nanos(s1.worst_ns)), set.worst_case(1), "the cost figures, exposed");
    assert_eq!(Some(Duration::from_nanos(s1.cost_ns)), set.cost_estimate(1));
    assert_eq!(f.lateness().iter().sum::<u64>(), 1, "the late result, retired at block 4");
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// `reset_counts` zeroes a strip's counts and nothing else: not another strip's, not its cost
/// history, not the histograms.
#[test]
fn reset_counts_zeroes_one_strips_counts_only() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    set.set_cost_estimate(0, Duration::from_micros(7));
    let f = set.figures();
    for s in 0..2 {
        for what in [Count::Busy, Count::Late, Count::Unstarted, Count::Inline] {
            f.count(s, what);
        }
    }
    f.retired_late(1, 1);
    set.reset_counts(0);
    let s0 = f.strip(0).unwrap();
    assert_eq!((s0.overruns_busy, s0.overruns_late, s0.overruns_unstarted, s0.inline_renders), (0, 0, 0, 0));
    assert_eq!((s0.cost_ns, s0.worst_ns), (7_000, 7_000), "the cost history is the chain's, not the name's");
    assert_eq!(f.strip(1).unwrap().overruns(), 3, "another strip keeps its counts");
    assert_eq!(f.lateness().iter().sum::<u64>(), 1, "the histograms are the set's");
    set.reset_counts(9);
    assert!(set.shutdown(STALL).is_empty());
}
