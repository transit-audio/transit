//! `the_reader_meets_its_acceptance_bounds` (ADR 0056, decision 8.4 and "The reader's acceptance
//! measurement"): the main sweep of the analytic response over the record's rate grids, phases and
//! frequencies, the refinement of each kind and band's six largest values, and the full `f32` reader at
//! every refined point, against the bounds (a figure beyond a bound fails). The convergence check (the
//! sweep at density 2) is `examples/acceptance.rs`, run in release and recorded with the build.

mod sweep;

use sweep::{BANDS, Kind, bound, db, eval, f32_figure, main_grid, refine};
use transit_resample::SincTable;

/// The worst cases decision 8.4 reports (kind, rate, output frequency): q <= 1 at 0.4158 fs, just above
/// q = 1 at 0.416 fs, q = 1.5148 at 0.4157 fs; aliases at q = 1.1686, output 0.5843 fs, and q = 1.5001,
/// 0.5842 fs.
const RECORD_WORST: &[(Kind, f64, f64)] = &[
    (Kind::Pass, 0.5, 0.4158),
    (Kind::Pass, 1.000_001, 0.416),
    (Kind::Pass, 1.5148, 0.4157),
    (Kind::Alias, 1.1686, 0.5843),
    (Kind::Alias, 1.5001, 0.5842),
];

#[test]
fn the_reader_meets_its_acceptance_bounds() {
    let t = SincTable::shared();
    let top = main_grid(t, 1);
    let mut failures = Vec::new();
    for (ki, kind) in [Kind::Pass, Kind::Alias].into_iter().enumerate() {
        for (b, name) in BANDS.iter().enumerate() {
            let Some(limit) = bound(kind, b) else {
                assert!(top[ki][b].0.is_empty(), "{kind:?} {name}: an alias figure below q = 1");
                continue;
            };
            let grid = top[ki][b].worst().unwrap_or_else(|| panic!("{kind:?} {name}: no points"));
            let mut worst = f64::NEG_INFINITY;
            let mut worst_f32 = f64::NEG_INFINITY;
            let mut at = grid;
            // The main grid's six largest values, plus the worst cases the record reports for this band
            // (each at its worst of 64 phases), each refined on its own.
            let mut seeds = top[ki][b].0.clone();
            for &(k, q, v) in RECORD_WORST {
                if k == kind && sweep::band(q) == b {
                    let phi = (0..64).map(|j| f64::from(j) / 64.0).max_by(|&p, &r| eval(t, kind, q, v, p).total_cmp(&eval(t, kind, q, v, r)));
                    let phi = phi.unwrap_or(0.0);
                    seeds.push(sweep::Found { value: eval(t, kind, q, v, phi), q, v, phi });
                }
            }
            for &p in &seeds {
                let r = refine(t, kind, b, p);
                let a = db(r.value);
                let f = db(f32_figure(t, kind, r));
                if a > worst {
                    worst = a;
                    at = r;
                }
                worst_f32 = worst_f32.max(f);
            }
            println!(
                "{kind:?} {name}: grid {:.2} dB (q {:.6}, v {:.5}, phi {:.5}); refined {worst:.2} dB (q {:.6}, v {:.5}, phi {:.5}); f32 {worst_f32:.2} dB; bound {limit} dB",
                db(grid.value),
                grid.q,
                grid.v,
                grid.phi,
                at.q,
                at.v,
                at.phi,
            );
            if worst > limit || worst_f32 > limit {
                failures.push(format!("{kind:?} {name}: {worst:.2} dB (f32 {worst_f32:.2} dB) above {limit} dB"));
            }
        }
    }
    // The rates the four callers name, each on the main sweep's phase and frequency sampling (64 phases):
    // device and sampler rate pairs both ways (44.1, 48 and 96 kHz), transpositions up to +24 semitones,
    // and both combined.
    for q in [
        44_100.0 / 48_000.0,
        48_000.0 / 44_100.0,
        48_000.0 / 96_000.0,
        96_000.0 / 48_000.0,
        44_100.0 / 96_000.0,
        96_000.0 / 44_100.0,
        2f64.powf(7.0 / 12.0),
        2.0 * 2.0,
        44_100.0 / 48_000.0 * 4.0,
        96_000.0 / 44_100.0 * 4.0,
    ] {
        let b = sweep::band(q);
        for (kind, limit) in [(Kind::Pass, bound(Kind::Pass, b)), (Kind::Alias, bound(Kind::Alias, b))] {
            let Some(limit) = limit else { continue };
            let worst = sweep::rate_worst(t, kind, q, 64);
            if worst == 0.0 {
                continue; // no source tone below the source Nyquist plays at 0.58 fs or above at this rate
            }
            println!("named rate q {q:.6}: {kind:?} {:.2} dB (bound {limit} dB)", db(worst));
            if db(worst) > limit {
                failures.push(format!("named rate q {q}: {kind:?} {:.2} dB above {limit} dB", db(worst)));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
