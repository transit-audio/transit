//! The single σ-domain fit (decision 2.2): bells, notch, band pass and the cuts. Each second-order
//! section maps its prototype's poles exactly and fits its zeros at DC, Nyquist and the band
//! frequency `ω0` (clamped to 0.49 of the rate), with the exceptions by type: high-pass sections keep
//! their double zero at DC and fit only their gain at Nyquist, the notch keeps its zeros on the unit
//! circle at `ω0` and fits its gain at DC, first-order sections fit `b0` and `b1` at DC and Nyquist.

use super::{Design, MATCH_MAX, amplitude_a, fit2, fit3, map_pair, map_real};
use crate::math::{self, PI};
use crate::section::{Fos, Sos};
use crate::{Slope, butterworth, prototype};

/// A bell of `gain_db` (non-zero). A boost maps the poles of `s² + s/(AQ) + 1` and fits the zeros; a
/// cut is designed as the reciprocal of its mirror (the boost of −G) and inverted, so the mapped
/// poles are always the lower-frequency pair and stay below Nyquist.
pub(super) fn bell(w0: f64, gain_db: f64, q: f64) -> Design {
    let am = amplitude_a(gain_db.abs());
    let c = map_pair(w0, 1.0 / (2.0 * am * q));
    let wm = w0.min(MATCH_MAX);
    let tpi = prototype::bell(PI / w0, am, q);
    let tm = prototype::bell(wm / w0, am, q);
    let (n, clamped) = fit3(c, 1.0, tpi, tm, wm);
    let mirror = Sos { n, c };
    let mut d = Design::default();
    d.push(if gain_db > 0.0 { mirror } else { mirror.inverted() });
    d.clamped = clamped as u32;
    d
}

/// The notch: poles mapped, zeros on the unit circle at `ω0` (`n1 = 0`, `n0/n2 = tan²(ω0/2)`), gain
/// fitted at DC.
pub(super) fn notch(w0: f64, q: f64) -> Design {
    let c = map_pair(w0, 1.0 / (2.0 * q));
    let s = math::sin(0.5 * w0);
    let co = math::cos(0.5 * w0);
    let n0 = c[0];
    let n2 = n0 * (co * co) / (s * s);
    let mut d = Design::default();
    d.push(Sos { n: [n0, 0.0, n2], c });
    d
}

/// The band pass: poles mapped, the general three-point fit (its zero at DC comes from `T(0) = 0`).
pub(super) fn band_pass(w0: f64, q: f64) -> Design {
    let c = map_pair(w0, 1.0 / (2.0 * q));
    let wm = w0.min(MATCH_MAX);
    let (n, clamped) = fit3(c, 0.0, prototype::band_pass(PI / w0, q), prototype::band_pass(wm / w0, q), wm);
    let mut d = Design::default();
    d.push(Sos { n, c });
    d.clamped = clamped as u32;
    d
}

/// A Butterworth cut of `slope` (decision 2.1): `N/2` second-order sections by ascending Q, then a
/// first-order section for odd `N`. `high_pass` is the low cut.
pub(super) fn cut(w0: f64, slope: Slope, high_pass: bool) -> Design {
    let mut d = Design::default();
    for &q in butterworth::QS[slope.index()] {
        let (s, clamped) = cut_section(w0, q, high_pass);
        d.clamped += clamped as u32;
        d.push(s);
    }
    if slope.order() % 2 == 1 {
        let den = map_real(w0);
        let u_pi = PI / w0;
        let n = if high_pass { fit2(den, 0.0, prototype::high_pass1(u_pi)) } else { fit2(den, 1.0, prototype::low_pass1(u_pi)) };
        d.fos = Some(Fos { n, d: den });
    }
    d
}

/// One second-order cut section of Q `q`: poles mapped; a high-pass section keeps its double zero at
/// DC (`b = g·(1, −2, 1)` is `4g·σ²`) and fits `g` at Nyquist, a low-pass section takes the general fit.
pub(crate) fn cut_section(w0: f64, q: f64, high_pass: bool) -> (Sos, bool) {
    let c = map_pair(w0, 1.0 / (2.0 * q));
    let u_pi = PI / w0;
    if high_pass {
        let n2 = math::sqrt(prototype::high_pass2(u_pi, q)) * c[2];
        (Sos { n: [0.0, 0.0, n2], c }, false)
    } else {
        let wm = w0.min(MATCH_MAX);
        let (n, clamped) = fit3(c, 1.0, prototype::low_pass2(u_pi, q), prototype::low_pass2(wm / w0, q), wm);
        (Sos { n, c }, clamped)
    }
}
