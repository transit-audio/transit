//! Inputs and measures shared by the FFT tests (ADR 0055 slice 2b).
//!
//! Every input comes from an integer generator and an exact scaling (a 24-bit integer divided by 2^23),
//! so it has the same bits on every platform and in both widths, and `tests/data/fft_reference.py`
//! reproduces it exactly.

#![allow(dead_code)]

use transit_math::fft::{self, Complex, Float};

/// Every supported size, 256 through 524,288.
pub fn sizes() -> impl Iterator<Item = usize> {
    (8..=19).map(|b| 1usize << b)
}

/// xorshift32 (Marsaglia's 13, 17, 5); the reference script runs the same steps.
pub struct Gen(pub u32);

impl Gen {
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// A value in [-1, 1): the top 24 bits as a signed integer over 2^23, exact in `f32` and `f64`.
    pub fn sample(&mut self) -> f64 {
        let i = i64::from(self.next_u32() >> 8) - (1 << 23);
        i as f64 / 8_388_608.0
    }
}

pub fn real<T: Float>(seed: u32, n: usize) -> Vec<T> {
    let mut g = Gen(seed);
    (0..n).map(|_| T::from_f64(g.sample())).collect()
}

pub fn cplx<T: Float>(seed: u32, n: usize) -> Vec<Complex<T>> {
    let mut g = Gen(seed);
    (0..n)
        .map(|_| {
            let re = g.sample();
            let im = g.sample();
            Complex::new(T::from_f64(re), T::from_f64(im))
        })
        .collect()
}

/// A twiddle table for every size up to `n`.
pub fn table<T: Float>(n: usize) -> Vec<Complex<T>> {
    let mut t = vec![Complex::default(); fft::twiddle_len(n)];
    fft::twiddles(&mut t).unwrap();
    t
}

/// Unit roundoff of each width (half an ulp of 1).
pub trait Eps: Float + Into<f64> {
    const EPS: f64;
}
impl Eps for f32 {
    const EPS: f64 = f32::EPSILON as f64 / 2.0;
}
impl Eps for f64 {
    const EPS: f64 = f64::EPSILON / 2.0;
}

pub fn c64<T: Eps>(z: Complex<T>) -> (f64, f64) {
    (z.re.into(), z.im.into())
}

/// The largest distance between two complex sequences, over the largest magnitude of `b`, in units of
/// the width's unit roundoff.
pub fn rel_err_eps<T: Eps>(a: &[Complex<T>], b: &[(f64, f64)]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (x, &(re, im)) in a.iter().zip(b) {
        let (xr, xi) = c64(*x);
        num = num.max((xr - re).hypot(xi - im));
        den = den.max(re.hypot(im));
    }
    num / den / T::EPS
}

pub fn rel_err_eps_real<T: Eps>(a: &[T], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (&x, &y) in a.iter().zip(b) {
        num = num.max((x.into() - y).abs());
        den = den.max(y.abs());
    }
    num / den / T::EPS
}

pub fn log2(n: usize) -> f64 {
    f64::from(n.trailing_zeros())
}
