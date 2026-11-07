//! Freeze and encoded mix export must share RangeRender's changing-latency placement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../mix/tests/support/offline_latency.rs"]
mod fixture;

use fixture::{IMPULSES, LEN, positions, session};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use transit_audio_io::{BitDepth, EncodeOptions, FileFormat};
use transit_engine::{Engine, io::Bounce};
use transit_time::Range;

#[test]
fn freeze_stores_the_corrected_pre_fader_render() {
    let (s, track) = session("compressor", &[0.0, 5.0, 0.0], &[2085, 6175]);
    let mut e = Engine::new(s);
    let range = Range::new(0, LEN as i64 + 96_000);
    let expected = transit_mix::render_track_pre_fader(e.session(), track, range);
    assert_eq!(positions(&expected), IMPULSES);
    e.execute("track.freeze", &json!({"tracks": [track.0]})).unwrap();
    let frozen = e.session().track(track).unwrap().freeze.as_ref().unwrap();
    let stored = &e.session().pool.get(frozen.source).unwrap().buffer.channels;
    assert_eq!(stored, &expected);
    assert_eq!(positions(stored), IMPULSES);
    assert_eq!(frozen.length, range.len());
}

#[test]
fn float_wav_export_contains_the_corrected_mix_sample_for_sample() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("transit-251-export-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Mix.wav");
    let (s, _) = session("compressor", &[0.0, 5.0, 0.0], &[2085, 6175]);
    let range = Range::new(0, LEN as i64);
    let expected = transit_mix::render_range(&s, range, 1024);
    let options = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None };
    Bounce::new(Arc::new(s), &path, range, &options, None, false, false).unwrap().run().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let (_, decoded) = transit_audio_io::decode(&bytes, None).unwrap();
    assert_eq!(decoded.channels, expected);
    assert_eq!(positions(&decoded.channels), IMPULSES.into_iter().cycle().take(6).collect::<Vec<_>>());
    std::fs::remove_dir_all(dir).unwrap();
}
