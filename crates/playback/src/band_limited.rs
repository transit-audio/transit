//! What the playback crate's rate converters share around the band-limited reader (ADR 0056
//! decision 8, `transit-resample`): the device-rate converter of the player (#601), the live input
//! converter of [`crate::record::InputRing`] (#601) and the System Audio converter (#603).
//!
//! **Positions.** A converter never adds `q` once per output frame. The device-rate and input
//! converters hold the position as an exact rational, `units / den` frames from an integer origin
//! (the pending render block's frame 0, the input queue's front), advanced by an integer number of
//! units per output frame and rebased by whole frames; [`position`] turns it into the reader's `f64`.
//! The System Audio converter's rate changes with its drift servo and is not rational; it forms
//! `x0 + n·q` from an anchor it moves at each change of `q` (see `system_audio.rs`).
//!
//! **Look-ahead.** The reader is linear phase: the output at `x` stands for the source at `x`, so a
//! converter whose positions are unchanged plays every source frame at the same output time as the
//! linear converter it replaces. What it needs is source frames up to [`transit_resample::reach`]`(q)` past `x`: the
//! device-rate converter renders that much further ahead of presentation, and the input converter
//! keeps that much more input queued in front of its read position (the backlog B). Both are
//! reported where those figures already are; neither moves a position on the timeline.
use core::ops::Range;
pub(crate) use transit_resample::{Frames, SincTable, direct, halo, read_frame, taps};

/// The one process-wide table. The first call builds it (64 KiB, allocating); every converter takes
/// it when it is constructed, off the audio thread, so the audio thread only reads it.
pub(crate) fn table() -> &'static SincTable {
    SincTable::shared()
}

/// Fraction bits of a converter position: [`position`] rounds the fraction down to a multiple of
/// 2^-32 frame. With an integer part below 2^21 the sum is then exact in `f64`, so the reader sees
/// the same fraction whatever integer origin the position is counted from, which keeps the output
/// independent of how the stream is cut into blocks. The rounding moves a position by less than
/// 2^-32 frame: a phase error of at most 2π·0.5·2^-32 ≈ 7.3e-10 at the source's Nyquist, about
/// -183 dB, far under the reader's own error (-73 dB).
pub(crate) const FRACTION_BITS: u32 = 32;

/// The reader's position for `units / den` frames (`den > 0`): the integer part exactly, the
/// fraction rounded down to a multiple of 2^-[`FRACTION_BITS`] (see there). The integer part must be
/// below 2^21 in magnitude for the sum to be exact; the converters keep it below the largest render
/// block or input queue (at most 2^22 samples).
pub(crate) fn position(units: i64, den: i64) -> f64 {
    let den = den.max(1);
    let index = units.div_euclid(den);
    let rem = units.rem_euclid(den) as u128;
    let fraction = ((rem << FRACTION_BITS) / den as u128) as f64 / (1u64 << FRACTION_BITS) as f64;
    index as f64 + fraction
}

/// The source frames one output frame at `x` and rate `q` reads: the direct index alone when
/// [`direct`] takes it (q exactly 1 and an integer position: bit-exact pass-through), else the
/// reader's taps. It always includes `floor(x)`, so a converter that renders or waits until the span
/// is available always makes progress, even for a rate the reader refuses (which reads silence).
pub(crate) fn span(x: f64, q: f64) -> Range<i64> {
    let floor = x.floor() as i64;
    match direct(x, q) {
        Some(i) => i..i + 1,
        None => {
            let t = taps(x, q);
            if t.is_empty() { floor..floor + 1 } else { t.start.min(floor)..t.end.max(floor + 1) }
        }
    }
}

/// One output frame of `channels` channels at `x` and rate `q` from `src` into `out`: the direct
/// index where [`direct`] takes it, else [`read_frame`].
#[inline]
pub(crate) fn frame<S: Frames>(table: &SincTable, src: &S, x: f64, q: f64, out: &mut [f32]) {
    match direct(x, q) {
        Some(i) => {
            for (channel, value) in out.iter_mut().enumerate() {
                *value = src.sample(channel, i);
            }
        }
        None => read_frame(table, src, x, q, out),
    }
}

/// The reader's look-ahead at `q`, in whole source frames: what a converter keeps (or renders) past
/// its read position, `halo(q)` (`ceil(16·max(1, q)) + 1`), and 0 where the converter passes frames
/// through (`q` exactly 1 at integer positions, [`direct`]).
pub(crate) fn lookahead(q: f64, passes_through: bool) -> usize {
    if passes_through { 0 } else { halo(q) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_position_is_the_same_fraction_from_any_integer_origin() {
        let den = 48_000_000i64;
        for units in [0i64, 1, 44_100_000, 44_100_001, 7 * 48_000_000 - 1, -1, -44_100_000 - 3] {
            let x = position(units, den);
            for origin in [-2000i64, -1, 1, 17, 4096, 100_000] {
                let shifted = position(units - origin * den, den);
                assert_eq!(x - origin as f64, shifted, "units {units} from origin {origin}");
                assert_eq!((x - x.floor()).to_bits(), (shifted - shifted.floor()).to_bits());
            }
            let exact = units as f64 / den as f64;
            assert!((x - exact).abs() < 1.0 / (1u64 << FRACTION_BITS) as f64 + 1e-12, "units {units}: {x} against {exact}");
        }
    }

    #[test]
    fn the_span_is_the_direct_frame_at_unit_rate_and_the_taps_otherwise() {
        assert_eq!(span(5.0, 1.0), 5..6);
        assert_eq!(span(5.5, 1.0), taps(5.5, 1.0));
        assert_eq!(span(5.0, 0.5), taps(5.0, 0.5));
        // A refused rate still spans the frame at the position.
        assert_eq!(span(5.25, 64.0), 5..6);
        assert_eq!(lookahead(1.0, true), 0);
        assert_eq!(lookahead(0.91875, false), 17);
        assert_eq!(lookahead(2.0, false), 33);
    }
}
