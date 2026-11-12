//! The FFT's pinned output table (ADR 0055 slice 2b): the exact bits of the real and complex, forward
//! and inverse transforms, in `f32` and `f64`, at every supported size from 256 through 524,288, of
//! inputs from an integer generator with exact bit patterns (`fft_support`), plus the two twiddle
//! tables. One FNV-1a 64 hash per case keeps the table small.
//!
//! The outputs are plain Rust arithmetic in one fixed sequence over twiddles from the exactly pinned
//! `libm`, so they are IEEE-exact by construction and the pins hold on every platform: proven on Windows
//! and Linux before review, on macOS by CI. A pin that moves is a change of the FFT's arithmetic or of
//! `libm` (a DSP change, ADR 0055 2.1), never a re-pin per platform. The tolerant comparisons (round
//! trip, the arbitrary-precision reference, `rustfft`) are separate tests.

mod common;
mod fft_support;

use common::Fnv;
use fft_support::{cplx, real, sizes, table};
use transit_math::fft::{self, Complex, Float, MAX_LEN};

trait Hash: Float {
    fn put(h: &mut Fnv, x: Self);
}
impl Hash for f32 {
    fn put(h: &mut Fnv, x: Self) {
        h.f32(x);
    }
}
impl Hash for f64 {
    fn put(h: &mut Fnv, x: Self) {
        h.f64(x);
    }
}

fn hash_c<T: Hash>(v: &[Complex<T>]) -> u64 {
    let mut h = Fnv::new();
    for z in v {
        T::put(&mut h, z.re);
        T::put(&mut h, z.im);
    }
    h.0
}

fn hash_r<T: Hash>(v: &[T]) -> u64 {
    let mut h = Fnv::new();
    for &x in v {
        T::put(&mut h, x);
    }
    h.0
}

/// The seed of one case: the size in the high bits, the case number in the low (never zero).
fn seed(n: usize, case: u32) -> u32 {
    (n as u32) ^ case.wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995
}

fn cases<T: Hash>(width: &str, out: &mut Vec<(String, u64)>) {
    let tw = table::<T>(MAX_LEN);
    out.push((format!("{width} twiddles {MAX_LEN}"), hash_c(&tw)));
    for n in sizes() {
        let mut x = cplx::<T>(seed(n, 1), n);
        fft::forward(&tw, &mut x).unwrap();
        out.push((format!("{width} complex forward {n}"), hash_c(&x)));

        let mut x = cplx::<T>(seed(n, 2), n);
        fft::inverse(&tw, &mut x).unwrap();
        out.push((format!("{width} complex inverse {n}"), hash_c(&x)));

        let x = real::<T>(seed(n, 3), n);
        let mut spec = vec![Complex::default(); n / 2 + 1];
        fft::forward_real(&tw, &x, &mut spec).unwrap();
        out.push((format!("{width} real forward {n}"), hash_c(&spec)));

        let spec = cplx::<T>(seed(n, 4), n / 2 + 1);
        let mut y = vec![T::default(); n];
        let mut scratch = vec![Complex::default(); n / 2];
        fft::inverse_real(&tw, &spec, &mut y, &mut scratch).unwrap();
        out.push((format!("{width} real inverse {n}"), hash_r(&y)));
    }
}

fn computed() -> Vec<(String, u64)> {
    let mut v = Vec::new();
    cases::<f32>("f32", &mut v);
    cases::<f64>("f64", &mut v);
    v
}

