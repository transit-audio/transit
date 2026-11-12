//! Bit-exact pass-through (Evidence "Bit-exact pass-through", decision 2.5, Zero latency): with no
//! used band, or with every used band on at a type and setting whose target is identity (a bell,
//! shelf, tilt shelf or flat tilt at exactly 0 dB, after smoothing has reached it), Output at 0 dB and
//! gain match off, the output equals the input bit for bit, mono and stereo, at every placement.
//! Negative control (recorded): computing identity bands instead of skipping them (designing a 0 dB
//! band by the fit) breaks bit-exactness. IEEE-exact; no platform math reaches the output.

use super::util::{band, noise};
use super::proc::{eq, run};
use crate::{BandType, Placement};

fn hostile_mix(io: &mut [Vec<f32>]) {
    // Quiet and extreme but finite samples, and signed zeros.
    for c in io.iter_mut() {
        c[10] = 1e-40;
        c[11] = -0.0;
        c[12] = f32::MAX;
        c[13] = -f32::MIN_POSITIVE;
        c[14] = 1.0;
    }
}

fn assert_exact(input: &[Vec<f32>], output: &[Vec<f32>]) {
    for (c, (i, o)) in input.iter().zip(output).enumerate() {
        for (n, (a, b)) in i.iter().zip(o).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "channel {c} frame {n}: {a:e} became {b:e}");
        }
    }
}

#[test]
fn no_used_band_passes_every_sample_bit_for_bit() {
    for channels in [1, 2, 3] {
        let mut e = eq(96_000.0, channels);
        let mut input = noise(5_000, channels, 5);
        hostile_mix(&mut input);
        let mut io = input.clone();
        run(&mut e, &mut io, 77);
        assert_exact(&input, &io);
    }
}

#[test]
fn identity_bands_pass_every_sample_bit_for_bit() {
    let kinds = [BandType::Bell, BandType::LowShelf, BandType::HighShelf, BandType::TiltShelf, BandType::FlatTilt];
    for channels in [1, 2] {
        let mut e = eq(48_000.0, channels);
        // 24 of the 25 type and placement combinations (one per slot).
        let combos = kinds.iter().flat_map(|&k| Placement::ALL.iter().map(move |&p| (k, p)));
        for (slot, (kind, place)) in combos.take(crate::BANDS).enumerate() {
            let mut b = band(kind, 50.0 + 700.0 * slot as f64, 0.0, 0.3 + slot as f64);
            b.place = place;
            e.set_band(slot, b);
        }
        e.reset();
        let mut input = noise(6_000, channels, 9);
        hostile_mix(&mut input);
        let mut io = input.clone();
        run(&mut e, &mut io, 64);
        assert_exact(&input, &io);
    }
}

/// A band whose gain is smoothed down to 0 dB becomes the identity once the smoother reaches it, and
/// from the next control tick the output is the input bit for bit again.
#[test]
fn a_band_smoothed_to_zero_decibels_passes_bit_for_bit_once_settled() {
    let mut e = eq(48_000.0, 2);
    // A shelf: its two factor sections are not numerically exact at 0 dB, so only skipping it
    // (not computing it) gives the input back bit for bit.
    e.set_band(0, band(BandType::HighShelf, 1_000.0, 6.0, 1.0));
    e.reset();
    e.set_gain(0, 0.0);
    let mut warm = noise(48_000, 2, 1);
    run(&mut e, &mut warm, 64);
    let mut input = noise(4_800, 2, 2);
    hostile_mix(&mut input);
    let mut io = input.clone();
    run(&mut e, &mut io, 64);
    assert_exact(&input, &io);
}
