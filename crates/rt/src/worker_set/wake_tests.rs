//! Wake selection with real threads: a wake counts only a parked worker (the `sched_equivalence`
//! hang on main, 2026-11-15). A worker that is awake between a sweep and its next claim may be
//! claiming another strip, one that stalls; a wake spent on it left the parked worker asleep and the
//! new strip unclaimed for as long as the stall lasted. The claim gate holds a worker at exactly that
//! point (claimed, `current` not yet marked), so the interleaving is the test's input, not a timing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{Behavior, Fake, STALL, set_of, wait_until};
use super::*;
use crate::strip_jobs::{JobSlot, State};

/// The test seam in `worker_main`: a worker that claims `strip` waits there, before it marks
/// itself busy, until the test opens the gate.
#[derive(Default)]
pub(super) struct ClaimGate {
    /// The strip to hold the claimer at, plus one (0: the gate is off).
    strip: AtomicUsize,
    /// The worker held at the gate, plus one (0: none yet).
    held: AtomicUsize,
    open: AtomicBool,
}

impl ClaimGate {
    /// Called by every worker right after each claim.
    pub(super) fn pass(&self, worker: usize, strip: usize) {
        if self.strip.load(Ordering::Acquire) != strip + 1 {
            return;
        }
        self.held.store(worker + 1, Ordering::Release);
        let t0 = Instant::now();
        while !self.open.load(Ordering::Acquire) {
            assert!(t0.elapsed() < STALL, "the claim gate was never opened");
            // timing: sleep-ok paces a wait for the test's gate flag, measures nothing
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Waits (on state, with the stall bound) until worker `w` is parked with no wake pending; `why`
/// names the point of the test, and the failure names the worker.
fn wait_parked(set: &WorkerSet, w: usize, why: &str) {
    wait_until(&format!("worker {w} parked ({why})"), || parked(set, w));
}

fn parked(set: &WorkerSet, w: usize) -> bool {
    // Acquire on the flag first: a consumed wake orders the worker's clear after `park` before the
    // read of `parked`, so `parked` here is the worker's next park, never the one it woke from.
    !set.shared.workers[w].woken.load(Ordering::Acquire) && set.shared.workers[w].parked.load(Ordering::Acquire)
}

/// Strip 0 stalls once claimed; worker A claims it and is held at the gate (awake, `current` 0)
/// while worker B is parked. Strip 1 is then published and woken for with the wake cursor on A.
/// Before the fix the wake counted A and stopped: B slept, strip 1 stayed published while strip 0
/// stalled, and only the stall bound below ended the test. Now B is woken and renders it.
///
/// The test asserts outcomes, never the count `wake` returns: that count reads each worker's
/// `parked` after unparking it, so a worker that runs at once (or returns from `park` spuriously)
/// can be uncounted. That errs toward one more wake, never a lost one, and under starvation it
/// happened (the chain hy starved run: `wake(2)` returned 1 with both workers seen parked).
#[test]
fn a_wake_reaches_a_parked_worker_while_an_awake_one_claims_a_strip_that_stalls() {
    let fake = Fake::new(2, Duration::ZERO);
    fake.set(0, Behavior::Stall);
    let mut set = set_of(&fake, 2);
    wait_parked(&set, 0, "after its first sweep");
    wait_parked(&set, 1, "after its first sweep");
    set.shared.claim_gate.strip.store(1, Ordering::Release);

    assert!(set.begin_block(1));
    assert_eq!(set.publish(0), Ok(Published::Published));
    // Both are flagged and unparked (each reads `current` 0); one claims strip 0 and is held.
    set.wake(2);
    wait_until("a worker held at the gate with strip 0 claimed", || set.shared.claim_gate.held.load(Ordering::Acquire) != 0);
    let a = set.shared.claim_gate.held.load(Ordering::Acquire) - 1;
    let b = 1 - a;
    // `parked` also requires B's wake taken: B swept after the publish and found nothing.
    wait_parked(&set, b, "again after an empty sweep");
    assert_eq!(set.shared.workers[a].current.load(Ordering::Relaxed), 0, "A has claimed but not marked itself busy");
    assert_eq!(set.jobs().slot(0).map(JobSlot::state), Some(State::Claimed));

    // The wake for strip 1 visits A first, as the cursor would after a wake that reached B last.
    set.wake_cursor = a;
    assert_eq!(set.publish(1), Ok(Published::Published));
    // A is awake (flagged, not counted), so the wake goes on to B; the old counting stopped at A.
    set.wake(1);
    set.shared.claim_gate.open.store(true, Ordering::Release);
    wait_until("strip 1 rendered by B while A is inside strip 0's stall", || set.jobs().slot(1).map(JobSlot::state) == Some(State::Done));
    wait_until("A inside strip 0's stall", || fake.stalled.load(Ordering::Acquire) == 1);
    assert_eq!(set.jobs().slot(0).map(JobSlot::state), Some(State::Claimed), "strip 0 is still held");

    fake.release(0);
    let r = set.finish(Instant::now() + STALL, |_| panic!("both strips belong to the workers"));
    assert_eq!(r.mixed, 2, "{r:?}");
    assert!(set.shutdown(STALL).is_empty());
    assert_eq!(set.jobs().faults(), 0);
}

/// Worker 0 is flagged (awake), so the pass skips it and wakes worker 1, the last one visited. The
/// cursor must move to one past worker 1, which is 0: before the fix it advanced by the one wake,
/// to 1, the worker just woken, and the next pass visited it first again.
#[test]
fn the_cursor_advances_past_the_last_worker_visited() {
    let fake = Fake::new(2, Duration::ZERO);
    let mut set = set_of(&fake, 2);
    wait_parked(&set, 0, "after its first sweep");
    wait_parked(&set, 1, "after its first sweep");
    set.shared.workers[0].woken.store(true, Ordering::Release);

    set.wake_cursor = 0;
    set.wake(1);
    assert_eq!(set.wake_cursor, 0, "the cursor must stop one past worker 1, the last visited");
    assert!(set.shutdown(STALL).is_empty());
}
