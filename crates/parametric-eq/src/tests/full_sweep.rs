//! Decision 2.5's slice 1 sweep: every type at 30 log-spaced band frequencies from 20 Hz to 20 kHz,
//! gains from −30 to +30 dB in 3 dB steps and the ten Qs, at 44.1, 48, 96 and 192 kHz and one rate of
//! each outer class (32 and 384 kHz). The deviation from the prototype on the 1000-point grid where
//! the prototype is above −40 dB is recorded per type and slope and pinned as measured plus 0.05 dB
//! (the measured values are in the result note). In-process comparison with the record's tolerance.

use super::sweep::{cases, matched, worst};
use super::util::{EvalGrid, OUTER_RATES, QS, RATES, band_freqs, gains};
use crate::{BandType, Slope};

/// The rates in column order: 44.1, 48, 96, 192, 32 and 384 kHz.
fn rates() -> Vec<f64> {
    RATES.iter().chain(OUTER_RATES.iter()).copied().collect()
}

/// (type, slope, pinned bound per rate column = measured + 0.05 dB).
type Pin = (BandType, Slope, [f64; 6]);

/// Measured on this implementation (the result note's table) plus 0.05 dB, rounded up.
fn pins() -> Vec<Pin> {
    use BandType::*;
    use Slope::*;
    vec![
        (Bell, Db24, [4.405, 3.424, 0.593, 0.170, 5.567, 0.080]),
        (LowShelf, Db24, [3.883, 2.809, 2.385, 0.117, 3.872, 0.054]),
        (HighShelf, Db24, [3.883, 2.809, 2.385, 0.117, 3.872, 0.054]),
        (TiltShelf, Db24, [3.883, 2.809, 2.385, 0.117, 3.872, 0.054]),
        (Notch, Db24, [9.339, 5.900, 0.684, 0.159, 13.390, 0.064]),
        (BandPass, Db24, [4.997, 3.922, 0.659, 0.180, 6.237, 0.082]),
        (FlatTilt, Db24, [0.831, 0.827, 0.648, 0.281, 0.841, 0.118]),
        (LowCut, Db6, [1.525, 1.369, 0.485, 0.169, 1.674, 0.081]),
        (HighCut, Db6, [1.017, 1.017, 0.503, 0.171, 1.017, 0.081]),
        (LowCut, Db12, [1.985, 1.538, 0.166, 0.058, 2.467, 0.051]),
        (HighCut, Db12, [2.446, 1.263, 0.160, 0.058, 7.204, 0.051]),
        (LowCut, Db18, [4.009, 3.510, 1.011, 0.294, 4.495, 0.112]),
        (HighCut, Db18, [2.060, 1.966, 0.606, 0.178, 6.929, 0.082]),
        (LowCut, Db24, [1.660, 0.946, 0.055, 0.051, 2.665, 0.051]),
        (HighCut, Db24, [4.576, 2.376, 0.244, 0.063, 13.492, 0.052]),
        (LowCut, Db36, [1.218, 0.511, 0.051, 0.051, 2.548, 0.051]),
        (HighCut, Db36, [6.749, 3.517, 0.315, 0.068, 19.644, 0.052]),
        (LowCut, Db48, [0.877, 0.280, 0.051, 0.051, 2.407, 0.051]),
        (HighCut, Db48, [8.959, 4.670, 0.344, 0.069, 25.879, 0.052]),
        (LowCut, Db72, [0.449, 0.105, 0.051, 0.051, 2.132, 0.051]),
        (HighCut, Db72, [13.405, 6.978, 0.336, 0.068, 38.556, 0.052]),
        (LowCut, Db96, [0.238, 0.064, 0.051, 0.051, 1.879, 0.051]),
        (HighCut, Db96, [17.856, 9.287, 0.431, 0.074, 51.343, 0.052]),
    ]
}

fn sweep(kind: BandType, slope: Slope) -> Vec<super::sweep::Case> {
    let g = if kind.uses_gain() { gains() } else { vec![0.0] };
    let q = if kind.uses_q() { QS.to_vec() } else { vec![1.0] };
    cases(kind, &[slope], &band_freqs(), &g, &q)
}

fn check(filter: impl Fn(BandType) -> bool) {
    let mut failures = Vec::new();
    for (kind, slope, pin) in pins().into_iter().filter(|p| filter(p.0)) {
        let cs = sweep(kind, slope);
        let mut line = format!("{:<10} {:>3} dB/oct", format!("{kind:?}"), slope.db_per_octave());
        for (col, fs) in rates().into_iter().enumerate() {
            let grid = EvalGrid::new(fs);
            let (m, at) = worst(&cs, |c| matched(c, fs, &grid, -40.0));
            line += &format!("  {m:>7.3}");
            if m > pin[col] {
                failures.push(format!("{kind:?} {slope:?} at {fs} Hz: {m:.3} dB > {:.3} at {at:?}", pin[col]));
            }
        }
        eprintln!("{line}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn bells_notches_and_band_passes_stay_within_their_pinned_bounds() {
    check(|k| matches!(k, BandType::Bell | BandType::Notch | BandType::BandPass));
}

#[test]
fn shelves_and_tilts_stay_within_their_pinned_bounds() {
    check(|k| matches!(k, BandType::LowShelf | BandType::HighShelf | BandType::TiltShelf | BandType::FlatTilt));
}

#[test]
fn cuts_stay_within_their_pinned_bounds() {
    check(|k| matches!(k, BandType::LowCut | BandType::HighCut));
}
