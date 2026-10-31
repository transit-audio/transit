//! The stmlib helpers the meta-modulator uses (`stmlib/dsp/*.h`, `stmlib/utils/random.h`), ported
//! from Émilie Gillet's MIT stmlib at `e3bd7c9c` with every arithmetic result flushed to zero
//! ([`F`]).

#![forbid(unsafe_code)]

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

/// A table index from a truncated integer the firmware keeps in range. An out-of-range index (an
/// out-of-bounds read in the C++, which no input of the port can reach) is clamped to the table.
#[inline(always)]
pub(crate) fn index(i: i32, len: usize) -> usize {
    usize::try_from(i).unwrap_or(0).min(len - 1)
}

/// `Interpolate(table, index, size)`: linear interpolation at `index * size`, for indices the
/// firmware keeps inside the table.
#[inline(always)]
#[must_use]
pub fn interpolate(table: &[f32], index_: F, size: f32) -> F {
    let (integral, fractional) = integral_fractional(index_ * size);
    let a = F(table[index(integral, table.len())]);
    let b = F(table[index(integral + 1, table.len())]);
    a + (b - a) * fractional
}

/// `Interpolate(table + base, index, size)` with both reads clamped to the table and the scaled
/// index bounded to plus or minus 65536 before its conversion: the oracle's
/// `TransitInterpolateClamped` (`patches/all/0010` of `transit-macro-osc-oracle`). Where the
/// upstream reads stay inside the table this is `Interpolate()` exactly; a read past an end returns
/// that end's value.
#[inline(always)]
#[must_use]
pub fn interpolate_clamped(table: &[f32], base: i32, index_: F, size: f32) -> F {
    let mut x = index_ * size;
    if x > 65_536.0 {
        x = F(65_536.0);
    }
    if x < -65_536.0 {
        x = F(-65_536.0);
    }
    let (integral, fractional) = integral_fractional(x);
    let last = i32::try_from(table.len()).unwrap_or(i32::MAX) - 1;
    let i = (base + integral).clamp(0, last);
    let j = (base + integral + 1).clamp(0, last);
    let a = F(table[index(i, table.len())]);
    let b = F(table[index(j, table.len())]);
    a + (b - a) * fractional
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
    }

    #[test]
    fn the_clamped_interpolation_is_interpolate_inside_and_holds_the_ends_outside() {
        let table = [0.0_f32, 1.0, 4.0, 9.0, 16.0];
        for x in [0.0_f32, 0.3, 0.55, 0.74] {
            assert_eq!(interpolate_clamped(&table, 0, F(x), 4.0), interpolate(&table, F(x), 4.0));
        }
        // Reaching the last entry exactly reads one past it at a zero fraction: the end holds.
        assert_eq!(interpolate_clamped(&table, 0, F(1.0), 4.0), F(16.0));
        assert_eq!(interpolate_clamped(&table, 0, F(1.7), 4.0), F(16.0));
        assert_eq!(interpolate_clamped(&table, 2, F(-1.0), 4.0), F(0.0));
        // Within (-1, 0) the index truncates toward zero and the fraction is negative, as in
        // `Interpolate()`: it extrapolates from the entry at the base.
        assert_eq!(interpolate_clamped(&table, 2, F(-0.125), 4.0), F(1.5));
        assert_eq!(interpolate_clamped(&table, 2, F(1e30), 4.0), F(16.0));
    }

    #[test]
    fn soft_clip_saturates_beyond_three() {
        assert_eq!(soft_clip(F(0.0)), F(0.0));
        assert_eq!(soft_clip(F(3.5)), F(1.0));
        assert_eq!(soft_clip(F(-4.0)), F(-1.0));
    }
}
