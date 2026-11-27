//! Test seams of the pool (feature `testing`, issue #210): the deterministic deadline the
//! scheduling-condition gate realizes its stall schedules with, and the pending MIDI of a strip
//! for its fingerprint ([`crate::fingerprint`]).
//!
//! **The deterministic deadline.** Armed ([`crate::MixEngine::hold_until_alone`], with `gate`
//! the strips a test holds this block, possibly none), no clock ends a block: the pool's deadline
//! is [`never`], so every published strip outside `gate` is waited for (rendered by a worker or
//! inline, whatever the host's speed), and once only held strips are unresolved and, with
//! workers, each of them is claimed by one (inside the test's hold), the deadline is now. A held
//! strip is then `Late` (a worker holds it) or, with no worker, `Unstarted` (the test seeds a
//! cost longer than any deadline, [`never`] included, so the callback refuses it inline), and nothing else is decided by the clock: which
//! strips miss the block is the test's input (E49), not a scheduling outcome. Before E72 the real
//! deadline still applied while a strip outside `gate` was unresolved (a test's far deadline, 60
//! s): a stall past it skipped an unheld strip. Disarmed ([`crate::MixEngine::clock_deadline`]),
//! the real deadline again. Both render paths honor it: the graph path re-reads the deadline at
//! each poll, the plain (unkeyed) path waits for it before `finish`
//! ([`StripPool::plain_gated_deadline`]); before that, a held strip on the plain path was waited
//! for to the far deadline, so a held worker never ended the block.

use super::*;
use transit_rt::strip_jobs::State;

/// What the pool parked for a strip ([`StripPool::parked`]).
#[derive(Default)]
pub(crate) struct Parked {
    /// The deferred ring's pending safety release: its silence mask.
    pub(crate) silence: Option<u16>,
    /// The ring's events, then the live events, in delivery order.
    pub(crate) events: Vec<ParkedEvent>,
}

/// One parked event.
pub(crate) enum ParkedEvent {
    /// A tagged event with its identity (source, transport, message, binding, due).
    Event(InstrumentEvent),
    /// An untagged event: delivery releases the instrument for it.
    Release,
    /// A tagged event whose side-table entry is gone: delivery drops it (its bytes).
    Dropped([u8; 3]),
}

/// A last-resort hang ceiling, not a latency claim: ten minutes without a claim turns a held strip no
/// worker ever claims into a failure. It is the tests' own ceiling (`tests/common/stall.rs`, `HANG`): a
/// host stall of a minute is not a hang (E72), and a claim by any held strip's worker renews the budget.
const CLAIM_HANG: Duration = Duration::from_secs(600);

/// The no-progress watch of [`StripPool::gated_deadline`]: no outcome depends on how long the pool waited
/// for a worker, only on whether workers keep claiming held strips. The clock and the progress read are
/// injected so tests cover the interleavings without sleeping (the same shape as `Watchdog` in
/// `tests/common/stall.rs`, which an integration test's module cannot share with the crate).
pub(crate) struct ClaimWatch {
    progress: usize,
    since: Duration,
}

impl ClaimWatch {
    pub(crate) fn new(progress: usize) -> Self {
        Self { progress, since: Duration::ZERO }
    }

    /// Read time before progress for expiry, but renew at a fresh time after observing progress: either
    /// read can be separated from the other by an arbitrarily long scheduler pause.
    pub(crate) fn stalled(&mut self, progress: impl FnOnce() -> usize, mut elapsed: impl FnMut() -> Duration) -> bool {
        let before = elapsed();
        let observed = progress();
        if observed != self.progress {
            self.progress = observed;
            self.since = elapsed();
            return false;
        }
        before.saturating_sub(self.since) >= CLAIM_HANG
    }
}

impl StripPool {
    pub(crate) fn arm_tail_slot(&mut self, slot: usize, value: transit_rt::worker_set::seams::SlotOverrides) {
        self.set.arm_slot_overrides(slot, value);
    }
    pub(crate) fn arm_tail_cost(&mut self, clock: transit_rt::worker_set::seams::CostClock) {
        self.set.arm_cost_clock(clock);
    }
    /// The plain path's deadline under the deterministic deadline. The graph path re-reads
    /// [`StripPool::gated_deadline`] at each poll and consumes finished strips in between; the
    /// plain path hands `finish` one deadline and consumes nothing before it, so outcomes stay
    /// `Pending` there and the job states decide: this waits (with workers) until every published
    /// strip outside `gate` is `Done` and each held strip is claimed (or done), then gives `finish`
    /// that moment, so the held strips are `Late` and the others `Mixed`. Unarmed, or with no
    /// worker (the callback renders inline in `finish`, which needs the far deadline), `deadline`
    /// unchanged.
    pub(crate) fn plain_gated_deadline(&self, deadline: Instant) -> Instant {
        if !self.gated || self.set.workers() == 0 {
            return deadline;
        }
        loop {
            let waiting = self.set.outcomes().iter().enumerate().any(|(i, o)| {
                let held = self.gate.get(i).copied().unwrap_or(false);
                let state = self.set.jobs().slot(i).map(|s| s.state());
                *o == Outcome::Pending
                    && match state {
                        Some(State::Done) | None => false,
                        Some(State::Claimed) => !held,
                        Some(_) => true,
                    }
            });
            if !waiting {
                return Instant::now();
            }
            std::hint::spin_loop();
        }
    }

