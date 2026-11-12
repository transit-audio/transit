//! The one FFT for rendered code (ADR 0055, 2.7 and slice 2b; the requirements of ADR 0054 decision 11).
//!
//! A plain iterative radix-2 transform, real and complex, forward and inverse, in `f32` and `f64`, over
//! the power-of-two sizes from [`MIN_LEN`] (256) through [`MAX_LEN`] (524,288). It allocates nothing: the
//! caller owns the twiddle table (filled once by [`twiddles`]) and, for [`inverse_real`], the scratch.
//! Any other size, or a buffer or table of the wrong length, is refused with an [`FftError`]; nothing
//! here panics on a caller's sizes in any profile.
//!
//! # Why the output is the same in every bit on every platform
//!
//! - **One arithmetic sequence.** Every result is a fixed sequence of separate IEEE multiplies, adds and
//!   subtracts, written out below and in the code. Rust rounds each operation on its own and never
//!   contracts `a * b + c` into a fused multiply-add or reassociates, in any profile or with any target
//!   feature, so the sequence written is the sequence run. The FFT calls no fused multiply-add (not the
//!   seam's `mul_add` either), no fast-math operation (`*_fast`, `algebraic_*`) and has no CPU-dependent
//!   path (no SIMD, no run-time dispatch). Source review verifies this: the module has no other
//!   arithmetic than the formulas documented here.
//! - **Twiddles from the seam.** [`twiddles`] fills the table from the seam's `f64` [`crate::f64::cos`]
//!   and [`crate::f64::sin`] (the pinned pure-Rust `libm`), each entry computed directly from its own
//!   angle: no recurrence, so nothing drifts. The `f32` table holds the same `f64` values rounded once.
//! - The crate's pinned output table covers every supported size, real and complex, forward and
//!   inverse, `f32` and `f64` (`tests/fft_pinned.rs`).
//!
//! # Conventions
//!
//! - **Sign and scaling.** Forward: `X[k] = sum_j x[j] * e^(-2*pi*i*j*k/n)`, unscaled. Inverse:
//!   `x[j] = (1/n) * sum_k X[k] * e^(+2*pi*i*j*k/n)`, so the inverse of the forward recovers the input
//!   (within rounding). `1/n` is a power of two, so the scaling is an exact multiplication (barring
//!   underflow into subnormals).
//! - **Real transforms.** [`forward_real`] takes `n` real samples and writes the `n/2 + 1` bins from DC to
//!   Nyquist; the imaginary parts of DC and Nyquist are written as exactly `0.0`. [`inverse_real`] takes
//!   those `n/2 + 1` bins (it assumes Hermitian symmetry for the rest; any imaginary part at DC or
//!   Nyquist enters the formula below as written) and writes `n` real samples.
//!
//! # Twiddles
//!
//! A table of `N/2` entries serves every supported size `n <= N`, real and complex (entry `k` of size `n`
//! is entry `k * N/n` of the table, the same bits as a table built for `n`). Entry `k` is
//! `(cos t, sin t)` with `t = 2*pi*k/N` (the forward transform multiplies by `cos t - i sin t`, the
//! inverse by `cos t + i sin t`). The angle is `TAU * k / N` in `f64` (`TAU` the `f64` literal, one
//! rounding for the product, an exact division by the power of two `N`), reduced by octant symmetry so
//! that the value at `k = N/4` is exactly `(0, 1)` and the table is exactly symmetric:
//!
//! - `8k <= N`: `(cos(t_k), sin(t_k))`;
//! - `4k <= N`: `(sin(t_j), cos(t_j))` with `j = N/4 - k`;
//! - otherwise (`N/4 < k < N/2`): `(-c, s)` where `(c, s)` is entry `N/2 - k` by the two rules above.
//!
//! # The arithmetic order
//!
//! Complex transform of size `m`: the bit-reversal permutation (swaps only), then `log2 m` passes with
//! block lengths `len = 2, 4, ..., m`; in each pass the blocks in increasing order, and in each block
//! `j = 0 .. len/2` in increasing order, the butterfly on `a = x[i]`, `b = x[i + len/2]` with the
//! twiddle `w = table[j * N/len]` (every product rounded, then the sum):
//!
//! ```text
//! forward: t.re = w.re*b.re + w.im*b.im      inverse: t.re = w.re*b.re - w.im*b.im
//!          t.im = w.re*b.im - w.im*b.re               t.im = w.re*b.im + w.im*b.re
//! x[i] = (a.re + t.re, a.im + t.im)          x[i + len/2] = (a.re - t.re, a.im - t.im)
//! ```
//!
//! No butterfly is special-cased (`w = (1, 0)` runs the same formula). The inverse then multiplies
//! every real and imaginary part by `1/m`, in index order.
//!
//! Real forward of size `n` (`m = n/2`): `z[j] = (x[2j], x[2j+1])` is transformed as a complex forward of
//! size `m` in the output buffer; then `X[0] = (z0.re + z0.im, 0)`, `X[m] = (z0.re - z0.im, 0)`, and for
//! `k = 1 ..= m/2` in increasing order, reading `a = Z[k]` and `b = Z[m-k]` first, `X[k] = split(a, b, w_k)`
//! and (for `k != m - k`) `X[m-k] = split(b, a, w_(m-k))`, with `w_k` the size-`n` twiddle `(c, s)`:
//!
//! ```text
//! e = ((a.re + b.re)*0.5, (a.im - b.im)*0.5)
//! o = ((a.im + b.im)*0.5, (b.re - a.re)*0.5)
//! split = (e.re + (c*o.re + s*o.im), e.im + (c*o.im - s*o.re))
//! ```
//!
//! Real inverse of size `n`: for `k = 0 .. m` in increasing order, `a = X[k]`, `b = X[m-k]`:
//!
//! ```text
//! e = ((a.re + b.re)*0.5, (a.im - b.im)*0.5)
//! d = (a.re - b.re, a.im + b.im)
//! o = ((c*d.re - s*d.im)*0.5, (c*d.im + s*d.re)*0.5)
//! scratch[k] = (e.re - o.im, e.im + o.re)
//! ```
//!
//! then a complex inverse of size `m` on the scratch (scaled by `1/m`), and `x[2j] = z[j].re`,
//! `x[2j+1] = z[j].im`.

