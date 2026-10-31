//! Captured on the unsplit renderer at bbd3721b, before #254.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "common/tempo_scene.rs"]
mod tempo_scene;
use transit_mix::{MixEngine, render_range};
use transit_model::AutoParam;
use transit_time::Range;

#[test]
fn unchanged_tempo_render_goldens() {
    MixEngine::force_serial(true);
    let mut rows = String::new();
    for (name, bpm) in [("default", 120.0), ("single", 137.0)] {
        let mut s = tempo_scene::scene();
        s.tempo.set_tempo(0, bpm).unwrap();
        let tr = &mut s.tracks[0];
        let instrument = tr.instrument.as_mut().unwrap();
        for (k, v) in [("shaper", 1.0), ("lfo2_sync", 1.0), ("lfo2_retrigger", 0.0), ("mod1_source", 2.0), ("mod1_dest", 1.0), ("mod1_amount", 25.0)]
        {
            instrument.params.insert(k.into(), v);
        }
        for (at, value) in [(0, -3.0), (371, -9.0), (4721, -2.0), (22013, -6.0)] {
            tr.lane_mut(&AutoParam::Volume).set_point(at, value);
        }
        for block in [64, 512, 4096] {
            let audio = render_range(&s, Range::new(0, 48000), block);
            assert!(audio.iter().flatten().any(|v| v.abs() > 0.01));
            // FNV-1a over channel-major IEEE-754 little-endian sample bits.
            let hash = audio
                .iter()
                .flatten()
                .flat_map(|v| v.to_bits().to_le_bytes())
                .fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
            rows.push_str(&format!("{name} {block} {hash:016x}\n"));
        }
    }
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tempo_goldens.txt");
    if std::env::var_os("TRANSIT_CAPTURE_TEMPO_GOLDENS").is_some() {
        std::fs::write(fixture, &rows).unwrap();
    }
    assert_golden_rows(&rows, &std::fs::read_to_string(fixture).unwrap());
}

/// Compares line by line so a fixture checked out with CRLF endings (git
/// `core.autocrlf` on Windows) still matches the LF rows built above.
fn assert_golden_rows(rows: &str, fixture_text: &str) {
    assert_eq!(rows.lines().collect::<Vec<_>>(), fixture_text.lines().collect::<Vec<_>>());
}

#[test]
fn the_golden_comparison_ignores_line_endings() {
    let rows = "default 64 0123456789abcdef\nsingle 512 fedcba9876543210\n";
    assert_golden_rows(rows, &rows.replace('\n', "\r\n"));
    assert_golden_rows(rows, rows);
}

#[test]
#[should_panic]
fn the_golden_comparison_still_catches_a_changed_row() {
    assert_golden_rows("default 64 0123456789abcdef\n", "default 64 0123456789abcdee\r\n");
}
