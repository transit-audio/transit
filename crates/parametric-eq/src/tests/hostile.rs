//! Hostile input (Evidence "Hostile input", decision 12): non-finite input samples are read as 0, so
//! NaN and infinity give finite output and the same state as zeros (negative control, recorded: without
//! the input sanitizing the output is non-finite); a run of 1e-30 values leaves no subnormal state; at
//! 192 kHz, 10 s of a full-scale 20 Hz tone through a +12 dB low shelf at 20 Hz and a 12 dB/oct high
//! cut at 40 Hz, then 30 s of silence, leave every f64 state exactly zero (negative control, recorded:
//! without the 1e-30 flush a subnormal state remains); silence in gives zero out after a reset at once.

use super::proc::{eq, fingerprint, run};
use super::util::{band, noise};
use crate::{BandParams, BandType, ParametricEq, Slope};

fn states(e: &ParametricEq) -> impl Iterator<Item = f64> + '_ {
    e.bands.iter().flat_map(|b| b.states.iter().flat_map(|s| s.sos.iter().flatten().copied().chain([s.fos])))
}

fn setup(e: &mut ParametricEq) {
    e.set_band(0, band(BandType::Bell, 900.0, 10.0, 3.0));
    e.set_band(1, BandParams { place: crate::Placement::Side, ..band(BandType::LowShelf, 200.0, -6.0, 1.0) });
    e.set_band(2, BandParams { slope: Slope::Db18, ..band(BandType::LowCut, 40.0, 0.0, 1.0) });
    e.reset();
}

#[test]
fn non_finite_input_reads_as_zero() {
    let input = noise(4_000, 2, 13);
    let mut bad = input.clone();
    let mut zeroed = input.clone();
    for (i, v) in [(5, f32::NAN), (100, f32::INFINITY), (101, f32::NEG_INFINITY), (2_000, f32::NAN)] {
        for c in 0..2 {
            bad[c][i] = v;
            zeroed[c][i] = 0.0;
        }
    }
    let (mut a, mut b) = (eq(48_000.0, 2), eq(48_000.0, 2));
    setup(&mut a);
    setup(&mut b);
    run(&mut a, &mut bad, 64);
    run(&mut b, &mut zeroed, 64);
    assert!(bad.iter().flatten().all(|x| x.is_finite()));
    for c in 0..2 {
        assert!(bad[c].iter().zip(&zeroed[c]).all(|(x, y)| x.to_bits() == y.to_bits()), "channel {c}");
    }
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

/// It cannot fail by construction (the states of a 1e-30 input sit near 1e-30, never subnormal); the
/// Evidence names it, and `a_long_decay_leaves_every_state_exactly_zero` carries the proof of the flush.
#[test]
fn a_run_of_tiny_values_leaves_no_subnormal_state() {
    let mut e = eq(48_000.0, 2);
    setup(&mut e);
    let mut io = vec![vec![1e-30f32; 48_000]; 2];
    run(&mut e, &mut io, 128);
    assert!(states(&e).all(|s| !s.is_subnormal()));
}

#[test]
fn a_long_decay_leaves_every_state_exactly_zero() {
    let fs = 192_000.0;
    let mut e = eq(fs, 1);
    e.set_band(0, band(BandType::LowShelf, 20.0, 12.0, 1.0));
    e.set_band(1, BandParams { slope: Slope::Db12, ..band(BandType::HighCut, 40.0, 0.0, 1.0) });
    e.reset();
    let block = 4_096;
    let mut buf = vec![vec![0.0f32; block]];
    let mut n = 0usize;
    let tone = 10 * fs as usize;
    let total = 40 * fs as usize;
    while n < total {
        for (i, x) in buf[0].iter_mut().enumerate() {
            let k = n + i;
            *x = if k < tone { (std::f64::consts::TAU * 20.0 * k as f64 / fs).sin() as f32 } else { 0.0 };
        }
        run(&mut e, &mut buf, block);
        n += block;
    }
    let nonzero: Vec<f64> = states(&e).filter(|&s| s != 0.0).collect();
    assert!(nonzero.is_empty(), "states left: {nonzero:?}");
    assert!(buf[0].iter().all(|&x| x == 0.0));
}

#[test]
fn silence_after_a_reset_is_silence_at_once() {
    let mut e = eq(44_100.0, 2);
    setup(&mut e);
    let mut warm = noise(10_000, 2, 4);
    run(&mut e, &mut warm, 64);
    e.reset();
    let mut io = vec![vec![0.0f32; 4_096]; 2];
    run(&mut e, &mut io, 64);
    assert!(io.iter().flatten().all(|&x| x == 0.0), "non-zero output after reset");
}
