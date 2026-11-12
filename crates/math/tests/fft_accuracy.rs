//! The FFT's accuracy (ADR 0055 slice 2b), tolerant comparisons beside the exact pin of
//! `fft_pinned.rs`:
//!
//! - forward then inverse recovers the input at every supported size, 256 through 524,288, real and
//!   complex, `f32` and `f64`, within `2 * log2(n)` unit roundoffs of the largest input magnitude;
//! - the real transforms agree with the complex transforms of the same data within `2 * log2(n)`;
//! - `rustfft` (a dev-dependency only: display code keeps it, rendered code may not) agrees within
//!   `2 * log2(n)` at every supported size.
//!
//! Each test prints its measured worst case per `log2(n)`; the slice's result records them.

mod fft_support;

use fft_support::{Eps, c64, cplx, log2, real, rel_err_eps, rel_err_eps_real, sizes, table};
use rustfft::FftPlanner;
use transit_math::fft::{self, Complex, MAX_LEN};

const C: f64 = 2.0;

fn round_trip<T: Eps>() {
    let tw = table::<T>(MAX_LEN);
    let mut worst = (0.0f64, 0.0f64);
    for n in sizes() {
        let bound = C * log2(n);
        let x = cplx::<T>(n as u32 ^ 0x00c0_ffee, n);
        let mut y = x.clone();
        fft::forward(&tw, &mut y).unwrap();
        fft::inverse(&tw, &mut y).unwrap();
        let want: Vec<_> = x.iter().map(|z| c64(*z)).collect();
        let e = rel_err_eps(&y, &want);
        println!("{} complex round trip n = {n}: {e:.3} eps", core::any::type_name::<T>());
        assert!(e <= bound, "complex round trip, n = {n}: {e} eps > {bound}");
        worst.0 = worst.0.max(e / log2(n));

        let r = real::<T>(n as u32 ^ 0x0bad_f00d, n);
        let mut spec = vec![Complex::default(); n / 2 + 1];
        let mut back = vec![T::default(); n];
        let mut scratch = vec![Complex::default(); n / 2];
        fft::forward_real(&tw, &r, &mut spec).unwrap();
        fft::inverse_real(&tw, &spec, &mut back, &mut scratch).unwrap();
        let want: Vec<f64> = r.iter().map(|&v| v.into()).collect();
        let e = rel_err_eps_real(&back, &want);
        println!("{} real round trip n = {n}: {e:.3} eps", core::any::type_name::<T>());
        assert!(e <= bound, "real round trip, n = {n}: {e} eps > {bound}");
        worst.1 = worst.1.max(e / log2(n));
    }
    println!("{}: worst round trip per log2(n): complex {:.3}, real {:.3} eps", core::any::type_name::<T>(), worst.0, worst.1);
}

#[test]
fn forward_then_inverse_recovers_the_input_at_every_supported_size() {
    round_trip::<f32>();
    round_trip::<f64>();
}

fn real_vs_complex<T: Eps>() {
    let tw = table::<T>(MAX_LEN);
    let mut worst = (0.0f64, 0.0f64);
    for n in sizes() {
        let bound = C * log2(n);
        let r = real::<T>(n as u32 ^ 0x1234_5678, n);
        let mut spec = vec![Complex::default(); n / 2 + 1];
        fft::forward_real(&tw, &r, &mut spec).unwrap();
        let mut x: Vec<_> = r.iter().map(|&v| Complex::new(v, T::default())).collect();
        fft::forward(&tw, &mut x).unwrap();
        let want: Vec<_> = x[..=n / 2].iter().map(|z| c64(*z)).collect();
        let e = rel_err_eps(&spec, &want);
        assert!(e <= bound, "real forward against complex, n = {n}: {e} eps > {bound}");
        worst.0 = worst.0.max(e / log2(n));

        // The inverse of a Hermitian spectrum (the half spectrum of a real signal, mirrored).
        let half = spec.clone();
        let mut full: Vec<_> = (0..n).map(|k| if k <= n / 2 { half[k] } else { let z = half[n - k]; Complex::new(z.re, T::default() - z.im) }).collect();
        fft::inverse(&tw, &mut full).unwrap();
        let mut y = vec![T::default(); n];
        let mut scratch = vec![Complex::default(); n / 2];
        fft::inverse_real(&tw, &half, &mut y, &mut scratch).unwrap();
        let want: Vec<f64> = full.iter().map(|z| z.re.into()).collect();
        let e = rel_err_eps_real(&y, &want);
        assert!(e <= bound, "real inverse against complex, n = {n}: {e} eps > {bound}");
        worst.1 = worst.1.max(e / log2(n));
    }
    println!("{}: worst real against complex per log2(n): forward {:.3}, inverse {:.3} eps", core::any::type_name::<T>(), worst.0, worst.1);
}

#[test]
fn the_real_transforms_agree_with_the_complex_transforms() {
    real_vs_complex::<f32>();
    real_vs_complex::<f64>();
}

trait Rust: Eps + rustfft::FftNum {}
impl Rust for f32 {}
impl Rust for f64 {}

fn against_rustfft<T: Rust>() {
    let tw = table::<T>(MAX_LEN);
    let mut planner = FftPlanner::<T>::new();
    let mut worst = (0.0f64, 0.0f64);
    for n in sizes() {
        let bound = C * log2(n);
        let x = cplx::<T>(n as u32 ^ 0x7777_0001, n);
        let mut ours = x.clone();
        fft::forward(&tw, &mut ours).unwrap();
        let mut theirs: Vec<_> = x.iter().map(|z| rustfft::num_complex::Complex::new(z.re, z.im)).collect();
        planner.plan_fft_forward(n).process(&mut theirs);
        let want: Vec<_> = theirs.iter().map(|z| (z.re.into(), z.im.into())).collect();
        let e = rel_err_eps(&ours, &want);
        assert!(e <= bound, "forward against rustfft, n = {n}: {e} eps > {bound}");
        worst.0 = worst.0.max(e / log2(n));

        let mut ours = x.clone();
        fft::inverse(&tw, &mut ours).unwrap();
        let mut theirs: Vec<_> = x.iter().map(|z| rustfft::num_complex::Complex::new(z.re, z.im)).collect();
        planner.plan_fft_inverse(n).process(&mut theirs);
        // rustfft's inverse is unscaled: divide by n (exact) for the comparison.
        let want: Vec<_> = theirs.iter().map(|z| (z.re.into() / n as f64, z.im.into() / n as f64)).collect();
        let e = rel_err_eps(&ours, &want);
        assert!(e <= bound, "inverse against rustfft, n = {n}: {e} eps > {bound}");
        worst.1 = worst.1.max(e / log2(n));
    }
    println!("{}: worst against rustfft per log2(n): forward {:.3}, inverse {:.3} eps", core::any::type_name::<T>(), worst.0, worst.1);
}

#[test]
fn rustfft_agrees_within_the_bound_at_every_supported_size() {
    against_rustfft::<f32>();
    against_rustfft::<f64>();
}
