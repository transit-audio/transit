//! The matching claim (decision 2.5, Evidence "The matching claim"): the deviation of the designed
//! magnitude from the analog prototype, on decision 2.5's 1000-point grid from 20 Hz to
//! min(20 kHz, 0.45·fs) where the prototype is above −40 dB, stays within each row of decision 2.2's
//! table plus 0.05 dB for the finer grid; the same measure on RBJ bilinear sections exceeds the
//! matched bound (the comparison and the negative control: bilinear fails at 44.1 kHz).
//!
//! These are in-process comparisons against an independent closed form (the prototype), with the
//! record's tolerance; the designs call the crate's `math` (platform `exp2`, `sin`, `cos`), whose
//! last-place differences across platforms are far below the 0.05 dB margin.

use super::sweep::{bilinear, cases, deviation, matched, worst};
use super::util::{EvalGrid, QS, band_freqs, gains};
use crate::design::cut_section;
use crate::math::PI;
use crate::{BandType, Slope, butterworth, prototype};

/// The table's three columns.
const COLUMNS: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
/// The finer grid's margin (decision 2.5).
const MARGIN: f64 = 0.05;

#[derive(Clone, Copy)]
enum Top {
    Hz(f64),
    FifthOfRate,
}

struct Row {
    name: &'static str,
    kind: BandType,
    slope: Slope,
    gains: Vec<f64>,
    qs: Vec<f64>,
    top: Top,
    floor: f64,
    matched: [f64; 3],
    /// Where this implementation does not reproduce the record's figure plus the margin, the bound
    /// pinned instead: the measured value plus 0.05 dB (decision 2.5's slice 1 rule), each listed in
    /// the result note as a figure of the record not reproduced.
    pin: [Option<f64>; 3],
    bilinear: Option<[f64; 3]>,
}

const RECORD: [Option<f64>; 3] = [None; 3];

fn freqs(top: Top, fs: f64) -> Vec<f64> {
    let limit = match top {
        Top::Hz(f) => f,
        Top::FifthOfRate => fs / 5.0,
    };
    band_freqs().into_iter().filter(|&f| f <= limit + 1e-9).collect()
}

fn restricted_gains() -> Vec<f64> {
    gains().into_iter().filter(|g| g.abs() <= 12.0).collect()
}

fn qs_between(lo: f64, hi: f64) -> Vec<f64> {
    QS.iter().copied().filter(|&q| q >= lo && q <= hi).collect()
}

