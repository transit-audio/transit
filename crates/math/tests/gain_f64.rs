//! The `f64`-input gain conversions of ADR 0058 decision 3 (`db_to_gain_f64` and
//! `db_to_gain_f64_unrounded`): the named caps, the floor, NaN and infinities, the decades for an
//! `f64` input, bit-identity with the `f32` helper for matching clamped inputs, faithful rounding at
//! adversarial inputs against a double-double reference, and the monotonicity evidence (record test 13).
//!
//! Every expected value is an exact bit pattern (the decades, the same pins as `gain.rs`), a value of
//! the existing `f32` helper computed in process, or the double-double reference of `common/dd.rs`,
//! which the gain tests validate against the committed arbitrary-precision table. Nothing here depends
//! on the platform's math library: every argument is built with exact IEEE operations or bit patterns.

mod common;

use common::dd;
use common::ord64;
use transit_math::consts::{DB_TO_LOG2_F64, LOG2_TO_DB_F64};
use transit_math::f64 as m64;
use transit_math::gain::{CLIP_CAP_DB, FADER_CAP_DB, MAX_DB, MIN_DB, TRIM_CAP_DB, db_to_gain, db_to_gain_f64, db_to_gain_f64_unrounded};

/// The three caps, as the `f64` the new functions take and the `f32` the old helper's clamp takes.
const CAPS: [(f64, f32); 3] = [(FADER_CAP_DB, 12.0), (TRIM_CAP_DB, 24.0), (CLIP_CAP_DB, 48.0)];

/// The correctly rounded `f32` of 10^(d/20) for d = -140, -120, ..., +40 dB (bits): the pins of
/// `gain.rs`, proven there against the arbitrary-precision computation.
const DECADES: &[(i32, u32)] = &[
    (-140, 0x33d6_bf95),
    (-120, 0x3586_37bd),
    (-100, 0x3727_c5ac),
    (-80, 0x38d1_b717),
    (-60, 0x3a83_126f),
    (-40, 0x3c23_d70a),
    (-20, 0x3dcc_cccd),
    (0, 0x3f80_0000),
    (20, 0x4120_0000),
    (40, 0x42c8_0000),
];

/// The `f64` `n` steps away from `x` (consecutive values; `+0.0` and `-0.0` count as one).
fn step(x: f64, n: i128) -> f64 {
    let o = ord64(x) + n;
    if o < 0 { f64::from_bits((-o) as u64 | 0x8000_0000_0000_0000) } else { f64::from_bits(o as u64) }
}

/// A deterministic integer generator (xorshift64*), so the input sets are the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[test]
fn the_caps_are_the_records_named_constants() {
    assert_eq!((FADER_CAP_DB, TRIM_CAP_DB, CLIP_CAP_DB), (12.0, 24.0, 48.0));
    for (cap, _) in CAPS {
        assert!(cap.is_finite() && cap <= f64::from(MAX_DB));
    }
}

#[test]
fn the_floor_and_negative_infinity_give_exactly_zero() {
    let floor = f64::from(MIN_DB);
    for (cap, _) in CAPS {
        for d in [floor, step(floor, -1), -144.000_000_1, -150.0, -1.0e300, f64::MIN, f64::NEG_INFINITY] {
            assert_eq!(db_to_gain_f64(d, cap).to_bits(), 0, "db_to_gain_f64({d:e}, {cap}) is +0.0");
            assert_eq!(db_to_gain_f64_unrounded(d, cap).to_bits(), 0, "db_to_gain_f64_unrounded({d:e}, {cap}) is +0.0");
        }
        // The next f64 above the floor is a real gain, about 6.3e-8: the step to 0 is on the floor.
        let above = step(floor, 1);
        assert!(above > floor);
        let g = db_to_gain_f64(above, cap);
        assert!(g > 6.3e-8 && g < 6.32e-8, "just above the floor: {g:e}");
        assert!(db_to_gain_f64_unrounded(above, cap) > 0.0);
        assert!(db_to_gain_f64(-143.99, cap) > 0.0);
    }
}

