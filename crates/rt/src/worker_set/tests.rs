//! The worker set with real threads: the deadline policy, admission, runner faults, retirement
//! and quarantine, promotion, and the allocation proof. A fake runner gives each strip a
//! behavior (normal, slow, stalled until released, panicking) and a reentry guard.
//!
//! Timing: the tests assert states, outcomes and counters. Wall-clock bounds appear only as
//! deadlines given to `finish`/`shutdown` and as stall bounds far above scheduling jitter, each
//! marked with its reason.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::strip_jobs::JobSlot;
use crate::testing::{AllocStats, count_this_thread, measure, thread_stats};
use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicU32};

/// A stall bound, not a latency claim: far above any scheduling jitter, it only turns a hang (a
/// lost wake-up, a slot stuck claimed) into a failure instead of a stuck test run.
pub(super) const STALL: Duration = Duration::from_secs(60);

/// How far past its deadline `finish` (or past its bound `shutdown`) may return. The overshoot
/// by design is one sweep of the slots (finish) or one 1 ms sleep (shutdown); on a loaded runner
/// the test thread (not promoted) can still be preempted for several scheduler quanta (about
/// 15 ms each on Windows). 200 ms is over ten quanta: a failure means the call waited for a
/// worker, not jitter.
pub(super) const SLACK: Duration = Duration::from_millis(200);

/// A worker-side behavior of the fake runner, per strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Behavior {
    /// Spin `spins[strip]` times.
    Normal = 0,
    /// Spin for longer than a block (`Fake::slow`).
    Slow = 1,
    /// Block until `Fake::release(strip)`.
    Stall = 2,
    /// Panic.
    Panic = 3,
}

/// Payload words per strip, standing in for its buffers (relaxed; only the slots order them).
pub(super) const WORDS: usize = 8;
/// What the executor adds to the generation when it writes the payload back.
pub(super) const MARK: u64 = 1 << 40;

/// One strip of the fake.
pub(super) struct FakeStrip {
    behavior: AtomicU8,
    pub(super) spins: AtomicU32,
    held: AtomicBool,
    /// Panic when the stall is released.
    panic_after_stall: AtomicBool,
    inside: AtomicBool,
    /// The last generation any thread executed (entered) for this strip.
    pub(super) last_gen: AtomicU64,
    pub(super) payload: [AtomicU64; WORDS],
}

/// The fake strip runner.
pub(super) struct Fake {
    pub(super) strips: Box<[FakeStrip]>,
    /// Strips currently stalled inside the runner.
    pub(super) stalled: AtomicUsize,
    /// How long `Slow` spins.
    slow: Duration,
    /// Check and write the payload on every execution.
    payload: AtomicBool,
    /// Count allocations on the worker threads between runner calls.
    count_allocs: AtomicBool,
    /// Heap operations seen on worker threads between two runner calls while counting.
    pub(super) worker_heap_ops: AtomicU64,
}

std::thread_local! {
    /// The worker thread's heap operations at its last runner call while counting.
    static LAST_OPS: Cell<Option<u64>> = const { Cell::new(None) };
}

fn ops(s: AllocStats) -> u64 {
    s.allocs + s.frees
}

/// Leaves the reentry guard also when the runner unwinds.
struct Inside<'a>(&'a AtomicBool);

impl Drop for Inside<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Fake {
    pub(super) fn new(strips: usize, slow: Duration) -> Arc<Self> {
        Arc::new(Self {
            strips: (0..strips)
                .map(|_| FakeStrip {
                    behavior: AtomicU8::new(Behavior::Normal as u8),
                    spins: AtomicU32::new(64),
                    held: AtomicBool::new(false),
                    panic_after_stall: AtomicBool::new(false),
                    inside: AtomicBool::new(false),
                    last_gen: AtomicU64::new(0),
                    payload: std::array::from_fn(|_| AtomicU64::new(0)),
                })
                .collect(),
            stalled: AtomicUsize::new(0),
            slow,
            payload: AtomicBool::new(false),
            count_allocs: AtomicBool::new(false),
            worker_heap_ops: AtomicU64::new(0),
        })
    }

    pub(super) fn set(&self, strip: usize, b: Behavior) {
        if b == Behavior::Stall {
            self.strips[strip].held.store(true, Ordering::Release);
        }
        self.strips[strip].behavior.store(b as u8, Ordering::Release);
    }

