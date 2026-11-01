//! The stmlib pieces the modal voice uses that the resonator's port does not carry
//! (`stmlib/dsp/filter.h`, `stmlib/dsp/dsp.h` at `e3bd7c9c`, Émilie Gillet, MIT): the state-variable
//! filter with its coefficient setters, normalized band-pass and multimode output, the naive SVF with
//! its exact tuning, and the wrapping table read. Every arithmetic result is flushed to zero ([`F`]).

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::filter::{FrequencyApproximation, tan};
use transit_resonator::stmlib::integral_fractional;

/// `M_PI_F`.
const PI_F: f32 = std::f32::consts::PI;

/// A table index from a truncated integer the firmware keeps in range; one outside (an
/// out-of-bounds read in the C++) is clamped to the table instead.
#[inline(always)]
pub(crate) fn index(i: i32, len: usize) -> usize {
    usize::try_from(i).unwrap_or(0).min(len - 1)
}

/// `InterpolateWrap(table, index, size)`: the index's fractional part times `size`, interpolated.
#[inline(always)]
#[must_use]
pub(crate) fn interpolate_wrap(table: &[f32], index_: F, size: f32) -> F {
    let wrapped = index_ - F::from_i32(index_.trunc_i32());
    let (integral, fractional) = integral_fractional(wrapped * size);
    let a = F(table[index(integral, table.len())]);
    let b = F(table[index(integral + 1, table.len())]);
    a + (b - a) * fractional
}

/// The outputs of a state-variable filter (`FilterMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    LowPass,
    BandPass,
    /// The band-pass scaled by the damping `r` (unity gain at the center).
    BandPassNormalized,
}

/// `stmlib::Svf`: the trapezoidal state-variable filter.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Svf {
    g: F,
    r: F,
    h: F,
    state_1: F,
    state_2: F,
}

impl Svf {
    /// `Init()`: 0.01 of the rate, Q 100 (the dirty tangent), state cleared.
    pub(crate) fn init(&mut self) {
        self.set_f_q(FrequencyApproximation::Dirty, F(0.01), F(100.0));
        self.state_1 = F::ZERO;
        self.state_2 = F::ZERO;
    }

    /// `g()`.
    pub(crate) fn g(&self) -> F {
        self.g
    }

    /// `set_g_r_h(g, r, h)`.
    pub(crate) fn set_g_r_h(&mut self, g: F, r: F, h: F) {
        self.g = g;
        self.r = r;
        self.h = h;
    }

    /// `set_g_r(g, r)`.
    pub(crate) fn set_g_r(&mut self, g: F, r: F) {
        self.g = g;
        self.r = r;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }

    /// `set_g_q(g, resonance)`.
    pub(crate) fn set_g_q(&mut self, g: F, resonance: F) {
        self.g = g;
        self.r = 1.0 / resonance;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }

    /// `set_f_q<approximation>(f, resonance)`.
    #[inline(always)]
    pub(crate) fn set_f_q(&mut self, approximation: FrequencyApproximation, f: F, resonance: F) {
        self.g = tan(approximation, f);
        self.r = 1.0 / resonance;
        self.h = 1.0 / (1.0 + self.r * self.g + self.g * self.g);
    }

    #[inline(always)]
    fn step(&self, input: F, s1: &mut F, s2: &mut F) -> (F, F, F) {
        let hp = (input - self.r * *s1 - self.g * *s1 - *s2) * self.h;
        let bp = self.g * hp + *s1;
        *s1 = self.g * hp + bp;
        let lp = self.g * bp + *s2;
        *s2 = self.g * bp + lp;
        (lp, bp, hp)
    }

    #[inline(always)]
    fn pick(&self, (lp, bp, _hp): (F, F, F), mode: Mode) -> F {
        match mode {
            Mode::LowPass => lp,
            Mode::BandPass => bp,
            Mode::BandPassNormalized => bp * self.r,
        }
    }

    /// `Process<mode>(in)`.
    #[inline(always)]
    pub(crate) fn process(&mut self, input: F, mode: Mode) -> F {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        let y = self.step(input, &mut s1, &mut s2);
        self.state_1 = s1;
        self.state_2 = s2;
        self.pick(y, mode)
    }

