//! The reader's acceptance measurement with its convergence check (ADR 0056, "The reader's acceptance
//! measurement"): the main sweep (density 1) and the halved sweep (density 2), each band's worst value
//! from both and their difference, and the refined figures. A recorded measurement, not a CI gate (the
//! test `the_reader_meets_its_acceptance_bounds` runs the main sweep against the bounds). Run in
//! release: `cargo run --release -p transit-resample --example acceptance`.

#[path = "../tests/sweep/mod.rs"]
mod sweep;

use sweep::{BANDS, Kind, bound, db, f32_figure, main_grid, refine};
use transit_resample::SincTable;

fn main() {
    let t = SincTable::shared();
    let main = main_grid(t, 1);
    let halved = main_grid(t, 2);
    for (ki, kind) in [Kind::Pass, Kind::Alias].into_iter().enumerate() {
        for (b, name) in BANDS.iter().enumerate() {
            let Some(limit) = bound(kind, b) else { continue };
            let (Some(g1), Some(g2)) = (main[ki][b].worst(), halved[ki][b].worst()) else { continue };
            let mut refined = g1;
            for &p in &main[ki][b].0 {
                let r = refine(t, kind, b, p);
                if r.value > refined.value {
                    refined = r;
                }
            }
            let f = f32_figure(t, kind, refined);
            println!(
                "{kind:?} {name}: main {:.3} dB, halved {:.3} dB (change {:+.3} dB); refined {:.3} dB at q {:.6} v {:.5} phi {:.5} (+{:.3} dB over the main grid); f32 {:.3} dB; bound {limit} dB",
                db(g1.value),
                db(g2.value),
                db(g2.value) - db(g1.value),
                db(refined.value),
                refined.q,
                refined.v,
                refined.phi,
                db(refined.value) - db(g1.value),
                db(f),
            );
        }
    }
}