#[test]
fn nan_inputs_quiet_and_signaling_of_both_signs_give_exactly_zero() {
    // f64 patterns: quiet and signaling, both signs, several payloads.
    let nans64: [u64; 9] = [
        0x7ff8_0000_0000_0000,
        0xfff8_0000_0000_0000,
        0x7ff8_0000_0000_0001,
        0x7fff_ffff_ffff_ffff,
        0xffff_ffff_ffff_ffff,
        0x7ff0_0000_0000_0001,
        0xfff0_0000_0000_0001,
        0x7ff4_0000_0000_0000,
        0xfff7_ffff_ffff_ffff,
    ];
    // f32 patterns, for the every-input equivalence with the f32 helper.
    let nans32: [u32; 6] = [0x7fc0_0000, 0xffc0_0000, 0x7f80_0001, 0xff80_0001, 0x7fa0_0000, 0xffbf_ffff];
    for (cap, cap32) in CAPS {
        for bits in nans64 {
            let d = f64::from_bits(bits);
            assert!(d.is_nan(), "{bits:#x} is a NaN");
            assert_eq!(db_to_gain_f64(d, cap).to_bits(), 0, "NaN {bits:#x}, cap {cap}");
            assert_eq!(db_to_gain_f64_unrounded(d, cap).to_bits(), 0, "NaN {bits:#x}, cap {cap}");
        }
        for bits in nans32 {
            let x = f32::from_bits(bits);
            assert!(x.is_nan(), "{bits:#x} is a NaN");
            let got = db_to_gain_f64(f64::from(x), cap);
            assert_eq!(got.to_bits(), 0, "NaN {bits:#x}, cap {cap}");
            // The every-input statement of the record: NaN is passed through, not clamped.
            let want = db_to_gain(if x.is_nan() { x } else { x.min(cap32) });
            assert_eq!(got.to_bits(), want.to_bits(), "NaN {bits:#x}, cap {cap}");
            // Why NaN must be tested before the clamp: f32::min returns the other operand, so the
            // naive equivalence would give the cap's gain (3.981071710586548 at +12 dB), not 0.
            let naive = db_to_gain(x.min(cap32));
            assert_eq!(naive, db_to_gain(cap32), "NaN.min(cap) is the cap");
            assert_ne!(naive.to_bits(), got.to_bits());
        }
    }
}

#[test]
fn positive_infinity_and_everything_above_the_cap_give_the_caps_gain() {
    // The record's value: +12 dB (and 12.01 dB, and anything above) give 3.981071710586548.
    assert_eq!(f64::from(db_to_gain_f64(12.0, FADER_CAP_DB)), 3.981_071_710_586_548);
    assert_eq!(db_to_gain_f64(12.01, FADER_CAP_DB), db_to_gain_f64(12.0, FADER_CAP_DB));
    // Not equal to the unclamped helper above the cap: 20 dB at the +12 dB cap is the cap's gain,
    // while db_to_gain(20) is exactly 10.
    assert_eq!(db_to_gain_f64(20.0, FADER_CAP_DB), db_to_gain_f64(12.0, FADER_CAP_DB));
    assert_eq!(db_to_gain(20.0), 10.0);
    for (cap, cap32) in CAPS {
        let at_cap = db_to_gain_f64(cap, cap);
        let at_cap64 = db_to_gain_f64_unrounded(cap, cap);
        assert_eq!(at_cap.to_bits(), db_to_gain(cap32).to_bits(), "the cap's gain is the f32 helper's at the cap");
        for d in [step(cap, 1), cap + 0.01, cap + 1.0, 199.9, 200.0, 1.0e300, f64::MAX, f64::INFINITY] {
            assert_eq!(db_to_gain_f64(d, cap).to_bits(), at_cap.to_bits(), "{d:e} dB at cap {cap}");
            assert_eq!(db_to_gain_f64_unrounded(d, cap).to_bits(), at_cap64.to_bits(), "{d:e} dB at cap {cap}");
        }
    }
}

#[test]
fn zero_db_and_the_decades_for_an_f64_input() {
    for (cap, _) in CAPS {
        for z in [0.0, -0.0] {
            assert_eq!(db_to_gain_f64(z, cap), 1.0);
            assert_eq!(db_to_gain_f64_unrounded(z, cap), 1.0);
        }
        for &(d, bits) in DECADES.iter().filter(|(d, _)| f64::from(*d) <= cap) {
            let g = db_to_gain_f64(f64::from(d), cap);
            assert_eq!(g.to_bits(), bits, "db_to_gain_f64({d}, {cap}) = {g:e}");
        }
    }
    assert_eq!(db_to_gain_f64(20.0, CLIP_CAP_DB), 10.0);
    assert_eq!(db_to_gain_f64(40.0, CLIP_CAP_DB), 100.0);
}