/// Captured at the commit that added the table; proven on Windows and Linux before review.
const PINNED: &[(&str, u64)] = &[
    ("f32 twiddles 524288", 0xced3d28590949005),
    ("f32 complex forward 256", 0x9a1e7b4a99a0c429),
    ("f32 complex inverse 256", 0x517e1987b5bb498e),
    ("f32 real forward 256", 0x72aa3d4bc4b93325),
    ("f32 real inverse 256", 0xbd220a2effaede11),
    ("f32 complex forward 512", 0xd49613b5016d3696),
    ("f32 complex inverse 512", 0x4133b22c6e23ce46),
    ("f32 real forward 512", 0xbb256813f012cc11),
    ("f32 real inverse 512", 0x16ef665c22101661),
    ("f32 complex forward 1024", 0x794d187810438148),
    ("f32 complex inverse 1024", 0xa0c4a788da9049ec),
    ("f32 real forward 1024", 0x50ec9d934b43a905),
    ("f32 real inverse 1024", 0xa979ee8f9738ea33),
    ("f32 complex forward 2048", 0xcc7072dc5594458d),
    ("f32 complex inverse 2048", 0x6fee461df89b27bd),
    ("f32 real forward 2048", 0x61709b32f01dcbe0),
    ("f32 real inverse 2048", 0x7b73e9ebca8b0e79),
    ("f32 complex forward 4096", 0x0e6efc5e60aca0d4),
    ("f32 complex inverse 4096", 0x3c6773959b5e2933),
    ("f32 real forward 4096", 0x160fc18f93e8ab28),
    ("f32 real inverse 4096", 0x60ba615e4862f97f),
    ("f32 complex forward 8192", 0xc102489e0ef55cdd),
    ("f32 complex inverse 8192", 0x24197b36565db8cb),
    ("f32 real forward 8192", 0x122738f38bf80f6a),
    ("f32 real inverse 8192", 0xeb71b65fbc3673ad),
    ("f32 complex forward 16384", 0xdfd9ea2f19b06651),
    ("f32 complex inverse 16384", 0x8b4a75bcb4bb9305),
    ("f32 real forward 16384", 0xa60e144cae935e8a),
    ("f32 real inverse 16384", 0x03d4e2078a8eaf6d),
    ("f32 complex forward 32768", 0xb45c032e1188b145),
    ("f32 complex inverse 32768", 0xec12e40d7a5fa17b),
    ("f32 real forward 32768", 0x42b0bdee82694526),
    ("f32 real inverse 32768", 0x15fdee9866b1bb7b),
    ("f32 complex forward 65536", 0x8700d995cdc1cc3d),
    ("f32 complex inverse 65536", 0x6c85053a39e67446),
    ("f32 real forward 65536", 0x3d99c1f890178bb1),
    ("f32 real inverse 65536", 0xe6a1c8fd9c472445),
    ("f32 complex forward 131072", 0xf437304a8f0f0d43),
    ("f32 complex inverse 131072", 0xd2fba8cf9eebd06e),
    ("f32 real forward 131072", 0x32c6ba1eb9af3ca7),
    ("f32 real inverse 131072", 0x94776b48d7842927),
    ("f32 complex forward 262144", 0xa5ce77446994edc9),
    ("f32 complex inverse 262144", 0x079827e02769cd12),
    ("f32 real forward 262144", 0x41aa977f0b3aa1b1),
    ("f32 real inverse 262144", 0x3b88306722220f12),
    ("f32 complex forward 524288", 0xf1ef0c4271b14d5a),
    ("f32 complex inverse 524288", 0x3d4258a79304b352),
    ("f32 real forward 524288", 0x5a77d35e7accfbea),
    ("f32 real inverse 524288", 0x74e4abdc9aee84d6),
    ("f64 twiddles 524288", 0xcffd9a5b26d5d3dd),
    ("f64 complex forward 256", 0x1d29cb2ac95483e8),
    ("f64 complex inverse 256", 0x9e3096012c2a32b7),
    ("f64 real forward 256", 0xc3f9f146e67405bd),
    ("f64 real inverse 256", 0x08dfca733956e07c),
    ("f64 complex forward 512", 0xd75d980aa4c8e40d),
    ("f64 complex inverse 512", 0x3bfe2258b787d3bb),
    ("f64 real forward 512", 0x9750670c70aaee13),
    ("f64 real inverse 512", 0x107d594e43b15f6c),
    ("f64 complex forward 1024", 0x712885f1b10c9d15),
    ("f64 complex inverse 1024", 0x91db72248925ef74),
    ("f64 real forward 1024", 0x093167c358cf8f54),
    ("f64 real inverse 1024", 0xb987de6baa8d4cbb),
    ("f64 complex forward 2048", 0x953001ab0805d61f),
    ("f64 complex inverse 2048", 0xb88afc0c6201dc2c),
    ("f64 real forward 2048", 0x4140a1d5a7c4dbae),
    ("f64 real inverse 2048", 0x77bb5e5116ac00e7),
    ("f64 complex forward 4096", 0x7dbea4feeb38075e),
    ("f64 complex inverse 4096", 0xd8638cda770dba2e),
    ("f64 real forward 4096", 0x372e40ee5e2247b2),
    ("f64 real inverse 4096", 0xb9118a65b3e1cf57),
    ("f64 complex forward 8192", 0xa5e89f1bf382fbb6),
    ("f64 complex inverse 8192", 0xa2d662b9747763af),
    ("f64 real forward 8192", 0xe19731fc495f1505),
    ("f64 real inverse 8192", 0x4f40309c0a377cbf),
    ("f64 complex forward 16384", 0x6e86f55ec9a01534),
    ("f64 complex inverse 16384", 0xaa9fd08ec314e6bc),
    ("f64 real forward 16384", 0x4edb56ada40049ad),
    ("f64 real inverse 16384", 0x5ae7fe86e3ba34cb),
    ("f64 complex forward 32768", 0xedb8986f9e3db5b4),
    ("f64 complex inverse 32768", 0x63a168e5dd2f0285),
    ("f64 real forward 32768", 0xc392b1d239b5a3a2),
    ("f64 real inverse 32768", 0xf58a40ef55a1d03e),
    ("f64 complex forward 65536", 0x07d27015fb6bd7f1),
    ("f64 complex inverse 65536", 0xee1078dd9ca3d9f8),
    ("f64 real forward 65536", 0x7da31702c181a2f3),
    ("f64 real inverse 65536", 0x04b28e4861a63c3d),
    ("f64 complex forward 131072", 0x45c707fe7518d503),
    ("f64 complex inverse 131072", 0x673123f248a43fc4),
    ("f64 real forward 131072", 0xf5315c0e54b224c9),
    ("f64 real inverse 131072", 0xfa76d10b301779ed),
    ("f64 complex forward 262144", 0xd8296426bd53439c),
    ("f64 complex inverse 262144", 0x438e0bf599211252),
    ("f64 real forward 262144", 0x40f77ec8d765e8ec),
    ("f64 real inverse 262144", 0x71cdeb4ca873cf5c),
    ("f64 complex forward 524288", 0xcf041e1358a7eb58),
    ("f64 complex inverse 524288", 0x25c1c0e5d1145845),
    ("f64 real forward 524288", 0x1e11aed344dfc312),
    ("f64 real inverse 524288", 0x11da76ab1dd0f67d),
];

#[test]
fn every_transform_matches_its_pinned_output_bits() {
    let got = computed();
    let mut bad = Vec::new();
    for (name, hash) in &got {
        match PINNED.iter().find(|(n, _)| n == name) {
            Some((_, pin)) if pin == hash => {}
            _ => bad.push(format!("    (\"{name}\", 0x{hash:016x}),")),
        }
    }
    assert_eq!(PINNED.len(), got.len(), "every case has exactly one pin; computed:\n{}", bad.join("\n"));
    assert!(bad.is_empty(), "FFT output bits moved (a change of the FFT's arithmetic or of libm, ADR 0055 2.1):\n{}", bad.join("\n"));
}

/// Capture helper, ignored by design: prints the whole table in the form of `PINNED`.
/// `cargo test -p transit-math --test fft_pinned -- --ignored --nocapture`.
#[test]
#[ignore = "capture helper: prints the pinned table"]
fn print_pinned_table() {
    for (name, hash) in computed() {
        println!("    (\"{name}\", 0x{hash:016x}),");
    }
}