    /// `Process<mode>(in, out, size)` with `in == out`.
    pub(crate) fn process_block(&mut self, in_out: &mut [F], mode: Mode) {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        for x in in_out {
            let y = self.step(*x, &mut s1, &mut s2);
            *x = self.pick(y, mode);
        }
        self.state_1 = s1;
        self.state_2 = s2;
    }

    /// `ProcessMultimode(in, out, size, mode)` with `in == out`: low-pass at 0, band-pass at 1,
    /// through a notch-like high-pass blend in between.
    pub(crate) fn process_multimode(&mut self, in_out: &mut [F], mode: F) {
        let (mut s1, mut s2) = (self.state_1, self.state_2);
        let hp_gain = if mode < 0.5 { -mode * 2.0 } else { -2.0 + mode * 2.0 };
        let lp_gain = if mode < 0.5 { 1.0 - mode * 2.0 } else { F::ZERO };
        let bp_gain = if mode < 0.5 { F::ZERO } else { mode * 2.0 - 1.0 };
        for x in in_out {
            let (lp, bp, hp) = self.step(*x, &mut s1, &mut s2);
            *x = hp_gain * hp + bp_gain * bp + lp_gain * lp;
        }
        self.state_1 = s1;
        self.state_2 = s2;
    }
}

/// `stmlib::NaiveSvf`: the Chamberlin state-variable filter.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NaiveSvf {
    f: F,
    damp: F,
    lp: F,
    bp: F,
}

impl NaiveSvf {
    /// `Init()`: the dirty tuning at 0.01 of the rate, Q 100, state cleared.
    pub(crate) fn init(&mut self) {
        let f = F(0.01);
        self.f = 2.0 * PI_F * f;
        self.damp = 1.0 / F(100.0);
        self.lp = F::ZERO;
        self.bp = F::ZERO;
    }

    /// `set_f_q<FREQUENCY_EXACT>(f, resonance)`: `2 sinf(pi f)`, `f` capped at 0.497.
    pub(crate) fn set_f_q_exact(&mut self, f: F, resonance: F) {
        let f = if f < 0.497 { f } else { F(0.497) };
        self.f = 2.0 * F(transit_resonator::float::ftz((PI_F * f).0.sin()));
        self.damp = 1.0 / resonance;
    }

    /// `Process<FILTER_MODE_LOW_PASS>(in, out, size)` (also its decimating form at a factor of 1).
    pub(crate) fn process_low_pass(&mut self, input: &[F], out: &mut [F]) {
        let (mut lp, mut bp) = (self.lp, self.bp);
        for (x, o) in input.iter().zip(out.iter_mut()) {
            let bp_normalized = bp * self.damp;
            let notch = *x - bp_normalized;
            lp += self.f * bp;
            let hp = notch - lp;
            bp += self.f * hp;
            *o = lp;
        }
        self.lp = lp;
        self.bp = bp;
    }

    /// [`Self::process_low_pass`] in place.
    pub(crate) fn process_low_pass_in_place(&mut self, in_out: &mut [F]) {
        let (mut lp, mut bp) = (self.lp, self.bp);
        for x in in_out {
            let bp_normalized = bp * self.damp;
            let notch = *x - bp_normalized;
            lp += self.f * bp;
            let hp = notch - lp;
            bp += self.f * hp;
            *x = lp;
        }
        self.lp = lp;
        self.bp = bp;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapping_read_keeps_the_fractional_part() {
        let table = [0.0_f32, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(interpolate_wrap(&table, F(1.25), 4.0), F(1.0));
        assert_eq!(interpolate_wrap(&table, F(0.375), 4.0), F(1.5));
    }

    #[test]
    fn the_normalized_band_pass_is_the_band_pass_times_r() {
        let mut a = Svf::default();
        let mut b = Svf::default();
        a.set_g_q(F(0.1), F(4.0));
        b.set_g_q(F(0.1), F(4.0));
        for x in [1.0_f32, 0.0, -0.5, 0.25] {
            let n = a.process(F(x), Mode::BandPassNormalized);
            let p = b.process(F(x), Mode::BandPass);
            assert_eq!(n, p * F(0.25));
        }
    }
}
