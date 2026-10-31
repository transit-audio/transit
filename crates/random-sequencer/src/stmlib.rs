//! The stmlib helpers the random sequencer uses (`stmlib/dsp/dsp.h`, `units.h`,
//! `hysteresis_quantizer.h`, `parameter_interpolator.h`), ported from Émilie Gillet's MIT stmlib at
//! `e3bd7c9c` with every arithmetic result flushed to zero ([`F`]).

#![forbid(unsafe_code)]

use crate::float::F;
use crate::pitch_tables;

/// `MAKE_INTEGRAL_FRACTIONAL(x)`: the truncated integer part and the remainder.
#[inline(always)]
#[must_use]
pub fn integral_fractional(x: F) -> (i32, F) {
    let integral = x.trunc_i32();
    (integral, x - F::from_i32(integral))
}

/// A table index from an integer the firmware keeps in range; one out of range (an out-of-bounds
/// read in the C++, which the patched oracle and the port both clamp) is clamped to the table.
#[inline(always)]
#[must_use]
pub fn index(i: i64, len: usize) -> usize {
    usize::try_from(i.max(0)).unwrap_or(0).min(len.saturating_sub(1))
}

/// `Interpolate(table + base, index, size)` with both reads clamped to the table: where the C++'s
/// reads stay inside the table this is `Interpolate()` exactly, and a read at an end returns that
/// end (the oracle's `patches/all/0011` and the reads the firmware keeps inside).
#[inline(always)]
#[must_use]
pub fn interpolate_clamped(table: &[f32], base: i64, index_: F, size: f32) -> F {
    let (integral, fractional) = integral_fractional(index_ * size);
    let i = base + i64::from(integral);
    let a = F(table.get(index(i, table.len())).copied().unwrap_or(0.0));
    let b = F(table.get(index(i + 1, table.len())).copied().unwrap_or(0.0));
    a + (b - a) * fractional
}

/// `SemitonesToRatio(semitones)`: two table lookups, valid for -128 to 127 semitones.
#[inline(always)]
#[must_use]
pub fn semitones_to_ratio(semitones: F) -> F {
    let (integral, fractional) = integral_fractional(semitones + 128.0);
    let high = &pitch_tables::LUT_PITCH_RATIO_HIGH;
    let low = &pitch_tables::LUT_PITCH_RATIO_LOW;
    let h = high.get(index(i64::from(integral), high.len())).copied().unwrap_or(1.0);
    let l = low.get(index(i64::from((fractional * 256.0).trunc_i32()), low.len())).copied().unwrap_or(1.0);
    F(h) * F(l)
}

/// `CONSTRAIN(var, min, max)` for floats.
#[inline(always)]
#[must_use]
pub fn constrain(x: F, min: f32, max: f32) -> F {
    if x < min {
        F(min)
    } else if x > max {
        F(max)
    } else {
        x
    }
}

/// `ONE_POLE(out, in, coefficient)`: `out += coefficient * (in - out)`.
#[inline(always)]
pub fn one_pole(out: &mut F, input: F, coefficient: F) {
    *out += coefficient * (input - *out);
}

/// `SLOPE(out, in, positive, negative)`.
#[inline(always)]
pub fn slope(out: &mut F, input: F, positive: f32, negative: f32) {
    let error = input - *out;
    *out += (if error > 0.0 { positive } else { negative }) * error;
}

/// `Crossfade(a, b, fade)`: `a + (b - a) * fade`.
#[inline(always)]
#[must_use]
pub fn crossfade(a: F, b: F, fade: F) -> F {
    a + (b - a) * fade
}

/// `stmlib::HysteresisQuantizer` (the first version, which the quantizer's level uses).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HysteresisQuantizer {
    quantized_value: i32,
}

impl HysteresisQuantizer {
    /// `Init()`.
    pub fn init(&mut self) {
        self.quantized_value = 0;
    }

