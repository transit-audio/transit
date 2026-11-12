//! Identities that must hold exactly (ADR 0055, 2.3), `powi`'s stated order, the exact re-exports and
//! the shared constants.

mod common;

use common::{args32, args64, canon32, canon64, lin32, lin64, specials32, specials64, triples32, triples64};
use transit_math::{consts, f32 as m32, f64 as m64};

#[test]
fn exp2_of_an_integer_and_log2_of_a_power_of_two_are_exact() {
    for k in -149i32..=127 {
        let p = if k >= -126 { f32::from_bits(((k + 127) as u32) << 23) } else { f32::from_bits(1 << (k + 149)) };
        assert_eq!(m32::exp2(k as f32).to_bits(), p.to_bits(), "exp2({k})");
        assert_eq!(m32::log2(p), k as f32, "log2(2^{k})");
    }
    for k in -1074i32..=1023 {
        let p = if k >= -1022 { f64::from_bits(((k + 1023) as u64) << 52) } else { f64::from_bits(1 << (k + 1074)) };
        assert_eq!(m64::exp2(f64::from(k)).to_bits(), p.to_bits(), "exp2({k})");
        assert_eq!(m64::log2(p), f64::from(k), "log2(2^{k})");
    }
}

#[test]
fn sine_and_cosine_at_zero_are_exact() {
    assert_eq!(m32::sin(0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(m32::sin(-0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(m32::cos(0.0), 1.0);
    assert_eq!(m32::cos(-0.0), 1.0);
    assert_eq!(m64::sin(0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(m64::sin(-0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(m64::cos(0.0), 1.0);
    assert_eq!(m64::cos(-0.0), 1.0);
}

#[test]
fn any_base_to_the_power_zero_is_one() {
    let mut xs = args32("ln");
    xs.extend(specials32());
    xs.extend(lin32(-100.0, 100.0, 1001));
    for x in xs {
        assert_eq!(m32::powf(x, 0.0), 1.0, "powf({x}, 0)");
        assert_eq!(m32::powf(x, -0.0), 1.0, "powf({x}, -0)");
    }
    let mut xs = args64("ln");
    xs.extend(specials64());
    xs.extend(lin64(-100.0, 100.0, 1001));
    for x in xs {
        assert_eq!(m64::pow(x, 0.0), 1.0, "pow({x}, 0)");
        assert_eq!(m64::pow(x, -0.0), 1.0, "pow({x}, -0)");
    }
}

#[test]
fn sin_cos_is_the_two_calls() {
    for x in args32("sin") {
        let (s, c) = m32::sin_cos(x);
        assert_eq!((canon32(s), canon32(c)), (canon32(m32::sin(x)), canon32(m32::cos(x))), "{x}");
    }
    for x in args64("sin") {
        let (s, c) = m64::sin_cos(x);
        assert_eq!((canon64(s), canon64(c)), (canon64(m64::sin(x)), canon64(m64::cos(x))), "{x}");
    }
}

fn repeated32(x: f32, n: u32) -> f32 {
    (0..n).fold(1.0, |acc, _| acc * x)
}

fn repeated64(x: f64, n: u32) -> f64 {
    (0..n).fold(1.0, |acc, _| acc * x)
}

#[test]
fn powi_matches_repeated_multiplication_where_the_order_cannot_matter() {
    let mut xs = lin32(-4.0, 4.0, 4001);
    xs.extend(specials32());
    for &x in &xs {
        // Up to the cube every order rounds the same (x·(x·x) = (x·x)·x: multiplication commutes).
        for n in 0..=3 {
            assert_eq!(canon32(m32::powi(x, n as i32)), canon32(repeated32(x, n)), "{x}^{n}");
        }
        assert_eq!(canon32(m32::powi(x, 4)), canon32((x * x) * (x * x)), "{x}^4 squares twice");
        for n in 1..=40 {
            assert_eq!(canon32(m32::powi(x, -n)), canon32(1.0 / m32::powi(x, n)), "{x}^-{n}");
        }
    }
    // Where every product is exact (powers of two, and three while 3^n fits 24 bits) any order agrees.
    for j in -4..=4 {
        let x = m32::exp2(j as f32);
        for n in 0..=30u32 {
            assert_eq!(m32::powi(x, n as i32), repeated32(x, n), "2^{j} to the {n}");
            assert_eq!(m32::powi(-x, n as i32), repeated32(-x, n), "-2^{j} to the {n}");
        }
    }
    for n in 0..=15u32 {
        assert_eq!(m32::powi(3.0, n as i32), repeated32(3.0, n), "3^{n}");
    }
    let mut xs = lin64(-4.0, 4.0, 4001);
    xs.extend(specials64());
    for &x in &xs {
        for n in 0..=3 {
            assert_eq!(canon64(m64::powi(x, n as i32)), canon64(repeated64(x, n)), "{x}^{n}");
        }
        assert_eq!(canon64(m64::powi(x, 4)), canon64((x * x) * (x * x)), "{x}^4 squares twice");
        for n in 1..=40 {
            assert_eq!(canon64(m64::powi(x, -n)), canon64(1.0 / m64::powi(x, n)), "{x}^-{n}");
        }
    }
    for n in 0..=33u32 {
        assert_eq!(m64::powi(3.0, n as i32), repeated64(3.0, n), "3^{n}");
    }
}

#[test]
fn the_exact_operations_equal_rusts_correctly_rounded_ones() {
    for name in ["sqrt", "floor", "ceil", "round", "trunc", "ln", "sin"] {
        for x in args32(name) {
            assert_eq!(canon32(m32::sqrt(x)), canon32(x.sqrt()), "sqrt({x})");
            assert_eq!(canon32(m32::floor(x)), canon32(x.floor()), "floor({x})");
            assert_eq!(canon32(m32::ceil(x)), canon32(x.ceil()), "ceil({x})");
            assert_eq!(canon32(m32::round(x)), canon32(x.round()), "round({x})");
            assert_eq!(canon32(m32::trunc(x)), canon32(x.trunc()), "trunc({x})");
        }
        for x in args64(name) {
            assert_eq!(canon64(m64::sqrt(x)), canon64(x.sqrt()), "sqrt({x})");
            assert_eq!(canon64(m64::floor(x)), canon64(x.floor()), "floor({x})");
            assert_eq!(canon64(m64::ceil(x)), canon64(x.ceil()), "ceil({x})");
            assert_eq!(canon64(m64::round(x)), canon64(x.round()), "round({x})");
            assert_eq!(canon64(m64::trunc(x)), canon64(x.trunc()), "trunc({x})");
        }
    }
    for (a, b, c) in triples32() {
        assert_eq!(canon32(m32::mul_add(a, b, c)), canon32(a.mul_add(b, c)), "mul_add({a}, {b}, {c})");
    }
    for (a, b, c) in triples64() {
        assert_eq!(canon64(m64::mul_add(a, b, c)), canon64(a.mul_add(b, c)), "mul_add({a}, {b}, {c})");
    }
}

#[test]
fn the_constants_are_the_correctly_rounded_literals() {
    // Bits of the correctly rounded values of 20·log10(2), log2(10)/20, log2(e) and log2(10), computed
    // offline with arbitrary precision.
    assert_eq!(consts::LOG2_TO_DB.to_bits(), 0x40c0_a8c1);
    assert_eq!(consts::DB_TO_LOG2.to_bits(), 0x3e2a_152d);
    assert_eq!(consts::LOG2_E.to_bits(), 0x3fb8_aa3b);
    assert_eq!(consts::LOG2_10.to_bits(), 0x4054_9a78);
    assert_eq!(consts::LOG2_TO_DB_F64.to_bits(), 0x4018_1518_24c7_587f);
    assert_eq!(consts::DB_TO_LOG2_F64.to_bits(), 0x3fc5_42a5_a12e_1c5b);
    assert_eq!(consts::LOG2_E_F64.to_bits(), 0x3ff7_1547_652b_82fe);
    assert_eq!(consts::LOG2_10_F64.to_bits(), 0x400a_934f_0979_a371);
    assert_eq!(consts::LOG2_E, core::f32::consts::LOG2_E);
    assert_eq!(consts::LOG2_10, core::f32::consts::LOG2_10);
    assert_eq!(consts::LOG2_E_F64, core::f64::consts::LOG2_E);
    assert_eq!(consts::LOG2_10_F64, core::f64::consts::LOG2_10);
}
