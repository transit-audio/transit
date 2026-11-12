//! The FFT against cases with known answers (ADR 0055 slice 2b): an impulse and a constant give exact
//! results in both directions; an impulse elsewhere, a sinusoid at a bin center and its inverse match
//! the seam's own `cos` and `sin` within a stated bound; Parseval and linearity hold within stated
//! bounds; the twiddle table's exact properties; and every unsupported size or length is refused
//! without writing anything.

mod fft_support;

use fft_support::{Eps, cplx, log2, real, sizes, table};
use transit_math::f64 as m64;
use transit_math::fft::{self, Complex, FftError, MAX_LEN, MIN_LEN};

/// `e^(sign * 2*pi*i*k/n)` through the seam, with `k` reduced mod `n` first (exact index arithmetic).
fn phasor(k: usize, n: usize, sign: f64) -> (f64, f64) {
    let t = core::f64::consts::TAU * ((k % n) as f64) / (n as f64);
    (m64::cos(t), sign * m64::sin(t))
}

fn impulse_and_constant<T: Eps>() {
    for n in sizes() {
        let tw = table::<T>(n);
        let one = T::from_f64(1.0);
        let c = T::from_f64(0.75);
        let nc = T::from_f64(0.75 * n as f64);
        let zero = T::default();

        // An impulse at 0: every bin exactly 1, both forward forms.
        let mut x = vec![Complex::default(); n];
        x[0] = Complex::new(one, zero);
        fft::forward(&tw, &mut x).unwrap();
        assert!(x.iter().all(|z| z.re == one && z.im == zero), "complex impulse, n = {n}");
        let mut r = vec![zero; n];
        r[0] = one;
        let mut spec = vec![Complex::default(); n / 2 + 1];
        fft::forward_real(&tw, &r, &mut spec).unwrap();
        assert!(spec.iter().all(|z| z.re == one && z.im == zero), "real impulse, n = {n}");

        // A constant: bin 0 exactly n*c, every other bin exactly zero.
        let mut x = vec![Complex::new(c, zero); n];
        fft::forward(&tw, &mut x).unwrap();
        assert_eq!(x[0], Complex::new(nc, zero), "complex constant, n = {n}");
        assert!(x[1..].iter().all(|z| z.re == zero && z.im == zero), "complex constant, n = {n}");
        let r = vec![c; n];
        fft::forward_real(&tw, &r, &mut spec).unwrap();
        assert_eq!(spec[0], Complex::new(nc, zero), "real constant, n = {n}");
        assert!(spec[1..].iter().all(|z| z.re == zero && z.im == zero), "real constant, n = {n}");

        // Inverse direction: all-ones spectrum is exactly the unit impulse; bin 0 = n*c is exactly c.
        let mut x = vec![Complex::new(one, zero); n];
        fft::inverse(&tw, &mut x).unwrap();
        assert_eq!(x[0], Complex::new(one, zero), "complex inverse impulse, n = {n}");
        assert!(x[1..].iter().all(|z| z.re == zero && z.im == zero), "complex inverse impulse, n = {n}");
        let spec = vec![Complex::new(one, zero); n / 2 + 1];
        let mut y = vec![zero; n];
        let mut scratch = vec![Complex::default(); n / 2];
        fft::inverse_real(&tw, &spec, &mut y, &mut scratch).unwrap();
        assert_eq!(y[0], one, "real inverse impulse, n = {n}");
        assert!(y[1..].iter().all(|&v| v == zero), "real inverse impulse, n = {n}");
        let mut x = vec![Complex::default(); n];
        x[0] = Complex::new(nc, zero);
        fft::inverse(&tw, &mut x).unwrap();
        assert!(x.iter().all(|z| z.re == c && z.im == zero), "complex inverse constant, n = {n}");
        let mut spec = vec![Complex::default(); n / 2 + 1];
        spec[0] = Complex::new(nc, zero);
        fft::inverse_real(&tw, &spec, &mut y, &mut scratch).unwrap();
        assert!(y.iter().all(|&v| v == c), "real inverse constant, n = {n}");
    }
}

