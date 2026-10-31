//! `stmlib/dsp/filter.h`: the zero-delay-feedback state-variable filter, with flush-to-zero
//! arithmetic ([`F`]).

#![forbid(unsafe_code)]

use crate::float::F;

/// `FrequencyApproximation`: how `tan(π f)` is approximated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrequencyApproximation {
    /// `FREQUENCY_ACCURATE`: an eleventh-order polynomial.
    Accurate,
    /// `FREQUENCY_FAST`: a fifth-order polynomial tuned for 16 Hz to 16 kHz at 48 kHz.
    Fast,
    /// `FREQUENCY_DIRTY`: a third-order polynomial for frequencies below 8 kHz.
    Dirty,
}

/// `coefficient * M_PI * ... * M_PI` (`n` factors) in double precision, rounded to `f32` once: the
/// firmware's `M_PI_POW_n` macros expand without parentheses, so `3.260e-01f * M_PI_POW_3` is
/// `((0.326f * π) * π) * π` in double, then stored as a float.
const fn pi_power(coefficient: f32, n: u32) -> f32 {
    let mut x = coefficient as f64;
    let mut i = 0;
    while i < n {
        x *= std::f64::consts::PI;
        i += 1;
    }
    x as f32
}

/// `M_PI_F`, `float(M_PI)`.
const PI_F: f32 = std::f32::consts::PI;
const DIRTY_A: f32 = pi_power(3.736e-01, 3);
const FAST_A: f32 = pi_power(3.260e-01, 3);
const FAST_B: f32 = pi_power(1.823e-01, 5);
// The firmware's literals, verbatim (each rounds to the nearest f32, as the C++'s `f` literals).
#[allow(clippy::excessive_precision)]
const ACCURATE_A: f32 = pi_power(3.333314036e-01, 3);
#[allow(clippy::excessive_precision)]
const ACCURATE_B: f32 = pi_power(1.333923995e-01, 5);
#[allow(clippy::excessive_precision)]
const ACCURATE_C: f32 = pi_power(5.33740603e-02, 7);
#[allow(clippy::excessive_precision)]
const ACCURATE_D: f32 = pi_power(2.900525e-03, 9);
#[allow(clippy::excessive_precision)]
const ACCURATE_E: f32 = pi_power(9.5168091e-03, 11);

/// `OnePole::tan<approximation>(f)`.
#[inline(always)]
#[must_use]
pub fn tan(approximation: FrequencyApproximation, f: F) -> F {
    match approximation {
        FrequencyApproximation::Dirty => f * (PI_F + DIRTY_A * f * f),
        FrequencyApproximation::Fast => {
            let f2 = f * f;
            f * (PI_F + f2 * (FAST_A + FAST_B * f2))
        }
        FrequencyApproximation::Accurate => {
            let f2 = f * f;
            f * (PI_F + f2 * (ACCURATE_A + f2 * (ACCURATE_B + f2 * (ACCURATE_C + f2 * (ACCURATE_D + f2 * ACCURATE_E)))))
        }
    }
}

/// `FilterMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// `FILTER_MODE_LOW_PASS`.
    LowPass,
    /// `FILTER_MODE_BAND_PASS`.
    BandPass,
    /// `FILTER_MODE_HIGH_PASS`.
    HighPass,
}

/// `stmlib::Svf`, the zero-delay-feedback state-variable filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    g: F,
    r: F,
    h: F,
    state_1: F,
    state_2: F,
}

impl Svf {
    /// `Init`: 0.01 of the sample rate, resonance 100, cleared.
    pub fn init(&mut self) {
        self.set_f_q(FrequencyApproximation::Dirty, F(0.01), F(100.0));
        self.reset();
    }

    /// `Reset`.
    pub fn reset(&mut self) {
        self.state_1 = F::ZERO;
        self.state_2 = F::ZERO;
    }

    /// `set_f_q<approximation>(f, resonance)`.
    #[inline(always)]
    pub fn set_f_q(&mut self, approximation: FrequencyApproximation, f: F, resonance: F) {
        self.g = tan(approximation, f);
        self.r = 1.0 / resonance;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }

    /// One step from `state`, returning the mode's output.
    #[inline(always)]
    fn step(&self, input: F, state_1: &mut F, state_2: &mut F, mode: FilterMode) -> F {
        let hp = (input - self.r * *state_1 - self.g * *state_1 - *state_2) * self.h;
        let bp = self.g * hp + *state_1;
        *state_1 = self.g * hp + bp;
        let lp = self.g * bp + *state_2;
        *state_2 = self.g * bp + lp;
        match mode {
            FilterMode::LowPass => lp,
            FilterMode::BandPass => bp,
            FilterMode::HighPass => hp,
        }
    }

    /// `Process<mode>(in)`: one sample.
    #[inline(always)]
    pub fn process(&mut self, input: F, mode: FilterMode) -> F {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        let y = self.step(input, &mut s1, &mut s2, mode);
        self.state_1 = s1;
        self.state_2 = s2;
        y
    }

    /// `set(f)`: copies another filter's coefficients (not its state).
    #[inline(always)]
    pub fn set(&mut self, other: &Self) {
        self.g = other.g;
        self.r = other.r;
        self.h = other.h;
    }

    /// `Process<mode>(in, out, size, 2)` with `in == out` on one channel of interleaved frames.
    pub fn process_channel(&mut self, frames: &mut [[F; 2]], channel: usize, mode: FilterMode) {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        for frame in frames {
            frame[channel] = self.step(frame[channel], &mut s1, &mut s2, mode);
        }
        self.state_1 = s1;
        self.state_2 = s2;
    }

    /// `Process<mode>(in, out, size)` with `in == out`: a block in place.
    pub fn process_block(&mut self, in_out: &mut [F], mode: FilterMode) {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        for x in in_out {
            *x = self.step(*x, &mut s1, &mut s2, mode);
        }
        self.state_1 = s1;
        self.state_2 = s2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tangent_approximations_are_close_to_tan() {
        for f in [0.001_f32, 0.01, 0.05, 0.1] {
            let exact = (std::f32::consts::PI * f).tan();
            for a in [FrequencyApproximation::Accurate, FrequencyApproximation::Fast, FrequencyApproximation::Dirty] {
                let t = tan(a, F(f)).0;
                assert!((t - exact).abs() / exact < 0.01, "{a:?} at {f}: {t} against {exact}");
            }
        }
    }

    #[test]
    fn the_coefficients_are_double_products_rounded_once() {
        assert_eq!(FAST_A, (f64::from(0.326_f32) * std::f64::consts::PI * std::f64::consts::PI * std::f64::consts::PI) as f32);
    }
}
