//! Gain match (Evidence "Gain match", decision 7; slice 1 for the function and its schedule):
//! - a flat curve gives 0 dB; a composite of +6 dB at every grid point gives −6 dB within f64 rounding;
//! - a +6 dB high shelf at 5 kHz (Q 0.707) gives −3.3 to −2.7 dB and a +6 dB low shelf at 200 Hz
//!   −1.5 to −1.0 dB (in-process negative control: a `1/f` weight on the log grid fails both);
//! - sweeping a +30 dB Q 40 bell across 1/12 octave at 1 kHz moves the match by at most 0.25 dB
//!   (in-process negative control: the 1/24-octave grid moves it by more than 4 dB);
//! - a Q 40 band pass gives exactly the +12 dB clamp; the weight table equals its pinned literals;
//! - on the processor's clock the match is identical at block sizes 1, 7 and 512 with all 24 bands
//!   changed in one block, and equals the pure function bit for bit once current;
//! - with band 1's gain automated every 64-sample block, a change of band 24 still reaches the match
//!   within 2·24·256 samples (negative control, recorded: lowest-numbered-first with restart never
//!   updates band 24);
//! - two runs with different program material give identical applied gains (it cannot pump;
//!   in-process negative control: a level follower differs).
//!
//! The examples depend on the crate's `math` (`exp2`, `log2`) and are compared with tolerances; the
//! weight table is literals and is pinned bit for bit.

use super::util::{band, noise};
use super::proc::{eq, run};
use crate::gain_match::{grid, match_of_powers};
use crate::prototype::Prototype;
use crate::weights::{POINTS, WEIGHTS};
use crate::{BANDS, BandParams, BandType, Placement, Slope, gain_match_db};

fn single(b: BandParams) -> f64 {
    gain_match_db(&[b], 48_000.0, 2)
}

/// The match of one band with a different weighting `w(i)` on a subset of the grid (step `stride`).
fn weighted(b: &BandParams, w: impl Fn(usize, f64) -> f64, stride: usize) -> f64 {
    let p = Prototype::new(b.kind, b.slope, b.freq_hz, b.gain_db, b.q);
    let f = grid();
    let (mut num, mut den) = (0.0, 0.0);
    for i in (0..POINTS).step_by(stride) {
        let wi = w(i, f[i]);
        num += wi * p.power(f[i]);
        den += wi;
    }
    -10.0 * (num / den).log10()
}

#[test]
fn flat_and_constant_curves_give_exact_matches() {
    assert_eq!(gain_match_db(&[], 48_000.0, 2), 0.0);
    assert_eq!(single(band(BandType::Bell, 1_000.0, 0.0, 1.0)), 0.0);
    let six = vec![10f64.powf(0.6); POINTS];
    let m = match_of_powers(&six);
    assert!((m + 6.0).abs() < 1e-12, "{m}");
}

#[test]
fn shelf_examples_fall_in_their_ranges_and_a_one_over_f_weight_fails_them() {
    let hs = band(BandType::HighShelf, 5_000.0, 6.0, 0.707);
    let ls = band(BandType::LowShelf, 200.0, 6.0, 0.707);
    let (mh, ml) = (single(hs), single(ls));
    let one_over_f = |b: &BandParams| weighted(b, |i, f| WEIGHTS[i] / f, 1);
    let (oh, ol) = (one_over_f(&hs), one_over_f(&ls));
    eprintln!("gain match: high shelf {mh:.3} dB, low shelf {ml:.3} dB; with 1/f: {oh:.2}, {ol:.2} dB");
    assert!((-3.3..=-2.7).contains(&mh), "{mh}");
    assert!((-1.5..=-1.0).contains(&ml), "{ml}");
    assert!(!(-3.3..=-2.7).contains(&oh) && !(-1.5..=-1.0).contains(&ol));
}

#[test]
fn a_narrow_bell_swept_across_a_twelfth_of_an_octave_moves_the_match_little() {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut lo24, mut hi24) = (f64::INFINITY, f64::NEG_INFINITY);
    for k in 0..=200 {
        let b = band(BandType::Bell, 1_000.0 * 2f64.powf(k as f64 / (12.0 * 200.0)), 30.0, 40.0);
        let m = single(b);
        lo = lo.min(m);
        hi = hi.max(m);
        // The 1/24-octave grid is every eighth point of the 1/192-octave grid.
        let m24 = weighted(&b, |i, _| WEIGHTS[i], 8);
        lo24 = lo24.min(m24);
        hi24 = hi24.max(m24);
    }
    eprintln!("Q 40 bell sweep: spread {:.3} dB at 1/192 octave, {:.2} dB at 1/24", hi - lo, hi24 - lo24);
    assert!(hi - lo <= 0.25, "{}", hi - lo);
    assert!(hi24 - lo24 > 4.0, "{}", hi24 - lo24);
}

#[test]
fn a_narrow_band_pass_hits_the_clamp_exactly() {
    assert_eq!(single(band(BandType::BandPass, 1_000.0, 0.0, 40.0)), 12.0);
    let cut = BandParams { slope: Slope::Db96, ..band(BandType::HighCut, 100.0, 0.0, 1.0) };
    assert_eq!(single(cut), 12.0);
}

