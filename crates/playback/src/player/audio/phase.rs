//! Converter phase in integer units. The denominator stays fixed for one device
//! incarnation, so callback partitions and speed changes cannot round its phase.
//!
//! The device-rate converter reads the rendered stream through the band-limited reader (ADR 0056
//! decision 8, #601) at `units / denominator` render frames from the pending block's frame 0. The
//! position is exact: it moves by `step` units per output frame and by whole frames when a block is
//! removed, so it is `x0 + n·q` with nothing accumulated in floating point, and a speed change
//! (a new `step`) continues from the same position with no discontinuity. The next block is
//! rendered as soon as an output frame's taps reach past the pending block's last frame (#525);
//! [`History`] keeps the frames before the pending block that the taps still reach. The output is
//! a function of the rendered stream alone, whatever the render and callback partitions.
use crate::band_limited::{self, Frames, SincTable};
use crate::timeline_map::MapError;
use transit_mix::RenderFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Phase {
    /// Position of the next output frame from render frame 0 of the pending block, in units of
    /// `1 / denominator` frames. Never at or below `-HISTORY` frames (what [`History`] keeps).
    units: i64,
    denominator: i64,
}

impl Phase {
    pub(super) fn new(device_rate: u32) -> Result<Self, MapError> {
        if device_rate == 0 {
            return Err(MapError::Invalid);
        }
        Ok(Self { units: 0, denominator: i64::from(device_rate) * 1000 })
    }

    pub(super) fn reset(&mut self) {
        self.units = 0;
    }

    /// The pending block has been read to its end (the next output frame is past its last frame).
    #[cfg(test)]
    pub(super) fn covers(self, frames: usize) -> bool {
        i128::from(self.units) >= frames as i128 * i128::from(self.denominator)
    }

    /// The next output frame's position from the pending block's frame 0, and the reader's rate at
    /// `step` (render frames per output frame).
    pub(super) fn position(self, step: u64) -> (f64, f64) {
        (band_limited::position(self.units, self.denominator), self.rate(step))
    }

    /// Render frames per output frame at `step`.
    pub(super) fn rate(self, step: u64) -> f64 {
        step as f64 / self.denominator as f64
    }

    /// The next output frame reads a frame after the pending block's last: render the next block
    /// first. At unit rate and a whole-frame position only that frame is read; otherwise every
    /// tap of the reader must be rendered.
    pub(super) fn needs(self, frames: usize, step: u64) -> bool {
        let (x, q) = self.position(step);
        band_limited::span(x, q).end > frames as i64
    }

    /// Move the origin to the next block's frame 0, `frames` after the pending block's. Only after
    /// [`Phase::needs`] said so: the phase then stays within the reader's reach of the new origin,
    /// far above `-HISTORY` frames.
    pub(super) fn remove(&mut self, frames: usize) -> Result<(), MapError> {
        let units = i64::try_from(frames).ok().and_then(|frames| frames.checked_mul(self.denominator)).ok_or(MapError::Arithmetic)?;
        let floor = (HISTORY as i64).checked_mul(self.denominator).ok_or(MapError::Arithmetic)?;
        let left = self.units.checked_sub(units).filter(|left| *left > -floor).ok_or(MapError::Arithmetic)?;
        self.units = left;
        Ok(())
    }

    pub(super) fn advance(&mut self, step: u64) -> Result<(), MapError> {
        let step = i64::try_from(step).map_err(|_| MapError::Arithmetic)?;
        self.units = self.units.checked_add(step).ok_or(MapError::Arithmetic)?;
        Ok(())
    }

    /// The render frame at or before the next output frame (negative: a frame of an earlier block,
    /// which [`History`] keeps).
    pub(super) fn index(self) -> isize {
        self.units.div_euclid(self.denominator) as isize
    }

    pub(super) fn quantum(self, output: usize, step: u64, max: usize) -> usize {
        let frames = (output as u128 * u128::from(step)).div_ceil(self.denominator as u128);
        frames.clamp(1, max.max(1) as u128) as usize
    }

