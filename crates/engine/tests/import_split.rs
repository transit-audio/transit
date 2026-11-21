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

#[test]
fn a_frozen_or_occupied_mono_track_below_is_never_edited_and_the_file_lands_whole() {
    let d = transit_test_support::TestDir::new("import-split-guarded");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let path = wav.to_string_lossy();
    let mut e = Engine::default();
    let (a, b) = (track(&mut e, "mono"), track(&mut e, "mono"));
    // The last track has no track below: the file lands whole on it.
    e.execute("file.import_audio", &json!({"path": path, "track": b})).unwrap();
    let below = clips(&e, b);
    e.execute("file.import_audio", &json!({"path": path, "track": a})).unwrap();
    assert_eq!(clips(&e, a), vec![(0, 480, "LAV-2".to_string(), 2, 0.5)], "B holds a clip there: whole on A");
    assert_eq!(clips(&e, b), below, "B is untouched");
    e.execute("file.import_audio", &json!({"path": path, "track": a, "at": 960})).unwrap();
    assert_eq!(clips(&e, b).len(), 2, "B is free from 960: the split lands there");
    e.execute("track.freeze", &json!({"tracks": [b]})).unwrap();
    let undo = e.undo_history().len();
    e.execute("file.import_audio", &json!({"path": path, "track": a, "at": 4_800})).unwrap();
    assert_eq!(clips(&e, a).last().map(|c| (c.0, c.3)), Some((4_800, 2)), "B is frozen: whole on A");
    assert_eq!(e.undo_history().len(), undo + 1);
}

#[test]
fn split_to_mono_tracks_puts_each_channel_of_a_stereo_clip_on_a_new_mono_track_below() {
    let d = transit_test_support::TestDir::new("split-to-mono");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(4_800, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let mics = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": "Mics"})).unwrap()["tracks"][0].as_u64().unwrap();
    track(&mut e, "mono");
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": mics, "at": 960})).unwrap();
    let cid = e.session().track(TrackId(mics)).unwrap().clips()[0].id.0;
    let undo = e.undo_history().len();
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    let names: Vec<String> = e.session().tracks.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names[..3], ["Mics", "Mics L", "Mics R"], "the new tracks sit right below: {names:?}");
    let (l, r) = (r["tracks"][0].as_u64().unwrap(), r["tracks"][1].as_u64().unwrap());
    assert_eq!(clips(&e, l), vec![(960, 4_800, "LAV-2.L".to_string(), 1, 0.5)]);
    assert_eq!(clips(&e, r), vec![(960, 4_800, "LAV-2.R".to_string(), 1, -0.25)]);
    assert!(e.session().find_clip(transit_engine::model::ClipId(cid)).unwrap().1.muted, "the original is muted");
    assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
    e.undo();
    assert_eq!(e.session().tracks.len(), 2, "one undo takes the tracks back");
    assert!(!e.session().find_clip(transit_engine::model::ClipId(cid)).unwrap().1.muted);
}

