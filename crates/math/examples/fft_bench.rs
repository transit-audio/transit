//! Speed of the seam's FFT (ADR 0055 slice 2b) against `rustfft` (display code's FFT, which picks SIMD
//! code at run time): a recorded wall-clock bench, not a CI gate. Each figure is the median of five runs,
//! in microseconds per transform. `rustfft` has no real transform here, so the real rows show the
//! seam's real forward and inverse beside `rustfft`'s complex forward of the same size. Each complex
//! timing includes copying the input back first (the same copy for both FFTs). Run in release:
//! `cargo run --release -p transit-math --example fft_bench`.

use std::hint::black_box;
use std::time::Instant;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex as RComplex;
use transit_math::fft::{self, Complex, Float};

const RUNS: usize = 5;

/// Median of five runs of `reps` calls, in microseconds per call.
fn time(reps: usize, mut f: impl FnMut()) -> f64 {
    let mut runs = [0.0f64; RUNS];
    for run in &mut runs {
        let t = Instant::now();
        for _ in 0..reps {
            f();
        }
        *run = t.elapsed().as_secs_f64() * 1e6 / reps as f64;
    }
    runs.sort_by(f64::total_cmp);
    runs[RUNS / 2]
}

fn bench<T: Float + rustfft::FftNum>(width: &str) {
    let mut planner = FftPlanner::<T>::new();
    for n in [256usize, 4096, 65_536, 524_288] {
        let reps = (4_000_000 / n).max(4);
        let mut tw = vec![Complex::<T>::default(); fft::twiddle_len(n)];
        let tw_us = time(1, || fft::twiddles(&mut tw).unwrap());
        let input: Vec<T> = (0..n).map(|i| <T as Float>::from_f64(((i * 7919) % 1000) as f64 / 1000.0 - 0.5)).collect();
        let xc: Vec<Complex<T>> = input.iter().map(|&v| Complex::new(v, T::default())).collect();
        let mut x = xc.clone();
        let seam_c = time(reps, || {
            x.copy_from_slice(&xc);
            fft::forward(&tw, &mut x).unwrap();
            black_box(&x);
        });
        let seam_ci = time(reps, || {
            x.copy_from_slice(&xc);
            fft::inverse(&tw, &mut x).unwrap();
            black_box(&x);
        });
        let mut spec = vec![Complex::<T>::default(); n / 2 + 1];
        let mut y = vec![T::default(); n];
        let mut scratch = vec![Complex::<T>::default(); n / 2];
        let seam_r = time(reps, || {
            fft::forward_real(&tw, &input, &mut spec).unwrap();
            black_box(&spec);
        });
        let seam_ri = time(reps, || {
            fft::inverse_real(&tw, &spec, &mut y, &mut scratch).unwrap();
            black_box(&y);
        });
        let plan = planner.plan_fft_forward(n);
        let rxc: Vec<RComplex<T>> = input.iter().map(|&v| RComplex::new(v, T::default())).collect();
        let mut rx = rxc.clone();
        let mut rscratch = vec![RComplex::<T>::default(); plan.get_inplace_scratch_len()];
        let rust_c = time(reps, || {
            rx.copy_from_slice(&rxc);
            plan.process_with_scratch(&mut rx, &mut rscratch);
            black_box(&rx);
        });
        println!(
            "{width} n={n:>7}: complex fwd {seam_c:>10.2} inv {seam_ci:>10.2} | real fwd {seam_r:>10.2} inv {seam_ri:>10.2} | rustfft complex fwd {rust_c:>10.2} | seam/rustfft complex {:>5.2}x | twiddle fill {tw_us:>10.2}",
            seam_c / rust_c
        );
    }
}

fn main() {
    println!("microseconds per transform, median of {RUNS} runs");
    bench::<f32>("f32");
    bench::<f64>("f64");
}
