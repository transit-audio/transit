//! The monitoring switch's ramp (#730): when an audio track starts or stops hearing its live input
//! (Audio > Monitor Armed Tracks, the track's Input Monitoring button or its arm changed, or the
//! transport moved between stopped, playing and recording) the strip crossfades between its input
//! and what it plays otherwise (its clips, or silence while stopped) instead of switching in one
//! sample.
//!
//! The gate is the hand gate of ADR 0058 decision 6, applied to the source switch: an integer
//! counter kept in the strip, stepping by one per rendered sample toward its target (`N_s` when
//! the track hears its input, 0 when it does not), over the switch ramp length `N_s` of decision 9
//! (5 ms, `(rate · 5 + 500) div 1000` samples). The input's weight at a sample is `c / N_s`, the
//! other source's `1 − c / N_s`, each computed in `f64` and rounded once to `f32`. A switch seen
//! at a block's start gives `(N_s − 1) / N_s` (or `1 / N_s`) on that block's first sample and
//! reaches its target on the `N_s`-th; a switch reversed mid-ramp turns back from where it is at
//! the same slope. The weight at a sample depends only on the counter at the block's start and the
//! offset, so for the same switches at the same samples the output does not depend on the block
//! size. These rounded `f32` weights are the monitor source crossfade only: they must not be
//! reused as factors in the mixer-wide `f64` gain expression of ADR 0058 (decisions 1 and 6),
//! whose gate stays `f64` until the stage's one rounding.
//!
//! **What survives what** (ADR 0058 decision 15's table for the hand gate): the counter lives
//! with the strip and moves with it into a new plan (`move_strip`); Stop, a recording's start and
//! end, seeks and loop wraps keep it, so a ramp in progress continues and a switch they cause
//! ramps like any other. A strip that has not rendered yet, or whose rate changed, starts at its
//! target with no ramp.
//!
//! **Playback start** (ADR 0058 is silent: its gates do not change target when the transport
//! starts). When the transport leaves stopped while the gate rests fully on the input and the
//! track no longer hears it (an armed track monitored while stopped, Monitor Armed Tracks on, and
//! Play), the gate goes to its target at once, so the track's clip sounds at full level from its
//! first sample, as it always has. Any other moment of playback start (a ramp already moving, a
//! track that keeps hearing its input) is not touched.
//!
//! **A frozen track** never hears its input ([`MonitorGate::exclude_input`]): the gate drops to 0
//! at once, so a track frozen while it was monitoring stops reading the input on the first sample
//! the freeze reaches it.
//!
//! **Clips during a ramp** (and their stateful clip effects) are processed for exactly the samples
//! where the clip's weight is not zero ([`GateBlock::clip_frames`]): a ramp toward the input ends
//! the clip render on the sample the weight reaches zero, whatever block that sample falls in, so
//! the clip effects' state does not depend on the block size.
//!
//! At rest (the counter at either end) the source is written exactly as before the ramp existed.
//! Audio thread: plain integers, no allocation, no lock.

/// The switch ramp length in samples at `rate` Hz: 5 ms rounded half up, in integers (ADR 0058
/// decision 9, `N_s`), at least 1.
pub(crate) fn switch_samples(rate: u32) -> u32 {
    let n = (u64::from(rate) * 5 + 500) / 1000;
    u32::try_from(n).unwrap_or(u32::MAX).max(1)
}

/// One strip's monitoring gate (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MonitorGate {
    /// Samples toward the input at the end of the last block (0 ..= `len`).
    count: u32,
    /// The ramp length the counter runs over.
    len: u32,
    /// The strip has rendered a block, so the counter holds a real position.
    primed: bool,
    /// The last block was rendered with the transport stopped (`monitor_only`).
    was_stopped: bool,
}

/// The gate for one block: where the counter starts and where it heads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GateBlock {
    from: u32,
    on: bool,
    len: u32,
}

impl MonitorGate {
    /// Starts a block of `frames` samples in which the track hears its input (`on`) or not, with
    /// the transport `stopped` or not and the switch ramp `len` samples long, and advances the
    /// counter past it.
    pub(crate) fn begin(&mut self, on: bool, stopped: bool, len: u32, frames: usize) -> GateBlock {
        let len = len.max(1);
        if !self.primed || self.len != len {
            // A new strip (or a new rate) starts at its target: nothing to ramp from.
            self.count = if on { len } else { 0 };
            self.len = len;
            self.primed = true;
        } else if self.was_stopped && !stopped && !on && self.count >= len {
            // Playback starts on a track that rested on its input: its clip from the first sample.
            self.count = 0;
        }
        self.was_stopped = stopped;
        let block = GateBlock { from: self.count.min(len), on, len };
        if frames > 0 {
            self.count = block.count(frames - 1);
        }
        block
    }

    /// A frozen track never hears its input: the gate goes to 0 at once, before the block's
    /// [`MonitorGate::begin`] (which then rests at 0).
    pub(crate) fn exclude_input(&mut self) {
        self.count = 0;
    }

    /// Test seam (feature `testing`, #742 section 14): the counter, the ramp length, whether the
    /// strip has rendered and whether the last block was stopped.
    #[cfg(feature = "testing")]
    pub(crate) fn record(&self, w: &mut transit_state_record::Writer<'_>) {
        let MonitorGate { count, len, primed, was_stopped } = self;
        w.u32(*count);
        w.u32(*len);
        w.bool(*primed);
        w.bool(*was_stopped);
    }

    /// Test seam (feature `testing`): moves the counter one sample (toward the input, or back
    /// from it at the top).
    #[cfg(feature = "testing")]
    pub(crate) fn nudge(&mut self) {
        self.count = if self.count < self.len { self.count + 1 } else { self.count.saturating_sub(1) };
    }
}

impl GateBlock {
    /// The counter at `offset` samples into the block.
    fn count(&self, offset: usize) -> u32 {
        let steps = u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(1);
        if self.on { self.from.saturating_add(steps).min(self.len) } else { self.from.saturating_sub(steps) }
    }

    /// The whole block is at the input (`Some(true)`) or at the other source (`Some(false)`);
    /// `None` while the gate moves.
    pub(crate) fn steady(&self) -> Option<bool> {
        match (self.on, self.from) {
            (true, f) if f >= self.len => Some(true),
            (false, 0) => Some(false),
            _ => None,
        }
    }

    /// How many of the block's first `frames` samples give the clips a weight above zero: the
    /// clips (and their effects) are rendered for exactly these. All of them unless the gate
    /// heads for the input; then the samples before the counter reaches it.
    pub(crate) fn clip_frames(&self, frames: usize) -> usize {
        if !self.on {
            return frames;
        }
        // The counter reaches `len` at offset `len − from − 1`; the clip weight is zero from there.
        let left = self.len.saturating_sub(self.from).saturating_sub(1);
        frames.min(usize::try_from(left).unwrap_or(usize::MAX))
    }

    /// The input's weight at `offset` samples into the block (the other source's is `1 − w`,
    /// [`GateBlock::other`]).
    pub(crate) fn input(&self, offset: usize) -> f32 {
        (f64::from(self.count(offset)) / f64::from(self.len)) as f32
    }

    /// The other source's weight at `offset` samples into the block.
    pub(crate) fn other(&self, offset: usize) -> f32 {
        (f64::from(self.len.saturating_sub(self.count(offset))) / f64::from(self.len)) as f32
    }
}
