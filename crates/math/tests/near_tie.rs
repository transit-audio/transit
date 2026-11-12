//! The exhaustive near-tie measurement (ADR 0055, 2.9): every `f32` input above -144 and up to +48 dB
//! through `db_to_gain`, its `f64` result screened against the nearest `f32` rounding midpoint, and each
//! candidate the bound admits decided with a double-double reference. Recorded, not gated: the count and
//! the inputs go into the slice's result. A miss on a 0.01 dB grid value fails (that grid is also checked
//! directly by the gain tests). The same sweep measures the pre-authorized `f32` fast pair's error
//! (ADR 0055, 2.5 and 6.1: `exp2f` and `log2f` of an `f32` product), which this slice does not build.
//!
//! About 2.2e9 inputs: run it in the release profile,
//! `cargo test --release -p transit-math --test near_tie -- --ignored --nocapture`.

mod common;

use common::dd;
use common::ulp32;
use transit_math::consts::{DB_TO_LOG2, DB_TO_LOG2_F64, LOG2_TO_DB, LOG2_TO_DB_F64};
use transit_math::f32 as m32;
use transit_math::f64 as m64;
use transit_math::gain::{db_to_gain, gain_to_db};

/// The screen: four times the record's bound on the `f64` path's relative error up to the +200 dB clamp
/// (2^-47.9), so no input whose rounding could go wrong escapes it.
const SCREEN: f64 = 1.0 / (1u64 << 46) as f64;

/// Is `x` a value of the 0.01 dB grid (the `f32` nearest some k/100)?
fn on_grid(x: f32) -> bool {
    let k = (f64::from(x) * 100.0).round();
    (k as f32 / 100.0).to_bits() == x.to_bits()
}

#[test]
#[ignore = "exhaustive over about 2.2e9 inputs; run in release and record the result"]
fn exhaustive_near_tie_count_over_every_f32_input_from_minus_144_to_plus_48_db() {
    let top = 48.0f32.to_bits();
    let floor = (-144.0f32).to_bits();
    // +0 ..= +48 dB, then -0 down to just above -144 dB.
    let inputs = (0..=top).chain(0x8000_0000..floor);
    let (mut count, mut candidates, mut checked) = (0u64, Vec::new(), 0u64);
    let mut misses = Vec::new();
    let mut fast_worst = (0.0f64, 0.0f32);
    for b in inputs {
        let x = f32::from_bits(b);
        count += 1;
        let y = m64::exp2(f64::from(x) * DB_TO_LOG2_F64);
        let fast = m32::exp2(x * DB_TO_LOG2);
        let e = (f64::from(fast) - y).abs() / ulp32(y);
        if e > fast_worst.0 {
            fast_worst = (e, x);
        }
        // Every 65,536th input is decided in full too, a check on the screen itself.
        let sampled = b & 0xffff == 0;
        if dd::midpoint_distance(y) < SCREEN || sampled {
            let want = dd::round_f32(dd::db_to_gain(x));
            let got = db_to_gain(x);
            assert_eq!(got, y as f32, "db_to_gain is the f64 path rounded once");
            if sampled {
                checked += 1;
            } else {
                candidates.push(x);
            }
            if got.to_bits() != want.to_bits() {
                misses.push((x, got, want));
                assert!(!on_grid(x), "a 0.01 dB grid value misrounds: {x} dB gives {got:e}, correctly rounded {want:e}");
                assert!(!sampled, "a sampled input outside the screen misrounds: {x}");
            }
        }
    }
    println!("inputs: {count}; screen 2^-46 relative; candidates: {}; sampled and decided: {checked}", candidates.len());
    println!("misses (input dB, result, correctly rounded): {}", misses.len());
    for (x, got, want) in &misses {
        println!("  {x:e} (0x{:08x}) -> {got:e} (0x{:08x}), correctly rounded {want:e} (0x{:08x})", x.to_bits(), got.to_bits(), want.to_bits());
    }
    println!("candidates: {candidates:?}");
    println!("fast db_to_gain_f32 (exp2f of the f32 product), -144 to +48 dB: worst {:.4} ulp at {:e} dB", fast_worst.0, fast_worst.1);

    // The fast gain_to_db_f32 over every gain whose level lies from -144 to +48 dB, against the f64 path.
    let (lo, hi) = (db_to_gain(-143.99).to_bits(), db_to_gain(48.0).to_bits());
    let mut worst = (0.0f64, 0.0f32);
    for b in lo..=hi {
        let g = f32::from_bits(b);
        let want = LOG2_TO_DB_F64 * m64::log2(f64::from(g));
        if want == 0.0 {
            assert_eq!(gain_to_db(g), 0.0);
            continue;
        }
        let fast = LOG2_TO_DB * m32::log2(g);
        let e = (f64::from(fast) - want).abs() / ulp32(want);
        if e > worst.0 {
            worst = (e, g);
        }
    }
    println!("fast gain_to_db_f32 (LOG2_TO_DB * log2f), gains from -144 to +48 dB: worst {:.4} ulp at gain {:e}", worst.0, worst.1);
}
