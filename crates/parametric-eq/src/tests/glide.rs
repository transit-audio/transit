//! The per-sample coefficient glide between control-rate designs (the click amendment to decision
//! 3.1): evidence that the interpolated sets are stable at the points sampled, that nothing blows up
//! across the shelf transition, and that a band at rest is bit-identical to the plain design.
//!
//! What this is evidence for, and what it is not. ADR 0054 decision 3.1 argues stability in exact
//! arithmetic (two consecutive steps strictly contract, for endpoint designs in a compact set with
//! positive, finite `g` and `k`, at a fixed sample rate). The frozen-coefficient criterion used here,
//! a spectral radius below 1, is the weaker property, and it is checked in floating point at the
//! sampled points only: it proves nothing between them, at other rates, or at 1.536 MHz. The state
//! update is `[ic1, ic2]' = M·[ic1, ic2] + b·x` with `M = [[2a1 − 1, −2a2], [2a2, 1 − 2a3]]`; the
//! one-pole is stable when `0 < G < 1`. The sweep follows the processor's own smoothing (20 ms
//! one-poles stepped per sample, a design every 16 samples) between every pair of corners of the
//! frequency, gain and Q ranges, and checks all 16 interpolated points of every interval.
//!
//! The argued property itself, in f64 (decision 3.1's norm argument): for random triples of designs
//! of one type, slope and rate, drawn from the Evidence grids by an integer-seeded generator (no
//! platform math in the draw), the zero-input update matrix of every glided step of two consecutive
//! 16-sample intervals has Euclidean norm at most 1, each design's own has norm 1 within rounding,
//! and the product of every two consecutive steps (across the interval boundary too) has norm
//! strictly below 1. The negative control feeds the same check a deliberately unstable coefficient
//! set (`k < 0`), which it rejects. This is a floating-point check at the sampled triples, not the
//! exact-arithmetic argument.

use super::proc::{eq, run};
use super::util::{QS, RATES, Rng, band, band_freqs, gains, noise};
use crate::design::{Used, design};
use crate::section::Svf;
use crate::smooth::{Smoother, time_coef};
use crate::{BandParams, BandType, Placement, Slope};

fn radius(s: &Svf) -> f64 {
    let (m11, m12, m21, m22) = (2.0 * s.a1 - 1.0, -2.0 * s.a2, 2.0 * s.a2, 1.0 - 2.0 * s.a3);
    let tr = m11 + m22;
    let det = m11 * m22 - m12 * m21;
    let disc = 0.25 * tr * tr - det;
    if disc < 0.0 {
        det.sqrt()
    } else {
        let r = disc.sqrt();
        (0.5 * tr + r).abs().max((0.5 * tr - r).abs())
    }
}

/// The largest spectral radius over every interpolated point of every interval of one trajectory,
/// and the largest excess of an interpolated radius over the larger of its two endpoints' radii.
fn trajectory(kind: BandType, slope: Slope, from: (f64, f64, f64), to: (f64, f64, f64), fs: f64) -> (f64, f64) {
    let coef = time_coef(20.0, fs);
    let mut sm = [Smoother::new(from.0.log2()), Smoother::new(from.1), Smoother::new(from.2.log2())];
    for s in sm.iter_mut() {
        s.coef = coef;
    }
    sm[0].target = to.0.log2();
    sm[1].target = to.1;
    sm[2].target = to.2.log2();
    let mk = |sm: &[Smoother; 3]| design(kind, slope, Used::from_smoothed(sm[0].cur, sm[1].cur, sm[2].cur, fs), fs).coefficients();
    let mut prev = mk(&sm);
    let mut worst: f64 = 0.0;
    let mut excess = f64::NEG_INFINITY;
    for _ in 0..(0.3 * fs / 16.0) as usize {
        for _ in 0..16 {
            for s in sm.iter_mut() {
                s.step();
            }
        }
        let next = mk(&sm);
        if next.n_sos == prev.n_sos && next.n_sos > 0 {
            for n in 0..16 {
                let t = (n + 1) as f64 / 16.0;
                for j in 0..next.n_sos {
                    let r = radius(&Svf::lerp(&prev.sos[j], &next.sos[j], t));
                    worst = worst.max(r);
                    excess = excess.max(r - radius(&prev.sos[j]).max(radius(&next.sos[j])));
                }
                if let (Some(a), Some(b)) = (&prev.fos, &next.fos) {
                    let g = a.big_g + (b.big_g - a.big_g) * t;
                    assert!(g > 0.0 && g < 1.0, "{kind:?} one-pole G {g}");
                }
            }
        }
        prev = next;
    }
    (worst, excess)
}

