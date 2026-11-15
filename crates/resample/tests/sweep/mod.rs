//! The reader's acceptance measurement (ADR 0056, "The reader's acceptance measurement"), shared by
//! `tests/acceptance.rs` (density 1, against decision 8.4's bounds) and `examples/acceptance.rs` (the
//! convergence check at density 2).
//!
//! - **The analytic response**: for a rate `q` and a position `x = 1000 + φ`, the reader's own
//!   coefficients `c_k` (the crate's `f32` table and delta table, `u` in `f64`, then the `f32` factor
//!   `1/q` for `q > 1`) and `H(f, φ) = Σ c_k·e^(i2πf(k − x))` in `f64` for the source frequency `f`.
//!   Over every tone phase the largest error is `|H − 1|` (passband) and the largest amplitude `|H|`
//!   (alias). Along a frequency run, `e^(i2πf(k − x))` is advanced tap by tap by its own rotation
//!   (the same points; the rotation's rounding is far below the figures).
//! - **Rates, bands, phases, frequencies, refinement**: as the record states, scaled by `density`.
//! - **The full `f32` check** ([`f32_figure`]): the crate's reader on an `f32` tone whose phase shows the
//!   analytic worst case at the position.

#![allow(dead_code)]

use transit_math::f64::{pow, sin, sin_cos};
use transit_resample::{Frames, SincTable, read_frame, taps};

const TAU: f64 = std::f64::consts::TAU;

/// Passband or alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pass,
    Alias,
}

/// The bands the record reports: q < 1 (with q = 1 exact), 1 < q < 1.5 and 1.5 <= q <= 32.
pub const BANDS: [&str; 3] = ["q < 1", "1 < q < 1.5", "1.5 <= q <= 32"];

/// Decision 8.4's bounds in dB, by kind and band (`None`: no alias figure at q < 1).
pub fn bound(kind: Kind, band: usize) -> Option<f64> {
    match (kind, band) {
        (Kind::Pass, 0 | 1) => Some(-73.0),
        (Kind::Pass, _) => Some(-78.0),
        (Kind::Alias, 0) => None,
        (Kind::Alias, 1) => Some(-73.5),
        (Kind::Alias, _) => Some(-78.0),
    }
}

pub fn band(q: f64) -> usize {
    if q < 1.0 {
        0
    } else if q < 1.5 {
        1
    } else {
        2
    }
}

pub fn db(v: f64) -> f64 {
    20.0 * v.log10()
}

/// One measured point.
#[derive(Debug, Clone, Copy)]
pub struct Found {
    pub value: f64,
    pub q: f64,
    pub v: f64,
    pub phi: f64,
}

/// The six largest values seen.
#[derive(Debug, Default, Clone)]
pub struct Top(pub Vec<Found>);

impl Top {
    fn offer(&mut self, f: Found) {
        if self.0.len() == 6 && f.value <= self.0[5].value {
            return;
        }
        let at = self.0.iter().position(|e| f.value > e.value).unwrap_or(self.0.len());
        self.0.insert(at, f);
        self.0.truncate(6);
    }

    pub fn worst(&self) -> Option<Found> {
        self.0.first().copied()
    }
}

/// The reader's coefficients for one output sample at `x = 1000 + φ`: the first tap, `x`, and
/// `c_k` (exact in `f64`: the `f32` coefficient times the `f32` factor `1/q` for `q > 1`).
pub fn coefficients(t: &SincTable, q: f64, phi: f64) -> (i64, f64, Vec<f64>) {
    let x = 1000.0 + phi;
    let m = q.max(1.0);
    let scale = if q > 1.0 { f64::from((1.0 / q) as f32) } else { 1.0 };
    let r = taps(x, q);
    let k0 = r.start;
    let c = r
        .map(|k| {
            let d = (x - k as f64).abs();
            let u = if q > 1.0 { d / q } else { d };
            debug_assert!(u < 16.0 && m >= 1.0);
            f64::from(t.coefficient(u)) * scale
        })
        .collect();
    (k0, x, c)
}

/// `H` at the source frequencies `f_start + n·f_step`, `n` in `0..count`, handed to `each(n, re, im)`.
fn run(k0: i64, x: f64, c: &[f64], f_start: f64, f_step: f64, count: usize, mut each: impl FnMut(usize, f64, f64)) {
    let mut pr = Vec::with_capacity(c.len());
    let mut pi = Vec::with_capacity(c.len());
    let mut sr = Vec::with_capacity(c.len());
    let mut si = Vec::with_capacity(c.len());
    for j in 0..c.len() {
        let t = (k0 + j as i64) as f64 - x;
        let (s0, c0) = sin_cos(TAU * f_start * t);
        let (s1, c1) = sin_cos(TAU * f_step * t);
        pr.push(c0);
        pi.push(s0);
        sr.push(c1);
        si.push(s1);
    }
    for n in 0..count {
        let (mut hr, mut hi) = (0.0, 0.0);
        for ((((&cj, pr), pi), &sr), &si) in c.iter().zip(pr.iter_mut()).zip(pi.iter_mut()).zip(&sr).zip(&si) {
            hr += cj * *pr;
            hi += cj * *pi;
            let (a, b) = (*pr, *pi);
            *pr = a * sr - b * si;
            *pi = a * si + b * sr;
        }
        each(n, hr, hi);
    }
}

