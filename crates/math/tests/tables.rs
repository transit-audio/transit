//! Pinned output tables (ADR 0055, 2.3): the bits of each function's outputs over its fixed argument
//! set, hashed with FNV-1a 64 and pinned. The outputs come from plain Rust over the exactly pinned
//! `libm`, so they are IEEE-exact by construction and the pins hold on every platform; NaN outputs hash
//! as one canonical pattern, because a NaN's sign and payload are unspecified. A `libm` change that
//! moves one bit fails here first: that is a DSP change under the upgrade procedure of 2.1, never a
//! re-pin per platform.

mod common;

use common::{Fnv, args32, args64, lin32, lin64, pairs32, pairs64, specials32, specials64, triples32, triples64};
use transit_math::{f32 as m32, f64 as m64};

fn one32(name: &str, f: fn(f32) -> f32) -> (String, u64) {
    let mut h = Fnv::new();
    for x in args32(name) {
        h.f32(f(x));
    }
    (format!("f32::{name}"), h.0)
}

fn one64(name: &str, f: fn(f64) -> f64) -> (String, u64) {
    let mut h = Fnv::new();
    for x in args64(name) {
        h.f64(f(x));
    }
    (format!("f64::{name}"), h.0)
}

fn two32(name: &str, f: fn(f32, f32) -> f32) -> (String, u64) {
    let mut h = Fnv::new();
    for (a, b) in pairs32(name) {
        h.f32(f(a, b));
    }
    (format!("f32::{name}"), h.0)
}

fn two64(name: &str, f: fn(f64, f64) -> f64) -> (String, u64) {
    let mut h = Fnv::new();
    for (a, b) in pairs64(name) {
        h.f64(f(a, b));
    }
    (format!("f64::{name}"), h.0)
}

fn computed() -> Vec<(String, u64)> {
    let mut v = vec![
        one32("sin", m32::sin),
        one32("cos", m32::cos),
        one32("tan", m32::tan),
        one32("exp", m32::exp),
        one32("exp2", m32::exp2),
        one32("ln", m32::ln),
        one32("log2", m32::log2),
        one32("log10", m32::log10),
        two32("powf", m32::powf),
        one32("tanh", m32::tanh),
        two32("atan2", m32::atan2),
        one32("sqrt", m32::sqrt),
        one32("floor", m32::floor),
        one32("ceil", m32::ceil),
        one32("round", m32::round),
        one32("trunc", m32::trunc),
        one64("sin", m64::sin),
        one64("cos", m64::cos),
        one64("tan", m64::tan),
        one64("exp", m64::exp),
        one64("exp2", m64::exp2),
        one64("ln", m64::ln),
        one64("log2", m64::log2),
        one64("log10", m64::log10),
        two64("pow", m64::pow),
        two64("atan2", m64::atan2),
        one64("sqrt", m64::sqrt),
        one64("floor", m64::floor),
        one64("ceil", m64::ceil),
        one64("round", m64::round),
        one64("trunc", m64::trunc),
    ];
    // powi: 64 bases from -4 to 4 and the specials, each to every power from -32 to 31.
    let mut h = Fnv::new();
    let mut xs = lin32(-4.0, 4.0, 64);
    xs.extend(specials32());
    for &x in &xs {
        for n in -32..32 {
            h.f32(m32::powi(x, n));
        }
    }
    v.push(("f32::powi".into(), h.0));
    let mut h = Fnv::new();
    let mut xs = lin64(-4.0, 4.0, 64);
    xs.extend(specials64());
    for &x in &xs {
        for n in -32..32 {
            h.f64(m64::powi(x, n));
        }
    }
    v.push(("f64::powi".into(), h.0));
    let mut h = Fnv::new();
    for (a, b, c) in triples32() {
        h.f32(m32::mul_add(a, b, c));
    }
    v.push(("f32::mul_add".into(), h.0));
    let mut h = Fnv::new();
    for (a, b, c) in triples64() {
        h.f64(m64::mul_add(a, b, c));
    }
    v.push(("f64::mul_add".into(), h.0));
    v
}

/// Captured at the commit that added each function; proven on Windows and Linux before review.
const PINNED: &[(&str, u64)] = &[
    ("f32::sin", 0xfae3ccc9a5b91c99),
    ("f32::cos", 0xb897b383f1c09ef5),
    ("f32::tan", 0x29d9dc13ceaee9ef),
    ("f32::exp", 0xbd16bc5d40cad7e7),
    ("f32::exp2", 0x436dc210dee5b3d0),
    ("f32::ln", 0x338e2448bfa6391a),
    ("f32::log2", 0xceacc92521e38608),
    ("f32::log10", 0xc3873bc4194b3f8c),
    ("f32::powf", 0x1e7ab79b7ac74959),
    ("f32::tanh", 0x377970a0bd2f694e),
    ("f32::atan2", 0x13a25fb81ba43180),
    ("f32::sqrt", 0xef4216c2e5a423a9),
    ("f32::floor", 0x2f6388b45fba07be),
    ("f32::ceil", 0xe246d85ca20c01a6),
    ("f32::round", 0x70741d22c771be08),
    ("f32::trunc", 0xa5074f29cf269380),
    ("f32::mul_add", 0xd49c9f742b42bd53),
    ("f32::powi", 0xd2ebefb616eb99da),
    ("f64::sin", 0x73d3fd62837394fc),
    ("f64::cos", 0xced317e21e5262f0),
    ("f64::tan", 0x3fedaebd2138c28c),
    ("f64::exp", 0x1d078a6e57740b14),
    ("f64::exp2", 0xf285dce7b7064642),
    ("f64::ln", 0x4b6a11816a1b26e1),
    ("f64::log2", 0xf8072563449b34c9),
    ("f64::log10", 0x65d138594a8c19f9),
    ("f64::pow", 0x20e1ea34c160e44b),
    ("f64::atan2", 0x893f7647db44ace7),
    ("f64::sqrt", 0xd14e69f3fdd828ec),
    ("f64::floor", 0x36a1c3a39834d806),
    ("f64::ceil", 0x8395771b8aa5e63a),
    ("f64::round", 0xcdba6a0db25dbd20),
    ("f64::trunc", 0xca6349ff909ea6f0),
    ("f64::mul_add", 0x79ce07ddb3d100a3),
    ("f64::powi", 0xc1346b34b8b5af72),
];

#[test]
fn every_function_matches_its_pinned_output_table() {
    let got = computed();
    let mut bad = Vec::new();
    for (name, hash) in &got {
        match PINNED.iter().find(|(n, _)| n == name) {
            Some((_, pin)) if pin == hash => {}
            _ => bad.push(format!("    (\"{name}\", 0x{hash:016x}),")),
        }
    }
    assert_eq!(PINNED.len(), got.len(), "every function has exactly one pin; computed:\n{}", bad.join("\n"));
    assert!(bad.is_empty(), "output tables moved or unpinned (a libm change is a DSP change, ADR 0055 2.1):\n{}", bad.join("\n"));
}