/// The rounded form is the unrounded form rounded once.
fn rounded_is_unrounded_rounded(d: f64, cap: f64) {
    assert_eq!(db_to_gain_f64(d, cap).to_bits(), (db_to_gain_f64_unrounded(d, cap) as f32).to_bits(), "{d:e} at cap {cap}");
}

#[test]
fn matching_clamped_inputs_are_bit_identical_to_the_f32_helper() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut inputs: Vec<f32> = Vec::with_capacity(200_000);
    // Half from every bit pattern (most clamp or floor), half uniformly over -150 ..= +60 dB.
    for _ in 0..100_000 {
        inputs.push(f32::from_bits((rng.next() >> 32) as u32));
    }
    for _ in 0..100_000 {
        let u = (rng.next() >> 40) as f32 / (1u32 << 24) as f32;
        inputs.push(-150.0 + 210.0 * u);
    }
    inputs.extend([0.0, -0.0, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, f32::from_bits(1), MIN_DB, 12.0, 24.0, 48.0]);
    let mut nans = 0;
    for (cap, cap32) in CAPS {
        for &x in &inputs {
            let got = db_to_gain_f64(f64::from(x), cap);
            rounded_is_unrounded_rounded(f64::from(x), cap);
            if x.is_nan() {
                nans += 1;
            } else {
                assert_eq!(got.to_bits(), db_to_gain(x.min(cap32)).to_bits(), "{x:e} ({:#x}) at cap {cap}", x.to_bits());
            }
            let every = db_to_gain(if x.is_nan() { x } else { x.min(cap32) });
            assert_eq!(got.to_bits(), every.to_bits(), "{x:e} ({:#x}) at cap {cap}", x.to_bits());
        }
    }
    println!("{} inputs per cap, {} of them NaN (covered by the every-input form)", inputs.len(), nans / 3);
}

/// The adversarial centers: for a set of `f32` gains across -143.9 ..= +47.9 dB (spread in bit pattern,
/// plus a dense run just around unity, where the argument is tiny), the `f64` dB value of the midpoint
/// between each gain and the next `f32`. The midpoint is exact in `f64`; its dB value is within a few
/// `f64` steps of the exact one (one `log2` and one product), so a run of +-32 consecutive inputs around
/// each center contains the nearest `f64` to the midpoint's exact dB value.
fn adversarial_centers() -> Vec<f64> {
    let lo = db_to_gain(-143.9).to_bits();
    let hi = db_to_gain(47.9).to_bits();
    let mut gains: Vec<u32> = (0..2048u32).map(|i| lo + ((u64::from(hi - lo) * u64::from(i)) / 2047) as u32).collect();
    gains.extend(0x3f7f_ffc0..0x3f80_0040);
    gains
        .into_iter()
        .map(|b| {
            let mid = (f64::from(f32::from_bits(b)) + f64::from(f32::from_bits(b + 1))) / 2.0;
            LOG2_TO_DB_F64 * m64::log2(mid)
        })
        .collect()
}

const RUN: i128 = 32;

#[test]
fn adversarial_midpoint_inputs_are_faithfully_rounded() {
    let bound = 1.0 / (1u64 << 48) as f64;
    let (mut total, mut correct, mut nearest_mid, mut worst_rel) = (0usize, 0usize, f64::INFINITY, 0.0f64);
    for c in adversarial_centers() {
        for n in -RUN..=RUN {
            let d = step(c, n);
            let exact = dd::db_to_gain_f64(d);
            let (lo, hi) = dd::bracket_f32(exact);
            for (cap, _) in CAPS.iter().filter(|(cap, _)| d < *cap) {
                let u = db_to_gain_f64_unrounded(d, *cap);
                let r = db_to_gain_f64(d, *cap);
                assert!(r == lo || r == hi, "{d:e} dB at cap {cap}: {r:e} is not one of {lo:e}, {hi:e}");
                let rel = ((u - exact.0) - exact.1).abs() / exact.0;
                assert!(rel <= bound, "{d:e} dB: relative error {rel:e} above 2^-48");
                worst_rel = worst_rel.max(rel);
                total += 1;
                correct += usize::from(r == dd::round_f32(exact));
                nearest_mid = nearest_mid.min(dd::midpoint_distance(u));
            }
        }
    }
    println!(
        "adversarial inputs: {total} evaluations, all faithful; {correct} correctly rounded; nearest f64 result to an f32 midpoint {nearest_mid:e} relative; worst f64 relative error {worst_rel:e}"
    );
    // The set is adversarial: some results lie far closer to an f32 midpoint than the error bound.
    assert!(nearest_mid < bound, "no input landed within the error bound of a midpoint: {nearest_mid:e}");
}

