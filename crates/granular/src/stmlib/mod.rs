//! The stmlib helpers the granular processor uses (`stmlib/dsp/*.h`, `stmlib/utils/random.h`),
//! ported from Émilie Gillet's MIT stmlib at `e3bd7c9c` with every arithmetic result flushed to
//! zero ([`F`]).

#![forbid(unsafe_code)]

pub mod cosine_oscillator;
pub mod filter;
pub mod parameter_interpolator;
mod pitch_tables;
pub mod random;

use crate::float::F;

/// `MAKE_INTEGRAL_FRACTIONAL(x)`: the truncated integer part and the remainder.
#[inline(always)]
#[must_use]
pub fn integral_fractional(x: F) -> (i32, F) {
    let integral = x.trunc_i32();
    (integral, x - F::from_i32(integral))
}

/// A table index from a truncated integer the firmware assumes in range. Out-of-range indices (an
/// out-of-bounds read in the C++) are clamped to the table instead.
#[inline(always)]
fn index(i: i32, len: usize) -> usize {
    usize::try_from(i).unwrap_or(0).min(len - 1)
}

/// `Interpolate(table, index, size)`: linear interpolation at `index * size`.
///
/// At an index of exactly 1.0 the firmware reads one entry past a table of `size + 1` entries and
/// multiplies it by a zero fraction (the grain window at its peak, size 1.0 on the module), and
/// the dry/wet ramp's rounding overshoot past 1.0 reads it at a small fraction; the port reads the
/// last entry instead, which is what the oracle reads too (its build repeats each table's last
/// entry).
#[inline(always)]
#[must_use]
pub fn interpolate(table: &[f32], index_: F, size: f32) -> F {
    let (integral, fractional) = integral_fractional(index_ * size);
    let a = F(table[index(integral, table.len())]);
    let b = F(table[index(integral + 1, table.len())]);
    a + (b - a) * fractional
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

/// `SoftConvert(x)`: `Clip16(int32_t(SoftLimit(x * 0.5) * 32768))`.
#[inline(always)]
#[must_use]
pub fn soft_convert(x: F) -> i16 {
    let clipped = clip16((soft_limit(x * 0.5) * 32768.0).trunc_i32());
    i16::try_from(clipped).unwrap_or(0)
}

/// `fast_rsqrt_carmack(x)` (`stmlib/dsp/rsqrt.h`): the bit-trick estimate and one Newton step.
#[inline(always)]
#[must_use]
pub fn fast_rsqrt_carmack(x: F) -> F {
    let i = 0x5f37_59df_u32.wrapping_sub(x.0.to_bits() >> 1);
    let y = F(f32::from_bits(i));
    let x2 = x * 0.5;
    y * (1.5 - x2 * y * y)
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
    fn the_rsqrt_estimate_is_within_a_fraction_of_a_percent() {
        for x in [1.0_f32, 2.0, 7.5, 31.0, 63.0] {
            let y = fast_rsqrt_carmack(F(x)).0;
            assert!((y - 1.0 / x.sqrt()).abs() < 2e-3 / x.sqrt(), "{x}: {y}");
        }
    }

    #[test]
    fn soft_convert_saturates_at_the_int16_limits() {
        assert_eq!(soft_convert(F(0.0)), 0);
        assert_eq!(soft_convert(F(100.0)), 32767);
        assert_eq!(soft_convert(F(-100.0)), -32768);
        assert_eq!(soft_convert(F(f32::NAN)), -32768);
    }
}