/// `H` at one source frequency, directly.
pub fn response(k0: i64, x: f64, c: &[f64], f: f64) -> (f64, f64) {
    let (mut hr, mut hi) = (0.0, 0.0);
    for (j, &cj) in c.iter().enumerate() {
        let (s, co) = sin_cos(TAU * f * ((k0 + j as i64) as f64 - x));
        hr += cj * co;
        hi += cj * s;
    }
    (hr, hi)
}

fn value(kind: Kind, hr: f64, hi: f64) -> f64 {
    match kind {
        Kind::Pass => ((hr - 1.0) * (hr - 1.0) + hi * hi).sqrt(),
        Kind::Alias => (hr * hr + hi * hi).sqrt(),
    }
}

/// One point's analytic figure.
pub fn eval(t: &SincTable, kind: Kind, q: f64, v: f64, phi: f64) -> f64 {
    let (k0, x, c) = coefficients(t, q, phi);
    let f = match kind {
        Kind::Pass => v / q.max(1.0),
        Kind::Alias => v / q,
    };
    let (hr, hi) = response(k0, x, &c, f);
    value(kind, hr, hi)
}

/// The main grid's six largest values per kind and band. `density` 1 is the record's main sweep, 2 its
/// convergence check (regular grids halved, every N doubled, every frequency step halved).
pub fn main_grid(t: &SincTable, density: u32) -> [[Top; 3]; 2] {
    let d = f64::from(density);
    let dn = density as usize;
    let mut top: [[Top; 3]; 2] = Default::default();
    let pass_step = 0.001 / d;
    let pass_count = 415 * dn + 1;
    let point = |t: &SincTable, q: f64, n_phi: usize, pass: bool, alias: bool, top: &mut [[Top; 3]; 2]| {
        let m = q.max(1.0);
        let b = band(q);
        for j in 0..n_phi {
            let phi = j as f64 / n_phi as f64;
            let (k0, x, c) = coefficients(t, q, phi);
            if pass {
                run(k0, x, &c, 0.005 / m, pass_step / m, pass_count, |n, hr, hi| {
                    let v = 0.005 + n as f64 * pass_step;
                    top[0][b].offer(Found { value: value(Kind::Pass, hr, hi), q, v, phi });
                });
            }
            if alias && q / 2.0 > 0.58 {
                let s1 = 0.0005 / d;
                let n1 = ((q / 2.0).min(1.2) - 0.58) / s1 - 1e-9;
                let n1 = n1.floor() as usize + 1;
                run(k0, x, &c, 0.58 / q, s1 / q, n1, |n, hr, hi| {
                    let v = 0.58 + n as f64 * s1;
                    if v / q < 0.5 {
                        top[1][b].offer(Found { value: value(Kind::Alias, hr, hi), q, v, phi });
                    }
                });
                if q / 2.0 > 1.2 {
                    let s2 = 0.01 / d;
                    let n2 = ((q / 2.0 - 1.2) / s2 - 1e-9).floor() as usize;
                    run(k0, x, &c, (1.2 + s2) / q, s2 / q, n2, |n, hr, hi| {
                        let v = 1.2 + (n + 1) as f64 * s2;
                        if v / q < 0.5 {
                            top[1][b].offer(Found { value: value(Kind::Alias, hr, hi), q, v, phi });
                        }
                    });
                }
            }
        }
    };
    // Below q = 1 the coefficients do not depend on q: one sweep at q = 0.5.
    point(t, 0.5, 2048 * dn, true, false, &mut top);
    // The low grid and the irregular rates.
    let delta = if density == 1 { 0.002 } else { 0.001 };
    let mut low: Vec<f64> = (1..=500 * dn).map(|i| 1.0 + i as f64 * delta).collect();
    low.extend([1.0 + 1e-9, 1.0 + 1e-6, 1.0 + 1e-4, 1.0 + 1e-3, 1.5 + 1e-9, 1.5 + 1e-6, 1.5 + 1e-4, 1.501, 1.506, 1.51]);
    low.sort_by(f64::total_cmp);
    low.dedup();
    for &q in &low {
        point(t, q, 64 * dn, true, true, &mut top);
    }
    // The high grids.
    for i in 1..=400 * dn {
        point(t, 2.0 * pow(16.0, i as f64 / (400 * dn) as f64), 32 * dn, true, false, &mut top);
    }
    for i in 1..=300 * dn {
        point(t, 2.0 * pow(16.0, i as f64 / (300 * dn) as f64), 16 * dn, false, true, &mut top);
    }
    top
}

