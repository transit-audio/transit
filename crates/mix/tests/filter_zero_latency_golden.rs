//! #247 baseline, captured before changing insert note timing. Every sample is retained.
#![allow(clippy::unwrap_used)]

use transit_midi::{Note, Sequence};
use transit_mix::{MixEngine, render_range};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use transit_time::Range;

#[test]
fn zero_latency_filter_amp_envelope_golden() {
    MixEngine::force_serial(true);
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    let instrument = transit_dsp::create("macro_oscillator").unwrap();
    assert_eq!(instrument.latency(), 0);
    let mut filter = Insert::new("filter");
    for (k, v) in [("amp_on", 1.0), ("amp_attack", 0.5), ("amp_release", 1.0), ("cutoff", 900.0), ("env_amount", 45.0)] {
        filter.params.insert(k.into(), v);
    }
    tr.mixer.inserts[0] = Some(filter);
    let mut seq = Sequence::default();
    for (pitch, start, length) in [(48, 1, 13), (60, 23, 17), (67, 42, 80)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "golden", 0, 4096, seq));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/filter_zero_latency.f32");
    let a = render_range(&s, Range::new(0, 4096), 64);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.001));
    let bytes: Vec<u8> = a.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    if std::env::var_os("CAPTURE_247_GOLDEN").is_some() {
        std::fs::write(&path, &bytes).unwrap();
    }
    let golden = std::fs::read(path).unwrap();
    for block in [64, 512] {
        let rendered = render_range(&s, Range::new(0, 4096), block);
        let got: Vec<u8> = rendered.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            assert_eq!(got, golden, "every sample matches the pre-change Windows golden, block {block}");
        } else {
            // As plan_parity: different platform libm implementations round DSP differently.
            assert_eq!(got.len(), golden.len());
            for (g, w) in got.as_chunks::<4>().0.iter().zip(golden.as_chunks::<4>().0) {
                let (g, w) = (f32::from_le_bytes(*g), f32::from_le_bytes(*w));
                assert!((g - w).abs() <= 3e-4 + 2e-3 * w.abs());
            }
        }
    }
}
