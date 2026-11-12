//! Flat tilt (Evidence "Flat tilt", decision 2.4): the analog cascade of fifteen first-order pairs
//! lies within 0.07 dB of the straight line through 0 dB at the band frequency from 20 Hz to 20 kHz at
//! ±3 dB per octave (G = ±3·9.966 dB), for band frequencies inside the range (100 Hz to 5 kHz). The
//! cascade ripples by 0.123 dB peak to peak around its line; scaled to 0 dB at a band frequency at the
//! very edge of the range (20 Hz or 20 kHz) the whole ripple falls on one side, so those reach 0.123 dB
//! (the result note lists it; checked here at 0.13 dB). In-process comparison against the line.
//! Negative control (recorded in the result note): dropping the top pair (k = 13) breaks it.

use super::util::{db, log_grid};
use crate::prototype::{Prototype, TILT_OCTAVES};
use crate::{BandType, Slope};

#[test]
fn the_flat_tilt_cascade_follows_its_line_within_seven_hundredths_of_a_decibel() {
    let freqs = log_grid(20.0, 20_000.0, 1000);
    let (mut worst, mut edge): (f64, f64) = (0.0, 0.0);
    for slope in [-3.0, 3.0] {
        let gain = slope * TILT_OCTAVES;
        for pivot in [20.0, 100.0, 1_000.0, 5_000.0, 20_000.0] {
            let inside = (100.0..=5_000.0).contains(&pivot);
            let p = Prototype::new(BandType::FlatTilt, Slope::Db24, pivot, gain, 1.0);
            for &f in &freqs {
                let d = (db(p.power(f)) - slope * (f / pivot).log2()).abs();
                if inside {
                    worst = worst.max(d);
                } else {
                    edge = edge.max(d);
                }
            }
            // 0 dB at the band frequency.
            assert!(db(p.power(pivot)).abs() < 1e-9, "{pivot}");
        }
    }
    eprintln!("flat tilt analog cascade: worst {worst:.4} dB from the line inside, {edge:.4} dB at the edges");
    assert!(worst <= 0.07, "{worst:.4} dB");
    assert!(edge <= 0.13, "{edge:.4} dB");
}