#[test]
fn an_impulse_and_a_constant_are_exact_in_both_directions() {
    impulse_and_constant::<f32>();
    impulse_and_constant::<f64>();
}

/// Bound for the phasor cases: 2 * log2(n) unit roundoffs of the largest magnitude.
fn phasors<T: Eps>() {
    let mut worst = 0.0f64;
    for n in sizes() {
        let bound = 2.0 * log2(n);
        let tw = table::<T>(n);
        let zero = T::default();
        // An impulse at p: X[k] = e^(-2*pi*i*k*p/n).
        let p = n / 2 - 3;
        let mut x = vec![Complex::default(); n];
        x[p] = Complex::new(T::from_f64(1.0), zero);
        fft::forward(&tw, &mut x).unwrap();
        let want: Vec<_> = (0..n).map(|k| phasor(k * p, n, -1.0)).collect();
        let e = fft_support::rel_err_eps(&x, &want);
        assert!(e <= bound, "impulse at {p}, n = {n}: {e} eps > {bound}");
        worst = worst.max(e / log2(n));

        // A cosine at bin k0 (input from the seam's cos, rounded once): n/2 at bins k0 and n - k0.
        let k0 = 5;
        let r: Vec<T> = (0..n).map(|j| T::from_f64(phasor(j * k0, n, 1.0).0)).collect();
        let mut spec = vec![Complex::default(); n / 2 + 1];
        fft::forward_real(&tw, &r, &mut spec).unwrap();
        let want: Vec<_> = (0..=n / 2).map(|k| if k == k0 { (n as f64 / 2.0, 0.0) } else { (0.0, 0.0) }).collect();
        let e = fft_support::rel_err_eps(&spec, &want);
        assert!(e <= bound, "cosine at bin {k0}, n = {n}: {e} eps > {bound}");
        worst = worst.max(e / log2(n));
        let mut x: Vec<_> = r.iter().map(|&v| Complex::new(v, zero)).collect();
        fft::forward(&tw, &mut x).unwrap();
        let want: Vec<_> = (0..n).map(|k| if k == k0 || k == n - k0 { (n as f64 / 2.0, 0.0) } else { (0.0, 0.0) }).collect();
        let e = fft_support::rel_err_eps(&x, &want);
        assert!(e <= bound, "complex cosine at bin {k0}, n = {n}: {e} eps > {bound}");
        worst = worst.max(e / log2(n));

        // Inverse: bin k0 = n gives e^(+2*pi*i*k0*j/n).
        let mut x = vec![Complex::default(); n];
        x[k0] = Complex::new(T::from_f64(n as f64), zero);
        fft::inverse(&tw, &mut x).unwrap();
        let want: Vec<_> = (0..n).map(|j| phasor(j * k0, n, 1.0)).collect();
        let e = fft_support::rel_err_eps(&x, &want);
        assert!(e <= bound, "inverse phasor at bin {k0}, n = {n}: {e} eps > {bound}");
        worst = worst.max(e / log2(n));
        // Real inverse: bins k0 = n/2 (and its mirror, implied) give cos(2*pi*k0*j/n).
        let mut spec = vec![Complex::default(); n / 2 + 1];
        spec[k0] = Complex::new(T::from_f64(n as f64 / 2.0), zero);
        let mut y = vec![zero; n];
        let mut scratch = vec![Complex::default(); n / 2];
        fft::inverse_real(&tw, &spec, &mut y, &mut scratch).unwrap();
        let want: Vec<f64> = (0..n).map(|j| phasor(j * k0, n, 1.0).0).collect();
        let e = fft_support::rel_err_eps_real(&y, &want);
        assert!(e <= bound, "real inverse cosine at bin {k0}, n = {n}: {e} eps > {bound}");
        worst = worst.max(e / log2(n));
    }
    println!("{}: worst phasor error {worst:.3} eps per log2(n)", core::any::type_name::<T>());
}