use core::fmt;
use core::ops::{Add, Mul, Sub};

use crate::f64 as m64;

/// The smallest supported transform size (ADR 0054 decision 11).
pub const MIN_LEN: usize = 256;
/// The largest supported transform size (ADR 0054 decision 11: the linear-phase grid at Maximum and
/// 176.4 or 192 kHz).
pub const MAX_LEN: usize = 524_288;

/// A complex value, real part first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex<T> {
    /// Real part.
    pub re: T,
    /// Imaginary part.
    pub im: T,
}

impl<T> Complex<T> {
    /// `re + i*im`.
    #[inline]
    pub const fn new(re: T, im: T) -> Self {
        Complex { re, im }
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// The two sample types the FFT runs in: `f32` and `f64`. Sealed.
pub trait Float: sealed::Sealed + Copy + Default + PartialEq + fmt::Debug + Add<Output = Self> + Sub<Output = Self> + Mul<Output = Self> {
    /// Exactly `0.5`.
    const HALF: Self;
    /// The `f64` value rounded once to this type (to nearest, ties to even).
    fn from_f64(x: f64) -> Self;
    /// Exactly `1 / n` for a supported size `n` (a power of two, so representable).
    fn recip_len(n: usize) -> Self;
}

impl Float for f32 {
    const HALF: Self = 0.5;
    #[inline]
    fn from_f64(x: f64) -> Self {
        x as f32
    }
    #[inline]
    fn recip_len(n: usize) -> Self {
        1.0 / n as f32
    }
}

impl Float for f64 {
    const HALF: Self = 0.5;
    #[inline]
    fn from_f64(x: f64) -> Self {
        x
    }
    #[inline]
    fn recip_len(n: usize) -> Self {
        1.0 / n as f64
    }
}

/// Why a transform was refused. Nothing is written when a call is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FftError {
    /// The transform size is not a power of two from [`MIN_LEN`] through [`MAX_LEN`].
    Size(usize),
    /// The twiddle table's length is not `N/2` for a supported `N` at or above the transform size.
    Twiddles(usize),
    /// An input, output or scratch buffer does not have the length the transform size needs.
    Buffer(usize),
}

impl fmt::Display for FftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FftError::Size(n) => write!(f, "FFT size {n} is not a power of two from {MIN_LEN} through {MAX_LEN}"),
            FftError::Twiddles(len) => write!(f, "twiddle table of {len} entries does not cover the transform size"),
            FftError::Buffer(len) => write!(f, "buffer of {len} elements does not fit the transform size"),
        }
    }
}

/// Whether `n` is a supported transform size: a power of two from [`MIN_LEN`] through [`MAX_LEN`].
#[inline]
#[must_use]
pub const fn is_supported(n: usize) -> bool {
    n.is_power_of_two() && n >= MIN_LEN && n <= MAX_LEN
}

