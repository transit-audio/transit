//! The closed fade law's factor function and the stable `f64` shape forms (ADR 0058, decision 13).
//! Expected values come from the law's definition (exact endpoints, the midpoint, exact rationals
//! such as 3/99 and 4/11, each the correctly rounded `f64` of a single IEEE division) or from an
//! independent series written out here; none is captured from the code under test.

use super::*;
use core::f64::consts::{FRAC_1_SQRT_2, PI};

const CURVES: [f32; 5] = [0.0, 0.5, -0.5, -1.0, 0.263_2];

fn fade(len: Samples, shape: FadeShape, curvature: f32) -> Fade {
    Fade { len, shape, curvature }
}

fn none() -> Fade {
    Fade { len: 0, ..Fade::default() }
}

#[test]
fn the_closed_law_starts_at_exactly_0_and_ends_at_exactly_1() {
    for shape in FadeShape::ALL {
        for c in CURVES {
            for m in [2, 3, 7, 64, 441] {
                let len = m + 10;
                let f = fade(m, shape, c);
                let fin = |k| fade_factors(len, f, none(), k).0;
                let fout = |k| fade_factors(len, none(), f, k).1;
                assert_eq!(fin(0), 0.0, "{shape:?} {c} {m}: a fade-in starts at 0");
                assert_eq!(fin(m - 1), 1.0, "{shape:?} {c} {m}: and reaches 1 on its last sample");
                assert_eq!(fin(m), 1.0);
                assert_eq!(fout(len - 1), 0.0, "{shape:?} {c} {m}: a fade-out's last sample is 0");
                assert_eq!(fout(len - m), 1.0, "{shape:?} {c} {m}: and its first is 1");
                assert_eq!(fout(len - m - 1), 1.0);
                for k in 1..m - 1 {
                    let g = fin(k);
                    assert!(g > 0.0 && g < 1.0, "{shape:?} {c} {m}: interior sample {k} is {g}");
                }
            }
        }
    }
}

#[test]
fn a_fade_out_is_the_fade_in_reversed_sample_for_sample() {
    for shape in FadeShape::ALL {
        for c in CURVES {
            for m in [1, 2, 5, 100] {
                let len = 2 * m + 3;
                let f = fade(m, shape, c);
                for j in 0..m {
                    let fin = fade_factors(len, f, none(), j).0;
                    let fout = fade_factors(len, none(), f, len - 1 - j).1;
                    assert_eq!(fin.to_bits(), fout.to_bits(), "{shape:?} {c} {m} at {j}");
                }
            }
        }
    }
}

#[test]
fn the_records_100_sample_linear_fade_out() {
    let f = fade(100, FadeShape::Linear, 0.0);
    let fout = |k| fade_factors(100, none(), f, k).1;
    assert_eq!(fout(96), 3.0 / 99.0);
    assert_eq!(fout(97), 2.0 / 99.0);
    assert_eq!(fout(99), 0.0);
    assert_eq!(fout(0), 1.0);
}

#[test]
fn a_one_sample_fade_is_the_midpoint() {
    let mid = |shape, c| fade_factors(10, fade(1, shape, c), none(), 0).0;
    assert_eq!(mid(FadeShape::Linear, 0.0), 0.5);
    assert_eq!(mid(FadeShape::Exponential, 0.0), 0.125);
    assert_eq!(mid(FadeShape::Logarithmic, 0.0), 0.875);
    assert!((mid(FadeShape::EqualPower, 0.0) - FRAC_1_SQRT_2).abs() < 1.0e-15);
    assert!((mid(FadeShape::SCurve, 0.0) - 0.5).abs() < 1.0e-15);
    // The record's curved case: Exponential at +0.5 and -0.5 give 4/11 and 1/29.
    assert_eq!(mid(FadeShape::Exponential, 0.5), 4.0 / 11.0);
    assert_eq!(mid(FadeShape::Exponential, -0.5), 1.0 / 29.0);
    // The one-sample fade-out is the same midpoint on the clip's last sample, and only there.
    for shape in FadeShape::ALL {
        let (_, out) = fade_factors(10, none(), fade(1, shape, 0.0), 9);
        assert_eq!(out, mid(shape, 0.0), "{shape:?}");
        assert_eq!(fade_factors(10, none(), fade(1, shape, 0.0), 8).1, 1.0);
        assert_eq!(fade_factors(10, fade(1, shape, 0.0), none(), 1).0, 1.0);
    }
}

