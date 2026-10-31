//! Captured before #251: every output bit, including the trimmed head and partial last block.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use transit_time::Range;

fn check(name: &str, latent: bool) {
    let mut s = Session::default();
    let samples: Vec<f32> = (0..4096).map(|i| ((i * 17 % 127) as f32 - 63.0) / 512.0).collect();
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![samples.clone(), samples] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let c = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(c, "golden", SourceId(900), 0, 0, 4096));
    if latent {
        let mut insert = Insert::new("compressor");
        insert.params.insert("lookahead".into(), 5.0);
        insert.params.insert("ratio".into(), 1.0);
        tr.mixer.inserts[0] = Some(insert);
    }
    let mut bytes = Vec::new();
    for block in [16, 100, 512] {
        let audio = transit_mix::render_range(&s, Range::new(37, 3078), block);
        assert_eq!(audio.len(), 2);
        assert!(audio.iter().all(|c| c.len() == 3041));
        bytes.extend(audio.iter().flatten().flat_map(|x| x.to_le_bytes()));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/offline").join(format!("{name}.f32"));
    if std::env::var_os("UPDATE_OFFLINE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    } else {
        let expected = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), expected.len());
        assert_eq!(bytes.iter().zip(expected).position(|(a, b)| *a != b), None, "{name}: first differing byte");
    }
}

#[test]
fn constant_latency_bounce_golden() {
    check("constant_latency", true);
}

#[test]
fn zero_latency_bounce_golden() {
    check("zero_latency", false);
}
