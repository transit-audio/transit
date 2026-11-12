//! The gain module's tests (ADR 0055, 2.9). Every pinned value is an exact bit pattern, which the
//! `f64` path over the pinned library makes portable. References: `data/gain_grid.txt` and
//! `data/pan_grid.txt`, computed once with arbitrary precision (their headers say how), and the decade
//! bits below from the same computation.

mod common;

use common::dd;
use common::{Fnv, hex_words, ord32, ulp32};
use transit_math::consts::DB_TO_LOG2;
use transit_math::f32 as m32;
use transit_math::gain::{MIN_DB, db_to_gain, gain_to_db, pan_gains};

const GAIN_GRID: &str = include_str!("data/gain_grid.txt");
const PAN_GRID: &str = include_str!("data/pan_grid.txt");

/// The 0.01 dB grid: entry `k + 14400` is the correctly rounded gain at the `f32` nearest `k / 100`.
fn gain_grid() -> Vec<u32> {
    let blocks = hex_words(GAIN_GRID);
    assert_eq!(blocks.len(), 1);
    blocks.into_iter().next().unwrap()
}

/// A grid value the way the engine holds it: the `f32` nearest `k / 100` (one IEEE division, the same
/// value a typed "12.62" parses to).
fn grid_db(k: i32) -> f32 {
    k as f32 / 100.0
}

fn hash(words: &[u32]) -> u64 {
    let mut h = Fnv::new();
    for w in words {
        h.bytes(&w.to_le_bytes());
    }
    h.0
}

/// The correctly rounded `f32` of 10^(d/20) for d = -140, -120, ..., +200 dB (bits).
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
    (60, 0x447a_0000),
    (80, 0x461c_4000),
    (100, 0x47c3_5000),
    (120, 0x4974_2400),
    (140, 0x4b18_9680),
    (160, 0x4cbe_bc20),
    (180, 0x4e6e_6b28),
    (200, 0x5015_02f9),
];

#[test]
fn zero_db_is_exactly_unity() {
    assert_eq!(db_to_gain(0.0), 1.0);
    assert_eq!(db_to_gain(-0.0), 1.0);
    assert_eq!(gain_to_db(1.0), 0.0);
    assert_eq!(gain_to_db(-1.0), 0.0);
}

#[test]
fn the_floor_negative_infinity_and_nan_are_exactly_zero_gain() {
    for db in [MIN_DB, -144.000_01, -150.0, -1.0e30, f32::MIN, f32::NEG_INFINITY, f32::NAN, -f32::NAN] {
        assert_eq!(db_to_gain(db).to_bits(), 0, "db_to_gain({db}) is +0.0");
    }
    // Just above the floor is a real gain, so the floor is the only special case below.
    assert!(db_to_gain(-143.99) > 0.0);
    // The other way: silence, NaN and tiny gains report the floor; infinity reports f32::MAX.
    for g in [0.0, -0.0, f32::from_bits(1), 1.0e-9, f32::NAN] {
        assert_eq!(gain_to_db(g), MIN_DB, "gain_to_db({g})");
    }
    assert_eq!(gain_to_db(f32::INFINITY), f32::MAX);
    assert_eq!(gain_to_db(f32::NEG_INFINITY), f32::MAX);
}

#[test]
fn the_top_of_the_range_clamps_at_plus_200_db() {
    for db in [200.0, 200.5, 1.0e9, f32::MAX, f32::INFINITY] {
        assert_eq!(db_to_gain(db), 1.0e10, "db_to_gain({db})");
    }
}

#[test]
fn every_decade_is_the_correctly_rounded_power_of_ten() {
    assert_eq!(db_to_gain(20.0), 10.0);
    assert_eq!(db_to_gain(40.0), 100.0);
    for &(d, bits) in DECADES {
        let g = db_to_gain(d as f32);
        assert_eq!(g.to_bits(), bits, "db_to_gain({d}) = {g:e}");
        let back = gain_to_db(g);
        assert!((f64::from(back) - f64::from(d)).abs() <= ulp32(f64::from(d)).max(ulp32(1.0e-30)), "gain_to_db(db_to_gain({d})) = {back}");
    }
    // Where the gain is exactly a power of ten, both directions are exact.
    let mut p = 1.0f32;
    for n in 0..=10 {
        assert_eq!(db_to_gain(20.0 * n as f32), p, "db_to_gain({})", 20 * n);
        assert_eq!(gain_to_db(p), 20.0 * n as f32, "gain_to_db(1e{n})");
        assert_eq!(gain_to_db(-p), 20.0 * n as f32, "gain_to_db(-1e{n})");
        p *= 10.0;
    }
}