#[test]
fn split_to_mono_tracks_writes_a_mono_file_per_channel_for_a_take_played_from_disk() {
    let d = transit_test_support::TestDir::new("split-to-mono-disk");
    std::fs::create_dir_all(&*d).unwrap();
    let take = d.join("Take.wav");
    std::fs::write(&take, stereo_wav(4_800, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    // A take longer than this plays from its file (ADR 0035): 0.001 minutes is 2880 frames.
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.001);
    let mics = track(&mut e, "stereo");
    let ids = transit_engine::io::add_recording_take(&mut e, &transit_engine::io::Recording::new(0), vec![(TrackId(mics), take)], false);
    assert!(e.session().sources.last().is_some_and(|s| s.disk), "the take plays from disk");
    let r = e.execute("clip.split_to_mono", &json!({"clips": [ids[0].0]})).unwrap();
    for (i, (side, want)) in [("L", 0.5), ("R", -0.25)].into_iter().enumerate() {
        let t = r["tracks"][i].as_u64().unwrap();
        let s = e.session();
        let c = &s.track(TrackId(t)).unwrap().clips()[0];
        let transit_engine::model::ClipContent::Audio { source, .. } = c.content else { panic!("an audio clip") };
        let src = s.source(source).unwrap();
        assert!(src.disk && src.channels == 1 && src.path.ends_with(&format!("Take.{side}.wav")), "{src:?}");
        let disk = s.pool.get(source).unwrap().disk.clone().unwrap();
        assert_eq!((disk.frames(), disk.with_sync(|read| read(0, 4_799))), (4_800, Some(want)), "Take.{side}.wav holds that channel");
    }
}

#[test]
fn a_split_that_fails_on_a_later_take_leaves_no_file_of_an_earlier_one() {
    let d = transit_test_support::TestDir::new("split-to-mono-cleanup");
    std::fs::create_dir_all(&*d).unwrap();
    let mut e = Engine::default();
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.001);
    let mut clips = Vec::new();
    for name in ["One", "Two"] {
        let take = d.join(format!("{name}.wav"));
        std::fs::write(&take, stereo_wav(4_800, 16_384, -8_192)).unwrap();
        let t = track(&mut e, "stereo");
        clips.extend(transit_engine::io::add_recording_take(&mut e, &transit_engine::io::Recording::new(0), vec![(TrackId(t), take)], false));
    }
    // Two.wav loses its audio after it was placed: reading it fails partway through the split.
    std::fs::OpenOptions::new().write(true).open(d.join("Two.wav")).unwrap().set_len(44).unwrap();
    let tracks = e.session().tracks.len();
    let r = e.execute("clip.split_to_mono", &json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}));
    assert!(r.is_err(), "{r:?}");
    // The takes' own peaks sidecars are built in the background; only split files are counted.
    let left: Vec<String> = std::fs::read_dir(&*d).unwrap().map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(left.iter().all(|f| !f.contains(".L") && !f.contains(".R")), "no split file stays behind: {left:?}");
    assert_eq!(e.session().tracks.len(), tracks, "the session is as it was");
}

#[test]
fn the_new_tracks_sit_right_below_the_clips_track_with_a_master_above_it() {
    let d = transit_test_support::TestDir::new("split-to-mono-master");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let mics = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": "Mics"})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("track.new", &json!({"kind": "audio", "format": "mono", "name": "Other"})).unwrap();
    let master = e.session_mut().add_track(transit_model::TrackKind::Master, transit_model::ChannelFormat::Stereo, Some("Master"));
    let s = e.session_mut();
    let m = s.tracks.iter().position(|t| t.id == master).unwrap();
    let moved = s.tracks.remove(m);
    s.tracks.insert(0, moved);
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": mics})).unwrap();
    let cid = e.session().track(TrackId(mics)).unwrap().clips()[0].id.0;
    e.execute("clip.split_to_mono", &json!({"clips": [cid, cid]})).unwrap();
    let names: Vec<String> = e.session().tracks.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, ["Master 1", "Mics", "Mics L", "Mics R", "Other"]);
    let per_track: Vec<usize> = e.session().tracks[2..4].iter().map(|t| t.clips().len()).collect();
    assert_eq!(per_track, [1, 1], "a clip named twice splits once");
}

#[test]
fn audio_is_never_imported_onto_a_midi_track() {
    let d = transit_test_support::TestDir::new("import-onto-midi");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Kick.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let midi = e.execute("track.new", &json!({"kind": "midi", "name": "Synth"})).unwrap()["tracks"][0].as_u64().unwrap();
    let r = e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": midi}));
    assert!(r.as_ref().is_err_and(|err| err.to_string().contains("Synth is not an audio track")), "{r:?}");
    assert!(e.session().track(TrackId(midi)).unwrap().clips().is_empty() && e.session().sources.is_empty());
}
