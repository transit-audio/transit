//! transit's shared instrument primitives (ADR 0041): the per-voice shaper (a multimode filter, two
//! LFOs, the amp and filter envelopes), the 16-row modulation matrix, the state-variable filter
//! sections, the LFO and the ADSR. The built-in instruments in `transit-dsp` use them, and the
//! sampler core will; they moved here from dsp unchanged (dsp re-exports them at their old paths).
//!
//! Everything is fixed-size: no allocation, clock, thread or logger; seeds and rates are inputs.

pub mod adsr;
pub mod lfo;
pub mod matrix;
pub mod shaper;
pub mod svf;
mod util;

#[cfg(test)]
mod tests {
    use crate::{lfo, matrix, shaper};

    /// The sizes dsp's instruments, their parameter tables and the panels assume.
    #[test]
    fn the_public_tables_keep_the_sizes_their_consumers_assume() {
        assert_eq!(matrix::ROWS, 16);
        assert_eq!(matrix::SOURCES.len(), 21);
        assert_eq!(matrix::NSRC, 21);
        assert_eq!(matrix::DESTINATIONS.len(), 15);
        assert_eq!(matrix::NDST, 18);
        assert_eq!(matrix::CURVES.len(), 5);
        assert_eq!(shaper::LFO_SHAPES.len(), 6);
        assert_eq!(shaper::FILTER_MODES.len(), 3);
        assert_eq!(lfo::DIVISIONS.len(), 14);
        assert_eq!(shaper::BLOCK, 12);
        assert_eq!(shaper::MOD_OCTAVES.to_bits(), 6.0f32.to_bits());
        assert_eq!(matrix::PITCH_SEMITONES.to_bits(), 24.0f32.to_bits());
        assert_eq!(matrix::RATE_OCTAVES.to_bits(), 4.0f32.to_bits());
    }
}
