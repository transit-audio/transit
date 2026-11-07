//! Opus from the bounce commands (#238): `file.bounce_mix`, `file.bounce_stems`,
//! `file.bounce_paths` and `file.export_clips` take `format: "opus"` and `bitrate` (kb/s), name
//! their files `.opus`, and write files that decode to the bounce's length at 48 kHz.

use serde_json::json;
use std::path::PathBuf;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_engine::Engine;
use transit_model::{ChannelFormat, Clip, Session, TrackKind};
use transit_time::SampleRate;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-opus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A session "Night Drive" at `rate` with two stereo audio tracks, each a two-second clip of a
/// sine (with a little hiss, so the encoder has something to spend bits on).
fn session(rate: u32) -> (Engine, Vec<u64>) {
    let mut s = Session::new("Night Drive", SampleRate::new(rate).unwrap());
    let mut clips = Vec::new();
    for (k, name) in ["Keys", "Bass"].into_iter().enumerate() {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
        let hz = 220.0 * (k + 1) as f64;
        let mut seed = 1u64 + k as u64;
        let ch: Vec<f32> = (0..2 * rate as usize)
            .map(|i| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let hiss = ((seed >> 40) as f64 / f64::from(1u32 << 24) - 0.5) * 0.02;
                ((2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() * 0.3 + hiss) as f32
            })
            .collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![ch.clone(), ch] };
        let src = transit_engine::io::add_source(&mut s, name, buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        transit_engine::edit::place_clip(&mut s, t, Clip::audio(id, name, src, 0, 0, 2 * i64::from(rate)));
        clips.push(id.0);
    }
    (Engine::new(s), clips)
}

fn at_48k(frames: u64, rate: u32) -> u64 {
    (frames * 48_000 + u64::from(rate) / 2) / u64::from(rate)
}

fn decoded_frames(path: &std::path::Path) -> (u16, u64) {
    let bytes = std::fs::read(path).unwrap();
    let (info, buf) = transit_audio_io::decode(&bytes, None).unwrap();
    assert_eq!(info.format, FileFormat::Opus, "{}", path.display());
    (info.channels, buf.frames() as u64)
}

#[test]
fn bounce_mix_writes_opus_at_the_bitrate_asked_for() {
    for rate in [44_100u32, 48_000] {
        let (mut e, _) = session(rate);
        let dir = scratch(&format!("mix-{rate}"));
        let len = u64::from(rate) * 3 / 2;
        let mut sizes = Vec::new();
        for kbps in [96, 256] {
            let path = dir.join(format!("mix-{kbps}.opus"));
            let r = e
                .execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "format": "opus", "bitrate": kbps, "start": 0, "end": len}))
                .unwrap();
            assert_eq!(r["bytes"].as_u64().unwrap(), std::fs::metadata(&path).unwrap().len());
            assert_eq!(decoded_frames(&path), (2, at_48k(len, rate)), "{rate} Hz {kbps} kb/s");
            let (_, comments) = transit_audio_io::opus_tags(&std::fs::read(&path).unwrap()).unwrap();
            assert!(comments.contains(&"TITLE=Night Drive".to_string()), "{comments:?}");
            sizes.push(std::fs::metadata(&path).unwrap().len() as f64);
        }
        // The bitrate reaches the encoder: 256 kb/s is about 2.7 times 96 kb/s.
        let ratio = sizes[1] / sizes[0];
        assert!((ratio / (256.0 / 96.0) - 1.0).abs() < 0.15, "size ratio {ratio:.2}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_extension_alone_picks_opus_and_the_default_bitrate() {
    let (mut e, _) = session(48_000);
    let dir = scratch("ext");
    let (a, b) = (dir.join("a.opus"), dir.join("b.opus"));
    e.execute("file.bounce_mix", &json!({"path": a.to_string_lossy(), "start": 0, "end": 48_000})).unwrap();
    e.execute("file.bounce_mix", &json!({"path": b.to_string_lossy(), "format": "opus", "bitrate": 160, "start": 0, "end": 48_000})).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "no bitrate is 160 kb/s");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_bitrate_is_refused_and_writes_nothing() {
    let (mut e, _) = session(48_000);
    let dir = scratch("bad-rate");
    let path = dir.join("mix.opus");
    for bad in [json!(0), json!(5), json!(511), json!("fast"), json!(-96)] {
        let err = e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "format": "opus", "bitrate": bad})).unwrap_err().to_string();
        assert!(err.contains("bitrate"), "{bad}: {err}");
    }
    assert!(!path.exists());
    // MP3 stays unwritable.
    let err = e.execute("file.bounce_mix", &json!({"path": dir.join("mix.mp3").to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("opus") && err.contains("mp3"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stems_paths_and_clips_write_opus_files() {
    let (mut e, clips) = session(44_100);
    let dir = scratch("stems");
    let stems = dir.join("stems");
    let r = e
        .execute("file.bounce_stems", &json!({"dir": stems.to_string_lossy(), "format": "opus", "bitrate": 128, "start": 0, "end": 44_100}))
        .unwrap();
    let files: Vec<String> = r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
    assert_eq!(files.len(), 2);
    for f in &files {
        assert!(f.ends_with(".opus"), "{f}");
        assert_eq!(decoded_frames(std::path::Path::new(f)), (2, 48_000));
    }
    let paths = dir.join("paths");
    let r = e.execute("file.bounce_paths", &json!({"dir": paths.to_string_lossy(), "format": "opus", "start": 0, "end": 44_100})).unwrap();
    let files = r["files"].as_array().unwrap();
    assert!(!files.is_empty());
    for f in files {
        let file = f["file"].as_str().unwrap();
        assert!(file.ends_with(".opus"), "{file}");
        assert_eq!(decoded_frames(std::path::Path::new(file)).1, 48_000);
    }
    let out = dir.join("clips");
    let r = e.execute("file.export_clips", &json!({"dir": out.to_string_lossy(), "clips": clips, "format": "opus", "bitrate": 192})).unwrap();
    assert_eq!(r["written"], 2);
    let mut names: Vec<String> = std::fs::read_dir(&out).unwrap().map(|d| d.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["Bass.opus", "Keys.opus"]);
    let (_, comments) = transit_audio_io::opus_tags(&std::fs::read(out.join("Keys.opus")).unwrap()).unwrap();
    assert!(comments.contains(&"TITLE=Keys".to_string()), "a clip's title is its name: {comments:?}");
    assert_eq!(decoded_frames(&out.join("Keys.opus")).1, 96_000);
    // Negative control: the default is still WAV.
    let wav = dir.join("clips-wav");
    e.execute("file.export_clips", &json!({"dir": wav.to_string_lossy(), "clips": [clips[0]]})).unwrap();
    assert!(wav.join("Keys.wav").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