#[test]
fn every_interpolated_coefficient_set_is_stable() {
    let f = [20.0, 20_000.0];
    let g = [-30.0, 30.0];
    let q = [0.1, 40.0];
    let mut corners = Vec::new();
    for a in f {
        for b in g {
            for c in q {
                corners.push((a, b, c));
            }
        }
    }
    let mut worst: f64 = 0.0;
    let mut excess = f64::NEG_INFINITY;
    let mut at = String::new();
    for fs in [44_100.0, 192_000.0] {
        for kind in BandType::ALL {
            let slopes: &[Slope] = if kind.uses_slope() { &Slope::ALL } else { &[Slope::Db24] };
            for &slope in slopes {
                for &a in &corners {
                    for &b in &corners {
                        if a == b {
                            continue;
                        }
                        let (r, x) = trajectory(kind, slope, a, b, fs);
                        excess = excess.max(x);
                        if r > worst {
                            worst = r;
                            at = format!("{kind:?} {slope:?} {a:?} -> {b:?} at {fs}");
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "glide stability: largest spectral radius {worst:.9} (margin {:.3e}) at {at}; largest excess over the endpoints' radii {excess:.3e}",
        1.0 - worst
    );
    assert!(worst < 1.0, "unstable interpolated coefficients: {worst} at {at}");
}

/// Shelves swept back and forth across the all-pole transition (0.9π to 0.99π) at the fastest rate
/// the smoothing allows keep a bounded state.
#[test]
fn shelves_gliding_across_the_transition_stay_bounded() {
    for fs in [44_100.0, 48_000.0] {
        for (kind, gain) in [(BandType::HighShelf, 30.0), (BandType::HighShelf, -30.0), (BandType::LowShelf, 30.0), (BandType::LowShelf, -30.0)] {
            for q in [0.707, 10.0, 40.0] {
                let mut e = eq(fs, 2);
                e.set_band(0, band(kind, 9_000.0, gain, q));
                e.reset();
                let mut peak: f64 = 0.0;
                for k in 0..80 {
                    e.set_freq(0, if k % 2 == 0 { 20_000.0 } else { 9_000.0 });
                    let mut io = noise(1_024, 2, k as u64 + 1);
                    run(&mut e, &mut io, 64);
                    for x in io.iter().flatten() {
                        assert!(x.is_finite());
                        peak = peak.max(f64::from(*x).abs());
                    }
                }
                assert!(peak < 1.0e3, "{kind:?} {gain} Q {q} at {fs}: peak {peak}");
            }
        }
    }
}

/// Once a band rests (no redesign in the interval), its coefficients are exactly the plain design's
/// and the output is bit-identical to a band that never moved, given the same states.
#[test]
fn a_band_at_rest_runs_the_plain_design_bit_for_bit() {
    let target = BandParams { place: Placement::Stereo, ..band(BandType::HighShelf, 7_000.0, 9.0, 2.0) };
    let mut moved = eq(48_000.0, 2);
    moved.set_band(0, band(BandType::HighShelf, 300.0, -12.0, 0.3));
    moved.reset();
    moved.set_band(0, target);
    let mut warm = noise(48_000, 2, 5);
    run(&mut moved, &mut warm, 64);
    assert!(!moved.bands[0].interp, "still gliding after one second");
    let mut still = eq(48_000.0, 2);
    still.set_band(0, target);
    still.reset();
    assert_eq!(moved.bands[0].coefs, still.bands[0].coefs);
    still.bands[0].states = moved.bands[0].states.clone();
    // Align the control phase: both continue from a tick boundary.
    assert_eq!(moved.clock % 16, 0);
    let input = noise(4_800, 2, 6);
    let (mut a, mut b) = (input.clone(), input);
    run(&mut moved, &mut a, 64);
    run(&mut still, &mut b, 64);
    for c in 0..2 {
        assert!(a[c].iter().zip(&b[c]).all(|(x, y)| x.to_bits() == y.to_bits()), "channel {c}");
    }
}

type Mat = [[f64; 2]; 2];

/// The zero-input update matrix of the trapezoidal tick.
fn matrix(s: &Svf) -> Mat {
    [[2.0 * s.a1 - 1.0, -2.0 * s.a2], [2.0 * s.a2, 1.0 - 2.0 * s.a3]]
}

/// `b·a`: the step `a` first, then the step `b`.
fn mul(b: &Mat, a: &Mat) -> Mat {
    let at = |i: usize, j: usize| b[i][0] * a[0][j] + b[i][1] * a[1][j];
    [[at(0, 0), at(0, 1)], [at(1, 0), at(1, 1)]]
}

/// The Euclidean (spectral) norm of a 2x2 matrix, without cancellation:
/// `(√((a + d)² + (b − c)²) + √((a − d)² + (b + c)²)) / 2`.
fn norm(m: &Mat) -> f64 {
    0.5 * ((m[0][0] + m[1][1]).hypot(m[0][1] - m[1][0]) + (m[0][0] - m[1][1]).hypot(m[0][1] + m[1][0]))
}

/// Checks one section across two glided intervals `a → b → c`: each design's norm is 1 within
/// rounding, every blend's norm is at most 1 (within rounding), and every two consecutive steps have
/// norm strictly below 1. Returns the largest two-step norm, or what failed.
fn two_step_contracts(a: &Svf, b: &Svf, c: &Svf) -> Result<f64, String> {
    for (name, d) in [("a", a), ("b", b), ("c", c)] {
        let n = norm(&matrix(d));
        if (n - 1.0).abs() > 1e-10 {
            return Err(format!("design {name} has norm {n}, not 1"));
        }
    }
    let mut steps = Vec::with_capacity(32);
    for (from, to) in [(a, b), (b, c)] {
        for n in 0..16 {
            steps.push(matrix(&Svf::lerp(from, to, (n + 1) as f64 / 16.0)));
        }
    }
    let mut worst: f64 = 0.0;
    for (i, m) in steps.iter().enumerate() {
        let n = norm(m);
        if n > 1.0 + 1e-10 {
            return Err(format!("step {i} has norm {n} > 1"));
        }
        if let Some(next) = steps.get(i + 1) {
            let two = norm(&mul(next, m));
            if two >= 1.0 {
                return Err(format!("steps {i} and {} have product norm {two}, not below 1", i + 1));
            }
            worst = worst.max(two);
        }
    }
    Ok(worst)
}

#[test]
fn two_consecutive_glided_steps_strictly_contract() {
    let (freqs, gs) = (band_freqs(), gains());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut pick = |n: usize| (rng.next_u64() % n as u64) as usize;
    let mut worst: f64 = 0.0;
    let mut sections = 0usize;
    for _ in 0..4_000 {
        let kind = BandType::ALL[pick(BandType::ALL.len())];
        let slope = Slope::ALL[pick(Slope::ALL.len())];
        let fs = RATES[pick(RATES.len())];
        let mut one = || {
            let used = Used::from_stored(freqs[pick(freqs.len())], gs[pick(gs.len())], QS[pick(QS.len())], fs);
            design(kind, slope, used, fs).coefficients()
        };
        let (a, b, c) = (one(), one(), one());
        // The 6 dB/oct cuts are one first-order section and have no second-order one to check here.
        assert!(a.n_sos == b.n_sos && b.n_sos == c.n_sos, "{kind:?} {slope:?}");
        for j in 0..a.n_sos {
            match two_step_contracts(&a.sos[j], &b.sos[j], &c.sos[j]) {
                Ok(w) => worst = worst.max(w),
                Err(e) => panic!("{kind:?} {slope:?} {fs} Hz section {j}: {e}"),
            }
            sections += 1;
        }
    }
    eprintln!("{sections} sections, largest two-step norm {worst:.12}");
    assert!(sections > 5_000 && worst < 1.0);
}

/// Negative control: a coefficient set whose `g` and `k` are not both positive (`g = 0.5`, `k = −0.5`,
/// so `a1 = 1`, `a2 = 0.5`, `a3 = 0.25`) is not a design with its poles inside the circle, and the
/// check rejects it.
#[test]
fn the_contraction_check_rejects_an_unstable_coefficient_set() {
    let (g, k): (f64, f64) = (0.5, -0.5);
    let a1 = 1.0 / (1.0 + g * (g + k));
    let unstable = Svf { g, k, a1, a2: g * a1, a3: g * g * a1, ..Svf::default() };
    assert!(norm(&matrix(&unstable)) > 1.0);
    assert!(two_step_contracts(&unstable, &unstable, &unstable).is_err());
}
