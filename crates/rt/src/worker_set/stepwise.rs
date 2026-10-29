//! The worker set's block, one resolution at a time, for the dependency graph (issue #174,
//! [`crate::task_graph`]): [`WorkerSet::poll`] is [`WorkerSet::finish`] cut at each strip it
//! resolves, so the callback can publish that strip's dependents before it waits on; with
//! [`WorkerSet::forgo`] and [`WorkerSet::skip_upstream`] for the strips the graph does not publish.
//! The policies are `finish`'s: the same sweep, admission, spin and deadline; only the return
//! points differ. Allocation-free; no `unsafe`.

use super::figures::Count;
use super::{OverrunCause, PublishRefused, SPINS_PER_SWEEP, State, WorkerSet, bump};
use super::{BlockReport, Outcome};
use std::sync::Arc;
use std::time::Instant;

/// What one [`WorkerSet::poll`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Poll {
    /// `strip` is resolved: its [`Outcome`] (not `Pending`). `late` when it was resolved in the
    /// deadline phase (no time is left for anything published after it).
    Resolved {
        /// The strip.
        strip: usize,
        /// Its outcome for the block.
        outcome: Outcome,
        /// Resolved at or past the deadline.
        late: bool,
    },
    /// Nothing published this block is pending any more.
    Finished,
}

impl WorkerSet {
    /// Publisher: waits, as [`WorkerSet::finish`] does, until one strip published this block is
    /// resolved by `deadline`, and returns it (a `Done` consumed, an admitted strip rendered
    /// inline through `inline(strip)`, or at the deadline an overrun); [`Poll::Finished`] when
    /// none is pending. Strips may be published between two calls. Never blocks on a worker;
    /// allocation-free. Call it until it returns [`Poll::Finished`], then read
    /// [`WorkerSet::block_report`].
    pub fn poll(&mut self, deadline: Instant, mut inline: impl FnMut(usize)) -> Poll {
        let g = self.generation;
        let shared = Arc::clone(&self.shared);
        loop {
            let mut open = false;
            // One clock read per sweep, as in `finish`.
            let now = Instant::now();
            for k in 0..self.published_len {
                let strip = self.published[k];
                if self.outcomes[strip] != Outcome::Pending {
                    continue;
                }
                open = true;
                let Some(slot) = shared.jobs.slot(strip) else { continue };
                match slot.state() {
                    State::Done => {
                        if let Some(o) = self.take_done(strip, g) {
                            self.outcomes[strip] = o;
                            return Poll::Resolved { strip, outcome: o, late: false };
                        }
                    }
                    State::Published if self.admits(strip, now, deadline) && slot.reclaim() => {
                        let t0 = Instant::now();
                        inline(strip);
                        let end = Instant::now();
                        shared.record_cost(strip, end.saturating_duration_since(t0));
                        if end > deadline {
                            self.bar(strip);
                        }
                        slot.retire();
                        bump(&self.shared.stats.inline_renders);
                        shared.figures.count(strip, Count::Inline);
                        self.outcomes[strip] = Outcome::Inline;
                        return Poll::Resolved { strip, outcome: Outcome::Inline, late: false };
                    }
                    _ => {}
                }
            }
            if !open {
                return Poll::Finished;
            }
            if now >= deadline {
                let deadline_ns = shared.figures.nanos(deadline).max(1);
                for k in 0..self.published_len {
                    let strip = self.published[k];
                    if self.outcomes[strip] == Outcome::Pending {
                        let outcome = self.overrun_strip(strip, g, deadline_ns);
                        if outcome != Outcome::Pending {
                            return Poll::Resolved { strip, outcome, late: true };
                        }
                    }
                }
                return Poll::Finished;
            }
            // Spin, bounded by the deadline (checked once per sweep above).
            for _ in 0..SPINS_PER_SWEEP {
                std::hint::spin_loop();
            }
        }
    }

    /// Publisher: `strip` became ready only when no time was left (the graph's deadline phase):
    /// it is not published, and counts as [`OverrunCause::Unstarted`], its own cause (or
    /// [`OverrunCause::Busy`] when a worker still holds it, as [`WorkerSet::publish`] does).
    ///
    /// # Errors
    ///
    /// As [`WorkerSet::publish`]: a caller error, counted in [`super::Stats::misuse`].
    pub fn forgo(&mut self, strip: usize) -> Result<Outcome, PublishRefused> {
        self.unpublished(strip)?;
        if self.busy[strip] {
            return self.publish(strip).map(|_| self.outcomes[strip]);
        }
        self.outcomes[strip] = Outcome::Overrun(OverrunCause::Unstarted);
        self.forgone += 1;
        bump(&self.shared.stats.overruns_unstarted);
        self.shared.figures.count(strip, Count::Unstarted);
        Ok(self.outcomes[strip])
    }

    /// Publisher: `strip` is skipped this block because `origin`, a task it depends on, had no
    /// result ([`OverrunCause::UpstreamMissed`], counted apart from the strip's own causes and
    /// recorded with the origin in its figures). A strip a worker still holds is a
    /// [`OverrunCause::Busy`] overrun instead, its own cause. No render bar: nothing ran.
    ///
    /// # Errors
    ///
    /// As [`WorkerSet::publish`]: a caller error, counted in [`super::Stats::misuse`].
    pub fn skip_upstream(&mut self, strip: usize, origin: usize) -> Result<Outcome, PublishRefused> {
        self.unpublished(strip)?;
        if self.busy[strip] {
            return self.publish(strip).map(|_| self.outcomes[strip]);
        }
        self.outcomes[strip] = Outcome::Overrun(OverrunCause::UpstreamMissed);
        self.upstream += 1;
        bump(&self.shared.stats.upstream_missed);
        self.shared.figures.upstream(strip, origin);
        Ok(self.outcomes[strip])
    }

    /// The open block's counts so far (the same as [`WorkerSet::finish`] returns).
    #[must_use]
    pub fn block_report(&self) -> BlockReport {
        self.report()
    }

    /// The checks `publish` makes before it touches a strip: a block is open, the strip exists
    /// and nothing was decided for it this block.
    fn unpublished(&mut self, strip: usize) -> Result<(), PublishRefused> {
        let why = if self.generation == 0 || self.shut {
            PublishRefused::NoBlock
        } else {
            match self.outcomes.get(strip) {
                None => PublishRefused::OutOfRange,
                Some(Outcome::NotPublished) => return Ok(()),
                Some(_) => PublishRefused::Twice,
            }
        };
        bump(&self.shared.stats.misuse);
        Err(why)
    }
}