/// Both forms at `d` for `cap`, as `f64` (the rounded one widened exactly).
fn both(d: f64, cap: f64) -> (f64, f64) {
    (f64::from(db_to_gain_f64(d, cap)), db_to_gain_f64_unrounded(d, cap))
}

#[test]
fn strictly_increasing_on_the_unsaturated_grid_through_each_cap() {
    for (cap, _) in CAPS {
        let top = (cap * 100.0) as i32;
        let mut prev = (0.0, 0.0);
        for k in -14_399..=top {
            let d = f64::from(k) / 100.0;
            let g = both(d, cap);
            assert!(g.0 > prev.0, "rounded form not increasing at {d} dB, cap {cap}");
            assert!(g.1 > prev.1, "unrounded form not increasing at {d} dB, cap {cap}");
            prev = g;
        }
    }
}

#[test]
fn non_decreasing_over_the_complete_grid_with_floor_and_cap_plateaus() {
    for (cap, _) in CAPS {
        let at_cap = both(cap, cap);
        let mut prev = (0.0, 0.0);
        for k in -15_000..=6_000 {
            let d = f64::from(k) / 100.0;
            let g = both(d, cap);
            if d <= f64::from(MIN_DB) {
                assert_eq!((g.0.to_bits(), g.1.to_bits()), (0, 0), "floor plateau at {d} dB, cap {cap}");
            }
            if d >= cap {
                assert_eq!((g.0.to_bits(), g.1.to_bits()), (at_cap.0.to_bits(), at_cap.1.to_bits()), "cap plateau at {d} dB, cap {cap}");
            }
            assert!(g.0 >= prev.0 && g.1 >= prev.1, "decreases at {d} dB, cap {cap}");
            prev = g;
        }
    }
}

/// The places the record names for the consecutive-input runs: each decade, the floor and each cap
/// (+-65,536 inputs), and each adversarial center (+-32).
fn run_places() -> Vec<(f64, i128)> {
    let mut v: Vec<(f64, i128)> = DECADES.iter().map(|(d, _)| (f64::from(*d), 65_536)).collect();
    v.push((f64::from(MIN_DB), 65_536));
    v.extend(CAPS.iter().map(|(cap, _)| (*cap, 65_536)));
    v.extend(adversarial_centers().into_iter().map(|c| (c, RUN)));
    v
}

#[test]
fn consecutive_f64_runs_are_non_decreasing_at_the_adversarial_inputs_decades_floor_and_caps() {
    let places = run_places();
    for (cap, _) in CAPS {
        for &(c, half) in &places {
            let mut prev = both(step(c, -half), cap);
            for n in (1 - half)..=half {
                let d = step(c, n);
                let g = both(d, cap);
                assert!(g.0 >= prev.0, "rounded form decreases at {d:e} dB (cap {cap})");
                assert!(g.1 >= prev.1, "unrounded form decreases at {d:e} dB (cap {cap})");
                prev = g;
            }
        }
    }
}

#[test]
fn the_seams_exp2_is_non_decreasing_over_consecutive_arguments_at_the_same_places() {
    for (c, half) in run_places() {
        let a = c * DB_TO_LOG2_F64;
        let mut prev = m64::exp2(step(a, -half));
        for n in (1 - half)..=half {
            let x = step(a, n);
            let y = m64::exp2(x);
            assert!(y >= prev, "exp2 decreases at {x:e} (near {c:e} dB)");
            prev = y;
        }
    }
}