    /// Output-device frames from the current output frame to render frame 0 of the next block
    /// (negative or zero: that frame is the current one or already behind it; positive: the block
    /// was rendered ahead for an output frame whose taps reach into it, by up to the reader's reach,
    /// which is how far the band-limited converter renders ahead). Render frame `k` of that block
    /// is at `lead + k * denominator / step` output frames.
    pub(super) fn lead(self, step: u64) -> f64 {
        if step == 0 { 0.0 } else { -(self.units as f64) / step as f64 }
    }

    /// Output frames per render frame at `step`.
    pub(super) fn ratio(self, step: u64) -> f64 {
        if step == 0 { 1.0 } else { self.denominator as f64 / step as f64 }
    }

    /// Map an output ordinal through the converter's current consumed phase.
    /// `next_render` is after all rendered pending frames; it is not the output
    /// playhead. Past events retain their past due frame so the mixer counts them
    /// as late, rather than adding another block of latency at dispatch.
    pub(super) fn project(
        self,
        next_render: RenderFrame,
        pending: usize,
        current_output: u64,
        output: u64,
        step: u64,
    ) -> Result<RenderFrame, MapError> {
        let pending = u64::try_from(pending).map_err(|_| MapError::Arithmetic)?;
        let origin = next_render.0.checked_sub(pending).ok_or(MapError::Arithmetic)?;
        let delta = i128::from(output) - i128::from(current_output);
        let units = delta.checked_mul(i128::from(step)).and_then(|delta| delta.checked_add(i128::from(self.units))).ok_or(MapError::Arithmetic)?;
        let frames = units.checked_add(i128::from(self.denominator / 2)).ok_or(MapError::Arithmetic)?.div_euclid(i128::from(self.denominator));
        let frame = i128::from(origin).checked_add(frames).ok_or(MapError::Arithmetic)?;
        // Before engine frame zero is necessarily late. Do not wrap it forward.
        Ok(RenderFrame(u64::try_from(frame.max(0)).map_err(|_| MapError::Arithmetic)?))
    }
}

/// Channels the converter keeps frames of: every channel a plan can render (Main's, then the
/// output sinks', ADR 0015).
pub(super) const HOLD_CHANNELS: usize = transit_mix::MAX_CHANNELS + transit_mix::MAX_SINK_CHANNELS;

/// Frames [`History`] keeps before the pending block: twice the reader's halo at its largest rate
/// (`2 · halo(32)`, 1,026). When the next block is needed the position is at most one reach plus a
/// frame before the pending block's end, and its taps reach one more reach back.
pub(super) const HISTORY: usize = 2 * (16 * transit_resample::MAX_RATE as usize + 1);

/// The frames before the pending block that the converter's taps still reach (#525, #601): the
/// last [`HISTORY`] rendered frames of each channel, in a ring. Fixed storage, allocated with the
/// player and written only on the audio thread.
pub(super) struct History {
    samples: Box<[f32]>,
    /// The ring index the next kept frame goes to.
    head: usize,
    /// Channels written since the last reset: the rest hold zeros.
    width: usize,
    /// Output frames whose taps reached outside the kept and the pending frames (they read zeros
    /// there). Never expected: the converter renders before a read needs it and keeps enough.
    misses: u64,
    table: &'static SincTable,
}

impl History {
    /// Allocates; on the control thread, with the player (the reader's shared table too).
    pub(super) fn new() -> Self {
        Self { samples: vec![0.0; HOLD_CHANNELS * HISTORY].into_boxed_slice(), head: 0, width: 0, misses: 0, table: band_limited::table() }
    }

    /// Forget the kept frames: the stream restarts (a start, a locate, a stop, the transport's end).
    pub(super) fn reset(&mut self) {
        self.samples[..self.width * HISTORY].fill(0.0);
        self.head = 0;
    }