/// The largest analytic figure at one rate over `n_phi` phases and the main sweep's frequencies.
pub fn rate_worst(t: &SincTable, kind: Kind, q: f64, n_phi: usize) -> f64 {
    let m = q.max(1.0);
    let mut worst = 0.0f64;
    for j in 0..n_phi {
        let (k0, x, c) = coefficients(t, q, j as f64 / n_phi as f64);
        let mut keep = |_: usize, hr: f64, hi: f64| worst = worst.max(value(kind, hr, hi));
        match kind {
            Kind::Pass => run(k0, x, &c, 0.005 / m, 0.001 / m, 416, &mut keep),
            Kind::Alias if q / 2.0 > 0.58 => {
                let n1 = (((q / 2.0).min(1.2) - 0.58) / 0.0005 - 1e-9).floor() as usize + 1;
                run(k0, x, &c, 0.58 / q, 0.0005 / q, n1, &mut keep);
                if q / 2.0 > 1.2 {
                    let n2 = ((q / 2.0 - 1.2) / 0.01 - 1e-9).floor() as usize;
                    run(k0, x, &c, 1.21 / q, 0.01 / q, n2, &mut keep);
                }
            }
            Kind::Alias => {}
        }
    }
    worst
}

fn admissible(kind: Kind, b: usize, q: f64, v: f64) -> bool {
    let q_ok = match b {
        0 => true,
        1 => q > 1.0 && q < 1.5,
        _ => (1.5..=32.0).contains(&q),
    };
    let v_ok = match kind {
        Kind::Pass => (0.005..=0.42).contains(&v),
        Kind::Alias => v >= 0.58 && v / q < 0.5,
    };
    q_ok && v_ok
}

/// The record's refinement of one main-grid point: five rounds of 125 points around the best so far,
/// the half-widths halved each round.
pub fn refine(t: &SincTable, kind: Kind, b: usize, start: Found) -> Found {
    let mut dq = match b {
        0 => 0.0,
        _ if start.q <= 2.0 => 0.002,
        _ => 0.007 * start.q,
    };
    let mut dv = if kind == Kind::Pass { 0.001 } else { 0.0005 };
    let mut dphi = 1.0 / 64.0;
    let mut best: Option<Found> = None;
    let mut centre = start;
    for _ in 0..5 {
        for a in -2i32..=2 {
            for bb in -2i32..=2 {
                for c in -2i32..=2 {
                    let q = centre.q + f64::from(a) * dq / 2.0;
                    let v = centre.v + f64::from(bb) * dv / 2.0;
                    let phi = (centre.phi + f64::from(c) * dphi / 2.0).rem_euclid(1.0);
                    if !admissible(kind, b, q, v) {
                        continue;
                    }
                    let value = eval(t, kind, q, v, phi);
                    if best.is_none_or(|e| value > e.value) {
                        best = Some(Found { value, q, v, phi });
                    }
                }
            }
        }
        centre = best.unwrap_or(centre);
        dq /= 2.0;
        dv /= 2.0;
        dphi /= 2.0;
    }
    best.unwrap_or(start)
}

/// An `f32` tone `sin(2πf·k + θ)` at the reader's taps (each sample computed in `f64`, rounded once).
struct Tone {
    f: f64,
    theta: f64,
}

impl Frames for Tone {
    fn channels(&self) -> usize {
        1
    }

    fn sample(&self, _channel: usize, frame: i64) -> f32 {
        sin(TAU * self.f * frame as f64 + self.theta) as f32
    }
}

/// The full `f32` reader at a point, the tone's phase chosen so that the position shows the analytic
/// worst case: the error against the exact tone (passband) or the output's magnitude (alias).
pub fn f32_figure(t: &SincTable, kind: Kind, p: Found) -> f64 {
    let (k0, x, c) = coefficients(t, p.q, p.phi);
    let f = match kind {
        Kind::Pass => p.v / p.q.max(1.0),
        Kind::Alias => p.v / p.q,
    };
    let (hr, hi) = response(k0, x, &c, f);
    let (re, im) = match kind {
        Kind::Pass => (hr - 1.0, hi),
        Kind::Alias => (hr, hi),
    };
    // Im(e^(i(2πfx + θ))·(re + i·im)) is largest, |re + i·im|, when the angle sum is π/2.
    let theta = std::f64::consts::FRAC_PI_2 - im.atan2(re) - TAU * f * x;
    let tone = Tone { f, theta };
    let mut y = [0.0f32];
    read_frame(t, &tone, x, p.q, &mut y);
    let y = f64::from(y[0]);
    match kind {
        Kind::Pass => (y - sin(TAU * f * x + theta)).abs(),
        Kind::Alias => y.abs(),
    }
}
