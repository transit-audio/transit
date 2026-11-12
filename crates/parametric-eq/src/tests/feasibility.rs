//! Feasibility (Evidence "Feasibility", decision 2.2): the drafting sweep on a finer grid. Every type
//! over 100 log-spaced band frequencies from 20 Hz to 20 kHz, gains every 1 dB from −30 to +30 dB, 25
//! log-spaced Qs from 0.1 to 40 (the listed ten included), every slope, at 44.1, 48, 96 and 192 kHz and
//! the outer classes (32 and 384 kHz). No three-point fit is infeasible (none clamped); every section
//! is stable (all σ denominator coefficients positive and finite). Clamped points would be recorded
//! in the result note.

use super::util::{OUTER_RATES, QS, RATES, log_grid};
use crate::design::{Used, design};
use crate::{BandType, Slope};

fn qs() -> Vec<f64> {
    let mut v = log_grid(0.1, 40.0, 25);
    v.extend_from_slice(&QS);
    v.sort_by(f64::total_cmp);
    v.dedup();
    v
}

#[test]
fn no_fit_is_infeasible_and_every_section_is_stable_on_the_fine_grid() {
    let freqs = log_grid(20.0, 20_000.0, 100);
    let gains: Vec<f64> = (-30..=30).filter(|&g| g != 0).map(f64::from).collect();
    let qs = qs();
    let mut clamped = Vec::new();
    let mut designs = 0u64;
    for fs in RATES.iter().chain(OUTER_RATES.iter()).copied() {
        for kind in BandType::ALL {
            let slopes: &[Slope] = if kind.uses_slope() { &Slope::ALL } else { &[Slope::Db24] };
            let g: &[f64] = if kind.uses_gain() { &gains } else { &[0.0] };
            let q: &[f64] = if kind.uses_q() { &qs } else { &[1.0] };
            for &slope in slopes {
                for &f in &freqs {
                    for &gain in g {
                        for &qq in q {
                            let d = design(kind, slope, Used::from_stored(f, gain, qq, fs), fs);
                            designs += 1;
                            if d.clamped > 0 {
                                clamped.push((fs, kind, slope, f, gain, qq));
                            }
                            for s in d.sections() {
                                assert!(s.c.iter().all(|&c| c > 0.0 && c.is_finite()), "{kind:?} {f} {gain} {qq} at {fs}: {s:?}");
                                assert!(s.n.iter().all(|&n| n >= 0.0 && n.is_finite()), "{kind:?} {f} {gain} {qq} at {fs}: {s:?}");
                            }
                            if let Some(fo) = &d.fos {
                                assert!(fo.d.iter().all(|&c| c > 0.0) && fo.n.iter().all(|&n| n >= 0.0), "{kind:?} {f} at {fs}");
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("feasibility: {designs} designs, {} clamped", clamped.len());
    assert!(clamped.is_empty(), "clamped fits: {:#?}", &clamped[..clamped.len().min(20)]);
}