    /// Releases a stalled strip, which then panics in the same run.
    pub(super) fn release_then_panic(&self, strip: usize) {
        self.strips[strip].panic_after_stall.store(true, Ordering::Release);
        self.release(strip);
    }

    pub(super) fn release(&self, strip: usize) {
        self.strips[strip].behavior.store(Behavior::Normal as u8, Ordering::Release);
        self.strips[strip].held.store(false, Ordering::Release);
    }

    pub(super) fn check_payload(&self, on: bool) {
        self.payload.store(on, Ordering::Release);
    }

    /// Publisher side of the payload: the strip's inputs for block `g`.
    pub(super) fn write_inputs(&self, strip: usize, g: u64) {
        for w in &self.strips[strip].payload {
            w.store(g, Ordering::Relaxed);
        }
    }

    /// Publisher side, after `Mixed` or `Inline`: the executor's output for block `g`.
    pub(super) fn assert_output(&self, strip: usize, g: u64) {
        for (i, w) in self.strips[strip].payload.iter().enumerate() {
            assert_eq!(w.load(Ordering::Relaxed), g + MARK, "strip {strip} word {i}: the publisher sees the executor's output");
        }
    }

    /// One execution of `strip` for `g`, by a worker (`worker`: its behavior applies) or inline.
    pub(super) fn execute(&self, strip: usize, g: u64, worker: bool) {
        let s = &self.strips[strip];
        assert!(!s.inside.swap(true, Ordering::AcqRel), "two threads inside strip {strip}");
        let _inside = Inside(&s.inside);
        let last = s.last_gen.swap(g, Ordering::AcqRel);
        assert!(last < g, "strip {strip}: generation {g} executed after {last}");
        let payload = self.payload.load(Ordering::Acquire);
        if payload {
            for (i, w) in s.payload.iter().enumerate() {
                assert_eq!(w.load(Ordering::Relaxed), g, "strip {strip} word {i}: the executor sees the publisher's inputs");
            }
        }
        let behavior = if worker { s.behavior.load(Ordering::Acquire) } else { Behavior::Normal as u8 };
        match behavior {
            1 => spin_for(self.slow),
            2 => {
                self.stalled.fetch_add(1, Ordering::AcqRel);
                let t0 = Instant::now();
                while s.held.load(Ordering::Acquire) {
                    assert!(t0.elapsed() < STALL, "strip {strip} never released");
                    // timing: sleep-ok paces a wait for a release flag, measures nothing
                    std::thread::sleep(Duration::from_millis(1));
                }
                self.stalled.fetch_sub(1, Ordering::AcqRel);
                let panic_now = s.panic_after_stall.swap(false, Ordering::AcqRel);
                assert!(!panic_now, "fake runner: strip {strip} panics after its stall on purpose");
            }
            3 => panic!("fake runner: strip {strip} panics on purpose"),
            _ => {
                for _ in 0..s.spins.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            }
        }
        if payload {
            for w in &s.payload {
                w.store(g + MARK, Ordering::Relaxed);
            }
        }
    }
}

/// Busy work for `d` (a workload, not a bound).
pub(super) fn spin_for(d: Duration) {
    let t0 = Instant::now();
    loop {
        if t0.elapsed() >= d {
            break;
        }
        std::hint::spin_loop();
    }
}

impl StripRunner for Fake {
    fn run(&self, strip: usize, generation: u64) {
        if self.count_allocs.load(Ordering::Acquire) {
            count_this_thread(true);
            let now = ops(thread_stats());
            if let Some(before) = LAST_OPS.get() {
                self.worker_heap_ops.fetch_add(now - before, Ordering::AcqRel);
            }
            LAST_OPS.set(Some(now));
        }
        self.execute(strip, generation, true);
    }
}

pub(super) fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < STALL, "stalled waiting for {what}");
        std::thread::yield_now();
    }
}

pub(super) fn set_of(fake: &Arc<Fake>, workers: usize) -> WorkerSet {
    let strips = fake.strips.len();
    WorkerSet::new(Config { workers, strips, promote: false }, Arc::clone(fake) as Arc<dyn StripRunner>).expect("spawn workers")
}

