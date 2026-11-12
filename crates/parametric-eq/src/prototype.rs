//! The analog prototypes (ADR 0054 decision 2.1) as squared magnitudes: rational functions of the
//! frequency, evaluated without a transcendental call. `u` is the frequency over the band
//! frequency (the prototype in hertz, unwarped). The fits of [`crate::design`] match these, the
//! matching tests measure against them, and the gain match evaluates them on its grid (decision 7).

use crate::math::{self, LOG2_10};
use crate::{BandType, Slope, butterworth};

/// Bell: `(s² + (A/Q)s + 1) / (s² + s/(AQ) + 1)` at `s = ju`.
#[inline]
pub(crate) fn bell(u: f64, a: f64, q: f64) -> f64 {
    let r = 1.0 - u * u;
    let r2 = r * r;
    let num = a * u / q;
    let den = u / (a * q);
    (r2 + num * num) / (r2 + den * den)
}

/// Notch: `(s² + 1) / (s² + s/Q + 1)`.
#[inline]
pub(crate) fn notch(u: f64, q: f64) -> f64 {
    let r = 1.0 - u * u;
    let r2 = r * r;
    let d = u / q;
    r2 / (r2 + d * d)
}

/// Band pass: `(s/Q) / (s² + s/Q + 1)`, unity at `u = 1`.
#[inline]
pub(crate) fn band_pass(u: f64, q: f64) -> f64 {
    let r = 1.0 - u * u;
    let d = u / q;
    let d2 = d * d;
    d2 / (r * r + d2)
}

/// Second-order Butterworth low-pass section `1 / (s² + s/Q + 1)`.
#[inline]
pub(crate) fn low_pass2(u: f64, q: f64) -> f64 {
    let r = 1.0 - u * u;
    let d = u / q;
    1.0 / (r * r + d * d)
}

/// Second-order high-pass section `s² / (s² + s/Q + 1)`.
#[inline]
pub(crate) fn high_pass2(u: f64, q: f64) -> f64 {
    let u2 = u * u;
    u2 * u2 * low_pass2(u, q)
}

/// First-order low pass `1 / (s + 1)`.
#[inline]
pub(crate) fn low_pass1(u: f64) -> f64 {
    1.0 / (1.0 + u * u)
}

/// First-order high pass `s / (s + 1)`.
#[inline]
pub(crate) fn high_pass1(u: f64) -> f64 {
    let u2 = u * u;
    u2 / (1.0 + u2)
}

/// The shelves' pole-pair factor `P(ω_f) = ω_f² / (s² + 2ζω_f s + ω_f²)` at `s = jω` (decision 2.2),
/// all in radians per sample.
#[inline]
pub(crate) fn pole_pair(w: f64, wf: f64, zeta: f64) -> f64 {
    let wf2 = wf * wf;
    let r = wf2 - w * w;
    let d = 2.0 * zeta * wf * w;
    wf2 * wf2 / (r * r + d * d)
}

/// One flat-tilt pair `(1 + s/z) / (1 + s/p)` at frequency `f` (all in hertz).
#[inline]
pub(crate) fn tilt_pair(f: f64, z: f64, p: f64) -> f64 {
    let a = f / z;
    let b = f / p;
    (1.0 + a * a) / (1.0 + b * b)
}

/// The flat tilt's pair frequencies `f_k = 10 * 2^k` Hz for k = -1 ... 13 (decision 2.4).
pub(crate) const TILT_PAIRS: usize = 15;

/// Octaves of the flat tilt's span: `G` dB across 20 Hz to 20 kHz is `G / 9.966` dB per octave.
pub(crate) const TILT_OCTAVES: f64 = 9.966;