    /// The deadline a poll uses this block (see the module docs).
    pub(crate) fn gated_deadline(&self, deadline: Instant) -> Instant {
        if !self.gated {
            return deadline;
        }
        // The first progress read precedes the clock origin, so a pause between them is never charged.
        let mut watch = ClaimWatch::new(0);
        let start = Instant::now();
        loop {
            let (mut unclaimed, mut claimed) = (false, 0usize);
            for (i, o) in self.set.outcomes().iter().enumerate() {
                if *o != Outcome::Pending {
                    continue;
                }
                // Outside the held set: waited for (`deadline` is `never` while armed).
                if !self.gate.get(i).copied().unwrap_or(false) {
                    return deadline;
                }
                if self.set.workers() > 0 && self.set.jobs().slot(i).is_some_and(|s| s.state() != State::Claimed) {
                    unclaimed = true;
                } else {
                    claimed += 1;
                }
            }
            if !unclaimed {
                return Instant::now();
            }
            assert!(!watch.stalled(|| claimed, || start.elapsed()), "a held strip was never claimed by a worker");
            std::hint::spin_loop();
        }
    }

    /// The messages parked for strip `i`, in delivery order (#742's canonical pending form): its
    /// deferred ring's safety release (its silence mask), the ring's events, then the live events
    /// taken while it was away; each with the identity its side-table entry holds, or as it is
    /// when it has none (delivery calls `release_instrument` for an untagged one and drops a
    /// tagged one whose entry is gone).
    pub(crate) fn parked(&self, i: usize) -> Parked {
        let side = self.side.get(i * SIDE..(i + 1) * SIDE).unwrap_or(&[]);
        let of = |e: &Event| match side.get(e.tag as usize) {
            Some(Some(ev)) => ParkedEvent::Event(*ev),
            _ if e.tag == Event::NO_TAG => ParkedEvent::Release,
            _ => ParkedEvent::Dropped([e.status, e.data1, e.data2]),
        };
        let mut parked = Parked::default();
        if let Some(r) = self.defer.ring(i) {
            parked.silence = r.is_silenced().then(|| r.silence_mask());
            parked.events.extend(r.events().iter().map(of));
        }
        let n = self.live_len.get(i).copied().unwrap_or(0);
        parked.events.extend(self.live.get(i * LIVE..i * LIVE + n).unwrap_or(&[]).iter().map(of));
        parked
    }

    /// Strip `i`'s outcome this block (`None` past the pool's strips).
    pub(crate) fn outcome(&self, i: usize) -> Option<Outcome> {
        self.set.outcome(i)
    }

    /// Strip `i`'s figures (its counts by cause, cumulative).
    pub(crate) fn strip_figures(&self, i: usize) -> Option<transit_rt::worker_set::figures::StripFigures> {
        self.set.figures().strip(i)
    }

    /// A worker holds strip `i`'s slot (claimed, not done).
    pub(crate) fn claimed(&self, i: usize) -> bool {
        self.set.jobs().slot(i).is_some_and(|s| s.state() == State::Claimed)
    }

    /// Strip `i`'s cost history becomes `cost` (between blocks).
    pub(crate) fn set_cost(&self, i: usize, cost: Duration) {
        self.set.set_cost_estimate(i, cost);
    }

    /// Late results retired unread so far.
    pub(crate) fn late_results(&self) -> u64 {
        self.set.stats().late_results
    }
}

/// The pool's deadline while the deterministic deadline is armed: one no clock reaches in a test
/// (ten years from the block's start; an hour where the platform's clock cannot represent that).
pub(crate) fn never(start: Instant) -> Instant {
    const YEARS: Duration = Duration::from_secs(10 * 365 * 24 * 3600);
    start.checked_add(YEARS).or_else(|| start.checked_add(Duration::from_secs(3600))).unwrap_or(start)
}

/// The graph path renders `t`'s strip on the callback (it reads the live input or the hardware).
pub(crate) fn pinned(t: &Track, st: &Strip) -> bool {
    reads_input(t, st)
}

#[cfg(test)]
mod tests;