#[test]
fn impulses_and_sinusoids_at_bin_centers_match_the_seam_within_the_bound() {
    phasors::<f32>();
    phasors::<f64>();
}

/// Neumaier's compensated sum.
fn sum(v: impl Iterator<Item = f64>) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for x in v {
        let t = s + x;
        c += if s.abs() >= x.abs() { (s - t) + x } else { (x - t) + s };
        s = t;
    }
    s + c
}

/// Parseval (`sum |x|^2 = (1/n) sum |X|^2`) within `4 * log2(n)` unit roundoffs, relative; linearity
/// (`F(a*x + b*y) = a*F(x) + b*F(y)`, `a = 0.5`, `b = -0.25`, exact scalings) within `4 * log2(n)`.
fn parseval_linearity<T: Eps>() {
    let mut worst_p = 0.0f64;
    let mut worst_l = 0.0f64;
    for n in sizes() {
        let tw = table::<T>(n);
        let x = cplx::<T>(n as u32 + 11, n);
        let mut big_x = x.clone();
        fft::forward(&tw, &mut big_x).unwrap();
        // Energies summed with compensation, so the test's own summation (about n unit roundoffs if naive,
        // in f64) does not swamp the transform's error.
        let ex = sum(x.iter().map(|z| { let (a, b) = fft_support::c64(*z); a * a + b * b }));
        let eb = sum(big_x.iter().map(|z| { let (a, b) = fft_support::c64(*z); a * a + b * b })) / n as f64;
        let e = ((eb - ex) / ex).abs() / T::EPS;
        assert!(e <= 4.0 * log2(n), "Parseval, n = {n}: {e} eps");
        worst_p = worst_p.max(e / log2(n));

        let y = cplx::<T>(n as u32 + 12, n);
        let (a, b) = (T::from_f64(0.5), T::from_f64(-0.25));
        let mut mix: Vec<_> = x.iter().zip(&y).map(|(p, q)| Complex::new(a * p.re + b * q.re, a * p.im + b * q.im)).collect();
        let mut big_y = y.clone();
        fft::forward(&tw, &mut big_y).unwrap();
        fft::forward(&tw, &mut mix).unwrap();
        let want: Vec<(f64, f64)> = big_x
            .iter()
            .zip(&big_y)
            .map(|(p, q)| {
                let (pr, pi) = fft_support::c64(*p);
                let (qr, qi) = fft_support::c64(*q);
                (0.5 * pr - 0.25 * qr, 0.5 * pi - 0.25 * qi)
            })
            .collect();
        let e = fft_support::rel_err_eps(&mix, &want);
        assert!(e <= 4.0 * log2(n), "linearity, n = {n}: {e} eps");
        worst_l = worst_l.max(e / log2(n));
    }
    println!("{}: worst Parseval {worst_p:.3}, linearity {worst_l:.3} eps per log2(n)", core::any::type_name::<T>());
}

#[test]
fn parseval_and_linearity_hold_within_the_bound() {
    parseval_linearity::<f32>();
    parseval_linearity::<f64>();
}