/// The flat tilt's zero and pole frequencies in hertz for a gain `gain_db`, and the amplitude scale
/// that puts the cascade at 0 dB at `f0` (decision 2.4).
pub(crate) fn tilt_pairs(gain_db: f64, f0: f64) -> ([(f64, f64); TILT_PAIRS], f64) {
    let t = gain_db / TILT_OCTAVES;
    // 10^(t/40): z_k = f_k / r, p_k = f_k * r, so p_k / z_k = 10^(t/20).
    let r = math::exp2(t / 40.0 * LOG2_10);
    let mut pairs = [(0.0, 0.0); TILT_PAIRS];
    let mut power = 1.0;
    let mut fk = 5.0;
    for pair in pairs.iter_mut() {
        let z = fk / r;
        let p = fk * r;
        *pair = (z, p);
        power *= tilt_pair(f0, z, p);
        fk *= 2.0;
    }
    (pairs, math::sqrt(1.0 / power))
}

/// A band's analog prototype, resolved from its used values. Evaluating it calls no transcendental.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Prototype {
    kind: BandType,
    f0: f64,
    a: f64,
    sa: f64,
    q: f64,
    slope: Slope,
    tilt: [(f64, f64); TILT_PAIRS],
    tilt_scale2: f64,
}

impl Prototype {
    /// A placeholder that is never evaluated (a slot not captured yet).
    pub(crate) const IDLE: Prototype = Prototype {
        kind: BandType::Bell,
        f0: 1_000.0,
        a: 1.0,
        sa: 1.0,
        q: 1.0,
        slope: Slope::Db24,
        tilt: [(1.0, 1.0); TILT_PAIRS],
        tilt_scale2: 1.0,
    };

    /// The prototype of a band at the used frequency `f0` (Hz, already clamped), gain and Q.
    pub(crate) fn new(kind: BandType, slope: Slope, f0: f64, gain_db: f64, q: f64) -> Self {
        let a = math::exp2(gain_db / 40.0 * LOG2_10);
        let (tilt, tilt_scale2) = if kind == BandType::FlatTilt {
            let (pairs, scale) = tilt_pairs(gain_db, f0);
            (pairs, scale * scale)
        } else {
            ([(1.0, 1.0); TILT_PAIRS], 1.0)
        };
        Self { kind, f0, a, sa: math::sqrt(a), q, slope, tilt, tilt_scale2 }
    }

    /// The squared magnitude at `f` hertz.
    pub(crate) fn power(&self, f: f64) -> f64 {
        let u = f / self.f0;
        let (a, sa, q) = (self.a, self.sa, self.q);
        match self.kind {
            BandType::Bell => bell(u, a, q),
            BandType::LowShelf => low_shelf(u, a, sa, q),
            BandType::HighShelf => high_shelf(u, a, sa, q),
            BandType::TiltShelf => high_shelf(u, a, sa, q) / (a * a),
            BandType::Notch => notch(u, q),
            BandType::BandPass => band_pass(u, q),
            BandType::LowCut | BandType::HighCut => {
                let high = self.kind == BandType::LowCut;
                let mut p = 1.0;
                for &qi in butterworth::QS[self.slope.index()] {
                    p *= if high { high_pass2(u, qi) } else { low_pass2(u, qi) };
                }
                if self.slope.order() % 2 == 1 {
                    p *= if high { high_pass1(u) } else { low_pass1(u) };
                }
                p
            }
            BandType::FlatTilt => {
                let mut p = self.tilt_scale2;
                for &(z, pk) in &self.tilt {
                    p *= tilt_pair(f, z, pk);
                }
                p
            }
        }
    }
}

/// Low shelf (`sa = √A`): `A (s² + (√A/Q)s + A) / (A s² + (√A/Q)s + 1)`.
#[inline]
pub(crate) fn low_shelf(u: f64, a: f64, sa: f64, q: f64) -> f64 {
    let u2 = u * u;
    let m = sa * u / q;
    let n = a - u2;
    let d = 1.0 - a * u2;
    a * a * (n * n + m * m) / (d * d + m * m)
}

/// High shelf (`sa = √A`): `A (A s² + (√A/Q)s + 1) / (s² + (√A/Q)s + A)`.
#[inline]
pub(crate) fn high_shelf(u: f64, a: f64, sa: f64, q: f64) -> f64 {
    let u2 = u * u;
    let m = sa * u / q;
    let n = 1.0 - a * u2;
    let d = a - u2;
    a * a * (n * n + m * m) / (d * d + m * m)
}