fn check(rows: &[Row]) {
    let mut failures = Vec::new();
    for row in rows {
        for (col, &fs) in COLUMNS.iter().enumerate() {
            let grid = EvalGrid::new(fs);
            let cs = cases(row.kind, &[row.slope], &freqs(row.top, fs), &row.gains, &row.qs);
            let (m, at) = worst(&cs, |c| matched(c, fs, &grid, row.floor));
            let bound = row.pin[col].unwrap_or(row.matched[col] + MARGIN);
            let b = row.bilinear.map(|_| worst(&cs, |c| bilinear(c, fs, &grid, row.floor)).0);
            eprintln!(
                "{:<52} {:>6.1} kHz matched {:>7.3} dB (record {:>6.2}, bound {:>6.3}){}",
                row.name,
                fs / 1000.0,
                m,
                row.matched[col],
                bound,
                match (b, row.bilinear) {
                    (Some(b), Some(r)) => format!("  bilinear {:>7.3} dB (record {:>6.2})", b, r[col]),
                    _ => String::new(),
                }
            );
            if m > bound {
                failures.push(format!("{} at {} Hz: matched {m:.3} dB > {bound:.2} dB at {at:?}", row.name, fs));
            }
            if let Some(b) = b
                && b <= row.matched[col]
            {
                failures.push(format!("{} at {} Hz: bilinear {b:.3} dB does not exceed the matched bound", row.name, fs));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn bells_and_band_passes_match_their_prototypes_within_the_table() {
    let all_f = Top::Hz(20_000.0);
    let low_f = Top::Hz(16_000.0);
    check(&[
        Row {
            name: "Bell, ±3..±12 dB, Q 0.5..4, f <= 16 kHz",
            kind: BandType::Bell,
            slope: Slope::Db24,
            gains: restricted_gains(),
            qs: qs_between(0.5, 4.0),
            top: low_f,
            floor: -40.0,
            matched: [1.36, 1.09, 0.36],
            pin: RECORD,
            bilinear: Some([7.79, 6.22, 1.41]),
        },
        Row {
            name: "Bell, full ranges",
            kind: BandType::Bell,
            slope: Slope::Db24,
            gains: gains(),
            qs: QS.to_vec(),
            top: all_f,
            floor: -40.0,
            matched: [4.30, 3.32, 0.54],
            pin: [Some(4.405), Some(3.424), None],
            bilinear: Some([16.40, 12.66, 2.36]),
        },
        Row {
            name: "Band pass, Q 0.5..4, f <= 16 kHz",
            kind: BandType::BandPass,
            slope: Slope::Db24,
            gains: vec![0.0],
            qs: qs_between(0.5, 4.0),
            top: low_f,
            floor: -40.0,
            matched: [2.49, 1.96, 0.61],
            pin: RECORD,
            bilinear: Some([14.23, 10.44, 1.80]),
        },
        Row {
            name: "Band pass, full ranges",
            kind: BandType::BandPass,
            slope: Slope::Db24,
            gains: vec![0.0],
            qs: QS.to_vec(),
            top: all_f,
            floor: -40.0,
            matched: [4.95, 3.87, 0.61],
            pin: RECORD,
            bilinear: None,
        },
        Row {
            name: "Notch, Q 0.5..40, above -20 dB",
            kind: BandType::Notch,
            slope: Slope::Db24,
            gains: vec![0.0],
            qs: qs_between(0.5, 40.0),
            top: all_f,
            floor: -20.0,
            matched: [6.44, 4.04, 0.18],
            pin: [Some(6.640), None, None],
            bilinear: Some([15.28, 12.48, 2.53]),
        },
    ]);
}

#[test]
fn cut_composites_match_their_prototypes_within_the_table() {
    let mut rows = Vec::new();
    let slopes = [Slope::Db12, Slope::Db24, Slope::Db48, Slope::Db96];
    let high_all = [[2.40, 1.21, 0.11], [4.53, 2.32, 0.20], [8.91, 4.62, 0.25], [17.8, 9.24, 0.29]];
    let high_fifth = [[1.09, 1.08, 0.11], [2.12, 2.10, 0.20], [0.91, 0.60, 0.25], [0.36, 0.27, 0.29]];
    // Not reproduced (the result note): the 96 kHz column of the 48 and 96 dB/oct rows, and the
    // f <= fs/5 rows of 48 and 96 dB/oct at 44.1 and 48 kHz.
    let high_all_pin = [RECORD, RECORD, [None, None, Some(0.344)], [None, None, Some(0.431)]];
    let high_fifth_pin = [RECORD, RECORD, [Some(1.577), Some(1.156), Some(0.344)], [Some(0.714), Some(0.520), Some(0.431)]];
    let low_all = [[1.93, 1.49, 0.12], [1.61, 0.89, 0.00], [0.83, 0.23, 0.00], [0.19, 0.01, 0.00]];
    let names_h = ["High cut 12 dB/oct", "High cut 24 dB/oct", "High cut 48 dB/oct", "High cut 96 dB/oct"];
    let names_hf =
        ["High cut 12 dB/oct, f <= fs/5", "High cut 24 dB/oct, f <= fs/5", "High cut 48 dB/oct, f <= fs/5", "High cut 96 dB/oct, f <= fs/5"];
    let names_l = ["Low cut 12 dB/oct", "Low cut 24 dB/oct", "Low cut 48 dB/oct", "Low cut 96 dB/oct"];
    for (i, &slope) in slopes.iter().enumerate() {
        let row = |name, kind, top, matched, pin| Row {
            name,
            kind,
            slope,
            gains: vec![0.0],
            qs: vec![1.0],
            top,
            floor: -40.0,
            matched,
            pin,
            bilinear: None,
        };
        rows.push(row(names_h[i], BandType::HighCut, Top::Hz(20_000.0), high_all[i], high_all_pin[i]));
        rows.push(row(names_hf[i], BandType::HighCut, Top::FifthOfRate, high_fifth[i], high_fifth_pin[i]));
        rows.push(row(names_l[i], BandType::LowCut, Top::Hz(20_000.0), low_all[i], RECORD));
    }
    check(&rows);
}

#[test]
fn flat_tilt_matches_its_prototype_within_the_table() {
    check(&[
        Row {
            name: "Flat tilt, ±10 dB",
            kind: BandType::FlatTilt,
            slope: Slope::Db24,
            gains: vec![-10.0, 10.0],
            qs: vec![1.0],
            top: Top::Hz(20_000.0),
            floor: -40.0,
            matched: [0.23, 0.23, 0.16],
            pin: RECORD,
            bilinear: None,
        },
        Row {
            name: "Flat tilt, ±30 dB",
            kind: BandType::FlatTilt,
            slope: Slope::Db24,
            gains: vec![-30.0, 30.0],
            qs: vec![1.0],
            top: Top::Hz(20_000.0),
            floor: -40.0,
            matched: [0.73, 0.72, 0.53],
            pin: [Some(0.831), Some(0.827), Some(0.648)],
            bilinear: None,
        },
    ]);
}

/// The table's single-section cut rows: every second-order Butterworth section of the eight slopes
/// (Q 0.50 to 5.10), frequencies to 20 kHz, against that section's prototype.
#[test]
fn cut_sections_match_their_prototypes_within_the_table() {
    let mut qs: Vec<f64> = butterworth::QS.iter().flat_map(|s| s.iter().copied()).collect();
    qs.sort_by(f64::total_cmp);
    qs.dedup();
    let rows = [
        ("High cut sections", false, [2.52, 1.28, 0.11], [25.95, 18.17, 2.76]),
        ("Low cut sections", true, [9.15, 6.85, 1.07], [27.11, 18.17, 2.76]),
    ];
    let mut failures = Vec::new();
    for (name, high, bound, bil) in rows {
        for (col, &fs) in COLUMNS.iter().enumerate() {
            let grid = EvalGrid::new(fs);
            let (mut m, mut b) = (0.0f64, 0.0f64);
            for f0 in band_freqs() {
                let f0 = f0.min(0.49 * fs);
                let w0 = std::f64::consts::TAU * f0 / fs;
                for &q in &qs {
                    let (s, _) = cut_section(w0, q, high);
                    let r = super::rbj::cut_section(w0, q, high);
                    let proto = |f: f64| if high { prototype::high_pass2(f / f0, q) } else { prototype::low_pass2(f / f0, q) };
                    m = m.max(deviation(&grid, -40.0, |i| s.power(grid.sc[i].0, grid.sc[i].1), proto));
                    b = b.max(deviation(&grid, -40.0, |i| r.power(2.0 * PI * grid.f[i] / fs), proto));
                }
            }
            eprintln!(
                "{name:<52} {:>6.1} kHz matched {m:>7.3} dB (record {:>6.2})  bilinear {b:>7.3} dB (record {:>6.2})",
                fs / 1000.0,
                bound[col],
                bil[col]
            );
            if m > bound[col] + MARGIN {
                failures.push(format!("{name} at {fs}: matched {m:.3} > {:.2}", bound[col] + MARGIN));
            }
            if b <= bound[col] {
                failures.push(format!("{name} at {fs}: bilinear {b:.3} does not exceed the bound"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
