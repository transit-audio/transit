//! No FFT entry point allocates (ADR 0055 slice 2b): a counting allocator (`transit-rt`'s test support,
//! a dev-dependency; this crate itself has no `unsafe`) watches `twiddles`, `forward`, `inverse`,
//! `forward_real` and `inverse_real` in `f32` and `f64` at the smallest and largest supported sizes,
//! and a refused call. The crate is `no_std` without `alloc`, so this holds by construction too.

mod fft_support;

use transit_math::fft::{self, Complex, Float, MAX_LEN, MIN_LEN};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn every_entry_point<T: Float>() {
    for n in [MIN_LEN, MAX_LEN] {
        let mut tw = vec![Complex::<T>::default(); fft::twiddle_len(n)];
        let mut x = fft_support::cplx::<T>(3, n);
        let r = fft_support::real::<T>(4, n);
        let mut spec = vec![Complex::default(); n / 2 + 1];
        let mut y = vec![T::default(); n];
        let mut scratch = vec![Complex::default(); n / 2];
        let (res, stats) = measure(|| {
            fft::twiddles(&mut tw)?;
            fft::forward(&tw, &mut x)?;
            fft::inverse(&tw, &mut x)?;
            fft::forward_real(&tw, &r, &mut spec)?;
            fft::inverse_real(&tw, &spec, &mut y, &mut scratch)?;
            let refused = fft::forward(&tw, &mut x[..n - 1]);
            assert!(refused.is_err());
            Ok::<(), fft::FftError>(())
        });
        assert_eq!(res, Ok(()));
        assert!(stats.is_zero(), "n = {n}: {stats:?}");
    }
}

#[test]
fn no_entry_point_allocates() {
    every_entry_point::<f32>();
    every_entry_point::<f64>();
}
