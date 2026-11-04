//! The shaped path: the voice's one amp envelope applies once through the shaper's external
//! amp-gain seam, and a held voice crossing the shaper's switch crossfades over its next 12-frame
//! grid block. References are built here from the shared shaper directly.

use transit_instrument_core::shaper::{FilterMods, Settings, Shaper, filter_mode};

use super::{SR, bind, core, render, unity};
use crate::MOD_BLOCK;
use crate::event::Event;
use crate::patch::{PlayMode, Zone};
use crate::reader::SliceSource;

const ONES: [f32; 256] = [1.0; 256];

fn filter() -> Settings {
    Settings { on: true, mode: filter_mode(0), cutoff_log2: 1000f32.log2(), resonance: 0.3, drive: 1.0, sr: SR, ..Settings::default() }
}

fn assert_bits(out: &[f32], want: &[f32]) {
    for (n, (a, b)) in out.iter().zip(want).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "frame {n}: {a} != {b}");
    }
}

#[test]
fn a_shaped_voice_applies_its_amp_envelope_once() {
    let mut c = core();
    bind(&mut c, 0, 0, 256, unity(PlayMode::Gate, 60, 8), 0);
    let zone = Zone { shaper: filter(), ..*c.zone(0).unwrap() };
    c.set_zone(0, Some(zone)).unwrap();
    let out = render(&mut c, &[SliceSource::mono(&ONES, SR)], &[Event::on(0, 60, 127), Event::off(16, 60)], 40);
    // The reference: the filter alone (the seam's `filter_frame`), times the release envelope once.
    let s = filter();
    let mut sh = Shaper::new(1);
    sh.note(&s, 1.0, [0.0; 2]);
    let want: Vec<f32> = (0..40)
        .map(|n| {
            if n % MOD_BLOCK == 0 {
                sh.design(&s, 60.0, FilterMods::default());
            }
            let env = match n {
                0..16 => 1.0,
                16..24 => 1.0 * (1.0 - (n - 15) as f32 / 8.0),
                _ => 0.0,
            };
            let v = sh.filter_frame(&s, true, 1.0);
            // The core sums into a zeroed buffer: 0 + x (a -0 product reads +0).
            0.0 + if n < 24 { v * env * 1.0 } else { 0.0 }
        })
        .collect();
    assert_bits(&out, &want);
}

#[test]
fn a_held_voice_crosses_the_shaper_switch_over_its_next_grid_block() {
    let mut c = core();
    bind(&mut c, 0, 0, 256, unity(PlayMode::Gate, 60, 0), 0);
    let sources = [SliceSource::mono(&ONES, SR)];
    let mut out = render(&mut c, &sources, &[Event::on(0, 60, 127)], 30);
    let zone = Zone { shaper: filter(), ..*c.zone(0).unwrap() };
    c.set_zone(0, Some(zone)).unwrap();
    out.extend(render(&mut c, &sources, &[], 42));
    // The voice's grid points are frames 0, 12, 24, 36, ...: the switch made at frame 30 crossfades
    // frames 36..48 from the dry path into the filtered one (the filter starting from rest), then
    // stays filtered.
    let s = filter();
    let mut sh = Shaper::new(1);
    sh.note(&s, 1.0, [0.0; 2]);
    let want: Vec<f32> = (0..72)
        .map(|n| {
            if n == 36 {
                sh.rest_filter();
            }
            if n >= 36 && n % MOD_BLOCK == 0 {
                sh.design(&s, 60.0, FilterMods::default());
            }
            let v = sh.filter_frame(&s, n >= 36, 1.0);
            let (dry, wet) = (1.0f32 * 1.0 * 1.0, v * 1.0 * 1.0);
            0.0 + match n {
                0..36 => dry,
                36..48 => dry + (wet - dry) * ((n - 36 + 1) as f32 / MOD_BLOCK as f32),
                _ => wet,
            }
        })
        .collect();
    assert_bits(&out, &want);
}
