//! The sections (ADR 0054 decision 2.3): σ-domain coefficients, their exact map to a trapezoidal
//! state-variable filter with mixed outputs, the 64-bit section ticks, and the closed-form
//! evaluation of a designed section's magnitude.
//!
//! With `σ = (1 − z⁻¹)/(1 + z⁻¹)` a second-order section is `(n2σ² + n1σ + n0)/(c2σ² + c1σ + c0)`.
//! On the unit circle `σ = j·tan(ω/2)`; multiplying numerator and denominator by `cos²(ω/2)` gives
//! the magnitude from the half angle's sine and cosine with no tangent and no cancellation.

use crate::math;

/// States below this magnitude are flushed to zero (decision 12): no subnormal state, no
/// flush-to-zero mode on the audio thread.
pub(crate) const FLUSH: f64 = 1.0e-30;

#[inline]
pub(crate) fn flush(x: f64) -> f64 {
    if x.abs() < FLUSH { 0.0 } else { x }
}

/// A second-order section in the σ domain: numerator `n` and denominator `c`, index = power of σ.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sos {
    pub(crate) n: [f64; 3],
    pub(crate) c: [f64; 3],
}

impl Sos {
    /// The section inverted (numerator and denominator swapped).
    pub(crate) fn inverted(self) -> Self {
        Self { n: self.c, c: self.n }
    }

    /// `|H|²` at the angle whose half has sine `s` and cosine `co`.
    #[inline]
    pub(crate) fn power(&self, s: f64, co: f64) -> f64 {
        let (s2, c2, sc) = (s * s, co * co, s * co);
        let nr = self.n[0] * c2 - self.n[2] * s2;
        let ni = self.n[1] * sc;
        let dr = self.c[0] * c2 - self.c[2] * s2;
        let di = self.c[1] * sc;
        (nr * nr + ni * ni) / (dr * dr + di * di)
    }
}

/// A first-order section in the σ domain: `(n1σ + n0)/(d1σ + d0)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Fos {
    pub(crate) n: [f64; 2],
    pub(crate) d: [f64; 2],
}

impl Fos {
    /// `|H|²` at the angle whose half has sine `s` and cosine `co`.
    #[inline]
    pub(crate) fn power(&self, s: f64, co: f64) -> f64 {
        let nr = self.n[0] * co;
        let ni = self.n[1] * s;
        let dr = self.d[0] * co;
        let di = self.d[1] * s;
        (nr * nr + ni * ni) / (dr * dr + di * di)
    }
}

/// The state-variable coefficients of a second-order section (decision 2.3): `g = √(c0/c2)`,
/// `k = c1/√(c0·c2)`, `m0 = n0/c0`, `m1 = n1·g/c0`, `m2 = n2/c2`, and the trapezoidal update's
/// `a1 = 1/(1 + g(g + k))`, `a2 = g·a1`, `a3 = g·a2`. Arithmetic and `sqrt` only.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Svf {
    pub(crate) g: f64,
    pub(crate) k: f64,
    pub(crate) m0: f64,
    pub(crate) m1: f64,
    pub(crate) m2: f64,
    pub(crate) a1: f64,
    pub(crate) a2: f64,
    pub(crate) a3: f64,
}

impl Svf {
    pub(crate) fn map(s: &Sos) -> Self {
        let [n0, n1, n2] = s.n;
        let [c0, c1, c2] = s.c;
        let g = math::sqrt(c0 / c2);
        let k = c1 / math::sqrt(c0 * c2);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { g, k, m0: n0 / c0, m1: n1 * g / c0, m2: n2 / c2, a1, a2, a3: g * a2 }
    }

    /// One sample: the SVF's unit low, band (before `k`) and high outputs mixed by `m0`, `m1`, `m2`.
    #[inline]
    pub(crate) fn tick(&self, st: &mut [f64; 2], x: f64) -> f64 {
        let [ic1, ic2] = *st;
        let v3 = x - ic2;
        let v1 = self.a1 * ic1 + self.a2 * v3;
        let v2 = ic2 + self.a2 * ic1 + self.a3 * v3;
        st[0] = flush(2.0 * v1 - ic1);
        st[1] = flush(2.0 * v2 - ic2);
        let high = x - self.k * v1 - v2;
        self.m0 * v2 + self.m1 * v1 + self.m2 * high
    }
}

/// The trapezoidal one-pole of a first-order section: `g = d0/d1`, `G = g/(1 + g)`, output
/// `m0·low + m1·high` with `m0 = n0/d0`, `m1 = n1/d1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OnePole {
    pub(crate) big_g: f64,
    pub(crate) m0: f64,
    pub(crate) m1: f64,
}

impl OnePole {
    pub(crate) fn map(f: &Fos) -> Self {
        let [n0, n1] = f.n;
        let [d0, d1] = f.d;
        let g = d0 / d1;
        Self { big_g: g / (1.0 + g), m0: n0 / d0, m1: n1 / d1 }
    }

    #[inline]
    pub(crate) fn tick(&self, st: &mut f64, x: f64) -> f64 {
        let v = (x - *st) * self.big_g;
        let low = v + *st;
        *st = flush(low + v);
        self.m0 * low + self.m1 * (x - low)
    }
}
