//! Shelves and the tilt shelf: two factor sections (decision 2.2).
//!
//! A shelf is a constant times a pole-pair factor `P(ω_f) = ω_f²/(s² + 2ζω_f s + ω_f²)` and a
//! zero-pair factor `Z(ω_f) = 1/P(ω_f)` with the same damping `ζ = 1/(2Q)`: the low shelf is
//! `A²·P(ω0/√A)·Z(ω0·√A)`, the high shelf `P(ω0·√A)·Z(ω0/√A)`, the tilt shelf the high shelf times
//! `10^(−G/40)`. `P` is a matched low-pass section; `Z` is the matched `P` inverted. A pair at the top
//! of the band blends continuously into an all-pole form: `β` rises by a smoothstep from 0 at
//! `θ = 0.9π` to 1 at `θ = 0.99π`; only the denominator is blended (both forms normalized so
//! `c0 + c1 + c2 = 1`) and the numerator is refitted to the true `|P|²` at DC, Nyquist and
//! `ωm = (1 − β)·min(ω_f, 0.9π) + β·π/2`. The constant multiplies the first section's numerator; the
//! sections run `P` then `Z`.

use super::{Design, amplitude_a, fit3, map_pair};
use crate::math::{self, PI};
use crate::prototype::pole_pair;
use crate::section::Sos;

/// The angle where the all-pole transition starts, `0.9π`.
const BLEND_START: f64 = 0.9 * PI;
/// The width of the transition, `0.09π` (it ends at `0.99π`).
const BLEND_WIDTH: f64 = 0.09 * PI;

/// The low shelf of `gain_db` (non-zero) at `w0` radians per sample.
pub(super) fn low(w0: f64, gain_db: f64, q: f64) -> Design {
    let a = amplitude_a(gain_db);
    let sa = math::sqrt(a);
    factored(w0 / sa, w0 * sa, q, a * a)
}

/// The high shelf, or with `tilt` the tilt shelf (the high shelf times `10^(−G/40)`).
pub(super) fn high(w0: f64, gain_db: f64, q: f64, tilt: bool) -> Design {
    let a = amplitude_a(gain_db);
    let sa = math::sqrt(a);
    let k = if tilt { amplitude_a(-gain_db) } else { 1.0 };
    factored(w0 * sa, w0 / sa, q, k)
}

/// `k·P(wp)·Z(wz)` with `ζ = 1/(2Q)`.
fn factored(wp: f64, wz: f64, q: f64, k: f64) -> Design {
    let zeta = 1.0 / (2.0 * q);
    let (p, cp) = pole_factor(wp, zeta);
    let (z, cz) = pole_factor(wz, zeta);
    let mut d = Design::default();
    d.push(p);
    d.push(z.inverted());
    d.scale_first(k);
    d.clamped = cp as u32 + cz as u32;
    d
}

/// The smoothstep weight `β` of the all-pole form for a pair at mapped angle `θ`.
fn blend_weight(theta: f64) -> f64 {
    if theta <= BLEND_START {
        0.0
    } else if theta >= BLEND_START + BLEND_WIDTH {
        1.0
    } else {
        let x = (theta - BLEND_START) / BLEND_WIDTH;
        x * x * (3.0 - 2.0 * x)
    }
}

/// The section of `P(ω_f)`, with whether a fit was clamped.
pub(crate) fn pole_factor(wf: f64, zeta: f64) -> (Sos, bool) {
    let target = |w: f64| pole_pair(w, wf, zeta);
    let mapped = |c: [f64; 3]| {
        let wm = wf.min(BLEND_START);
        let (n, clamped) = fit3(c, 1.0, target(PI), target(wm), wm);
        (Sos { n, c }, clamped)
    };
    if zeta >= 1.0 {
        // Real pairs always map, whatever their frequency.
        return mapped(map_pair(wf, zeta));
    }
    let beta = blend_weight(wf * math::sqrt(1.0 - zeta * zeta));
    if beta == 0.0 {
        return mapped(map_pair(wf, zeta));
    }
    // The all-pole form: the inverse of a zeros-only section (denominator 1, `c = (1, 2, 1)` in σ)
    // whose numerator is fitted to `1/|P|²` at DC, π/2 and Nyquist.
    let (nz, cl_ap) = fit3([1.0, 2.0, 1.0], 1.0, 1.0 / target(PI), 1.0 / target(0.5 * PI), 0.5 * PI);
    if beta == 1.0 {
        return (Sos { n: [1.0, 2.0, 1.0], c: nz }, cl_ap);
    }
    let cm = normalized(map_pair(wf, zeta));
    let ca = normalized(nz);
    let mut c = [0.0; 3];
    for i in 0..3 {
        c[i] = (1.0 - beta) * cm[i] + beta * ca[i];
    }
    let wm = (1.0 - beta) * BLEND_START + beta * (0.5 * PI);
    let (n, clamped) = fit3(c, 1.0, target(PI), target(wm), wm);
    (Sos { n, c }, clamped || cl_ap)
}

fn normalized(c: [f64; 3]) -> [f64; 3] {
    let s = c[0] + c[1] + c[2];
    [c[0] / s, c[1] / s, c[2] / s]
}

/// Test comparison: `P(ω_f)` with the round-2 hard switch at `θ = 0.9π` (all-pole at or past it).
#[cfg(test)]
pub(crate) fn factor_hard_switch(wf: f64, zeta: f64) -> Sos {
    if zeta < 1.0 && wf * math::sqrt(1.0 - zeta * zeta) >= BLEND_START {
        let target = |w: f64| pole_pair(w, wf, zeta);
        let (nz, _) = fit3([1.0, 2.0, 1.0], 1.0, 1.0 / target(PI), 1.0 / target(0.5 * PI), 0.5 * PI);
        Sos { n: [1.0, 2.0, 1.0], c: nz }
    } else {
        // Below the switch the continuous design is the mapped section (β = 0).
        pole_factor(wf, zeta).0
    }
}

/// Test comparison: a shelf by the single three-point fit (the rejected design of decision 2.2): the
/// lower-frequency pair as mapped poles (a cut low shelf and a boost high shelf are their mirror
/// inverted, as for bells), the zeros fitted at DC, Nyquist and `ω0`. `low` picks the type.
#[cfg(test)]
pub(crate) fn shelf_single_fit(w0: f64, gain_db: f64, q: f64, low: bool) -> Sos {
    use crate::prototype::{high_shelf, low_shelf};
    // The designed form: a low shelf boost or a high shelf cut, whose poles are the lower pair.
    let g = if low { gain_db.abs() } else { -gain_db.abs() };
    let a = amplitude_a(g);
    let sa = math::sqrt(a);
    let zeta = 1.0 / (2.0 * q);
    let c = map_pair(if low { w0 / sa } else { w0 * sa }, zeta);
    let t = |u: f64| if low { low_shelf(u, a, sa, q) } else { high_shelf(u, a, sa, q) };
    let wm = w0.min(super::MATCH_MAX);
    let (n, _) = fit3(c, t(0.0), t(PI / w0), t(wm / w0), wm);
    let s = Sos { n, c };
    if g == gain_db { s } else { s.inverted() }
}