/// The twiddle table length for transforms up to size `n`: `n / 2`.
#[inline]
#[must_use]
pub const fn twiddle_len(n: usize) -> usize {
    n / 2
}

/// The angle `2*pi*k/n` in `f64`: one rounding for the product, an exact division by the power of two.
#[inline]
fn angle(k: usize, n: usize) -> f64 {
    core::f64::consts::TAU * (k as f64) / (n as f64)
}

/// `(cos, sin)` of `2*pi*k/n` for `k < n/2`, by octant symmetry (module docs).
fn entry(k: usize, n: usize) -> (f64, f64) {
    if 8 * k <= n {
        let t = angle(k, n);
        (m64::cos(t), m64::sin(t))
    } else if 4 * k <= n {
        let t = angle(n / 4 - k, n);
        (m64::sin(t), m64::cos(t))
    } else {
        let j = n / 2 - k;
        let (c, s) = if 8 * j <= n {
            let t = angle(j, n);
            (m64::cos(t), m64::sin(t))
        } else {
            let t = angle(n / 4 - j, n);
            (m64::sin(t), m64::cos(t))
        };
        (-c, s)
    }
}

/// Fills a twiddle table for every supported transform size up to `N = 2 * table.len()`. The table must
/// have [`twiddle_len`]`(N)` entries for a supported `N`. Entry `k` is `(cos t, sin t)`, `t = 2*pi*k/N`,
/// from the seam's `f64` functions (rounded once for `f32`).
///
/// # Errors
///
/// [`FftError::Twiddles`] when `2 * table.len()` is not a supported size; the table is then untouched.
pub fn twiddles<T: Float>(table: &mut [Complex<T>]) -> Result<(), FftError> {
    let n = table.len().wrapping_mul(2);
    if !is_supported(n) {
        return Err(FftError::Twiddles(table.len()));
    }
    for (k, w) in table.iter_mut().enumerate() {
        let (c, s) = entry(k, n);
        *w = Complex::new(T::from_f64(c), T::from_f64(s));
    }
    Ok(())
}

/// Checks a table against a transform size `n` (already checked) and returns `N`, the table's size.
fn table_size<T>(table: &[Complex<T>], n: usize) -> Result<usize, FftError> {
    let big = table.len().wrapping_mul(2);
    if is_supported(big) && big >= n { Ok(big) } else { Err(FftError::Twiddles(table.len())) }
}

/// The complex transform of `x.len()` (a power of two, at least 2) points in place, with a table of size
/// `big` covering it; `INV` selects the inverse (conjugate twiddles and the `1/m` scaling).
fn complex<T: Float, const INV: bool>(table: &[Complex<T>], big: usize, x: &mut [Complex<T>]) {
    let m = x.len();
    let bits = m.trailing_zeros();
    for i in 0..m {
        let r = i.reverse_bits() >> (usize::BITS - bits);
        if i < r {
            x.swap(i, r);
        }
    }
    let mut len = 2;
    while len <= m {
        let half = len / 2;
        let stride = big / len;
        let mut start = 0;
        while start < m {
            for j in 0..half {
                let w = table[j * stride];
                let i = start + j;
                let a = x[i];
                let b = x[i + half];
                let t = if INV {
                    Complex::new(w.re * b.re - w.im * b.im, w.re * b.im + w.im * b.re)
                } else {
                    Complex::new(w.re * b.re + w.im * b.im, w.re * b.im - w.im * b.re)
                };
                x[i] = Complex::new(a.re + t.re, a.im + t.im);
                x[i + half] = Complex::new(a.re - t.re, a.im - t.im);
            }
            start += len;
        }
        len *= 2;
    }
    if INV {
        let scale = T::recip_len(m);
        for v in x.iter_mut() {
            *v = Complex::new(v.re * scale, v.im * scale);
        }
    }
}

/// The forward complex transform of `data` in place (unscaled). `data.len()` is the transform size.
///
/// # Errors
///
/// [`FftError::Size`] for an unsupported size, [`FftError::Twiddles`] for a table that does not cover
/// it; `data` is then untouched.
pub fn forward<T: Float>(table: &[Complex<T>], data: &mut [Complex<T>]) -> Result<(), FftError> {
    let n = data.len();
    if !is_supported(n) {
        return Err(FftError::Size(n));
    }
    let big = table_size(table, n)?;
    complex::<T, false>(table, big, data);
    Ok(())
}

