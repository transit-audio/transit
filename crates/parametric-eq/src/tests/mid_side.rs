//! Mid/side exactness (Evidence "Mid/side exactness", decision 4, the narrowed claim):
//! - encode then decode with nothing between returns every f32 pair bit for bit, over random pairs
//!   within a factor of 2^29, pairs with a zero (equal in value; a −0 returns as +0) and full-scale
//!   extremes (negative control in-process: the f32 encode and decode fails);
//! - with Mid bands only, the output's side component equals the input's within one unit in the last
//!   place of the larger output;
//! - with a Mid band at identity (skipped), every pair, `(1, 1e-40)` included, passes bit for bit
//!   (negative control, recorded: running the encode and decode for a skipped band loses the quiet
//!   channel);
//! - with no Mid or Side band, the output is bit-identical to a build with the stage removed.
//!
//! The round trip and the pass-through are IEEE-exact (additions, subtractions and halvings).

use super::proc::{eq, run};
use super::util::{Rng, band, noise};
use crate::chain::{decode_ms, encode_ms};
use crate::{BandType, Placement};

fn round_trip(l: f32, r: f32) -> (f32, f32) {
    let (m, s) = encode_ms(f64::from(l), f64::from(r));
    let (a, b) = decode_ms(m, s);
    (a as f32, b as f32)
}

fn pairs() -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mag = |rng: &mut Rng, e: i32| {
        let sign = if rng.next_u64() & 1 == 0 { 1.0 } else { -1.0 };
        (sign * (0.5 + 0.25 * (rng.uniform() + 1.0)) * 2f64.powi(-e)) as f32
    };
    for _ in 0..200_000 {
        // Exponents at most 28 apart, so the magnitudes are within a factor of 2^29.
        let a = (rng.next_u64() % 100) as i32 - 20;
        let b = a + (rng.next_u64() % 57) as i32 - 28;
        let l = mag(&mut rng, a);
        let r = mag(&mut rng, b);
        v.push((l, r));
    }
    for x in [1.0f32, -1.0, 0.5, f32::MIN_POSITIVE, 1e-40, f32::MAX, -f32::MAX, 3.0e38] {
        v.push((x, 0.0));
        v.push((0.0, x));
        v.push((-0.0, x));
        v.push((x, x));
        v.push((x, -x));
    }
    v
}

#[test]
fn encode_then_decode_returns_every_pair_bit_for_bit() {
    let ps = pairs();
    for &(l, r) in &ps {
        let (a, b) = round_trip(l, r);
        if l == 0.0 || r == 0.0 {
            // A signed zero comes back as +0 (`(1 + −1)·0.5` is +0): equal in value, not in bits.
            // The record's "bit for bit" holds for every non-zero sample; the result note lists this.
            assert_eq!((a, b), (l, r), "({l:e}, {r:e})");
            assert_eq!((a.to_bits() & 0x7fff_ffff, b.to_bits() & 0x7fff_ffff), (l.to_bits() & 0x7fff_ffff, r.to_bits() & 0x7fff_ffff));
        } else {
            assert_eq!((a.to_bits(), b.to_bits()), (l.to_bits(), r.to_bits()), "({l:e}, {r:e})");
        }
    }
    // Negative control: the same steps in f32 lose pairs.
    let lost = ps
        .iter()
        .filter(|&&(l, r)| {
            let (m, s) = (l + r, l - r);
            ((m + s) * 0.5).to_bits() != l.to_bits() || ((m - s) * 0.5).to_bits() != r.to_bits()
        })
        .count();
    eprintln!("mid/side: {} pairs exact in f64; the f32 encode and decode loses {lost}", ps.len());
    assert!(lost > 0);
}

fn ulp(x: f32) -> f64 {
    let x = x.abs();
    f64::from(f32::from_bits(x.to_bits() + 1)) - f64::from(x)
}

#[test]
fn mid_bands_leave_the_side_within_one_unit_in_the_last_place() {
    let fs = 48_000.0;
    let mut e = eq(fs, 2);
    let mut b = band(BandType::Bell, 800.0, 9.0, 1.5);
    b.place = Placement::Mid;
    e.set_band(0, b);
    let mut b2 = band(BandType::HighShelf, 6_000.0, -6.0, 0.707);
    b2.place = Placement::Mid;
    e.set_band(3, b2);
    e.reset();
    let input = noise(9_600, 2, 7);
    let mut io = input.clone();
    run(&mut e, &mut io, 64);
    for i in 0..input[0].len() {
        let side_in = f64::from(input[0][i]) - f64::from(input[1][i]);
        let side_out = f64::from(io[0][i]) - f64::from(io[1][i]);
        let tol = ulp(if io[0][i].abs() > io[1][i].abs() { io[0][i] } else { io[1][i] });
        assert!((side_out - side_in).abs() <= tol, "frame {i}: side {side_out:e} vs {side_in:e} (ulp {tol:e})");
    }
}

#[test]
fn a_skipped_mid_band_keeps_every_pair_bit_for_bit() {
    let mut e = eq(48_000.0, 2);
    for (slot, place) in [(0, Placement::Mid), (5, Placement::Side)] {
        let mut b = band(BandType::Bell, 1_000.0, 0.0, 1.0);
        b.place = place;
        e.set_band(slot, b);
    }
    e.reset();
    let mut input = noise(4_096, 2, 11);
    input[0][100] = 1.0;
    input[1][100] = 1e-40;
    input[0][200] = 1e-40;
    input[1][200] = -1.0;
    let mut io = input.clone();
    run(&mut e, &mut io, 100);
    for c in 0..2 {
        for i in 0..input[c].len() {
            assert_eq!(io[c][i].to_bits(), input[c][i].to_bits(), "channel {c} frame {i}");
        }
    }
}

#[test]
fn without_mid_or_side_bands_the_output_equals_a_build_without_the_stage() {
    let setup = |e: &mut crate::ParametricEq| {
        e.set_band(0, band(BandType::Bell, 300.0, 5.0, 2.0));
        let mut l = band(BandType::LowCut, 80.0, 0.0, 1.0);
        l.place = Placement::Left;
        e.set_band(1, l);
        let mut r = band(BandType::HighShelf, 9_000.0, -4.0, 0.9);
        r.place = Placement::Right;
        e.set_band(2, r);
        e.reset();
    };
    let input = noise(8_000, 2, 3);
    let mut a = eq(44_100.0, 2);
    setup(&mut a);
    let mut b = eq(44_100.0, 2);
    b.no_stage2 = true;
    setup(&mut b);
    let (mut oa, mut ob) = (input.clone(), input.clone());
    run(&mut a, &mut oa, 64);
    run(&mut b, &mut ob, 64);
    for c in 0..2 {
        assert!(oa[c].iter().zip(&ob[c]).all(|(x, y)| x.to_bits() == y.to_bits()), "channel {c}");
    }
}