pub(super) fn publish_all(set: &mut WorkerSet) {
    for s in 0..set.strips() {
        assert_eq!(set.publish(s), Ok(Published::Published));
    }
}

/// Every published strip done by the workers before `finish` (so `finish` only consumes).
pub(super) fn wait_done(set: &WorkerSet, done: usize, claimed: usize) {
    wait_until("the workers", || {
        let c = set.jobs().counts();
        c.done == done && c.claimed == claimed
    });
}

#[test]
fn workers_render_published_strips_and_finish_consumes_them_once() {
    let fake = Fake::new(8, Duration::ZERO);
    let mut set = set_of(&fake, 3);
    for g in 1..=20 {
        assert!(set.begin_block(g));
        publish_all(&mut set);
        // A worker whose last wake it has not yet taken is not woken again (it will sweep).
        assert!(set.wake(3) <= 3);
        if g.is_multiple_of(2) {
            wait_done(&set, 8, 0);
        }
        let r = set.finish(Instant::now() + STALL, |s| fake.execute(s, g, false));
        assert_eq!(r.mixed + r.inline, 8, "block {g}: {r:?}");
        assert_eq!(r.overrun + r.faulted, 0);
        if g.is_multiple_of(2) {
            assert_eq!(r.mixed, 8, "everything was done before finish");
        }
        for s in 0..8 {
            assert!(matches!(set.outcome(s), Some(Outcome::Mixed | Outcome::Inline)));
            assert_eq!(fake.strips[s].last_gen.load(Ordering::Acquire), g);
        }
        assert_eq!(set.jobs().counts().idle, 8, "every slot consumed or retired");
    }
    let st = set.stats();
    assert_eq!((st.blocks, st.mixed + st.inline_renders, st.overruns), (20, 160, 0));
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Gate 3: one stalled strip among eight. `finish` returns at the deadline (not when the stall
/// ends), the stalled strip is an overrun, the next block skips it, and after the release its
/// late result is retired and the strip resumes.
#[test]
fn a_stalled_strip_cannot_hold_finish_past_its_deadline() {
    let fake = Fake::new(8, Duration::ZERO);
    fake.set(5, Behavior::Stall);
    let mut set = set_of(&fake, 3);

    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(3);
    wait_done(&set, 7, 1);
    // Claimed is not yet stalled: wait for the worker to enter the stall.
    wait_until("the worker on strip 5", || fake.stalled.load(Ordering::Acquire) == 1);
    let deadline = Instant::now() + Duration::from_millis(50);
    let r = set.finish(deadline, |_| panic!("nothing is left to render inline"));
    let returned = Instant::now();
    assert!(returned >= deadline, "finish waited for the claimed strip until the deadline");
    // timing: stall-bound SLACK is over ten scheduler quanta; see its definition
    assert!(returned - deadline < SLACK, "finish returned {:?} past its deadline", returned - deadline);
    assert_eq!(set.outcome(5), Some(Outcome::Overrun(OverrunCause::Late)));
    assert_eq!(r, BlockReport { mixed: 7, inline: 0, overrun: 1, faulted: 0, inline_refused: 0 });
    assert_eq!(set.jobs().slot(5).map(JobSlot::state), Some(State::Claimed), "a claimed slot is left alone");

    // The next block skips it: a second overrun, the others render.
    assert!(set.begin_block(2));
    assert!(set.is_busy(5));
    for s in 0..8 {
        let p = set.publish(s).expect("publish");
        assert_eq!(p == Published::Skipped, s == 5);
    }
    set.wake(3);
    wait_done(&set, 7, 1);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r, BlockReport { mixed: 7, inline: 0, overrun: 1, faulted: 0, inline_refused: 0 });
    assert_eq!(set.outcome(5), Some(Outcome::Overrun(OverrunCause::Busy)));
    let st = set.stats();
    assert_eq!((st.overruns, st.overruns_late, st.overruns_busy), (2, 1, 1));

    // Released: its block-1 result is late, retired unread at the next begin; the strip resumes.
    fake.release(5);
    wait_until("the late result", || set.jobs().slot(5).map(JobSlot::state) == Some(State::Done));
    assert_eq!(fake.strips[5].last_gen.load(Ordering::Acquire), 1, "the late worker ran block 1");
    assert!(set.begin_block(3));
    assert!(!set.is_busy(5));
    assert_eq!(set.stats().late_results, 1);
    publish_all(&mut set);
    set.wake(3);
    wait_done(&set, 8, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r, BlockReport { mixed: 8, inline: 0, overrun: 0, faulted: 0, inline_refused: 0 });
    assert_eq!(fake.strips[5].last_gen.load(Ordering::Acquire), 3);
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Gate 3, admission: every worker stalled, eight unstarted strips of known cost. `finish`
/// renders inline only what fits before the deadline, overruns the rest, and returns no later
/// than one strip's cost past the deadline.
#[test]
fn admission_renders_inline_only_what_fits_before_the_deadline() {
    const COST: Duration = Duration::from_millis(20);
    const BUDGET: Duration = Duration::from_millis(70);
    let fake = Fake::new(10, Duration::ZERO);
    for s in 0..10 {
        fake.set(s, Behavior::Stall);
    }
    let mut set = set_of(&fake, 2);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(2);
    wait_until("both workers stalled", || fake.stalled.load(Ordering::Acquire) == 2);
    let unstarted: Vec<usize> = (0..10).filter(|&s| set.jobs().slot(s).map(JobSlot::state) == Some(State::Published)).collect();
    assert_eq!(unstarted.len(), 8);
    for &s in &unstarted {
        set.set_cost_estimate(s, COST);
    }
    // One strip that can never fit.
    let huge = unstarted[0];
    set.set_cost_estimate(huge, STALL);

    let mut rendered = Vec::with_capacity(8);
    let deadline = Instant::now() + BUDGET;
    let r = set.finish(deadline, |s| {
        rendered.push(s);
        spin_for(COST);
    });
    let returned = Instant::now();
    // Each admitted render starts with at least its estimate left and takes at least COST, and
    // estimates only grow from COST here: at most BUDGET / COST renders fit.
    assert!(rendered.len() <= 3, "{rendered:?}");
    assert!(!rendered.contains(&huge), "a strip that cannot fit is never started");
    assert_eq!(r.inline, rendered.len());
    assert_eq!(r.overrun, 10 - rendered.len());
    // At most one admitted render runs past the deadline, by no more than its own cost.
    // timing: stall-bound one strip's cost plus SLACK, over ten scheduler quanta
    assert!(returned.saturating_duration_since(deadline) < COST + SLACK, "overshoot {:?}", returned - deadline);
    for s in 0..10 {
        let o = set.outcome(s).unwrap();
        if rendered.contains(&s) {
            assert_eq!(o, Outcome::Inline);
            assert!(set.cost_estimate(s).unwrap() >= COST, "the inline render updated the estimate");
        } else if unstarted.contains(&s) {
            assert_eq!(o, Outcome::Overrun(OverrunCause::Unstarted));
            assert_eq!(set.jobs().slot(s).map(JobSlot::state), Some(State::Idle), "taken back unrendered");
            assert_eq!(fake.strips[s].last_gen.load(Ordering::Acquire), 0, "never executed");
        } else {
            assert_eq!(o, Outcome::Overrun(OverrunCause::Late));
        }
    }
    let st = set.stats();
    assert_eq!(st.overruns_unstarted, 8 - rendered.len() as u64);
    assert_eq!(st.overruns_late, 2);
    for s in 0..10 {
        fake.release(s);
    }
    wait_until("the stalled workers", || set.jobs().counts().claimed == 0);
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// The other side of admission: with time to spare, the callback renders every unstarted strip
/// that has a cost history inline.
#[test]
fn admission_renders_every_unstarted_strip_when_time_allows() {
    let fake = Fake::new(10, Duration::ZERO);
    for s in 0..10 {
        fake.set(s, Behavior::Stall);
    }
    let mut set = set_of(&fake, 2);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(2);
    wait_until("both workers stalled", || fake.stalled.load(Ordering::Acquire) == 2);
    let r = set.finish(Instant::now(), |_| {});
    assert_eq!((r.inline, r.overrun), (0, 10), "no time left: nothing admitted, nothing waited for");
    assert_eq!(r.inline_refused, 8, "the eight unstarted strips have no history: refused, whatever the time");
    assert_eq!(set.stats().inline_refused_no_history, 8);
    for s in 0..10 {
        fake.release(s);
    }
    wait_until("the stalled workers", || set.jobs().counts().claimed == 0);
    assert!(set.shutdown(STALL).is_empty());

    // A new set (the two strips that were late above are barred for a few blocks): every strip
    // seeded with a small cost history.
    let mut set = set_of(&fake, 2);
    for s in 0..10 {
        fake.set(s, Behavior::Stall);
        set.set_cost_estimate(s, Duration::from_micros(1));
    }
    assert!(set.begin_block(2));
    publish_all(&mut set);
    set.wake(2);
    wait_until("both workers stalled", || fake.stalled.load(Ordering::Acquire) == 2);
    let deadline = Instant::now() + Duration::from_millis(300);
    let r = set.finish(deadline, |s| fake.execute(s, 2, false));
    assert_eq!((r.inline, r.overrun), (8, 2), "{r:?}");
    for s in 0..10 {
        fake.release(s);
    }
    wait_until("the stalled workers", || set.jobs().counts().claimed == 0);
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Gate 5 groundwork: a panic in the runner is caught by the worker, the slot completes (never
/// left `Claimed` by an unwind), the strip is `Faulted` and counted, and the worker lives on.
#[test]
fn a_panicking_runner_completes_its_slot_as_a_counted_fault() {
    let fake = Fake::new(4, Duration::ZERO);
    fake.set(2, Behavior::Panic);
    let mut set = set_of(&fake, 1);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_done(&set, 4, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r, BlockReport { mixed: 3, inline: 0, overrun: 0, faulted: 1, inline_refused: 0 });
    assert_eq!(set.outcome(2), Some(Outcome::Faulted));
    assert_eq!(set.stats().runner_panics, 1);
    assert_eq!(set.jobs().counts().idle, 4);

    fake.set(2, Behavior::Normal);
    assert!(set.begin_block(2));
    assert!(set.is_suspect(2), "a faulted strip's instance is suspect");
    for s in 0..4 {
        let expect = if s == 2 { Err(PublishRefused::Suspect) } else { Ok(Published::Published) };
        assert_eq!(set.publish(s), expect);
    }
    set.wake(1);
    wait_done(&set, 3, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r.mixed, 3);
    assert_eq!(set.outcome(2), Some(Outcome::NotPublished));
    assert_eq!(set.stats().misuse, 0, "a suspect refusal is not a caller error");

    // The caller replaced the instance.
    set.clear_suspect(2);
    assert!(set.begin_block(3));
    publish_all(&mut set);
    set.wake(1);
    wait_done(&set, 4, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r.mixed, 4, "the one worker survived the panic");
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

#[test]
fn zero_workers_render_everything_with_a_history_inline() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    for s in 0..4 {
        set.set_cost_estimate(s, Duration::from_micros(1));
    }
    assert!(set.begin_block(1));
    publish_all(&mut set);
    assert_eq!(set.wake(4), 0);
    let r = set.finish(Instant::now() + STALL, |s| fake.execute(s, 1, false));
    assert_eq!(r.inline, 4);
    assert!(set.shutdown(STALL).is_empty());
}

#[test]
fn publisher_misuse_is_refused_and_counted_and_changes_nothing() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 1);
    assert_eq!(set.publish(0), Err(PublishRefused::NoBlock));
    assert!(set.begin_block(3));
    assert!(!set.begin_block(3), "not newer");
    assert!(!set.begin_block(2), "older");
    assert_eq!(set.publish(9), Err(PublishRefused::OutOfRange));
    assert_eq!(set.publish(0), Ok(Published::Published));
    assert_eq!(set.publish(0), Err(PublishRefused::Twice));
    assert_eq!(set.stats().misuse, 5);
    set.wake(1);
    let r = set.finish(Instant::now() + STALL, |s| fake.execute(s, 3, false));
    assert_eq!(r.mixed + r.inline, 1);
    assert!(set.shutdown(STALL).is_empty());
    assert!(!set.begin_block(4), "shut down");
    assert_eq!(set.publish(0), Err(PublishRefused::NoBlock));
    assert!(set.shutdown(STALL).is_empty(), "a second shutdown is a no-op");
    assert_eq!(set.jobs().faults(), 0, "refused before reaching a slot");
}

#[test]
fn workers_are_promoted_once_at_start() {
    let before = crate::testing::promotion_calls();
    let fake = Fake::new(2, Duration::ZERO);
    let set = WorkerSet::new(Config { workers: 3, strips: 2, promote: true }, Arc::clone(&fake) as Arc<dyn StripRunner>).unwrap();
    wait_until("every worker's promotion", || (0..3).all(|k| set.promotion(k).is_some()));
    assert!(crate::testing::promotion_calls() - before >= 3);
    assert_eq!(set.promotion(3), None);
    let off = set_of(&fake, 2);
    assert_eq!((off.promotion(0), off.promotion(1)), (None, None), "promotion off");
    drop(set);
    drop(off);
}

/// Gate 5: an idle set retires at once.
#[test]
fn shutdown_of_an_idle_set_returns_every_worker_at_once() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut set = set_of(&fake, 4);
    let t0 = Instant::now();
    let q = set.shutdown(STALL);
    assert!(q.is_empty(), "{q:?}");
    // timing: stall-bound an idle worker returns on its wake; this only proves no worker took
    // the bound
    assert!(t0.elapsed() < Duration::from_secs(10));
}

/// Gate 5: a worker stalled inside a strip is quarantined within the bound, its slot stays
/// `Claimed`, and releasing it later does not crash the process (its state was leaked, not
/// freed).
#[test]
fn shutdown_quarantines_a_stalled_worker_within_its_bound() {
    const BOUND: Duration = Duration::from_millis(200);
    let fake = Fake::new(4, Duration::ZERO);
    fake.set(1, Behavior::Stall);
    let mut set = set_of(&fake, 2);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(2);
    wait_done(&set, 3, 1);
    let r = set.finish(Instant::now(), |_| panic!("nothing is left to render inline"));
    assert_eq!(r, BlockReport { mixed: 3, inline: 0, overrun: 1, faulted: 0, inline_refused: 0 });

    let t0 = Instant::now();
    let q = set.shutdown(BOUND);
    let took = t0.elapsed();
    assert!(took >= BOUND, "the stalled worker cannot have returned");
    // timing: stall-bound SLACK is over ten scheduler quanta; see its definition
    assert!(took < BOUND + SLACK, "shutdown took {took:?}");
    assert_eq!(q.workers.len(), 1, "{q:?}");
    assert_eq!(q.strips, vec![1]);
    assert_eq!(set.jobs().slot(1).map(JobSlot::state), Some(State::Claimed));
    drop(set);

    // The abandoned worker finishes into leaked state and exits on the stop flag.
    fake.release(1);
    wait_until("the abandoned worker", || !fake.strips[1].inside.load(Ordering::Acquire) && fake.stalled.load(Ordering::Acquire) == 0);
}

/// Gate 3: after construction, the publisher's block path allocates and frees nothing, the
/// worker loop (claim, run, complete, park, unpark) allocates nothing between runner calls, and
/// retiring an idle set allocates nothing (joining frees the threads' handles).
#[test]
fn no_block_operation_or_worker_loop_allocates_after_construction() {
    let fake = Fake::new(8, Duration::ZERO);
    let mut set = set_of(&fake, 3);
    let block = |set: &mut WorkerSet, g: u64| {
        assert!(set.begin_block(g));
        for s in 0..8 {
            assert_eq!(set.publish(s), Ok(Published::Published));
        }
        set.wake(3);
        if g.is_multiple_of(2) {
            // Worker coverage that does not depend on the scheduler: every strip by a worker.
            wait_done(set, 8, 0);
        }
        let r = set.finish(Instant::now() + STALL, |s| fake.execute(s, g, false));
        assert_eq!(r.mixed + r.inline, 8);
        if g.is_multiple_of(2) {
            assert_eq!(r.mixed, 8);
        }
    };
    for g in 1..=8 {
        block(&mut set, g);
    }
    fake.count_allocs.store(true, Ordering::Release);
    let ((), stats) = measure(|| {
        for g in 9..=300 {
            block(&mut set, g);
            std::hint::black_box((set.stats(), set.outcomes(), set.jobs().counts()));
        }
        // The refusal and bar paths (#168): every history cleared, then blocks at a passed
        // deadline with no wake. Each strip is refused for no history (counted) and taken back,
        // or a sweeping worker has it (`Late`, then `Busy`: both bar the strip).
        for s in 0..8 {
            set.reset_history(s);
        }
        for g in 301..=304 {
            assert!(set.begin_block(g));
            for s in 0..8 {
                let _ = set.publish(s).expect("publish");
            }
            let r = set.finish(Instant::now(), |_| panic!("no strip has a history"));
            assert_eq!(r.inline, 0);
        }
    });
    wait_until("the workers", || set.jobs().counts().claimed == 0);
    fake.count_allocs.store(false, Ordering::Release);
    assert!(stats.is_zero(), "publisher: {stats:?}");
    assert!(set.stats().mixed > 0, "workers rendered some strips");
    // #169: every worker run stamped its claim (the wake histogram) inside the counted loop.
    assert_eq!(set.figures().wake().iter().sum::<u64>(), set.stats().worker_runs, "one wake sample per worker run");
    assert_eq!(fake.worker_heap_ops.load(Ordering::Acquire), 0, "worker loop");
    let (q, stats) = measure(|| set.shutdown(STALL));
    assert!(q.is_empty());
    assert_eq!(stats.allocs, 0, "shutdown of an idle set: {stats:?}");
    assert_eq!(set.jobs().faults(), 0);

    // The refusal path by construction (#168): no worker, no history, a passed deadline.
    let mut none = set_of(&fake, 0);
    let ((), stats) = measure(|| {
        for g in 1..=4 {
            for s in 0..8 {
                none.reset_history(s);
            }
            assert!(none.begin_block(g));
            for s in 0..8 {
                assert_eq!(none.publish(s), Ok(Published::Published));
            }
            let r = none.finish(Instant::now(), |_| panic!("no strip has a history"));
            assert_eq!((r.overrun, r.inline_refused), (8, 8));
        }
    });
    assert!(stats.is_zero(), "refusals: {stats:?}");
    assert_eq!(none.stats().inline_refused_no_history, 32);
}

/// Review F2: a runner panic in a late result (retired unread at a later `begin_block`) makes
/// the strip suspect too, so the next publish cannot hand the suspect instance to a worker.
#[test]
fn a_late_result_whose_run_panicked_keeps_the_strip_suspect() {
    let fake = Fake::new(2, Duration::ZERO);
    fake.set(1, Behavior::Stall);
    let mut set = set_of(&fake, 2);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(2);
    wait_done(&set, 1, 1);
    // Claimed is not yet stalled: a release before the worker reads its behavior would run the
    // strip normally, with no panic to observe.
    wait_until("the worker on strip 1", || fake.stalled.load(Ordering::Acquire) == 1);
    let r = set.finish(Instant::now(), |_| panic!("nothing is left to render inline"));
    assert_eq!(set.outcome(1), Some(Outcome::Overrun(OverrunCause::Late)), "{r:?}");

    fake.release_then_panic(1);
    wait_until("the late, panicking run", || set.jobs().slot(1).map(JobSlot::state) == Some(State::Done));
    assert!(!set.is_suspect(1), "not seen until the publisher retires the result");
    assert!(set.begin_block(2));
    assert!(set.is_suspect(1));
    assert_eq!(set.publish(1), Err(PublishRefused::Suspect));
    assert_eq!(set.publish(0), Ok(Published::Published));
    set.wake(2);
    wait_done(&set, 1, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r.mixed, 1);
    let st = set.stats();
    assert_eq!((st.runner_panics, st.late_results, st.misuse), (1, 1, 0));

    set.clear_suspect(1);
    assert!(set.begin_block(3));
    publish_all(&mut set);
    set.wake(2);
    wait_done(&set, 2, 0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("nothing is left to render inline"));
    assert_eq!(r.mixed, 2, "resumed once the instance was replaced");
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Review F1: a wake issued while the one worker is finishing a run. The publisher may skip the
/// worker as busy only if the worker's next sweep is then certain to see the new job (the `SeqCst`
/// fence pair in `wake` and `worker_main`); a lost wake parks the worker with a strip published
/// and fails here at the stall bound. The window is a few instructions wide, so this exercises
/// the path rather than proving the fences; store-load reordering is allowed on x86 too, and the
/// weaker `AArch64` model is covered by the macos-aarch64 leg of `rt.yml`, which runs this.
#[test]
fn a_wake_while_the_worker_finishes_a_run_is_never_lost() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 1);
    let mut x = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for g in 1..=5000 {
        assert!(set.begin_block(g));
        fake.strips[0].spins.store(u32::try_from(next() % 2000).unwrap_or(0), Ordering::Relaxed);
        assert_eq!(set.publish(0), Ok(Published::Published));
        set.wake(1);
        // The worker is inside strip 0; publish strip 1 at a random point of its run, so some
        // publishes land while it clears `current` and sweeps again.
        wait_until("the worker on strip 0", || fake.strips[0].last_gen.load(Ordering::Acquire) == g);
        for _ in 0..next() % 2000 {
            std::hint::spin_loop();
        }
        assert_eq!(set.publish(1), Ok(Published::Published));
        set.wake(1);
        wait_done(&set, 2, 0);
        let r = set.finish(Instant::now() + STALL, |_| panic!("the worker rendered both"));
        assert_eq!(r.mixed, 2);
    }
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Review F3: a bound past what `Instant` can represent is unbounded, not zero.
#[test]
fn shutdown_with_an_unrepresentable_bound_waits_for_the_worker() {
    let fake = Fake::new(1, Duration::ZERO);
    fake.set(0, Behavior::Stall);
    let mut set = set_of(&fake, 1);
    assert!(set.begin_block(1));
    publish_all(&mut set);
    set.wake(1);
    wait_until("the stalled worker", || fake.stalled.load(Ordering::Acquire) == 1);
    let shutting = std::thread::spawn(move || set.shutdown(Duration::MAX));
    // timing: sleep-ok gives a broken shutdown time to return early; the assertion below holds
    // by construction once fixed (the worker cannot return while stalled)
    std::thread::sleep(Duration::from_millis(300));
    assert!(!shutting.is_finished(), "shutdown(Duration::MAX) returned while a worker was running");
    fake.release(0);
    let q = shutting.join().expect("shutdown thread");
    assert!(q.is_empty(), "{q:?}");
}

/// Review of slice 4: with a busy normal-priority thread on every core, `finish` still returns
/// at its deadline (it spins between sweeps, it never yields its core to the load) and leaves the
/// stalled strip a counted `Late` overrun. Timing is asserted only as a stall bound: the test
/// thread is not promoted, so the load can preempt it like any thread.
#[test]
fn finish_spins_to_its_deadline_under_a_load_on_every_core() {
    /// Stops the load when dropped, also when an assertion fails.
    struct Load(Arc<AtomicBool>, Vec<std::thread::JoinHandle<()>>);
    impl Drop for Load {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
            for h in self.1.drain(..) {
                let _ = h.join();
            }
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let handles: Vec<_> = (0..cores)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
                }
            })
        })
        .collect();
    let load = Load(stop, handles);
    let fake = Fake::new(4, Duration::ZERO);
    fake.set(2, Behavior::Stall);
    let mut set = set_of(&fake, 2);
    // Too costly to render inline: a worker takes it (and stalls), or it is taken back unstarted.
    set.set_cost_estimate(2, Duration::from_secs(1));
    let mut worst = Duration::ZERO;
    let mut overran = false;
    for g in 1..=20u64 {
        assert!(set.begin_block(g));
        for s in 0..4 {
            let _ = set.publish(s).expect("publish");
        }
        set.wake(4);
        let deadline = Instant::now() + Duration::from_millis(5);
        let _ = set.finish(deadline, |_| {});
        let returned = Instant::now();
        assert!(returned >= deadline || (0..4).all(|s| set.outcome(s) != Some(Outcome::Pending)));
        worst = worst.max(returned.saturating_duration_since(deadline));
        assert!((0..4).all(|s| set.outcome(s) != Some(Outcome::Pending)), "every strip resolved");
        overran |= matches!(set.outcome(2), Some(Outcome::Overrun(_)));
    }
    assert!(overran, "once a worker took the stalled strip it was a counted overrun");
    drop(load);
    fake.release(2);
    println!("worst overshoot past the deadline under load: {worst:?}");
    // timing: stall-bound SLACK is over ten scheduler quanta; see its definition
    assert!(worst < SLACK, "finish returned {worst:?} past its deadline under load");
    let _ = set.shutdown(STALL);
}