    /// Keep the `frames` rendered frames of `block` before the next block overwrites them (the last
    /// [`HISTORY`] of them). Channels the block does not have are silence, never frames kept from an
    /// older layout.
    pub(super) fn keep(&mut self, block: &[Vec<f32>], frames: usize) {
        let width = block.len().min(HOLD_CHANNELS);
        if width < self.width {
            self.samples[width * HISTORY..self.width * HISTORY].fill(0.0);
        }
        self.width = width;
        let skip = frames.saturating_sub(HISTORY);
        let count = frames - skip;
        for (channel, data) in block.iter().take(width).enumerate() {
            let ring = &mut self.samples[channel * HISTORY..(channel + 1) * HISTORY];
            let mut at = self.head;
            for k in skip..frames {
                ring[at] = data.get(k).copied().unwrap_or(0.0);
                at = if at + 1 == HISTORY { 0 } else { at + 1 };
            }
        }
        self.head = (self.head + count) % HISTORY;
    }

    /// The converter's output frame at `x` render frames from the pending block's frame 0 and rate
    /// `q`, of the `pending` rendered frames in `block`, into `out` (one value per channel; channels
    /// past the block's read 0). A whole-frame position at unit rate is that frame exactly
    /// (bit-exact pass-through at equal rates); otherwise the reader, decision 8.3.
    pub(super) fn frame(&mut self, block: &[Vec<f32>], pending: usize, x: f64, q: f64, out: &mut [f32]) {
        let span = band_limited::span(x, q);
        if span.start < -(HISTORY as i64) || span.end > pending as i64 {
            self.misses += 1;
        }
        let stream = Stream { history: self, block, pending, channels: block.len().min(out.len()) };
        band_limited::frame(self.table, &stream, x, q, out);
    }

    #[cfg(test)]
    pub(super) fn misses(&self) -> u64 {
        self.misses
    }
}

/// The rendered stream as the reader sees it: the pending block from frame 0, the kept frames
/// before it at negative indices, zeros anywhere else.
struct Stream<'a> {
    history: &'a History,
    block: &'a [Vec<f32>],
    pending: usize,
    channels: usize,
}

