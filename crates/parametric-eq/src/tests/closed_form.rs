//! Every band type against its closed form (Evidence item 1): the impulse response of each type and
//! slope, realized by the 64-bit state-variable sections, evaluated by a direct DFT in f64, matches the
//! digital transfer function of the designed σ coefficients within 0.05 dB where it is above −60 dB,
//! else within 1e-4 linear (the Filter test's bounds). This checks the realization of decision 2.3
//! against the designed transfer function to those tolerances. Negative control (recorded in the result note): an SVF map with `m1 = n1/c0` (the factor
//! `g` dropped) fails. In-process comparison; no pinned values.

use super::sweep::Case;
use super::util::{RATES, log_grid};
use crate::design::Design;
use crate::{BandType, Slope};

/// Runs the design's realization on a unit impulse until it has decayed (2048 samples in a row below
/// 1e-15, at most 2^19 samples) and returns the response.
pub(crate) fn impulse(d: &Design) -> Vec<f64> {
    let co = d.coefficients();
    let mut st = [[0.0f64; 2]; 8];
    let mut fos = 0.0f64;
    let mut out = Vec::new();
    let mut quiet = 0;
    for n in 0..(1usize << 19) {
        let mut y = if n == 0 { 1.0 } else { 0.0 };
        for (s, state) in co.sos[..co.n_sos].iter().zip(st.iter_mut()) {
            y = s.tick(state, y);
        }
        if let Some(f) = &co.fos {
            y = f.tick(&mut fos, y);
        }
        out.push(y);
        quiet = if y.abs() < 1e-15 { quiet + 1 } else { 0 };
        if quiet >= 2048 && n > 4096 {
            break;
        }
    }
    out
}

/// `|DFT|` of `h` at `w` radians per sample, by a rotating phasor renormalized every 1024 steps.
pub(crate) fn dft_mag(h: &[f64], w: f64) -> f64 {
    let (c, s) = (w.cos(), w.sin());
    let (mut pr, mut pi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &x) in h.iter().enumerate() {
        re += x * pr;
        im -= x * pi;
        let t = pr * c - pi * s;
        pi = pr * s + pi * c;
        pr = t;
        if n % 1024 == 1023 {
            let m = (pr * pr + pi * pi).sqrt();
            pr /= m;
            pi /= m;
        }
    }
    (re * re + im * im).sqrt()
}

fn all_cases() -> Vec<Case> {
    let mut v = Vec::new();
    let freqs = [200.0, 3_000.0, 15_000.0];
    for f0 in freqs {
        for kind in [BandType::Bell, BandType::LowShelf, BandType::HighShelf, BandType::TiltShelf] {
            for gain in [-12.0, 12.0] {
                for q in [0.7, 4.0] {
                    v.push(Case { kind, slope: Slope::Db24, f0, gain, q });
                }
            }
        }
        for kind in [BandType::Notch, BandType::BandPass] {
            for q in [0.7, 4.0] {
                v.push(Case { kind, slope: Slope::Db24, f0, gain: 0.0, q });
            }
        }
        for gain in [-12.0, 12.0] {
            v.push(Case { kind: BandType::FlatTilt, slope: Slope::Db24, f0, gain, q: 1.0 });
        }
        for slope in Slope::ALL {
            for kind in [BandType::LowCut, BandType::HighCut] {
                v.push(Case { kind, slope, f0, gain: 0.0, q: 1.0 });
            }
        }
    }
    v
}

#[test]
fn every_band_type_matches_its_designed_transfer_function() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for fs in RATES {
        let freqs = log_grid(20.0, 0.49 * fs, 24);
        for c in all_cases() {
            let d = c.design(fs);
            let h = impulse(&d);
            assert!(h.len() < 1 << 19, "{c:?} at {fs} did not decay");
            for &f in &freqs {
                let w = std::f64::consts::TAU * f / fs;
                let measured = dft_mag(&h, w);
                let exact = d.power((0.5 * w).sin(), (0.5 * w).cos()).sqrt();
                let ok =
                    if 20.0 * exact.log10() > -60.0 { (20.0 * (measured / exact).log10()).abs() <= 0.05 } else { (measured - exact).abs() <= 1e-4 };
                checked += 1;
                if !ok {
                    failures.push(format!("{c:?} at {fs} Hz, {f:.1} Hz: impulse {measured:.6e}, closed form {exact:.6e}"));
                }
            }
        }
    }
    eprintln!("closed form: {checked} points checked");
    assert!(failures.is_empty(), "{} failures, first: {:#?}", failures.len(), &failures[..failures.len().min(5)]);
}
