//! Pending test overrides, installed once per host block (not once per sub-block).

use super::WorkerSet;
use std::time::{Duration, Instant};

/// Cost readout for callback aux renders. Does not change the scheduling clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CostClock {
    /// Production elapsed duration.
    #[default]
    Real,
    /// Deterministic duration for each render in this host block.
    Fixed(Duration),
}

impl CostClock {
    /// Reads a render's cost using its production stamps or the installed deterministic value.
    #[must_use]
    pub fn elapsed(self, start: Instant, end: Instant) -> Duration {
        match self {
            Self::Real => end.saturating_duration_since(start),
            Self::Fixed(cost) => cost,
        }
    }
}

/// Per-slot overrides. All default off; force-publish bypasses only the host's fit predicate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotOverrides {
    /// Substitute time left at the aux fit decision.
    pub budget_left: Option<Duration>,
    /// Force a fit; never bypass ownership, panic, suspect, pinned or window checks.
    pub force_publish: bool,
    /// Suppress only pre-deadline inline reclaim.
    pub no_reclaim: bool,
}

pub(super) struct Seams {
    pending: Box<[SlotOverrides]>,
    current: Box<[SlotOverrides]>,
    pending_clock: CostClock,
    current_clock: CostClock,
}

impl Seams {
    pub(super) fn new(slots: usize) -> Self {
        Self {
            pending: vec![SlotOverrides::default(); slots].into(),
            current: vec![SlotOverrides::default(); slots].into(),
            pending_clock: CostClock::Real,
            current_clock: CostClock::Real,
        }
    }
}

impl WorkerSet {
    /// Injects the block-opening observation of a still-claimed slot for host refusal tests.
    pub fn mark_busy(&mut self, slot: usize) {
        if let Some(busy) = self.busy.get_mut(slot) {
            *busy = true;
        }
    }
    /// Injects the retained suspect state of a completed runner fault, without a thread race.
    pub fn mark_suspect(&mut self, slot: usize) {
        if let Some(suspect) = self.shared.suspect.get(slot) {
            suspect.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// Arms next host block's slot overrides, between blocks. Does not alter the current block.
    pub fn arm_slot_overrides(&mut self, slot: usize, value: SlotOverrides) {
        if let Some(pending) = self.seams.pending.get_mut(slot) {
            *pending = value;
        }
    }

    /// Arms the next host block's callback cost readout.
    pub fn arm_cost_clock(&mut self, clock: CostClock) {
        self.seams.pending_clock = clock;
    }

    /// Host-block entry, including serial paths: expires previous overrides and installs the
    /// pending ones. Call once at `render_active` entry, before measurement settlement. Values
    /// remain fixed across all sub-block `begin_block` calls.
    pub fn install_test_seams(&mut self) {
        for (current, pending) in self.seams.current.iter_mut().zip(self.seams.pending.iter_mut()) {
            *current = std::mem::take(pending);
        }
        self.seams.current_clock = std::mem::take(&mut self.seams.pending_clock);
    }

    /// Current host block's slot overrides (off for an absent slot).
    #[must_use]
    pub fn slot_overrides(&self, slot: usize) -> SlotOverrides {
        self.seams.current.get(slot).copied().unwrap_or_default()
    }

    /// Current host block's cost clock, for the direct callback aux render.
    #[must_use]
    pub fn cost_clock(&self) -> CostClock {
        self.seams.current_clock
    }
}
