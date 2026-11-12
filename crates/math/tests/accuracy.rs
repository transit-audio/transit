//! Accuracy bounds (ADR 0055, 2.3): each function's worst error over its argument set, in ulps, against
//! a higher-precision reference: each `f32` function against its `f64` counterpart (the unrounded `f64`
//! value, so the figure is fractional), each `f64` function against reference values computed once with
//! arbitrary precision (`data/f64_reference.txt`, whose header says how). The bound each test pins is the
//! measured worst case, so a change in either direction is seen; a function above 2 ulp is reported to
//! the maintainer before its first caller converts.

mod common;

use common::{args32, ord64, pairs32, ulp32};
use transit_math::{f32 as m32, f64 as m64};

/// Error of `got` against the exact-enough value `want`, in `f32` ulps at `want`. `None` where the
/// comparison is not one of finite values in `f32` range (non-finite outputs are covered by the tables).
fn err32(got: f32, want: f64) -> Option<f64> {
    if !got.is_finite() || !want.is_finite() || want.abs() > f64::from(f32::MAX) {
        return None;
    }
    Some((f64::from(got) - want).abs() / ulp32(want))
}

fn worst_one(name: &str, f: fn(f32) -> f32, g: fn(f64) -> f64) -> (f64, f32) {
    let mut worst = (0.0, 0.0);
    for x in args32(name) {
        if let Some(e) = err32(f(x), g(f64::from(x)))
            && e > worst.0
        {
            worst = (e, x);
        }
    }
    worst
}

fn worst_two(name: &str, f: fn(f32, f32) -> f32, g: fn(f64, f64) -> f64) -> (f64, (f32, f32)) {
    let mut worst = (0.0, (0.0, 0.0));
    for (a, b) in pairs32(name) {
        if let Some(e) = err32(f(a, b), g(f64::from(a), f64::from(b)))
            && e > worst.0
        {
            worst = (e, (a, b));
        }
    }
    worst
}

/// The measured worst cases, in ulps, rounded up in the third decimal.
const BOUNDS_F32: &[(&str, f64)] = &[
    ("sin", 0.501),
    ("cos", 0.501),
    ("tan", 0.793),
    ("exp", 0.798),
    ("exp2", 0.501),
    ("ln", 0.628),
    ("log2", 0.592),
    ("log10", 0.689),
    ("tanh", 1.740),
    ("powf", 0.771),
    ("atan2", 1.201),
];
const BOUNDS_F64: &[(&str, f64)] = &[
    ("sin", 0.565),
    ("cos", 0.513),
    ("tan", 0.546),
    ("exp", 0.722),
    ("exp2", 0.500),
    ("ln", 0.158),
    ("log2", 0.500),
    ("log10", 0.494),
    ("pow", 0.712),
    ("atan2", 1.014),
];

fn check(kind: &str, measured: &[(String, f64, String)], bounds: &[(&str, f64)]) {
    let mut report = String::new();
    let mut bad = Vec::new();
    for (name, e, at) in measured {
        report.push_str(&format!("{kind}::{name:<6} worst {e:.4} ulp at {at}\n"));
        match bounds.iter().find(|(n, _)| n == name) {
            Some((_, b)) if e <= b && *e > b - 0.002 => {}
            _ => bad.push(format!("    (\"{name}\", {:.3}),", (e * 1000.0).ceil() / 1000.0)),
        }
        if *e > 2.0 {
            println!("REPORT: {kind}::{name} is above 2 ulp ({e:.4}); the maintainer decides before a caller converts");
        }
    }
    println!("{report}");
    assert!(bad.is_empty() && bounds.len() == measured.len(), "accuracy bounds moved or unpinned:\n{}", bad.join("\n"));
}

#[test]
fn f32_functions_stay_within_their_measured_bounds_against_f64() {
    let mut m = Vec::new();
    let mut one = |name: &str, f: fn(f32) -> f32, g: fn(f64) -> f64| {
        let (e, x) = worst_one(name, f, g);
        m.push((name.to_string(), e, format!("{x:e}")));
    };
    one("sin", m32::sin, m64::sin);
    one("cos", m32::cos, m64::cos);
    one("tan", m32::tan, m64::tan);
    one("exp", m32::exp, m64::exp);
    one("exp2", m32::exp2, m64::exp2);
    one("ln", m32::ln, m64::ln);
    one("log2", m32::log2, m64::log2);
    one("log10", m32::log10, m64::log10);
    // The seam has no f64 tanh (none of its callers needs one); libm's f64 tanh is the reference.
    one("tanh", m32::tanh, libm::tanh);
    let mut two = |name: &str, f: fn(f32, f32) -> f32, g: fn(f64, f64) -> f64| {
        let (e, (a, b)) = worst_two(name, f, g);
        m.push((name.to_string(), e, format!("({a:e}, {b:e})")));
    };
    two("powf", m32::powf, m64::pow);
    two("atan2", m32::atan2, m64::atan2);
    check("f32", &m, BOUNDS_F32);
}

const F64_REFERENCE: &str = include_str!("data/f64_reference.txt");

fn f64_of(hex: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(hex, 16).expect("hex"))
}

#[test]
fn f64_functions_stay_within_their_measured_bounds_against_the_reference_table() {
    let mut worst: Vec<(String, f64, String)> = Vec::new();
    let mut count = 0;
    for line in F64_REFERENCE.lines().filter(|l| !l.starts_with('#')) {
        let w: Vec<&str> = line.split_whitespace().collect();
        let (got, args, want, resid) = match w[0] {
            "pow" | "atan2" => {
                let (a, b) = (f64_of(w[1]), f64_of(w[2]));
                let got = if w[0] == "pow" { m64::pow(a, b) } else { m64::atan2(a, b) };
                (got, format!("({a:e}, {b:e})"), f64_of(w[3]), w[4])
            }
            name => {
                let x = f64_of(w[1]);
                let f: fn(f64) -> f64 = match name {
                    "sin" => m64::sin,
                    "cos" => m64::cos,
                    "tan" => m64::tan,
                    "exp" => m64::exp,
                    "exp2" => m64::exp2,
                    "ln" => m64::ln,
                    "log2" => m64::log2,
                    "log10" => m64::log10,
                    _ => panic!("unknown function {name}"),
                };
                (f(x), format!("{x:e}"), f64_of(w[2]), w[3])
            }
        };
        let resid: f64 = resid.parse().expect("residual");
        // Floats between the result and the correctly rounded value, less the exact value's offset.
        let e = ((ord64(got) - ord64(want)) as f64 - resid).abs();
        count += 1;
        match worst.iter_mut().find(|(n, _, _)| n == w[0]) {
            Some(slot) if e > slot.1 => *slot = (w[0].to_string(), e, args),
            Some(_) => {}
            None => worst.push((w[0].to_string(), e, args)),
        }
    }
    assert_eq!(count, 2560, "the reference table holds 256 values for each of the ten f64 functions");
    check("f64", &worst, BOUNDS_F64);
}
