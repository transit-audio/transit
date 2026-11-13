//! Clip fade factors in `f64` under the closed fade law (ADR 0058, decision 13; ADR 0056's question 9,
//! decided 2026-11-12): the stable shape forms with their operation order fixed, the curvature weighting,
//! and the factor function that gives a clip sample's fade-in and fade-out factors.
//!
//! - **Shapes** `F(t)`, `t` in [0, 1] (NaN is 0, the rest clamped): Linear `t`; Equal Power
//!   `sin(t · π/2)`; S-Curve `v · v` with `v = sin(t · π/2)`; Exponential `(t · t) · t`; Logarithmic
//!   `t · ((3 − 3·t) + t · t)`. Each form avoids a subtraction that cancels near either end, where a
//!   fade's gain is small (the cosine and cubic forms lose up to hundreds of `f32` steps there).
//! - **Curvature** `c` (sanitized as [`crate::Fade::sanitize`] does: non-finite is 0, then clamped to
//!   [−1, 1]): for `c == 0`, `b ≤ 0` or `b ≥ 1` the factor is `b = F(t)`; otherwise `w = b · 2^(4c)` and
//!   the factor is `w / ((w + 1) − b)`. `2^(4c)` depends on the fade alone and is computed once per
//!   fade ([`FadeKernel::new`]) through the portable seam.
//! - **The closed law**, for a clip of `length ≥ 1` samples and `0 ≤ k < length` (a caller outside the
//!   domain gets the value at its nearest end), each fade length clamped to `0..=length`:
//!   `Fin(k)` is 1 for `m_in = 0` or `k ≥ m_in`, `F_in(0.5)` for `m_in = 1` (the midpoint), and
//!   `F_in(k / (m_in − 1))` otherwise; `Fout(k)` is 1 for `m_out = 0` or `k < length − m_out`,
//!   `F_out(0.5)` for `m_out = 1`, and `F_out((length − 1 − k) / (m_out − 1))` otherwise. A fade-in
//!   starts at exactly 0 and reaches exactly 1 on its last sample; a fade-out is the fade-in of the same
//!   length reversed sample for sample.
//!
//! Not used by rendering or drawing before ADR 0058 slice G3, which makes [`fade_factors`] the one
//! evaluator for the render, the waveform, the fade overlay and drag previews. Allocation-free.

use crate::{Fade, FadeShape};
use core::f64::consts::FRAC_PI_2;
use transit_math::f64 as m64;
use transit_time::Samples;

/// The shape kernel `F(t)` in its stable `f64` form (`t` already sanitized).
fn shape(shape: FadeShape, t: f64) -> f64 {
    match shape {
        FadeShape::Linear => t,
        FadeShape::EqualPower => m64::sin(t * FRAC_PI_2),
        FadeShape::SCurve => {
            let v = m64::sin(t * FRAC_PI_2);
            v * v
        }
        FadeShape::Exponential => (t * t) * t,
        FadeShape::Logarithmic => t * ((3.0 - 3.0 * t) + t * t),
    }
}

/// One fade's `f64` kernel: its shape and its curvature weight `2^(4c)`, computed once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeKernel {
    shape: FadeShape,
    /// `2^(4c)`, or `None` when the sanitized curvature is 0.
    weight: Option<f64>,
}

impl FadeKernel {
    /// The kernel of `fade`, with its curvature sanitized (non-finite is 0, clamped to [−1, 1]).
    #[must_use]
    pub fn new(fade: Fade) -> Self {
        let c = if fade.curvature.is_finite() { fade.curvature.clamp(-1.0, 1.0) } else { 0.0 };
        let c = f64::from(c);
        FadeKernel { shape: fade.shape, weight: (c != 0.0).then(|| m64::exp2(4.0 * c)) }
    }

    /// The curved shape at `t` (NaN is 0, clamped to [0, 1]).
    #[must_use]
    pub fn factor(&self, t: f64) -> f64 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let b = shape(self.shape, t);
        match self.weight {
            Some(k) if b > 0.0 && b < 1.0 => {
                let w = b * k;
                w / ((w + 1.0) - b)
            }
            _ => b,
        }
    }
}

/// The fade-in and fade-out factors `(Fin(k), Fout(k))` of clip sample `k` in a clip of `length`
/// samples under the closed law (ADR 0058, decision 13). `length` below 1 counts as 1; `k` outside
/// `0..length` is moved to the nearest end; each fade's length is clamped to `0..=length`.
#[must_use]
pub fn fade_factors(length: Samples, fade_in: Fade, fade_out: Fade, k: Samples) -> (f64, f64) {
    let length = length.max(1);
    let k = k.clamp(0, length - 1);
    let m_in = fade_in.len.clamp(0, length);
    let m_out = fade_out.len.clamp(0, length);
    let fin = if m_in == 0 || k >= m_in {
        1.0
    } else if m_in == 1 {
        FadeKernel::new(fade_in).factor(0.5)
    } else {
        FadeKernel::new(fade_in).factor(k as f64 / (m_in - 1) as f64)
    };
    let fout = if m_out == 0 || k < length - m_out {
        1.0
    } else if m_out == 1 {
        FadeKernel::new(fade_out).factor(0.5)
    } else {
        FadeKernel::new(fade_out).factor((length - 1 - k) as f64 / (m_out - 1) as f64)
    };
    (fin, fout)
}

#[cfg(test)]
#[path = "fade_law_tests.rs"]
mod tests;
