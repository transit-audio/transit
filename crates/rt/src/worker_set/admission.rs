use super::{InlineRefusal, OVERRUN_BAR_BLOCKS, WorkerSet, bump};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

impl WorkerSet {
    /// Allows a published tail task to render inline when reclaimed at the deadline.
    /// Call before publication. Every successful `begin_block` clears every slot's flag.
    pub fn set_inline_at_deadline(&mut self, strip: usize) {
        if let Some(flag) = self.inline_at_deadline.get_mut(strip) {
            *flag = true;
        }
    }

    /// Records one direct callback aux render through the worker cost update (EWMA and
    /// decaying maximum). Call exactly once while the callback owns the strip.
    pub fn record_callback_cost(&mut self, strip: usize, took: Duration) {
        self.shared.record_cost(strip, took);
        if strip < self.strips() {
            bump(&self.shared.queue.counts.tail_callback);
        }
    }

    /// True when the strip is barred from inline renders in the open block, because it overran
    /// within the last [`OVERRUN_BAR_BLOCKS`] blocks.
    #[must_use]
    pub fn inline_barred(&self, strip: usize) -> bool {
        self.barred_until.get(strip).is_some_and(|&until| self.block_index < until)
    }

    /// Bars `strip` from inline renders for the next [`OVERRUN_BAR_BLOCKS`] blocks.
    pub(super) fn bar(&mut self, strip: usize) {
        if let Some(b) = self.barred_until.get_mut(strip) {
            *b = self.block_index + OVERRUN_BAR_BLOCKS + 1;
        }
    }

    /// The admission rule (see the module docs): `Ok` when the callback may start an inline
    /// render of `strip` with `left` before the deadline; `Err(None)` when no time is left (not a
    /// refusal), `Err(Some(reason))` when the rule refuses it.
    pub(super) fn admission(&self, strip: usize, left: Duration) -> Result<(), Option<InlineRefusal>> {
        let worst = self.shared.figures.cells(strip).map_or(0, |c| c.worst_ns.load(Ordering::Relaxed));
        if worst == 0 {
            return Err(Some(InlineRefusal::NoHistory));
        }
        if self.inline_barred(strip) {
            return Err(Some(InlineRefusal::RecentOverrun));
        }
        if left.is_zero() {
            return Err(None);
        }
        if u128::from(worst) > left.as_nanos() {
            return Err(Some(InlineRefusal::WorstCase));
        }
        Ok(())
    }

    /// Admission at `now`; a refusal is counted once per strip per block.
    pub(super) fn admits(&mut self, strip: usize, now: Instant, deadline: Instant) -> bool {
        match self.admission(strip, deadline.saturating_duration_since(now)) {
            Ok(()) => true,
            Err(None) => false,
            Err(Some(why)) => {
                if let Some(seen) = self.refused.get_mut(strip)
                    && !std::mem::replace(seen, true)
                {
                    let c = &self.shared.stats;
                    bump(match why {
                        InlineRefusal::NoHistory => &c.refused_no_history,
                        InlineRefusal::WorstCase => &c.refused_worst_case,
                        InlineRefusal::RecentOverrun => &c.refused_recent_overrun,
                    });
                }
                false
            }
        }
    }
}