/// FNV-1a 64 of the grid tables' values, pinned so a reference cannot change unnoticed.
const GAIN_GRID_HASH: u64 = 0x4a46_0ec6_37a0_e18c;
const PAN_GRID_HASH: u64 = 0x1648_bbb3_a5ab_5229;

#[test]
fn the_0_01_and_0_1_db_grids_match_the_arbitrary_precision_table() {
    let grid = gain_grid();
    assert_eq!(grid.len(), 19_201);
    assert_eq!(hash(&grid), GAIN_GRID_HASH, "the reference table changed");
    let mut misses = Vec::new();
    for k in -14_400..=4_800 {
        let x = grid_db(k);
        let want = if k == -14_400 { 0 } else { grid[(k + 14_400) as usize] };
        if db_to_gain(x).to_bits() != want {
            misses.push(k);
        }
    }
    assert!(misses.is_empty(), "0.01 dB grid values off the correctly rounded gain (k in hundredths of a dB): {misses:?}");
    // The 0.1 dB grid is every tenth entry: its values are the same f32s.
    for k in -1_440..=480 {
        assert_eq!((k as f32 / 10.0).to_bits(), grid_db(10 * k).to_bits(), "k = {k}");
    }
}

#[test]
fn the_double_double_reference_agrees_with_the_arbitrary_precision_table() {
    // The near-tie measurement decides each candidate with this reference; it must be right everywhere
    // the committed table speaks.
    let grid = gain_grid();
    for k in -14_400..=4_800 {
        let got = dd::round_f32(dd::db_to_gain(grid_db(k)));
        assert_eq!(got.to_bits(), grid[(k + 14_400) as usize], "k = {k}");
    }
    for &(d, bits) in DECADES {
        assert_eq!(dd::round_f32(dd::db_to_gain(d as f32)).to_bits(), bits, "{d} dB");
    }
}

#[test]
fn the_round_trip_stays_within_a_millionth_of_a_db_plus_one_ulp() {
    let mut worst = (0.0f64, 0);
    for k in -14_400..=4_800 {
        let x = grid_db(k);
        let back = gain_to_db(db_to_gain(x));
        let err = (f64::from(back) - f64::from(x)).abs();
        let bound = 1.0e-6 + ulp32(f64::from(x));
        assert!(err <= bound, "{x} dB comes back as {back} (error {err:e}, bound {bound:e})");
        if err > worst.0 {
            worst = (err, k);
        }
    }
    println!("round trip: worst {:e} dB at {} dB", worst.0, grid_db(worst.1));
}

#[test]
fn db_to_gain_is_strictly_increasing_on_the_0_01_db_grid_and_never_decreases_near_a_decade() {
    let mut prev = -1.0f32;
    for k in -14_400..=4_800 {
        let g = db_to_gain(grid_db(k));
        assert!(g > prev, "not increasing at {} dB", grid_db(k));
        prev = g;
    }
    for &(d, _) in DECADES.iter().filter(|(d, _)| *d <= 40) {
        let centre = ord32(d as f32);
        let mut prev = -1.0f32;
        for o in (centre - 32_768)..=(centre + 32_768) {
            let x = if o < 0 { f32::from_bits((-o) as u32 | 0x8000_0000) } else { f32::from_bits(o as u32) };
            let g = db_to_gain(x);
            assert!(g >= prev, "decreases at {x:e} dB near {d} dB");
            prev = g;
        }
    }
}

const LAWS: [f32; 3] = [-3.0, -4.5, -6.0];

