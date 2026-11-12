//! Shelf continuity across the all-pole transition (Evidence "Shelves over the whole Q range",
//! continuity): sweeping each shelf type's frequency from 20 Hz to the top of the range (20 kHz or
//! 0.49·fs) in steps of 1 cent, at Qs 0.1, 0.707, 2, 10 and 40 and gains ±3, ±12 and ±30 dB, the
//! response at 200 log-spaced frequencies changes in no step by more than the prototype's own change
//! in the same step plus 0.2 dB, nor by more than 0.45 dB; at the worst case the change for a 0.1-cent
//! step is a tenth of the 1-cent change within 5 % (a slope, not a jump). Negative control
//! (in-process comparison): the round-2 hard switch at `θ = 0.9π` steps by more than 5 dB at 44.1 kHz.

use super::util::{db, log_grid};
use crate::design::{Used, design, factor_hard_switch};
use crate::math::TAU;
use crate::prototype::Prototype;
use crate::section::Sos;
use crate::{BandType, Slope};

const QS: [f64; 5] = [0.1, 0.707, 2.0, 10.0, 40.0];
const GAINS: [f64; 6] = [-30.0, -12.0, -3.0, 3.0, 12.0, 30.0];
const TYPES: [BandType; 2] = [BandType::LowShelf, BandType::HighShelf];

struct Eval {
    f: Vec<f64>,
    sc: Vec<(f64, f64)>,
}

impl Eval {
    fn new(fs: f64) -> Self {
        let f = log_grid(20.0, 20_000f64.min(0.45 * fs), 200);
        let sc = f.iter().map(|&f| ((std::f64::consts::PI * f / fs).sin(), (std::f64::consts::PI * f / fs).cos())).collect();
        Self { f, sc }
    }
}

/// The designed and the prototype responses in dB of a shelf at `f0`.
fn responses(kind: BandType, f0: f64, gain: f64, q: f64, fs: f64, e: &Eval, d_out: &mut [f64], p_out: &mut [f64]) {
    let used = Used { f0, gain_db: gain, q };
    let d = design(kind, Slope::Db24, used, fs);
    let p = Prototype::new(kind, Slope::Db24, f0, gain, q);
    for i in 0..e.f.len() {
        d_out[i] = db(d.power(e.sc[i].0, e.sc[i].1));
        p_out[i] = db(p.power(e.f[i]));
    }
}

/// The hard-switch shelf's response in dB (the negative control's design).
fn hard_responses(kind: BandType, f0: f64, gain: f64, q: f64, fs: f64, e: &Eval, out: &mut [f64]) {
    let w0 = TAU * f0 / fs;
    let a = 10f64.powf(gain / 40.0);
    let sa = a.sqrt();
    let zeta = 1.0 / (2.0 * q);
    let (wp, wz, k) = if kind == BandType::LowShelf { (w0 / sa, w0 * sa, a * a) } else { (w0 * sa, w0 / sa, 1.0) };
    let p: Sos = factor_hard_switch(wp, zeta);
    let z: Sos = factor_hard_switch(wz, zeta).inverted();
    for i in 0..e.f.len() {
        let (s, c) = e.sc[i];
        out[i] = db(k * k * p.power(s, c) * z.power(s, c));
    }
}

struct Worst {
    step: f64,
    excess: f64,
    at: (BandType, f64, f64, f64),
}

fn sweep(fs: f64) -> Worst {
    let e = Eval::new(fs);
    let n = e.f.len();
    let top = 20_000f64.min(0.49 * fs);
    let steps = (1200.0 * (top / 20.0).log2()).floor() as usize;
    let mut w = Worst { step: 0.0, excess: 0.0, at: (BandType::LowShelf, 0.0, 0.0, 0.0) };
    let (mut d0, mut p0, mut d1, mut p1) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    for kind in TYPES {
        for gain in GAINS {
            for q in QS {
                responses(kind, 20.0, gain, q, fs, &e, &mut d0, &mut p0);
                for k in 1..=steps {
                    let f0 = 20.0 * 2f64.powf(k as f64 / 1200.0);
                    responses(kind, f0, gain, q, fs, &e, &mut d1, &mut p1);
                    for i in 0..n {
                        let sd = (d1[i] - d0[i]).abs();
                        let sp = (p1[i] - p0[i]).abs();
                        if sd > w.step {
                            w.step = sd;
                            w.at = (kind, f0 / 2f64.powf(1.0 / 1200.0), gain, q);
                        }
                        w.excess = w.excess.max(sd - sp);
                    }
                    std::mem::swap(&mut d0, &mut d1);
                    std::mem::swap(&mut p0, &mut p1);
                }
            }
        }
    }
    w
}

