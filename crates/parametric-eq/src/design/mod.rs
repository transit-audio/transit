//! The zero-latency band designs (ADR 0054 decision 2.2): magnitude-matched sections.
//!
//! - **Poles** by the exact mapping `z = e^(sT)` of the prototype's poles ([`map_pair`]).
//! - **Zeros** fitted to the prototype's squared magnitude at DC, at Nyquist and at a match
//!   frequency, in the σ domain where the fit is well conditioned ([`fit3`]); minimum phase.
//! - Bells, cuts, notch and band pass use that single fit ([`single`]); shelves and the tilt shelf
//!   use two factor sections with the continuous all-pole transition ([`shelf`]); flat tilt is
//!   fifteen fitted first-order pairs multiplied two by two ([`tilt`]).
//!
//! Transcendentals (all through [`crate::math`]): `exp2` for pole radii and gains, `sin` and `cos`
//! for pole angles and the match frequency's half-angle tangent. Control rate only.

mod shelf;
mod single;
mod tilt;

use crate::math::{self, LOG2_E, PI, TAU};
use crate::section::{Fos, OnePole, Sos, Svf};
use crate::{BandType, Slope, clamp_freq, clamp_gain, clamp_q};

#[cfg(test)]
pub(crate) use shelf::{factor_hard_switch, shelf_single_fit};
#[cfg(test)]
pub(crate) use single::cut_section;

/// The most second-order sections a band needs (a 96 dB/oct cut).
pub(crate) const MAX_SOS: usize = 8;

/// A designed band: its second-order sections in run order (ascending Q, decision 2.6), then the
/// optional first-order section. No section at all is the identity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Design {
    pub(crate) sos: [Sos; MAX_SOS],
    pub(crate) n_sos: usize,
    pub(crate) fos: Option<Fos>,
    /// How many three-point fits were infeasible and clamped (`n1² < 0` set to 0, decision 2.2).
    pub(crate) clamped: u32,
}

impl Design {
    pub(crate) fn identity() -> Self {
        Self::default()
    }

    pub(crate) fn is_identity(&self) -> bool {
        self.n_sos == 0 && self.fos.is_none()
    }

    pub(crate) fn push(&mut self, s: Sos) {
        if self.n_sos < MAX_SOS {
            self.sos[self.n_sos] = s;
            self.n_sos += 1;
        }
    }

    /// The second-order sections in run order.
    pub(crate) fn sections(&self) -> &[Sos] {
        &self.sos[..self.n_sos]
    }

    /// Scales the first section's numerator by an amplitude constant (shelves, tilt, flat tilt).
    pub(crate) fn scale_first(&mut self, k: f64) {
        if self.n_sos > 0 {
            for v in self.sos[0].n.iter_mut() {
                *v *= k;
            }
        } else if let Some(f) = self.fos.as_mut() {
            for v in f.n.iter_mut() {
                *v *= k;
            }
        }
    }

    /// `|H|²` at the angle whose half has sine `s` and cosine `co`.
    pub(crate) fn power(&self, s: f64, co: f64) -> f64 {
        let mut p = 1.0;
        for sec in self.sections() {
            p *= sec.power(s, co);
        }
        if let Some(f) = &self.fos {
            p *= f.power(s, co);
        }
        p
    }

    /// The realization's coefficients.
    pub(crate) fn coefficients(&self) -> Coefficients {
        let mut c = Coefficients { n_sos: self.n_sos, ..Coefficients::default() };
        for (dst, src) in c.sos.iter_mut().zip(self.sections()) {
            *dst = Svf::map(src);
        }
        c.fos = self.fos.as_ref().map(OnePole::map);
        c
    }
}

/// A design's state-variable coefficients, ready to run.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Coefficients {
    pub(crate) sos: [Svf; MAX_SOS],
    pub(crate) n_sos: usize,
    pub(crate) fos: Option<OnePole>,
}

/// A band's values as used: frequency through the log2 domain and clamped to 0.49 of the rate, Q
/// through the log2 domain, gain in dB (decisions 3.1 and 12). The engine's settled smoothers and the
/// panel's functions both arrive here, so the curve is what the engine converges to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Used {
    pub(crate) f0: f64,
    pub(crate) gain_db: f64,
    pub(crate) q: f64,
}

impl Used {
    /// From smoothed values: `log2` of the frequency, `log2` of Q, gain in dB.
    pub(crate) fn from_smoothed(log2_f: f64, gain_db: f64, log2_q: f64, fs: f64) -> Self {
        Self { f0: math::exp2(log2_f).min(0.49 * fs), gain_db, q: math::exp2(log2_q) }
    }

    /// From stored parameter values, as the settled smoothers would hold them.
    pub(crate) fn from_stored(freq_hz: f64, gain_db: f64, q: f64, fs: f64) -> Self {
        Self::from_smoothed(math::log2(clamp_freq(freq_hz)), clamp_gain(gain_db), math::log2(clamp_q(q)), fs)
    }
}