#[test]
fn pan_gains_match_the_arbitrary_precision_table_at_the_panels_positions() {
    let blocks = hex_words(PAN_GRID);
    assert_eq!(blocks.len(), 3);
    let all: Vec<u32> = blocks.concat();
    assert_eq!(hash(&all), PAN_GRID_HASH, "the reference table changed");
    let mut misses = Vec::new();
    for (law, block) in LAWS.iter().zip(&blocks) {
        assert_eq!(block.len(), 2 * 199);
        for k in -99..=99 {
            let i = 2 * (k + 99) as usize;
            let (l, r) = pan_gains(k as f32 / 100.0, *law);
            if (l.to_bits(), r.to_bits()) != (block[i], block[i + 1]) {
                misses.push((*law, k));
            }
        }
    }
    assert!(misses.is_empty(), "pan gains off the correctly rounded value (law, pan in hundredths): {misses:?}");
}

#[test]
fn pan_ends_center_and_clamps_are_exact() {
    for law in [-3.0, -4.5, -6.0, 0.0] {
        for p in [-1.0, -1.5, -1.0e30, f32::NEG_INFINITY] {
            assert_eq!(pan_gains(p, law), (1.0, 0.0), "hard left, law {law}, pan {p}");
        }
        for p in [1.0, 1.5, 1.0e30, f32::INFINITY] {
            assert_eq!(pan_gains(p, law), (0.0, 1.0), "hard right, law {law}, pan {p}");
        }
        let c = pan_gains(0.0, law);
        assert_eq!(pan_gains(f32::NAN, law), c, "NaN pans center, law {law}");
        assert_eq!(pan_gains(-0.0, law), c);
        assert_eq!(c.0, c.1, "center is symmetric, law {law}");
    }
    // The balance law: the far side fades out linearly, the near side stays at unity.
    assert_eq!(pan_gains(0.0, 0.0), (1.0, 1.0));
    assert_eq!(pan_gains(-0.5, 0.0), (1.0, 0.5));
    assert_eq!(pan_gains(0.25, 0.0), (0.75, 1.0));
}

#[test]
fn pan_gains_are_mirror_images_and_monotonic_across_the_panel() {
    for law in LAWS {
        let mut prev = (f32::INFINITY, -1.0f32);
        for k in -100..=100 {
            let p = k as f32 / 100.0;
            let (l, r) = pan_gains(p, law);
            let (ml, mr) = pan_gains(-p, law);
            assert_eq!((l, r), (mr, ml), "mirror at {p}, law {law}");
            assert!(l <= prev.0 && r >= prev.1, "monotonic at {p}, law {law}");
            prev = (l, r);
        }
    }
}

/// The form the first draft proposed: an `f32` product and an `f32` exp2.
fn db_to_gain_f32_only(db: f32) -> f32 {
    m32::exp2(db * DB_TO_LOG2)
}

#[test]
fn negative_control_an_f32_only_conversion_fails_the_decade_test() {
    let plus20 = db_to_gain_f32_only(20.0);
    assert_ne!(plus20, 10.0, "the f32-only form must miss +20 dB");
    assert_eq!(plus20, 9.999_999, "+20 dB gives 9.999999 in f32 alone");
    let wrong: Vec<i32> = DECADES.iter().filter(|(d, _)| *d <= 40).filter(|(d, bits)| db_to_gain_f32_only(*d as f32).to_bits() != *bits).map(|(d, _)| *d).collect();
    println!("f32-only form: {} of 10 decades from -140 to +40 dB wrong: {wrong:?}", wrong.len());
    assert!(!wrong.is_empty());
}

/// Today's `transit-dsp` form, run through the same checks and recorded, never asserted: its result
/// depends on each platform's `powf`.
#[test]
fn todays_platform_powf_form_is_recorded() {
    let today = |db: f32| if db.is_nan() || db <= MIN_DB { 0.0 } else { 10f32.powf(db.min(200.0) / 20.0) };
    let wrong: Vec<i32> = DECADES.iter().filter(|(d, bits)| today(*d as f32).to_bits() != *bits).map(|(d, _)| *d).collect();
    let grid = gain_grid();
    let misses = (-14_399..=4_800).filter(|&k| today(grid_db(k)).to_bits() != grid[(k + 14_400) as usize]).count();
    println!("platform 10f32.powf(db / 20): decades off the correctly rounded value {wrong:?}; 0.01 dB grid values off: {misses} of 19200");
}
