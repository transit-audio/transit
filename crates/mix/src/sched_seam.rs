//! Test seams for the scheduling-condition gate (feature `testing`, issue #210, ADR 0029
//! amendment section 8): what a test needs to replay a stall schedule on the graph path and to
//! render its reference, the serial path with the same blocks muted.
//!
//! - **Muted serial blocks** ([`MixEngine::mute_next_block`]): on the serial path, the next block
//!   leaves the given phase-1 strips out as the graph path leaves out a strip that missed it. A
//!   [`Mute::Skip`] strip is not rendered at all (its keys are not filled, so their lines are
//!   neither written nor idled; its own lines do not run; its instrument's events stay queued for
//!   its next processed block) and is silent and not metered. A [`Mute::Discard`] strip is
//!   rendered (its state moves on, as a late worker's render does) and its output is dropped. An
//!   aux keyed from a muted strip (or from an aux skipped by this rule) is skipped by the graph's
//!   tail rule. The graph path ignores the mutes.
//! - **The deterministic deadline** ([`MixEngine::hold_until_alone`], `strip_pool::seam`).
//! - **Views** of the last block: each phase-1 strip's outcome, silence and figures, the tail's
//!   skips, a worker's claim, the late results retired unread; and a strip's cost history.
//!
//! Nothing here runs on the audio path in a build without the feature.

use super::*;
use std::time::Duration;
use transit_rt::worker_set::Outcome;
use transit_rt::worker_set::figures::StripFigures;

/// How a muted serial block leaves a strip out (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mute {
    /// Not rendered: as a strip that was busy, unstarted or upstream-missed.
    Skip,
    /// Rendered and its output dropped: as a strip whose worker was late.
    Discard,
}

/// The next block's mutes, per phase-1 strip (0: none, 1: skip, 2: discard).
#[derive(Default)]
pub(crate) struct Mutes(Vec<u8>);

impl Mutes {
    pub(crate) fn any(&self) -> bool {
        self.0.iter().any(|&m| m != 0)
    }

    /// Strip `i` is not rendered.
    pub(crate) fn skips(&self, i: usize) -> bool {
        self.0.get(i) == Some(&1)
    }

    /// Strip `i` is silent (skipped or discarded).
    pub(crate) fn silent(&self, i: usize) -> bool {
        self.0.get(i).is_some_and(|&m| m != 0)
    }

    /// After the block (keeps the storage).
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

impl MixEngine {
    /// Mutes phase-1 strips (by table index) in the next serial block (see the module docs).
    /// Off the audio path, between blocks.
    pub fn mute_next_block(&mut self, mutes: &[(usize, Mute)]) {
        self.mutes.0.clear();
        for &(i, m) in mutes {
            if self.mutes.0.len() <= i {
                self.mutes.0.resize(i + 1, 0);
            }
            self.mutes.0[i] = match m {
                Mute::Skip => 1,
                Mute::Discard => 2,
            };
        }
    }

    /// The deterministic deadline for the next graph blocks: the deadline is reached once only
    /// `held` strips (table indices) are unresolved and, with workers, each is claimed by one
    /// (`strip_pool::seam`). Empty: the real deadline. Between blocks.
    pub fn hold_until_alone(&mut self, held: &[usize]) {
        if let Some(pool) = self.pool.as_mut() {
            pool.gate.clear();
            for &i in held {
                if pool.gate.len() <= i {
                    pool.gate.resize(i + 1, false);
                }
                pool.gate[i] = true;
            }
        }
    }

    /// The table index of `track`'s strip in the active plan.
    #[must_use]
    pub fn strip_index(&self, track: TrackId) -> Option<usize> {
        self.plan.as_deref().and_then(|p| p.tables.strip_of(track))
    }

    /// The active plan's phase-1 strips and auxes: (phase 1's end, the auxes' end).
    #[must_use]
    pub fn strip_phases(&self) -> (usize, usize) {
        self.plan.as_deref().map_or((0, 0), |p| (p.tables.phase1, p.tables.aux_end))
    }

    /// Per phase-1 strip, the last block's outcome on the pool path (`None` without a pool).
    #[must_use]
    pub fn strip_outcomes(&self) -> Vec<Option<Outcome>> {
        let n = self.strip_phases().0;
        (0..n).map(|i| self.pool.as_ref().and_then(|p| p.outcome(i))).collect()
    }

    /// Per phase-1 strip, silent in the last block on the pool path (an overrun, a skip, a fault,
    /// a stale result) or away.
    #[must_use]
    pub fn strip_silence(&self) -> Vec<bool> {
        let n = self.strip_phases().0;
        (0..n).map(|i| self.pool.as_ref().is_some_and(|p| p.is_silent(i) || p.is_away(i))).collect()
    }

    /// Per phase-1 strip, its figures so far (counts by cause, cumulative; default without a pool).
    #[must_use]
    pub fn strip_counts(&self) -> Vec<StripFigures> {
        let n = self.strip_phases().0;
        (0..n).map(|i| self.pool.as_ref().and_then(|p| p.strip_figures(i)).unwrap_or_default()).collect()
    }

    /// Per aux, skipped in the last block by the graph's tail rule.
    #[must_use]
    pub fn tail_skips(&self) -> Vec<bool> {
        self.plan.as_deref().map_or_else(Vec::new, |p| p.tables.graph.tail_skipped.to_vec())
    }

    /// The graph path renders phase-1 strip `i` on the callback, never on a worker (it reads the
    /// live input or the hardware): a test cannot hold it.
    #[must_use]
    pub fn strip_pinned(&self, i: usize) -> bool {
        self.plan.as_deref().is_some_and(|p| {
            let st = p.tables.strips.get(i);
            st.and_then(|st| p.session.tracks.get(usize::from(st.track)).map(|t| strip_pool::pinned(t, st))).unwrap_or(false)
        })
    }

    /// A render worker holds strip `i` (claimed, not done).
    #[must_use]
    pub fn strip_claimed(&self, i: usize) -> bool {
        self.pool.as_ref().is_some_and(|p| p.claimed(i))
    }

    /// Strip `i`'s cost history becomes `cost` (its estimate and worst case). Between blocks.
    pub fn set_strip_cost(&mut self, i: usize, cost: Duration) {
        if let Some(p) = self.pool.as_ref() {
            p.set_cost(i, cost);
        }
    }

    /// Late results the pool retired unread so far.
    #[must_use]
    pub fn late_results(&self) -> u64 {
        self.pool.as_ref().map_or(0, strip_pool::StripPool::late_results)
    }
}