    /// `Process(value, num_steps)`: the hysteresis is 0.25.
    pub fn process(&mut self, value: F, num_steps: i32) -> i32 {
        let hysteresis = 0.25_f32;
        let mut value = value * F::from_i32(num_steps - 1);
        value += F::from_i32(0);
        let feedback = if value > F::from_i32(self.quantized_value) { -hysteresis } else { hysteresis };
        let q = (value + feedback + 0.5).trunc_i32().clamp(0, num_steps - 1);
        self.quantized_value = q;
        q
    }
}

/// `stmlib::HysteresisQuantizer2`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HysteresisQuantizer2 {
    num_steps: i32,
    hysteresis: F,
    scale: F,
    offset: F,
    quantized_value: i32,
}

impl HysteresisQuantizer2 {
    /// `Init(num_steps, hysteresis, symmetric)`.
    pub fn init(&mut self, num_steps: i32, hysteresis: f32, symmetric: bool) {
        self.num_steps = num_steps;
        self.hysteresis = F(hysteresis);
        self.scale = F::from_i32(if symmetric { num_steps - 1 } else { num_steps });
        self.offset = F(if symmetric { 0.0 } else { -0.5 });
        self.quantized_value = 0;
    }

    /// `Process(value)`.
    pub fn process(&mut self, value: F) -> i32 {
        let mut value = value * self.scale;
        value += self.offset;
        value += F::from_i32(0);
        let sign = if value > F::from_i32(self.quantized_value) { -1.0_f32 } else { 1.0 };
        let q = (value + sign * self.hysteresis + 0.5).trunc_i32().clamp(0, self.num_steps - 1);
        self.quantized_value = q;
        q
    }

    /// `Lookup(array, value)`.
    pub fn lookup<T: Copy>(&mut self, array: &[T], value: F) -> T {
        let i = self.process(value);
        array[index(i64::from(i), array.len())]
    }
}

/// `stmlib::ParameterInterpolator(state, new_value, size)`. The C++ destructor writes the last
/// value back to `*state`; here the owner does it with [`ParameterInterpolator::value`] when the
/// block ends, whether or not every step was taken.
#[derive(Clone, Copy, Debug)]
pub struct ParameterInterpolator {
    value: F,
    increment: F,
}

impl ParameterInterpolator {
    /// Ramps from `state` to `new_value` over `size` steps.
    #[must_use]
    pub fn new(state: F, new_value: F, size: usize) -> Self {
        Self { value: state, increment: (new_value - state) / F::from_usize(size) }
    }

    /// `Next()`.
    #[allow(clippy::should_implement_trait)] // The firmware's name; not an iterator.
    pub fn next(&mut self) -> F {
        self.value += self.increment;
        self.value
    }

    /// The value the destructor writes back.
    #[must_use]
    pub fn value(&self) -> F {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semitones_to_ratio_matches_the_octave_points() {
        assert_eq!(semitones_to_ratio(F(0.0)), F(1.0));
        assert_eq!(semitones_to_ratio(F(12.0)), F(2.0));
        assert_eq!(semitones_to_ratio(F(36.0)), F(8.0));
    }

    #[test]
    fn the_clamped_interpolation_holds_the_ends() {
        let table = [0.0_f32, 1.0, 4.0, 9.0, 16.0];
        assert_eq!(interpolate_clamped(&table, 0, F(0.5), 4.0), F(4.0));
        assert_eq!(interpolate_clamped(&table, 0, F(1.0), 4.0), F(16.0));
        assert_eq!(interpolate_clamped(&table, 2, F(0.6), 4.0), F(16.0));
    }

    #[test]
    fn the_hysteresis_quantizers_hold_near_a_boundary() {
        let mut q = HysteresisQuantizer2::default();
        q.init(4, 0.1, false);
        assert_eq!(q.process(F(0.6)), 2);
        // 0.4875 * 4 - 0.5 = 1.45 rounds to 1, but the hysteresis holds 2 (1.45 + 0.1 + 0.5).
        assert_eq!(q.process(F(0.4875)), 2);
        assert_eq!(q.process(F(0.3)), 1);
        let mut h = HysteresisQuantizer::default();
        h.init();
        assert_eq!(h.process(F(1.0), 8), 7);
        assert_eq!(h.process(F(0.0), 8), 0);
    }
}
