//! Test seams of the pool (feature `testing`, issue #210): the deterministic deadline the
//! scheduling-condition gate realizes its stall schedules with, and the pending MIDI of a strip
//! for its fingerprint ([`crate::fingerprint`]).
//!
//! **The deterministic deadline.** With `gate` set (the strips a test holds this block), the
//! graph driver's polls wait for the deadline as follows: while a published strip outside
//! `gate` is unresolved, the real deadline applies (with the far strips' deadline the tests use,
//! every such strip is rendered by a worker or inline); once only held strips are unresolved
//! and, with workers, each of them is claimed by one (inside the test's hold), the deadline is
//! now. A held strip is then `Late` (a worker holds it) or, with no worker, `Unstarted` (the test
//! seeds its cost so the callback refuses it inline), and nothing else is decided by the clock:
//! which strips miss the block is the test's input (E49), not a scheduling outcome.

use super::*;
use std::hash::Hasher;
use transit_rt::strip_jobs::State;

/// A stall bound, not a latency claim: turns a held strip no worker claims into a failure.
const CLAIM_BOUND: Duration = Duration::from_secs(60);

impl StripPool {
    /// The deadline a poll uses this block (see the module docs).
    pub(crate) fn gated_deadline(&self, deadline: Instant) -> Instant {
        if self.gate.is_empty() {
            return deadline;
        }
        let t0 = Instant::now();
        loop {
            let mut unclaimed = false;
            for (i, o) in self.set.outcomes().iter().enumerate() {
                if *o != Outcome::Pending {
                    continue;
                }
                if !self.gate.get(i).copied().unwrap_or(false) {
                    return deadline;
                }
                if self.set.workers() > 0 && self.set.jobs().slot(i).is_some_and(|s| s.state() != State::Claimed) {
                    unclaimed = true;
                }
            }
            if !unclaimed {
                return Instant::now();
            }
            assert!(t0.elapsed() < CLAIM_BOUND, "a held strip was never claimed by a worker");
            std::hint::spin_loop();
        }
    }

    /// The messages parked for strip `i`, in delivery order: its deferred ring, then the live
    /// events taken while it was away.
    pub(crate) fn pending_midi(&self, i: usize, h: &mut dyn Hasher) {
        let side = self.side.get(i * SIDE..(i + 1) * SIDE).unwrap_or(&[]);
        let put = |h: &mut dyn Hasher, e: &Event| {
            if let Some(Some(ev)) = side.get(e.tag as usize) {
                h.write(format!("{:?}", (ev.source, ev.transport, ev.message)).as_bytes());
                h.write_u8(0);
            } else {
                h.write(format!("{:?}", (e.status, e.data1, e.data2)).as_bytes());
                h.write_u8(1);
            }
        };
        if let Some(r) = self.defer.ring(i) {
            r.events().iter().for_each(|e| put(h, e));
            if r.is_silenced() {
                h.write_u16(r.silence_mask());
            }
        }
        let n = self.live_len.get(i).copied().unwrap_or(0);
        self.live.get(i * LIVE..i * LIVE + n).unwrap_or(&[]).iter().for_each(|e| put(h, e));
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

/// The graph path renders `t`'s strip on the callback (it reads the live input or the hardware).
pub(crate) fn pinned(t: &Track, st: &Strip) -> bool {
    reads_input(t, st)
}