fn check(fs: f64) {
    let w = sweep(fs);
    eprintln!(
        "continuity {:>6.1} kHz: largest step {:.3} dB, largest excess over the prototype's step {:.3} dB, at {:?}",
        fs / 1000.0,
        w.step,
        w.excess,
        w.at
    );
    assert!(w.excess <= 0.2, "{fs}: excess {:.3} dB", w.excess);
    assert!(w.step <= 0.45, "{fs}: step {:.3} dB", w.step);
    // A slope, not a jump: at the worst case the 0.1-cent change is a tenth of the 1-cent change.
    let (kind, f0, gain, q) = w.at;
    let e = Eval::new(fs);
    let n = e.f.len();
    let (mut d0, mut p0, mut d1, mut d2) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    responses(kind, f0, gain, q, fs, &e, &mut d0, &mut p0);
    responses(kind, f0 * 2f64.powf(1.0 / 1200.0), gain, q, fs, &e, &mut d1, &mut p0);
    responses(kind, f0 * 2f64.powf(0.1 / 1200.0), gain, q, fs, &e, &mut d2, &mut p0);
    let big = (0..n).map(|i| (d1[i] - d0[i]).abs()).fold(0.0, f64::max);
    let small = (0..n).map(|i| (d2[i] - d0[i]).abs()).fold(0.0, f64::max);
    let ratio = small / big;
    eprintln!("  worst case: 1 cent {big:.4} dB, 0.1 cent {small:.5} dB, ratio {ratio:.4}");
    assert!((ratio - 0.1).abs() <= 0.005, "{fs}: 0.1-cent change is {ratio:.4} of the 1-cent change");
}

#[test]
fn shelves_are_continuous_across_the_transition_at_44k1() {
    check(44_100.0);
}

#[test]
fn shelves_are_continuous_across_the_transition_at_48k() {
    check(48_000.0);
}

#[test]
fn shelves_are_continuous_across_the_transition_at_96k() {
    check(96_000.0);
}

#[test]
fn shelves_are_continuous_across_the_transition_at_192k() {
    check(192_000.0);
}

/// Negative control: the round-2 hard switch steps the response by more than 5 dB at 44.1 kHz. The
/// sweep covers the switch's neighborhood (6 kHz up) where every pair that can reach `0.9π` lies.
#[test]
fn the_hard_switch_steps_by_more_than_five_decibels() {
    let fs = 44_100.0;
    let e = Eval::new(fs);
    let n = e.f.len();
    let steps = (1200.0 * (20_000f64 / 6_000.0).log2()).floor() as usize;
    let mut worst: f64 = 0.0;
    let (mut a, mut b) = (vec![0.0; n], vec![0.0; n]);
    for kind in TYPES {
        for gain in GAINS {
            for q in QS {
                hard_responses(kind, 6_000.0, gain, q, fs, &e, &mut a);
                for k in 1..=steps {
                    hard_responses(kind, 6_000.0 * 2f64.powf(k as f64 / 1200.0), gain, q, fs, &e, &mut b);
                    for i in 0..n {
                        worst = worst.max((b[i] - a[i]).abs());
                    }
                    std::mem::swap(&mut a, &mut b);
                }
            }
        }
    }
    eprintln!("hard switch at 44.1 kHz: largest 1-cent step {worst:.2} dB");
    assert!(worst > 5.0, "the hard switch only steps {worst:.2} dB");
}
