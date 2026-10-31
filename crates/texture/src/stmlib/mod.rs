//! The stmlib helpers the texture processor uses (`stmlib/dsp/*.h`, `stmlib/utils/*.h`), ported
//! from Émilie Gillet's MIT stmlib at `e3bd7c9c` with every arithmetic result flushed to zero
//! ([`F`]).

#![forbid(unsafe_code)]

pub mod cosine_oscillator;
pub mod delay_line;
pub mod filter;
pub mod hysteresis;
pub mod parameter_interpolator;
mod pitch_tables;
pub mod random;
pub mod sample_rate_converter;

use crate::float::F;

/// `GateFlags` bits (`stmlib/utils/gate_flags.h`): the gate is high.
pub const GATE_FLAG_HIGH: u8 = 1;
/// The gate rose on this sample.
pub const GATE_FLAG_RISING: u8 = 2;
/// The gate fell on this sample.
pub const GATE_FLAG_FALLING: u8 = 4;

/// `ExtractGateFlags(previous, current)`: the edge flags of a gate from its previous flags.
#[inline(always)]
#[must_use]
pub fn extract_gate_flags(previous: u8, current: bool) -> u8 {
    let previous = previous & GATE_FLAG_HIGH;
    match (current, previous != 0) {
        (true, true) => GATE_FLAG_HIGH,
        (true, false) => GATE_FLAG_RISING | GATE_FLAG_HIGH,
        (false, true) => GATE_FLAG_FALLING,
        (false, false) => 0,
    }
}

/// `MAKE_INTEGRAL_FRACTIONAL(x)`: the truncated integer part and the remainder.
#[inline(always)]
#[must_use]
pub fn integral_fractional(x: F) -> (i32, F) {
    let integral = x.trunc_i32();
    (integral, x - F::from_i32(integral))
}

/// A table index from a truncated integer the firmware keeps in range. An out-of-range index (an
/// out-of-bounds read in the C++, which no input of the port can reach) is clamped to the table.
#[inline(always)]
pub(crate) fn index(i: i32, len: usize) -> usize {
    usize::try_from(i).unwrap_or(0).min(len - 1)
}

/// `Interpolate(table, index, size)`: linear interpolation at `index * size`.
#[inline(always)]
#[must_use]
pub fn interpolate(table: &[f32], index_: F, size: f32) -> F {
    let (integral, fractional) = integral_fractional(index_ * size);
    let a = F(table[index(integral, table.len())]);
    let b = F(table[index(integral + 1, table.len())]);
    a + (b - a) * fractional
}

/// `SmoothStep(x)`: `x² (3 - 2x)`.
#[inline(always)]
#[must_use]
pub fn smooth_step(x: F) -> F {
    x * x * (3.0 - 2.0 * x)
}

/// `Crossfade(a, b, fade)`.
#[inline(always)]
#[must_use]
pub fn crossfade(a: F, b: F, fade: F) -> F {
    a + (b - a) * fade
}

/// `SoftLimit(x)`: `x (27 + x²) / (27 + 9 x²)`.
#[inline(always)]
#[must_use]
pub fn soft_limit(x: F) -> F {
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// `SoftClip(x)`: [`soft_limit`] within plus or minus 3, saturated at plus or minus 1 beyond.
#[inline(always)]
#[must_use]
pub fn soft_clip(x: F) -> F {
    if x < -3.0 {
        F(-1.0)
    } else if x > 3.0 {
        F(1.0)
    } else {
        soft_limit(x)
    }
}

/// `ONE_POLE(out, in, coefficient)`.
#[inline(always)]
pub fn one_pole(out: &mut F, input: F, coefficient: F) {
    *out += coefficient * (input - *out);
}

/// `SLOPE(out, in, positive, negative)`: the coefficient picked by the sign of the error
/// multiplies the error.
#[inline(always)]
pub fn slope(out: &mut F, input: F, positive: F, negative: F) {
    let error = input - *out;
    *out += (if error > 0.0 { positive } else { negative }) * error;
}

/// `CONSTRAIN(var, min, max)`.
#[inline(always)]
#[must_use]
pub fn constrain(x: F, min: F, max: F) -> F {
    if x < min {
        min
    } else if x > max {
        max
    } else {
        x
    }
}

/// `Clip16(x)`.
#[inline(always)]
#[must_use]
pub fn clip16(x: i32) -> i32 {
    x.clamp(-32768, 32767)
}

/// `SemitonesToRatio(semitones)`: two table lookups, valid for -128 to 127 semitones.
#[inline(always)]
#[must_use]
pub fn semitones_to_ratio(semitones: F) -> F {
    let (integral, fractional) = integral_fractional(semitones + 128.0);
    let high = &pitch_tables::LUT_PITCH_RATIO_HIGH;
    let low = &pitch_tables::LUT_PITCH_RATIO_LOW;
    F(high[index(integral, high.len())]) * F(low[index((fractional * 256.0).trunc_i32(), low.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semitones_to_ratio_matches_the_octave_points() {
        assert_eq!(semitones_to_ratio(F(0.0)), F(1.0));
        assert_eq!(semitones_to_ratio(F(12.0)), F(2.0));
        assert_eq!(semitones_to_ratio(F(-24.0)), F(0.25));
        assert_eq!(semitones_to_ratio(F(48.0)), F(16.0));
    }

    #[test]
    fn soft_clip_saturates_beyond_three() {
        assert_eq!(soft_clip(F(0.0)), F(0.0));
        assert_eq!(soft_clip(F(3.5)), F(1.0));
        assert_eq!(soft_clip(F(-4.0)), F(-1.0));
        assert_eq!(soft_clip(F(3.0)), F(1.0));
    }

    #[test]
    fn gate_flags_tag_the_edges() {
        assert_eq!(extract_gate_flags(0, true), 3);
        assert_eq!(extract_gate_flags(3, true), 1);
        assert_eq!(extract_gate_flags(1, false), 4);
        assert_eq!(extract_gate_flags(4, false), 0);
    }
}
