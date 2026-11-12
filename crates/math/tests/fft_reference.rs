//! The FFT against an arbitrary-precision DFT (ADR 0055 slice 2b): `tests/data/fft_reference.txt`,
//! generated once by `tests/data/fft_reference.py` (stdlib Python, `python -I`) from the same
//! integer-generated inputs, holds the true forward DFT to 30 significant digits for a real and a
//! complex transform of 256 points and a real transform of 1024 points.
//!
//! The measure is the largest bin error over the largest bin magnitude, in unit roundoffs of the width
//! (for `f64` it includes the reference's own rounding to `f64`, at most half an ulp of each part).
//! Bound: `log2(n)` unit roundoffs. The measured values are printed and recorded in the slice's result.

mod fft_support;

use fft_support::{Eps, cplx, log2, real, rel_err_eps, table};
use transit_math::fft::{self, Complex};

const DATA: &str = include_str!("data/fft_reference.txt");

struct Case {
    kind: &'static str,
    n: usize,
    seed: u32,
    bins: Vec<(f64, f64)>,
}

fn cases() -> Vec<Case> {
    let mut out: Vec<Case> = Vec::new();
    for line in DATA.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f[0] == "case" {
            let kind = if f[1] == "real" { "real" } else { "complex" };
            let seed = u32::from_str_radix(f[3].trim_start_matches("0x"), 16).unwrap();
            out.push(Case { kind, n: f[2].parse().unwrap(), seed, bins: Vec::new() });
        } else {
            let case = out.last_mut().unwrap();
            assert_eq!(f[0].parse::<usize>().unwrap(), case.bins.len());
            case.bins.push((f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    out
}

fn measure<T: Eps>() -> f64 {
    let mut worst = 0.0f64;
    let all = cases();
    assert_eq!(all.len(), 3);
    for c in &all {
        let tw = table::<T>(c.n);
        let got: Vec<Complex<T>> = if c.kind == "real" {
            assert_eq!(c.bins.len(), c.n / 2 + 1);
            let x = real::<T>(c.seed, c.n);
            let mut spec = vec![Complex::default(); c.n / 2 + 1];
            fft::forward_real(&tw, &x, &mut spec).unwrap();
            spec
        } else {
            assert_eq!(c.bins.len(), c.n);
            let mut x = cplx::<T>(c.seed, c.n);
            fft::forward(&tw, &mut x).unwrap();
            x
        };
        let e = rel_err_eps(&got, &c.bins);
        println!("{} {} {}: {e:.3} unit roundoffs of the largest bin", core::any::type_name::<T>(), c.kind, c.n);
        assert!(e <= log2(c.n), "{} {}: {e} > {}", c.kind, c.n, log2(c.n));
        worst = worst.max(e);
    }
    worst
}

#[test]
fn the_fft_matches_the_arbitrary_precision_dft_within_the_bound() {
    measure::<f32>();
    measure::<f64>();
}
