//! The Filter's state-variable sections (#242): the trapezoidal-integrator SVF (a two-pole analog
//! prototype through the bilinear transform with the cutoff prewarped), its coefficient design,
//! the soft clip in front of it, and the analytic magnitude response the panel draws and the tests
//! check against.
//!
//! One section is the analog prototype `1 / (s² + k·s + 1)` family at `s = j·tan(ω/2) / g`,
//! `g = tan(π·fc/fs)`, `k = 1/Q`: low pass `1/D`, high pass `s²/D`, band pass `k·s/D` (unity gain
//! at the cutoff whatever the resonance), notch `(s² + 1)/D`. The 24 dB slope cascades two
//! sections at the same cutoff: the first with the resonance's Q, the second a Butterworth section
//! (Q = 1/√2), so the resonance peak keeps its height instead of doubling in dB.

use std::f64::consts::PI;

/// The four modes, in the order of the `mode` parameter's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Low,
    High,
    Band,
    Notch,
}

impl Mode {
    pub(crate) fn from_index(i: usize) -> Mode {
        match i {
            1 => Mode::High,
            2 => Mode::Band,
            3 => Mode::Notch,
            _ => Mode::Low,
        }
    }
}

/// Q at 0% resonance (Butterworth: no peak) and at 100%.
pub const Q_MIN: f32 = std::f32::consts::FRAC_1_SQRT_2;
pub const Q_MAX: f32 = 25.0;
/// `k = 1/Q` of the 24 dB slope's second, Butterworth section.
pub(crate) const K_BUTTERWORTH: f32 = std::f32::consts::SQRT_2;
/// The lowest cutoff the modulation can reach; the highest is [`max_cutoff`].
pub const MIN_CUTOFF: f32 = 16.0;

/// The resonance knob (0..100 %) as Q: exponential from [`Q_MIN`] to [`Q_MAX`].
pub fn q_of(resonance_pct: f32) -> f32 {
    let r = if resonance_pct.is_finite() { (resonance_pct / 100.0).clamp(0.0, 1.0) } else { 0.0 };
    Q_MIN * (Q_MAX / Q_MIN).powf(r)
}

/// The highest cutoff at `sr`: just under Nyquist, where `tan` stays finite.
pub fn max_cutoff(sr: f32) -> f32 {
    0.49 * sr
}

/// One section's coefficients (shared by every channel: stereo-linked).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Coefs {
    pub(crate) k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
}

impl Coefs {
    /// `g = tan(π·fc/fs)` and `k = 1/Q`.
    pub(crate) fn new(g: f32, k: f32) -> Coefs {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Coefs { k, a1, a2, a3: g * a2 }
    }

    /// The prewarped integrator gain of a cutoff, clamped to the usable range.
    pub(crate) fn g_of(cutoff: f32, sr: f32) -> f32 {
        let f = cutoff.clamp(MIN_CUTOFF, max_cutoff(sr));
        (std::f32::consts::PI * f / sr).tan()
    }
}

impl Default for Coefs {
    fn default() -> Self {
        Coefs::new(Coefs::g_of(1000.0, 48_000.0), 1.0 / Q_MIN)
    }
}

/// One channel's integrator memories of one section.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Section {
    ic1: f32,
    ic2: f32,
}

impl Section {
    /// One sample through the section in `mode`.
    #[inline]
    pub(crate) fn tick(&mut self, c: &Coefs, mode: Mode, v0: f32) -> f32 {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = crate::util::flush(2.0 * v1 - self.ic1);
        self.ic2 = crate::util::flush(2.0 * v2 - self.ic2);
        match mode {
            Mode::Low => v2,
            Mode::High => v0 - c.k * v1 - v2,
            Mode::Band => c.k * v1,
            Mode::Notch => v0 - c.k * v1,
        }
    }

    #[cfg(feature = "testing")]
    pub(crate) fn bits(&self) -> [u32; 2] {
        [self.ic1.to_bits(), self.ic2.to_bits()]
    }
}

/// The drive stage: exactly linear up to full scale, a smooth (C1) knee to a ceiling of 1.5 at
/// twice full scale, flat beyond: `t - (t - 1)²/2` for `1 < |x| < 2`.
#[inline]
pub(crate) fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        let y = if a < 2.0 { a - 0.5 * (a - 1.0) * (a - 1.0) } else { 1.5 };
        y.copysign(x)
    }
}

/// The analytic magnitude (linear) of one section at `freq`, in f64.
fn section_magnitude(mode: Mode, g: f64, k: f64, sr: f64, freq: f64) -> f64 {
    let w = (PI * freq / sr).tan() / g;
    // s = j·w: s² = -w², so D = (1 - w²) + j·k·w.
    let (dr, di) = (1.0 - w * w, k * w);
    let d = (dr * dr + di * di).sqrt();
    let n = match mode {
        Mode::Low => 1.0,
        Mode::High => w * w,
        Mode::Band => k * w,
        Mode::Notch => (1.0 - w * w).abs(),
    };
    n / d
}

/// The analytic magnitude (linear) of the whole filter at `freq`: `mode`, 24 dB slope or not,
/// cutoff and Q, at sample rate `sr` (the cutoff clamped as the filter clamps it).
pub fn magnitude(mode: Mode, slope24: bool, cutoff: f64, q: f64, sr: f64, freq: f64) -> f64 {
    let f = cutoff.clamp(f64::from(MIN_CUTOFF), 0.49 * sr);
    let g = (PI * f / sr).tan();
    let one = section_magnitude(mode, g, 1.0 / q, sr, freq);
    if slope24 { one * section_magnitude(mode, g, f64::from(K_BUTTERWORTH), sr, freq) } else { one }
}
