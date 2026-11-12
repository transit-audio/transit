//! Converter phase in integer units. The denominator stays fixed for one device
//! incarnation, so callback partitions and speed changes cannot round its phase.
//!
//! The device-rate converter interpolates linearly between render frame `index` and `index + 1`.
//! The next block is rendered as soon as an output frame needs its first frame (#525): the phase
//! then sits before that block's frame 0, at most one render frame, and [`Hold`] keeps the
//! previous block's last frame for that interval. The output is a function of the rendered stream
//! alone, whatever the render and callback partitions.
use crate::timeline_map::MapError;
use transit_mix::RenderFrame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Phase {
    /// Position of the next output frame from render frame 0 of the pending block, in units of
    /// `1 / denominator` frames. Never at or below `-denominator` (one frame before frame 0).
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

    /// The next output frame reads a frame after the pending block's last: render the next block
    /// first. A frame exactly on the last frame (fraction zero) needs no neighbor.
    pub(super) fn needs(self, frames: usize) -> bool {
        i128::from(self.units) + i128::from(self.denominator) > frames as i128 * i128::from(self.denominator)
    }

    /// Move the origin to the next block's frame 0, `frames` after the pending block's. Only after
    /// [`Phase::needs`] said so: the phase stays above one frame before the new origin.
    pub(super) fn remove(&mut self, frames: usize) -> Result<(), MapError> {
        let units = i64::try_from(frames).ok().and_then(|frames| frames.checked_mul(self.denominator)).ok_or(MapError::Arithmetic)?;
        let left = self.units.checked_sub(units).filter(|left| *left > -self.denominator).ok_or(MapError::Arithmetic)?;
        self.units = left;
        Ok(())
    }

    pub(super) fn advance(&mut self, step: u64) -> Result<(), MapError> {
        let step = i64::try_from(step).map_err(|_| MapError::Arithmetic)?;
        self.units = self.units.checked_add(step).ok_or(MapError::Arithmetic)?;
        Ok(())
    }

    /// The render frame at or before the next output frame (`-1`: the previous block's last, which
    /// [`Hold`] keeps) and the fraction toward the frame after it. The fraction is the exact ratio
    /// of two integers rounded once, so it is the same on every system.
    pub(super) fn index(self) -> (isize, f32) {
        let index = self.units.div_euclid(self.denominator);
        let fraction = self.units.rem_euclid(self.denominator) as f64 / self.denominator as f64;
        // `units` is bounded by the pending block (at most the engine's maximum block) plus one.
        (index as isize, fraction as f32)
    }

    pub(super) fn quantum(self, output: usize, step: u64, max: usize) -> usize {
        let frames = (output as u128 * u128::from(step)).div_ceil(self.denominator as u128);
        frames.clamp(1, max.max(1) as u128) as usize
    }

    /// Output-device frames from the current output frame to render frame 0 of the next block
    /// (negative or zero: that frame is the current one or already behind it; positive, below one
    /// output frame's worth: the block was rendered for an output frame that interpolates into
    /// it). Render frame `k` of that block is at `lead + k * denominator / step` output frames.
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

/// Channels the converter keeps a frame of: every channel a plan can render (Main's, then the
/// output sinks', ADR 0015).
pub(super) const HOLD_CHANNELS: usize = transit_mix::MAX_CHANNELS + transit_mix::MAX_SINK_CHANNELS;

/// The previous block's last frame, per channel, for the output frames that interpolate from it
/// into the next block (#525). Fixed storage, written only on the audio thread.
pub(super) struct Hold {
    samples: [f32; HOLD_CHANNELS],
}

impl Hold {
    pub(super) fn new() -> Self {
        Self { samples: [0.0; HOLD_CHANNELS] }
    }

    /// Forget the kept frame: the stream restarts (a start, a locate, a stop, the transport's end).
    pub(super) fn reset(&mut self) {
        self.samples = [0.0; HOLD_CHANNELS];
    }

    /// Keep the last of `frames` rendered frames, before the next block overwrites them. Nothing
    /// was rendered (`frames == 0`): the frame kept before stays.
    pub(super) fn keep(&mut self, block: &[Vec<f32>], frames: usize) {
        let Some(last) = frames.checked_sub(1) else { return };
        // Channels the block does not have are silence, never a frame kept from an older layout.
        for (index, kept) in self.samples.iter_mut().enumerate() {
            *kept = block.get(index).and_then(|channel| channel.get(last)).copied().unwrap_or(0.0);
        }
    }

    /// The converter's output for `channel` at render frame `index` plus `fraction`, of `frames`
    /// rendered frames in `buffer`. A whole-frame position is that frame exactly (bit-exact
    /// pass-through at equal rates); otherwise linear interpolation, `a + (b - a) * fraction`.
    pub(super) fn sample(&self, buffer: Option<&Vec<f32>>, channel: usize, index: isize, fraction: f32, frames: usize) -> f32 {
        let rendered = buffer.map_or(&[][..], |buffer| buffer.get(..frames).unwrap_or(buffer.as_slice()));
        let at = |index: isize| match usize::try_from(index) {
            Ok(index) => rendered.get(index).copied(),
            Err(_) => self.samples.get(channel).copied(),
        };
        let a = at(index).unwrap_or(0.0);
        if fraction == 0.0 {
            return a;
        }
        // `Phase::needs` renders the next block before a fractional read past this one's last
        // frame, so `index + 1` is always rendered; unrendered capacity is never read.
        let b = at(index + 1).unwrap_or(a);
        a + (b - a) * fraction
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
        assert_eq!(phase.index().0, 2);
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
                        while phase.needs(pending) {
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
        assert_eq!(phase.index(), (0, 0.0));
        assert_eq!(Phase::new(0), Err(MapError::Invalid));
    }

    #[test]
    fn the_next_block_is_needed_exactly_when_a_read_passes_the_last_frame() {
        let mut phase = Phase::new(48_000).unwrap();
        // Fresh: nothing is pending.
        assert!(phase.needs(0));
        phase.advance(48_000 * 1000 * 3).unwrap();
        // On the last of four frames, fraction zero: no neighbor needed.
        assert!(!phase.needs(4));
        phase.advance(1).unwrap();
        // One unit past it: the next block's frame 0 is the neighbor.
        assert!(phase.needs(4));
        phase.remove(4).unwrap();
        let (index, fraction) = phase.index();
        assert_eq!(index, -1);
        assert!(fraction > 0.0 && fraction < 1.0);
        // A removal that would leave the phase a whole frame before the new origin is refused.
        let mut early = Phase::new(48_000).unwrap();
        assert_eq!(early.remove(1), Err(MapError::Arithmetic));
    }
}