/// The inverse complex transform of `data` in place, scaled by `1/n`. `data.len()` is the size.
///
/// # Errors
///
/// As [`forward`].
pub fn inverse<T: Float>(table: &[Complex<T>], data: &mut [Complex<T>]) -> Result<(), FftError> {
    let n = data.len();
    if !is_supported(n) {
        return Err(FftError::Size(n));
    }
    let big = table_size(table, n)?;
    complex::<T, true>(table, big, data);
    Ok(())
}

/// `X[k]` of the real forward transform from `a = Z[k]`, `b = Z[m-k]` and the size-`n` twiddle `w`.
#[inline]
fn split<T: Float>(a: Complex<T>, b: Complex<T>, w: Complex<T>) -> Complex<T> {
    let e_re = (a.re + b.re) * T::HALF;
    let e_im = (a.im - b.im) * T::HALF;
    let o_re = (a.im + b.im) * T::HALF;
    let o_im = (b.re - a.re) * T::HALF;
    Complex::new(e_re + (w.re * o_re + w.im * o_im), e_im + (w.re * o_im - w.im * o_re))
}

/// The complex input of the half-size inverse from `a = X[k]`, `b = X[m-k]` and the twiddle `w`.
#[inline]
fn unsplit<T: Float>(a: Complex<T>, b: Complex<T>, w: Complex<T>) -> Complex<T> {
    let e_re = (a.re + b.re) * T::HALF;
    let e_im = (a.im - b.im) * T::HALF;
    let d_re = a.re - b.re;
    let d_im = a.im + b.im;
    let o_re = (w.re * d_re - w.im * d_im) * T::HALF;
    let o_im = (w.re * d_im + w.im * d_re) * T::HALF;
    Complex::new(e_re - o_im, e_im + o_re)
}

/// The forward transform of `input.len()` real samples (the transform size) into `output`, which must
/// hold `n/2 + 1` bins (DC through Nyquist). Unscaled; DC's and Nyquist's imaginary parts are `0.0`.
///
/// # Errors
///
/// [`FftError::Size`], [`FftError::Twiddles`], or [`FftError::Buffer`] when `output` is not `n/2 + 1`
/// long; nothing is written then.
pub fn forward_real<T: Float>(table: &[Complex<T>], input: &[T], output: &mut [Complex<T>]) -> Result<(), FftError> {
    let n = input.len();
    if !is_supported(n) {
        return Err(FftError::Size(n));
    }
    let big = table_size(table, n)?;
    let m = n / 2;
    if output.len() != m + 1 {
        return Err(FftError::Buffer(output.len()));
    }
    for (z, &[re, im]) in output.iter_mut().zip(input.as_chunks::<2>().0) {
        *z = Complex::new(re, im);
    }
    complex::<T, false>(table, big, &mut output[..m]);
    let stride = big / n;
    let z0 = output[0];
    output[0] = Complex::new(z0.re + z0.im, T::default());
    output[m] = Complex::new(z0.re - z0.im, T::default());
    for k in 1..=m / 2 {
        let a = output[k];
        let b = output[m - k];
        output[k] = split(a, b, table[k * stride]);
        if k != m - k {
            output[m - k] = split(b, a, table[(m - k) * stride]);
        }
    }
    Ok(())
}

/// The inverse of [`forward_real`]: `spectrum` holds the `n/2 + 1` bins, `output` receives the `n` real
/// samples (its length is the transform size), scaled by `1/n`; `scratch` must hold `n/2` values and is
/// overwritten.
///
/// # Errors
///
/// [`FftError::Size`], [`FftError::Twiddles`], or [`FftError::Buffer`] for a `spectrum` or `scratch` of
/// the wrong length; nothing is written then.
pub fn inverse_real<T: Float>(
    table: &[Complex<T>],
    spectrum: &[Complex<T>],
    output: &mut [T],
    scratch: &mut [Complex<T>],
) -> Result<(), FftError> {
    let n = output.len();
    if !is_supported(n) {
        return Err(FftError::Size(n));
    }
    let big = table_size(table, n)?;
    let m = n / 2;
    if spectrum.len() != m + 1 {
        return Err(FftError::Buffer(spectrum.len()));
    }
    if scratch.len() != m {
        return Err(FftError::Buffer(scratch.len()));
    }
    let stride = big / n;
    for (k, z) in scratch.iter_mut().enumerate() {
        *z = unsplit(spectrum[k], spectrum[m - k], table[k * stride]);
    }
    complex::<T, true>(table, big, scratch);
    for (pair, z) in output.as_chunks_mut::<2>().0.iter_mut().zip(scratch.iter()) {
        *pair = [z.re, z.im];
    }
    Ok(())
}