impl Frames for Stream<'_> {
    fn channels(&self) -> usize {
        self.channels
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        if frame >= 0 {
            let k = frame as usize;
            return if k < self.pending { self.block.get(channel).and_then(|c| c.get(k)).copied().unwrap_or(0.0) } else { 0.0 };
        }
        let back = frame.unsigned_abs() as usize;
        if back > HISTORY || channel >= self.history.width {
            return 0.0;
        }
        let at = if self.history.head >= back { self.history.head - back } else { self.history.head + HISTORY - back };
        self.history.samples[channel * HISTORY + at]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn projection_uses_consumed_phase_not_the_render_ahead_boundary() {
        let mut phase = Phase::new(48_000).unwrap();
        phase.advance(44_100 * 1000 * 3).unwrap();
        assert_eq!(phase.index(), 2);
        assert_eq!(phase.project(RenderFrame(512), 512, 3, 480, 44_100 * 1000).unwrap(), RenderFrame(441));
        assert_eq!(phase.project(RenderFrame(512), 512, 3, 0, 44_100 * 1000).unwrap(), RenderFrame(0));
    }

    #[test]
    fn projection_is_independent_of_render_partition_and_retains_large_ordinals() {
        for device in [44_100, 48_000, 96_000] {
            for session in [44_100, 48_000, 96_000] {
                for quantum in [1, 17, 64, 127, 512] {
                    let step = session * 1000;
                    let mut phase = Phase::new(device).unwrap();
                    let origin = 1_u64 << 56;
                    let mut next = origin;
                    let mut pending = 0;
                    for output in 0..4000 {
                        while phase.needs(pending, step) {
                            phase.remove(pending).unwrap();
                            pending = quantum;
                            next += quantum as u64;
                        }
                        let expected = origin + (4096 * step + u64::from(device) * 500) / (u64::from(device) * 1000);
                        assert_eq!(phase.project(RenderFrame(next), pending, output, 4096, step).unwrap(), RenderFrame(expected));
                        phase.advance(step).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn speed_change_keeps_the_existing_fraction() {
        let mut phase = Phase::new(48_000).unwrap();
        phase.advance(44_100 * 1000).unwrap();
        phase.advance(44_100 * 1250).unwrap();
        assert_eq!(phase.units, 99_225_000);
        assert_eq!(phase.project(RenderFrame(512), 512, 2, 4, 44_100 * 1250).unwrap(), RenderFrame(4));
        phase.reset();
        assert_eq!(phase.index(), 0);
        assert_eq!(phase.position(48_000_000), (0.0, 1.0));
        assert_eq!(Phase::new(0), Err(MapError::Invalid));
    }

    #[test]
    fn the_next_block_is_needed_exactly_when_a_read_passes_the_last_frame() {
        let unit = 48_000 * 1000;
        let mut phase = Phase::new(48_000).unwrap();
        // Fresh: nothing is pending.
        assert!(phase.needs(0, unit));
        phase.advance(unit * 3).unwrap();
        // Equal rates: on the last of four frames only that frame is read.
        assert!(!phase.needs(4, unit));
        phase.advance(unit).unwrap();
        assert!(phase.needs(4, unit));
        // A fractional position reads its taps: 16 frames each side of it (#601).
        let mut phase = Phase::new(48_000).unwrap();
        phase.advance(1).unwrap();
        assert!(phase.needs(16, unit), "taps up to frame 16");
        assert!(!phase.needs(17, unit));
        // Removing the block leaves the position at most the reader's reach before the new origin.
        phase.advance(unit * 1000).unwrap();
        assert!(phase.needs(1016, unit));
        phase.remove(1016).unwrap();
        assert_eq!(phase.index(), -16);
        // A removal that would leave the phase a whole history before the new origin is refused.
        let mut early = Phase::new(48_000).unwrap();
        assert_eq!(early.remove(HISTORY), Err(MapError::Arithmetic));
        assert!(early.remove(HISTORY - 1).is_ok());
    }

    #[test]
    fn the_history_ring_keeps_the_last_frames_of_every_channel_in_order() {
        let mut history = History::new();
        let block = vec![(0..300).map(|n| n as f32).collect::<Vec<_>>(), (0..300).map(|n| -(n as f32)).collect()];
        for _ in 0..5 {
            history.keep(&block, 300);
        }
        let stream = Stream { history: &history, block: &block, pending: 0, channels: 2 };
        // Five blocks of 300: the last HISTORY frames, the newest at -1.
        for back in 1..=HISTORY as i64 {
            let expected = ((1500 - back) % 300) as f32;
            assert_eq!(stream.sample(0, -back), expected, "frame -{back}");
            assert_eq!(stream.sample(1, -back), -expected, "frame -{back}");
        }
        assert_eq!(stream.sample(0, -(HISTORY as i64) - 1), 0.0);
        // A narrower block leaves silence on the channels it lacks, never an older layout's frames,
        // also once a wider block comes back.
        history.keep(&block[..1], 1);
        let stream = Stream { history: &history, block: &block, pending: 0, channels: 2 };
        assert_eq!(stream.sample(1, -2), 0.0);
        history.keep(&block, 1);
        let stream = Stream { history: &history, block: &block, pending: 0, channels: 2 };
        assert_eq!(stream.sample(1, -1), 0.0, "the new block's frame 0");
        for back in 2..=HISTORY as i64 {
            assert_eq!(stream.sample(1, -back), 0.0, "frame -{back}: an older layout's frame");
        }
        history.reset();
        let stream = Stream { history: &history, block: &block, pending: 0, channels: 2 };
        assert_eq!(stream.sample(0, -1), 0.0);
    }
}
