//! The monitoring switch's ramp (#730): when an audio track starts or stops hearing its live input
//! because Audio > Monitor Armed Tracks, the track's Input Monitoring button or its arm changed,
//! the strip crossfades between its input and what it plays otherwise (its clips, or silence while
//! stopped) instead of switching in one sample. A switch that comes with the transport moving
//! between stopped, playing and recording is not ramped: it switches on the block's first sample
//! as it always has, so a clip still starts on its first sample when playback starts.
//!
//! The gate is the hand gate of ADR 0058 decision 6, applied to the source switch: an integer
//! counter kept in the strip, stepping by one per rendered sample toward its target (`N_s` when
//! the track hears its input, 0 when it does not), over the switch ramp length `N_s` of decision 9
//! (5 ms, `(rate · 5 + 500) div 1000` samples). The input's weight at a sample is `c / N_s`, the
//! other source's `1 − c / N_s`, each computed in `f64` and rounded once. A switch seen at a
//! block's start gives `(N_s − 1) / N_s` (or `1 / N_s`) on that block's first sample and reaches
//! its target on the `N_s`-th; a switch reversed mid-ramp turns back from where it is at the same
//! slope. The weight at a sample depends only on the counter at the block's start and the offset,
//! so for the same switches at the same samples the output does not depend on the block size.
//! A strip that has not rendered yet, and a block whose transport state (stopped, recording)
//! differs from the last block's, start at the target with no ramp. At rest (the counter at either
//! end) the source is written exactly as before the ramp existed.
//!
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
    /// The last block's transport state: [`Transport::bits`].
    transport: u8,
}

/// The transport state a switch is ramped within: stopped (`monitor_only`) and recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Transport {
    pub(crate) stopped: bool,
    pub(crate) recording: bool,
}

impl Transport {
    fn bits(self) -> u8 {
        u8::from(self.stopped) | (u8::from(self.recording) << 1)
    }
}

/// The gate for one block: where the counter starts and where it heads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GateBlock {
    from: u32,
    on: bool,
    len: u32,
}

impl MonitorGate {
    /// Starts a block of `frames` samples under `transport` in which the track hears its input
    /// (`on`) or not, with the switch ramp `len` samples long, and advances the counter past it.
    pub(crate) fn begin(&mut self, on: bool, transport: Transport, len: u32, frames: usize) -> GateBlock {
        let len = len.max(1);
        let bits = transport.bits();
        if !self.primed || self.len != len || self.transport != bits {
            // A new strip (or a new rate) starts at its target: nothing to ramp from. A transport
            // change switches at once, as it always has (see the module docs).
            self.count = if on { len } else { 0 };
            self.len = len;
            self.primed = true;
            self.transport = bits;
        }
        let block = GateBlock { from: self.count.min(len), on, len };
        if frames > 0 {
            self.count = block.count(frames - 1);
        }
        block
    }

    /// Test seam (feature `testing`, #742 section 14): the counter, the ramp length, whether the
    /// strip has rendered and the last block's transport state.
    #[cfg(feature = "testing")]
    pub(crate) fn record(&self, w: &mut transit_state_record::Writer<'_>) {
        let MonitorGate { count, len, primed, transport } = self;
        w.u32(*count);
        w.u32(*len);
        w.bool(*primed);
        w.u8(*transport);
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
