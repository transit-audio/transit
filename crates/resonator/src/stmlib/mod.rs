//! The stmlib helpers the resonator uses (`stmlib/dsp/*.h`, `stmlib/utils/random.h`), ported from
//! Émilie Gillet's MIT stmlib at `e3bd7c9c` with every arithmetic result flushed to zero ([`F`]).

#![forbid(unsafe_code)]

pub mod cosine_oscillator;
pub mod delay_line;
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

/// `SLEW(out, in, delta)`: the error clamped to `±delta`.
#[inline(always)]
pub fn slew(out: &mut F, input: F, delta: F) {
    let mut error = input - *out;
    if error > delta {
        error = delta;
    } else if error < -delta {
        error = -delta;
    }
    *out += error;
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
        assert!((semitones_to_ratio(F(7.0)).0 - 1.498_307).abs() < 1e-5);
    }

    #[test]
    fn slope_and_slew_follow_the_macros() {
        let mut x = F(0.0);
        slope(&mut x, F(1.0), F(0.5), F(0.1));
        assert_eq!(x, F(0.5));
        slope(&mut x, F(0.0), F(0.5), F(0.1));
        assert_eq!(x, F(0.45));
        let mut y = F(0.0);
        slew(&mut y, F(1.0), F(0.25));
        assert_eq!(y, F(0.25));
        slew(&mut y, F(-1.0), F(0.25));
        assert_eq!(y, F(0.0));
    }
}
