//! A stereo file imported onto a mono track with a mono track below it splits across the two: the
//! left channel on the first, the right on the second, each its own mono source, in one undo step.

use serde_json::json;
use transit_engine::Engine;
use transit_engine::model::TrackId;

/// A 16-bit stereo PCM WAV: every left sample is `l`, every right one `r`.
fn stereo_wav(frames: u32, l: i16, r: i16) -> Vec<u8> {
    let data = frames * 4;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 0x0002_0001, 48_000, 48_000 * 4, 0x0010_0004] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for _ in 0..frames {
        b.extend_from_slice(&l.to_le_bytes());
        b.extend_from_slice(&r.to_le_bytes());
    }
    b
}

fn track(e: &mut Engine, format: &str) -> u64 {
    e.execute("track.new", &json!({"kind": "audio", "format": format})).unwrap()["tracks"][0].as_u64().unwrap()
}

/// Each clip on `t`: its start, length, name, its source's channel count and first sample.
fn clips(e: &Engine, t: u64) -> Vec<(i64, i64, String, usize, f32)> {
    let s = e.session();
    let tr = s.track(TrackId(t)).unwrap();
    tr.clips()
        .iter()
        .map(|c| {
            let transit_engine::model::ClipContent::Audio { source, .. } = c.content else { panic!("an audio clip") };
            let buf = &s.pool.get(source).unwrap().buffer;
            (c.start, c.length, c.name.clone(), buf.num_channels(), buf.channels[0][0])
        })
        .collect()
}

#[test]
fn a_stereo_file_on_a_mono_track_with_a_mono_track_below_splits_and_saves_two_mono_files() {
    let d = transit_test_support::TestDir::new("import-split");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(4_800, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let (a, b) = (track(&mut e, "mono"), track(&mut e, "mono"));
    let undo = e.undo_history().len();
    let r = e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": a, "at": 960})).unwrap();
    assert_eq!(r["imported"][0]["split"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(clips(&e, a), vec![(960, 4_800, "LAV-2.L".to_string(), 1, 0.5)], "the left channel on the first track");
    assert_eq!(clips(&e, b), vec![(960, 4_800, "LAV-2.R".to_string(), 1, -0.25)], "the right channel on the track below");
    assert_eq!(e.undo_history().len(), undo + 1, "one undo step");

    let file = d.join("split.transit");
    e.execute("session.save", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut f = Engine::default();
    f.execute("session.open", &json!({"path": file.to_string_lossy()})).unwrap();
    let paths: Vec<&str> = f.session().sources.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["Audio Files/LAV-2.L.wav", "Audio Files/LAV-2.R.wav"]);
    assert_eq!((clips(&f, a), clips(&f, b)), (clips(&e, a), clips(&e, b)), "the split reopens from its two mono files");

    e.undo();
    assert!(clips(&e, a).is_empty() && clips(&e, b).is_empty() && e.session().sources.is_empty(), "one undo takes it back");
}

#[test]
fn without_a_mono_track_below_the_stereo_file_lands_whole() {
    let d = transit_test_support::TestDir::new("import-split-whole");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let (a, b) = (track(&mut e, "mono"), track(&mut e, "stereo"));
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": a})).unwrap();
    assert_eq!(clips(&e, a), vec![(0, 480, "LAV-2".to_string(), 2, 0.5)], "a stereo track below: one stereo clip");
    assert!(clips(&e, b).is_empty());
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": b, "at": 960})).unwrap();
    assert_eq!(clips(&e, b).len(), 1, "a stereo track takes the file whole");
    assert_eq!(clips(&e, b)[0].3, 2);
}

#[test]
fn a_hidden_track_below_is_passed_over_for_the_next_shown_mono_track() {
    let d = transit_test_support::TestDir::new("import-split-hidden");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let (a, hidden, b) = (track(&mut e, "mono"), track(&mut e, "mono"), track(&mut e, "mono"));
    e.execute("track.hide", &json!({"tracks": [hidden], "hidden": true})).unwrap();
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": a})).unwrap();
    assert!(clips(&e, hidden).is_empty(), "nothing lands on a track the Edit window hides");
    assert_eq!(clips(&e, b), vec![(0, 480, "LAV-2.R".to_string(), 1, -0.25)]);
}