/// Designs one band at sample rate `fs`. A type with gain at exactly 0 dB is the identity.
pub(crate) fn design(kind: BandType, slope: Slope, used: Used, fs: f64) -> Design {
    let Used { f0, gain_db, q } = used;
    if kind.identity_at_zero_gain() && gain_db == 0.0 {
        return Design::identity();
    }
    let w0 = TAU * f0 / fs;
    match kind {
        BandType::Bell => single::bell(w0, gain_db, q),
        BandType::Notch => single::notch(w0, q),
        BandType::BandPass => single::band_pass(w0, q),
        BandType::LowCut => single::cut(w0, slope, true),
        BandType::HighCut => single::cut(w0, slope, false),
        BandType::LowShelf => shelf::low(w0, gain_db, q),
        BandType::HighShelf => shelf::high(w0, gain_db, q, false),
        BandType::TiltShelf => shelf::high(w0, gain_db, q, true),
        BandType::FlatTilt => tilt::flat(f0, gain_db, fs),
    }
}

/// The highest match frequency of the single fit, 0.49 of the rate in radians per sample.
pub(crate) const MATCH_MAX: f64 = 0.98 * PI;

/// The σ-domain denominator `(c0, c1, c2)` of a pole pair at `wp` radians per sample with damping
/// `zeta`, mapped exactly (decision 2.2). A complex pair (`ζ < 1`) maps to radius `r = e^(−ζ·wp)` and
/// angle `θ = wp·√(1 − ζ²)` with `c0 = (1 − r)² + 4r·sin²(θ/2)`, `c1 = 2(1 − r²)`,
/// `c2 = (1 + r)² − 4r·sin²(θ/2)` (decision 2.3's cancellation-free forms); real poles map each root.
pub(crate) fn map_pair(wp: f64, zeta: f64) -> [f64; 3] {
    if zeta < 1.0 {
        let r = math::exp2(-zeta * wp * LOG2_E);
        let theta = wp * math::sqrt(1.0 - zeta * zeta);
        let sh = math::sin(0.5 * theta);
        let s2 = 4.0 * r * sh * sh;
        let one_m = 1.0 - r;
        let one_p = 1.0 + r;
        [one_m * one_m + s2, 2.0 * one_m * one_p, one_p * one_p - s2]
    } else {
        // Roots at −wp·(ζ ± √(ζ² − 1)); the small one through the product of the roots.
        let d = math::sqrt(zeta * zeta - 1.0);
        let big = wp * (zeta + d);
        let small = wp / (zeta + d);
        let p1 = math::exp2(-small * LOG2_E);
        let p2 = math::exp2(-big * LOG2_E);
        [(1.0 - p1) * (1.0 - p2), 2.0 * (1.0 - p1 * p2), (1.0 + p1) * (1.0 + p2)]
    }
}

/// The σ-domain denominator `(d0, d1)` of a real pole at `wp` radians per sample, mapped exactly.
pub(crate) fn map_real(wp: f64) -> [f64; 2] {
    let p = math::exp2(-wp * LOG2_E);
    [1.0 - p, 1.0 + p]
}

/// The σ-domain three-point fit (decision 2.2): given the denominator `c`, the numerator whose
/// squared magnitude equals the target `T` at DC (`t0`), at Nyquist (`tpi`) and at the match
/// frequency `wm` (`tm`): `n0 = √T(0)·c0`, `n2 = √T(π)·c2`,
/// `n1² = (T(ωm)·|D(t)|² − (n0 − n2·t²)²)/t²` with `t = tan(ωm/2)` as `sin/cos` and
/// `|D(t)|² = (c0 − c2·t²)² + (c1·t)²`. Minimum phase (all three non-negative). An infeasible point
/// (`n1² < 0`) is clamped to 0, a continuous fallback; the second value says whether it was.
pub(crate) fn fit3(c: [f64; 3], t0: f64, tpi: f64, tm: f64, wm: f64) -> ([f64; 3], bool) {
    let t = math::sin(0.5 * wm) / math::cos(0.5 * wm);
    let t2 = t * t;
    let n0 = math::sqrt(t0) * c[0];
    let n2 = math::sqrt(tpi) * c[2];
    let dr = c[0] - c[2] * t2;
    let di = c[1] * t;
    let nr = n0 - n2 * t2;
    let n1sq = (tm * (dr * dr + di * di) - nr * nr) / t2;
    if n1sq < 0.0 { ([n0, 0.0, n2], true) } else { ([n0, math::sqrt(n1sq), n2], false) }
}

/// The first-order fit at DC and Nyquist: `n0 = √T(0)·d0`, `n1 = √T(π)·d1`, both non-negative.
pub(crate) fn fit2(d: [f64; 2], t0: f64, tpi: f64) -> [f64; 2] {
    [math::sqrt(t0) * d[0], math::sqrt(tpi) * d[1]]
}

/// `10^(G/40)`, RBJ's `A`.
pub(crate) fn amplitude_a(gain_db: f64) -> f64 {
    math::exp2(gain_db / 40.0 * math::LOG2_10)
}