#[test]
fn the_weight_table_equals_its_pinned_literals() {
    let (mut x, mut s) = (0u64, 0u64);
    for w in WEIGHTS {
        let b = w.to_bits();
        x ^= b;
        s = s.wrapping_mul(31).wrapping_add(b);
    }
    assert_eq!((x, s), (0x005f_df10_dde7_2be3, 0xab07_8ac7_2f53_dbd5));
    assert_eq!(WEIGHTS[0].to_bits(), 0.047_039_556_749_839_7f64.to_bits());
    assert_eq!(WEIGHTS[POINTS - 1].to_bits(), 2.536_947_055_620_974f64.to_bits());
    // And they are |K|² of the standard's 48 kHz biquads (an in-process recomputation).
    let k = |b: [f64; 3], a: [f64; 3], f: f64| {
        let w = std::f64::consts::TAU * f / 48_000.0;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let (nr, ni) = (b[0] + b[1] * c1 + b[2] * c2, b[1] * s1 + b[2] * s2);
        let (dr, di) = (a[0] + a[1] * c1 + a[2] * c2, a[1] * s1 + a[2] * s2);
        (nr * nr + ni * ni) / (dr * dr + di * di)
    };
    for (i, &f) in grid().iter().enumerate() {
        let v = k([1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85], [1.0, -1.690_659_293_182_41, 0.732_480_774_215_85], f)
            * k([1.0, -2.0, 1.0], [1.0, -1.990_047_454_833_98, 0.990_072_250_366_21], f);
        assert!((v / WEIGHTS[i] - 1.0).abs() < 1e-9, "point {i}");
    }
}

fn all_bands() -> [BandParams; BANDS] {
    core::array::from_fn(|i| {
        let kind = BandType::ALL[i % 9];
        BandParams {
            place: Placement::ALL[i % 5],
            slope: Slope::ALL[i % 8],
            ..band(kind, 40.0 * 1.3f64.powi(i as i32), (i as f64 - 11.5) * 2.0, 0.3 + 0.4 * i as f64)
        }
    })
}

/// The processor with all 24 bands changed in one block, run at `block`; the computed match after
/// 7,000 samples (all 24 refreshes take 6,144) and the applied gain's state.
fn scheduled(block: usize) -> (u64, u64) {
    let mut e = eq(48_000.0, 2);
    e.set_gain_match(true);
    for (i, b) in all_bands().into_iter().enumerate() {
        e.set_band(i, b);
    }
    let mut io = noise(7_000, 2, 17);
    run(&mut e, &mut io, block);
    (e.gain_match_db().to_bits(), e.match_gain.cur.to_bits())
}

#[test]
fn the_scheduled_match_is_block_size_independent_and_equals_the_function() {
    let reference = scheduled(512);
    for block in [1, 7] {
        assert_eq!(scheduled(block), reference, "block {block}");
    }
    let expected = gain_match_db(&all_bands(), 48_000.0, 2);
    assert_eq!(reference.0, expected.to_bits(), "scheduled {} vs function {expected}", f64::from_bits(reference.0));
}

#[test]
fn round_robin_reaches_band_twenty_four_while_band_one_is_automated() {
    let mut e = eq(48_000.0, 2);
    e.set_band(0, band(BandType::Bell, 500.0, 3.0, 1.0));
    e.set_band(23, band(BandType::HighShelf, 8_000.0, 2.0, 0.707));
    e.reset();
    let mut io = noise(64, 2, 1);
    // Band 1 automated every block from here on; band 24 changes once at frame 0.
    e.set_gain(23, 9.0);
    let limit = 2 * 24 * 256;
    let mut frames = 0;
    let mut k = 0;
    while !e.gm.current_for(23) {
        assert!(frames < limit, "band 24 not current after {frames} samples");
        k += 1;
        e.set_gain(0, if k % 2 == 0 { 3.0 } else { 4.5 } + (k % 7) as f64 * 0.1);
        run(&mut e, &mut io, 64);
        frames += 64;
    }
    eprintln!("band 24 current after {frames} samples (bound {limit})");
}

#[test]
fn the_applied_gain_does_not_depend_on_the_program() {
    let trace = |seed: u64, level: f32| {
        let mut e = eq(48_000.0, 2);
        e.set_gain_match(true);
        e.set_band(0, band(BandType::Bell, 2_000.0, 9.0, 1.0));
        let mut io = noise(9_600, 2, seed);
        for c in io.iter_mut() {
            for x in c.iter_mut() {
                *x *= level;
            }
        }
        let mut gains = Vec::new();
        let mut follower = Vec::new();
        let mut env = 0.0f64;
        for chunk in 0..150 {
            let mut views: Vec<&mut [f32]> = io.iter_mut().map(|c| &mut c[chunk * 64..(chunk + 1) * 64]).collect();
            for &x in views[0].iter() {
                env = env.max(f64::from(x).abs()) * 0.999;
            }
            e.process(&mut views, 64);
            gains.push(e.match_gain.cur.to_bits());
            follower.push(env.to_bits());
        }
        (gains, follower)
    };
    let (a, fa) = trace(1, 1.0);
    let (b, fb) = trace(99, 0.1);
    assert_eq!(a, b);
    // The negative control: a level follower of the program differs between the two runs.
    assert_ne!(fa, fb);
}
