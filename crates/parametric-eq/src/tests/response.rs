//! The response functions (decision 8) and the IEEE-exact pins of decision 19 ("Pinned results"):
//! the Butterworth Q constants as literals and the SVF map from given σ coefficients.

use super::util::{band, db, log_grid};
use crate::design::{Used, design};
use crate::section::{Sos, Svf};
use crate::{BandParams, BandType, CurveChannel, PhaseMode, Placement, band_response_db, butterworth, response_db};

#[test]
fn band_response_is_the_designed_magnitude() {
    let fs = 44_100.0;
    let freqs = log_grid(20.0, 21_000.0, 300);
    let b = band(BandType::HighShelf, 12_000.0, 9.0, 2.0);
    let mut out = vec![0.0; freqs.len()];
    band_response_db(&b, fs, PhaseMode::ZeroLatency, &freqs, &mut out);
    let d = design(b.kind, b.slope, Used::from_stored(b.freq_hz, b.gain_db, b.q, fs), fs);
    for (&f, &o) in freqs.iter().zip(&out) {
        let h = std::f64::consts::PI * f / fs;
        assert!((o - db(d.power(h.sin(), h.cos()))).abs() < 1e-9, "{f}");
    }
    // Nothing above 0.49 of the rate.
    let mut top = [0.0];
    band_response_db(&b, fs, PhaseMode::ZeroLatency, &[0.5 * fs], &mut top);
    assert!(top[0].is_nan());
}

#[test]
fn the_composite_is_the_sum_of_the_bands_on_the_channel_path() {
    let fs = 48_000.0;
    let freqs = log_grid(20.0, 20_000.0, 200);
    let bands = [
        band(BandType::Bell, 300.0, 6.0, 1.0),
        BandParams { place: Placement::Left, ..band(BandType::LowCut, 60.0, 0.0, 1.0) },
        BandParams { place: Placement::Side, ..band(BandType::HighShelf, 5_000.0, -4.0, 0.707) },
        BandParams { on: false, ..band(BandType::Bell, 2_000.0, 12.0, 1.0) },
        BandParams { used: false, ..band(BandType::Bell, 4_000.0, 12.0, 1.0) },
    ];
    let one = |b: &BandParams| {
        let mut o = vec![0.0; freqs.len()];
        band_response_db(b, fs, PhaseMode::ZeroLatency, &freqs, &mut o);
        o
    };
    let (b0, b1, b2) = (one(&bands[0]), one(&bands[1]), one(&bands[2]));
    for (channel, parts) in
        [(CurveChannel::Left, vec![&b0, &b1]), (CurveChannel::Right, vec![&b0]), (CurveChannel::Mid, vec![&b0]), (CurveChannel::Side, vec![&b0, &b2])]
    {
        let mut out = vec![0.0; freqs.len()];
        response_db(&bands, fs, PhaseMode::ZeroLatency, channel, &freqs, &mut out);
        for i in 0..freqs.len() {
            let sum: f64 = parts.iter().map(|p| p[i]).sum();
            assert!((out[i] - sum).abs() < 1e-9, "{channel:?} at {}", freqs[i]);
        }
    }
    let mut flat = vec![1.0; freqs.len()];
    response_db(&[], fs, PhaseMode::ZeroLatency, CurveChannel::Left, &freqs, &mut flat);
    assert!(flat.iter().all(|&v| v == 0.0));
}

#[test]
fn the_butterworth_literals_are_the_formula() {
    let orders = [1usize, 2, 3, 4, 6, 8, 12, 16];
    for (qs, n) in butterworth::QS.iter().zip(orders) {
        assert_eq!(qs.len(), n / 2);
        for (i, &q) in qs.iter().enumerate() {
            let k = (i + 1) as f64;
            let exact = 1.0 / (2.0 * (std::f64::consts::PI * (2.0 * k - 1.0) / (2.0 * n as f64)).cos());
            assert!((q / exact - 1.0).abs() < 4.0 * f64::EPSILON, "N {n} i {i}");
        }
        assert!(qs.windows(2).all(|w| w[0] < w[1]), "ascending");
    }
    assert_eq!(butterworth::QS[1][0].to_bits(), 0.707_106_781_186_547_5f64.to_bits());
    assert_eq!(butterworth::QS[7][7].to_bits(), 5.101_148_618_689_155f64.to_bits());
}

/// The SVF map is arithmetic and `sqrt` only, so exactly representable inputs give exact outputs.
#[test]
fn the_svf_map_is_exact_on_exact_inputs() {
    let s = Svf::map(&Sos { n: [3.0, 2.0, 8.0], c: [1.0, 2.0, 4.0] });
    assert_eq!(s.g.to_bits(), 0.5f64.to_bits());
    assert_eq!(s.k.to_bits(), 1.0f64.to_bits());
    assert_eq!((s.m0, s.m1, s.m2), (3.0, 1.0, 2.0));
    assert_eq!(s.a1.to_bits(), (1.0f64 / 1.75).to_bits());
    assert_eq!(s.a2.to_bits(), (0.5f64 / 1.75).to_bits());
}