#[test]
fn the_twiddle_table_is_exactly_symmetric_and_one_table_serves_every_smaller_size() {
    let big = table::<f64>(MAX_LEN);
    let n = MAX_LEN;
    assert_eq!(big[0], Complex::new(1.0, 0.0));
    assert_eq!(big[n / 4], Complex::new(0.0, 1.0));
    for k in 1..n / 4 {
        let (a, b) = (big[k], big[n / 2 - k]);
        assert!(a.re.to_bits() == (-b.re).to_bits() && a.im.to_bits() == b.im.to_bits(), "mirror at {k}");
        // The octant rule computes k <= N/8 directly, so at k = N/8 itself the two sides are cos and sin
        // of the same rounded angle, which need not agree in the last bit; every other k is exact.
        let c = big[n / 4 - k];
        assert!(k == n / 8 || (a.re.to_bits() == c.im.to_bits() && a.im.to_bits() == c.re.to_bits()), "octant at {k}");
    }
    // Each size's own table holds the same bits as every k * N/n entry of the large one, so every
    // transform gives the same bits with either.
    for m in sizes() {
        let small = table::<f64>(m);
        let step = n / m;
        assert!(small.iter().enumerate().all(|(k, w)| *w == big[k * step]), "table for {m}");
        let small32 = table::<f32>(m);
        let big32 = table::<f32>(MAX_LEN);
        assert!(small32.iter().enumerate().all(|(k, w)| *w == big32[k * step]), "f32 table for {m}");
        let mut a = cplx::<f64>(m as u32, m);
        let mut b = a.clone();
        fft::forward(&small, &mut a).unwrap();
        fft::forward(&big, &mut b).unwrap();
        assert!(a.iter().zip(&b).all(|(p, q)| p.re.to_bits() == q.re.to_bits() && p.im.to_bits() == q.im.to_bits()), "n = {m}");
    }
    // f32 entries are the f64 entries rounded once.
    let t32 = table::<f32>(MAX_LEN);
    assert!(t32.iter().zip(&big).all(|(s, d)| s.re == d.re as f32 && s.im == d.im as f32));
}

#[test]
fn unsupported_sizes_and_lengths_are_refused_and_nothing_is_written() {
    let tw = table::<f64>(4096);
    let sentinel = Complex::new(7.0, -7.0);
    for n in [0, 1, 2, 128, 255, 300, 1000, 1 << 20] {
        let mut x = vec![sentinel; n];
        assert_eq!(fft::forward(&tw, &mut x), Err(FftError::Size(n)));
        assert_eq!(fft::inverse(&tw, &mut x), Err(FftError::Size(n)));
        assert!(x.iter().all(|z| *z == sentinel));
        let r = vec![1.0; n];
        let mut spec = vec![sentinel; n / 2 + 1];
        assert_eq!(fft::forward_real(&tw, &r, &mut spec), Err(FftError::Size(n)));
        assert!(spec.iter().all(|z| *z == sentinel));
        let mut y = vec![7.0; n];
        let mut scratch = vec![sentinel; n / 2];
        assert_eq!(fft::inverse_real(&tw, &spec, &mut y, &mut scratch), Err(FftError::Size(n)));
        assert!(y.iter().all(|&v| v == 7.0));
    }
    // A table too small for the size, or of a length that is no supported size's.
    let mut x = vec![sentinel; 8192];
    assert_eq!(fft::forward(&tw, &mut x), Err(FftError::Twiddles(2048)));
    assert!(x.iter().all(|z| *z == sentinel));
    let odd = vec![Complex::new(1.0, 0.0); 1000];
    let mut x = vec![sentinel; 256];
    assert_eq!(fft::forward(&odd, &mut x), Err(FftError::Twiddles(1000)));
    let mut t = vec![Complex::<f64>::default(); MIN_LEN / 4];
    assert_eq!(fft::twiddles(&mut t), Err(FftError::Twiddles(MIN_LEN / 4)));
    assert!(t.iter().all(|w| *w == Complex::default()));
    let mut t = vec![Complex::<f64>::default(); MAX_LEN];
    assert_eq!(fft::twiddles(&mut t), Err(FftError::Twiddles(MAX_LEN)));
    // Buffers of the wrong length.
    let r = real::<f64>(1, 1024);
    let mut spec = vec![sentinel; 512];
    assert_eq!(fft::forward_real(&tw, &r, &mut spec), Err(FftError::Buffer(512)));
    assert!(spec.iter().all(|z| *z == sentinel));
    let spec = vec![sentinel; 513];
    let mut y = vec![7.0; 1024];
    let mut scratch = vec![sentinel; 511];
    assert_eq!(fft::inverse_real(&tw, &spec, &mut y, &mut scratch), Err(FftError::Buffer(511)));
    let mut scratch = vec![sentinel; 512];
    assert_eq!(fft::inverse_real(&tw, &spec[..512], &mut y, &mut scratch), Err(FftError::Buffer(512)));
    assert!(y.iter().all(|&v| v == 7.0));
}