#[test]
fn zero_length_overlong_and_out_of_domain_cases_clamp() {
    let len = 50;
    for shape in FadeShape::ALL {
        for k in [-3, 0, 25, 49, 60] {
            assert_eq!(fade_factors(len, none(), none(), k), (1.0, 1.0));
            assert_eq!(fade_factors(len, fade(-7, shape, 0.5), fade(-7, shape, 0.5), k), (1.0, 1.0), "a negative length is no fade");
        }
        let long = fade(4 * len, shape, 0.25);
        let whole = fade(len, shape, 0.25);
        for k in 0..len {
            assert_eq!(fade_factors(len, long, long, k), fade_factors(len, whole, whole, k), "{shape:?} at {k}: clamped to the clip");
        }
        assert_eq!(fade_factors(len, long, none(), len - 1).0, 1.0);
        // Outside the domain: the value at the nearest end.
        assert_eq!(fade_factors(len, whole, whole, -5), fade_factors(len, whole, whole, 0));
        assert_eq!(fade_factors(len, whole, whole, len + 7), fade_factors(len, whole, whole, len - 1));
        // A clip of length 0 counts as one sample.
        assert_eq!(fade_factors(0, fade(1, shape, 0.0), none(), 0), fade_factors(1, fade(1, shape, 0.0), none(), 0));
    }
}

#[test]
fn the_four_sample_exponential_fade_in_gives_the_correctly_rounded_1_27() {
    let (fin, _) = fade_factors(10, fade(4, FadeShape::Exponential, 0.0), none(), 1);
    // 0x3d17_b426 is 0.03703703731298447, the correctly rounded f32 of 1/27 (the record's value).
    assert_eq!((fin as f32).to_bits(), 0x3d17_b426, "1/27 rounded once");
    assert_eq!(fin as f32, (1.0f64 / 27.0) as f32);
}

/// sin(x) by its series for a tiny `x`: x - x^3/6 + x^5/120, far beyond f64 precision there.
fn sin_tiny(x: f64) -> f64 {
    x - x * x * x / 6.0 + x * x * x * x * x / 120.0
}

/// The distance in f32 steps between two positive f32 values.
fn steps(a: f32, b: f32) -> i64 {
    (i64::from(a.to_bits()) - i64::from(b.to_bits())).abs()
}

#[test]
fn the_stable_forms_hold_at_the_first_samples_of_long_fades() {
    // S-Curve, a closed fade of 1,000,001 samples, sample 1: t = 1e-6.
    let m = 1_000_001;
    let (s, _) = fade_factors(m, fade(m, FadeShape::SCurve, 0.0), none(), 1);
    let t = 1.0 / (m - 1) as f64;
    let v = sin_tiny(t * FRAC_PI_2);
    assert_eq!(s as f32, (v * v) as f32, "against the series reference");
    // 0x2c2d_a0bf is 2.467401066449937e-12, the record's correctly rounded value.
    assert_eq!((s as f32).to_bits(), 0x2c2d_a0bf, "the record's correctly rounded value");
    // The cosine form cancels there (the record: 65 f32 steps off).
    let cosine = 0.5 - 0.5 * transit_math::f64::cos(PI * t);
    assert!(steps(cosine as f32, s as f32) > 8, "the cosine form is {} steps off", steps(cosine as f32, s as f32));

    // Logarithmic, a closed fade of 10^12 + 1 samples, sample 1: t = 1e-12.
    let m = 1_000_000_000_001;
    let (g, _) = fade_factors(m, fade(m, FadeShape::Logarithmic, 0.0), none(), 1);
    let t = 1.0 / (m - 1) as f64;
    let reference = 3.0 * t * (1.0 - t) + t * t * t;
    assert_eq!(g as f32, reference as f32, "against the expanded polynomial");
    let cubic = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
    assert!(steps(cubic as f32, g as f32) > 100, "the cubic form is {} steps off", steps(cubic as f32, g as f32));
}

#[test]
fn curvature_and_the_argument_are_sanitized_and_the_weighting_follows_its_formula() {
    for shape in FadeShape::ALL {
        let flat = FadeKernel::new(fade(10, shape, 0.0));
        for c in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0] {
            let k = FadeKernel::new(fade(10, shape, c));
            for i in 0..=64 {
                let t = f64::from(i) / 64.0;
                assert_eq!(k.factor(t).to_bits(), flat.factor(t).to_bits(), "{shape:?}, curvature {c} is none");
            }
        }
        for (wild, tame) in [(10.0, 1.0), (-10.0, -1.0)] {
            let (a, b) = (FadeKernel::new(fade(10, shape, wild)), FadeKernel::new(fade(10, shape, tame)));
            for i in 0..=64 {
                let t = f64::from(i) / 64.0;
                assert_eq!(a.factor(t).to_bits(), b.factor(t).to_bits(), "{shape:?}: {wild} clamps to {tame}");
            }
        }
        let k = FadeKernel::new(fade(10, shape, 0.5));
        assert_eq!(k.factor(f64::NAN), 0.0, "a NaN argument is 0");
        assert_eq!(k.factor(-1.0), 0.0);
        assert_eq!(k.factor(2.0), 1.0);
    }
    // Linear at curvature 0.25, t = 1/2: b = 1/2, w = b * 2 = 1, the factor 1 / (2 - 1/2) = 2/3.
    assert_eq!(FadeKernel::new(fade(10, FadeShape::Linear, 0.25)).factor(0.5), 2.0 / 3.0);
}
