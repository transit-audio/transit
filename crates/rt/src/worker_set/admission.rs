use super::super::strip_jobs::JobSlot;
use super::{Count, InlineRefusal, OVERRUN_BAR_BLOCKS, Outcome, WorkerSet, bump};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

impl WorkerSet {
    /// The pre-deadline ownership transition of an admitted strip, counted at reclaim rather than
    /// completion. `race` runs just before the compare-and-swap (the test seam of a worker
    /// claiming at that point; production passes a no-op).
    pub(super) fn reclaim_before_deadline(&mut self, strip: usize, now: Instant, deadline: Instant, race: &mut impl FnMut(usize)) -> bool {
        if !self.admits(strip, now, deadline) {
            return false;
        }
        race(strip);
        if !self.shared.jobs.slot(strip).is_some_and(JobSlot::reclaim) {
            return false;
        }
        bump(&self.shared.queue.counts.inline_pre);
        true
    }

    /// The reclaim of a lone unclaimed aux (ADR 0029 section 11): the pre-deadline transition of a tail task (flagged with
    /// [`WorkerSet::set_inline_at_deadline`]) still unclaimed while the callback waits for nothing
    /// a worker holds. The callback would render it at the deadline whatever its history (ADR 0029
    /// B2), and no executor gains by its waiting: a worker that claimed it now, when it no longer
    /// fits the time left, would only turn a late render into a silent bus. So it is taken now,
    /// without the history, recent-overrun and worst-case checks of [`Self::admission`], and
    /// counted as a pre-deadline reclaim. The test seam that suppresses pre-deadline reclaims
    /// suppresses this one too.
    pub(super) fn reclaim_tail_alone(&mut self, strip: usize, race: &mut impl FnMut(usize)) -> bool {
        #[cfg(any(test, feature = "testing"))]
        if self.slot_overrides(strip).no_reclaim {
            return false;
        }
        if !self.inline_at_deadline.get(strip).copied().unwrap_or(false) {
            return false;
        }
        race(strip);
        if !self.shared.jobs.slot(strip).is_some_and(JobSlot::reclaim) {
            return false;
        }
        bump(&self.shared.queue.counts.inline_pre);
        true
    }

    /// Renders an already reclaimed slot. The clock argument is the deterministic cost/readout
    /// seam for rt unit tests; production passes `Instant::now`.
    pub(super) fn render_inline(
        &mut self,
        strip: usize,
        deadline: Instant,
        inline: &mut impl FnMut(usize),
        mut clock: impl FnMut() -> Instant,
    ) -> Instant {
        let start = clock();
        inline(strip);
        let end = clock();
        self.shared.record_cost(strip, end.saturating_duration_since(start));
        if end > deadline {
            self.bar(strip);
        }
        if let Some(slot) = self.shared.jobs.slot(strip) {
            slot.retire();
        }
        bump(&self.shared.stats.inline_renders);
        self.shared.figures.count(strip, Count::Inline);
        self.outcomes[strip] = Outcome::Inline;
        end
    }

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
        #[cfg(any(test, feature = "testing"))]
        if self.slot_overrides(strip).no_reclaim {
            return false;
        }
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
