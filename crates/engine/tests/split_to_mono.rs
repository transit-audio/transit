//! Split to Mono Tracks (`clip.split_to_mono`): each channel of a stereo or wider clip on a new mono
//! track below its track, in one undo step. Import itself keeps a file's channels as they are.

use serde_json::json;
use transit_engine::Engine;
use transit_engine::model::TrackId;

/// A 16-bit stereo PCM WAV: every left sample is `l`, every right one `r`.
fn stereo_wav(frames: u32, l: i16, r: i16) -> Vec<u8> {
    pcm_wav(frames, 2, |_, ch| if ch == 0 { l } else { r })
}

/// A 16-bit PCM WAV of `channels` channels at 48 kHz: frame `i` of channel `ch` is `sample(i, ch)`.
fn pcm_wav(frames: u32, channels: u16, sample: impl Fn(u32, u16) -> i16) -> Vec<u8> {
    let align = u32::from(channels) * 2;
    let data = frames * align;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, (u32::from(channels) << 16) | 1, 48_000, 48_000 * align, (16 << 16) | align] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        for ch in 0..channels {
            b.extend_from_slice(&sample(i, ch).to_le_bytes());
        }
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
fn import_keeps_a_stereo_file_whole_even_on_a_mono_track_with_a_free_mono_track_below() {
    let d = transit_test_support::TestDir::new("import-stays-whole");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let (a, b) = (track(&mut e, "mono"), track(&mut e, "mono"));
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": a})).unwrap();
    assert_eq!(clips(&e, a), vec![(0, 480, "LAV-2".to_string(), 2, 0.5)], "one stereo clip: a split is asked for, never done on import");
    assert!(clips(&e, b).is_empty());
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

    let file = d.join("split.transit");
    e.execute("session.save", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut f = Engine::default();
    f.execute("session.open", &json!({"path": file.to_string_lossy()})).unwrap();
    let paths: Vec<&str> = f.session().sources.iter().map(|s| s.path.as_str()).collect();
    assert!(paths.contains(&"Audio Files/LAV-2.L.wav") && paths.contains(&"Audio Files/LAV-2.R.wav"), "{paths:?}");
    assert_eq!((clips(&f, l), clips(&f, r)), (clips(&e, l), clips(&e, r)), "the split reopens from its two mono files");

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

#[test]
fn a_take_whose_file_is_back_after_a_failed_read_splits_again() {
    let d = transit_test_support::TestDir::new("split-to-mono-reopen");
    std::fs::create_dir_all(&*d).unwrap();
    let take = d.join("Take.wav");
    let bytes = stereo_wav(4_800, 16_384, -8_192);
    std::fs::write(&take, &bytes).unwrap();
    let mut e = Engine::default();
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.001);
    let mics = track(&mut e, "stereo");
    let ids = transit_engine::io::add_recording_take(&mut e, &transit_engine::io::Recording::new(0), vec![(TrackId(mics), take.clone())], false);
    // The file loses its audio (a drive unplugged): the split's read fails and its reader with it.
    std::fs::OpenOptions::new().write(true).open(&take).unwrap().set_len(44).unwrap();
    assert!(e.execute("clip.split_to_mono", &json!({"clips": [ids[0].0]})).is_err());
    // The file is back: the split opens the take anew, as an offline render does.
    std::fs::write(&take, &bytes).unwrap();
    let r = e.execute("clip.split_to_mono", &json!({"clips": [ids[0].0]}));
    assert!(r.as_ref().is_ok_and(|r| r["tracks"].as_array().map(Vec::len) == Some(2)), "{r:?}");
}

/// #771: the two mono tracks of a split stereo clip are panned hard left and hard right, so the
/// split renders as the stereo track did at its default pan. A mono track hard left is exactly
/// (1, 0) under every pan law and a stereo track's two panners are exactly (1, 0) and (0, 1) at
/// their defaults, so the comparison is IEEE-exact.
#[test]
fn a_split_stereo_clip_renders_exactly_as_its_stereo_track_did() {
    let d = transit_test_support::TestDir::new("split-to-mono-render");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Mics.wav");
    // Two different signals, so a channel landing on the wrong side shows.
    let sample = |i: u32, ch: u16| (((i * if ch == 0 { 37 } else { 91 }) % 2_000) as i16 - 1_000) * 16;
    std::fs::write(&wav, pcm_wav(4_800, 2, sample)).unwrap();
    let mut e = Engine::default();
    let mics = track(&mut e, "stereo");
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": mics, "at": 960})).unwrap();
    let range = transit_time::Range::new(0, 6_400);
    let before = transit_mix::render_range(e.session(), range, 512);
    assert!(before.len() == 2 && before.iter().all(|ch| ch.iter().any(|v| *v != 0.0)), "the stereo track sounds on both sides");
    let cid = e.session().track(TrackId(mics)).unwrap().clips()[0].id.0;
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    let pans: Vec<Vec<f32>> = (0..2).map(|i| e.session().track(TrackId(r["tracks"][i].as_u64().unwrap())).unwrap().mixer.pan.clone()).collect();
    assert_eq!(pans, [vec![-1.0], vec![1.0]], "L hard left, R hard right");
    let after = transit_mix::render_range(e.session(), range, 512);
    let diff = before.iter().flatten().zip(after.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(after == before, "the split renders exactly as the stereo track did (largest difference {diff})");
}

/// #771: a source wider than stereo has no left and right: its mono tracks stay centered.
#[test]
fn the_mono_tracks_of_a_wider_clip_stay_centered() {
    let d = transit_test_support::TestDir::new("split-to-mono-wide");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Room.wav");
    std::fs::write(&wav, pcm_wav(480, 4, |_, ch| 1_000 * (i16::try_from(ch).unwrap() + 1))).unwrap();
    let mut e = Engine::default();
    let room = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": "Room"})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": room})).unwrap();
    let cid = e.session().track(TrackId(room)).unwrap().clips()[0].id.0;
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    let tracks = r["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 4);
    for t in tracks {
        let tr = e.session().track(TrackId(t.as_u64().unwrap())).unwrap();
        assert_eq!(tr.mixer.pan, [0.0], "{} is centered", tr.name);
    }
}

/// #771: the new tracks get no inserts. When the clip's track has any, the result carries a
/// notice for the status line saying they were not copied; with none it carries no notice.
#[test]
fn a_split_copies_no_insert_and_says_so_when_the_track_had_one() {
    let d = transit_test_support::TestDir::new("split-to-mono-inserts");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("LAV-2.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let split = |e: &mut Engine, name: &str| {
        let t = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": name})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": t})).unwrap();
        (t, e.session().track(TrackId(t)).unwrap().clips()[0].id.0)
    };
    let (_, plain) = split(&mut e, "Plain");
    let r = e.execute("clip.split_to_mono", &json!({"clips": [plain]})).unwrap();
    assert!(r.get("notice").is_none(), "no insert, no notice: {r}");
    let (mics, cid) = split(&mut e, "Mics");
    e.execute("mix.insert", &json!({"track": mics, "slot": 0, "plugin": "eq_7band"})).unwrap();
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    assert_eq!(r["notice"], "Split to 2 mono tracks. The inserts on Mics were not copied.");
    for t in r["tracks"].as_array().unwrap() {
        let tr = e.session().track(TrackId(t.as_u64().unwrap())).unwrap();
        assert!(tr.mixer.inserts.iter().all(Option::is_none), "{} has no insert", tr.name);
    }
}

/// #771: a name too long to take its channel ending is shortened in its stem, so the track, clip,
/// source and file names of a split all keep `L` and `R`, and stay within their limits.
#[test]
fn long_names_keep_their_channel_endings() {
    let d = transit_test_support::TestDir::new("split-to-mono-long");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Mics.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let mics = track(&mut e, "stereo");
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": mics})).unwrap();
    let max = transit_model::MAX_NAME_CHARS;
    let s = e.session_mut();
    s.track_mut(TrackId(mics)).unwrap().name = "T".repeat(max);
    s.sources.last_mut().unwrap().name = "S".repeat(max);
    s.track_mut(TrackId(mics)).unwrap().playlist_mut().unwrap().clips[0].name = "C".repeat(max);
    let cid = e.session().track(TrackId(mics)).unwrap().clips()[0].id.0;
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    for (i, side) in ["L", "R"].into_iter().enumerate() {
        let s = e.session();
        let tr = s.track(TrackId(r["tracks"][i].as_u64().unwrap())).unwrap();
        assert!(tr.name.ends_with(&format!("T {side}")) && tr.name.chars().count() == max, "track {:?}", tr.name);
        let c = &tr.clips()[0];
        assert!(c.name.ends_with(&format!("C.{side}")) && c.name.chars().count() == max, "clip {:?}", c.name);
        let transit_engine::model::ClipContent::Audio { source, .. } = c.content else { panic!("an audio clip") };
        let src = s.source(source).unwrap();
        assert!(src.name.ends_with(&format!("S.{side}")) && src.name.chars().count() <= 100, "source {:?}", src.name);
    }
    let file = d.join("long.transit");
    e.execute("session.save", &json!({"path": file.to_string_lossy()})).unwrap();
    let paths: Vec<String> = e.session().sources.iter().map(|s| s.path.clone()).collect();
    for side in ["L", "R"] {
        assert!(paths.iter().any(|p| p.ends_with(&format!("S.{side}.wav"))), "the {side} file keeps its ending: {paths:?}");
    }
}

/// #771: the file written beside a long take played from disk keeps its channel ending too.
#[test]
fn a_long_takes_split_files_keep_their_channel_endings() {
    let d = transit_test_support::TestDir::new("split-to-mono-long-disk");
    std::fs::create_dir_all(&*d).unwrap();
    let take = d.join("Take.wav");
    std::fs::write(&take, stereo_wav(4_800, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.001);
    let mics = track(&mut e, "stereo");
    let ids = transit_engine::io::add_recording_take(&mut e, &transit_engine::io::Recording::new(0), vec![(TrackId(mics), take)], false);
    e.session_mut().sources.last_mut().unwrap().name = "S".repeat(150);
    let r = e.execute("clip.split_to_mono", &json!({"clips": [ids[0].0]})).unwrap();
    for (i, side) in ["L", "R"].into_iter().enumerate() {
        let s = e.session();
        let c = &s.track(TrackId(r["tracks"][i].as_u64().unwrap())).unwrap().clips()[0];
        let transit_engine::model::ClipContent::Audio { source, .. } = c.content else { panic!("an audio clip") };
        let src = s.source(source).unwrap();
        assert!(src.disk && src.path.ends_with(&format!("S.{side}.wav")), "{:?}", src.path);
    }
}

/// #771: two tracks whose 256-character names differ only at the very end get names that are
/// still different after a split, a save and a reopen: a numbered name is cut at its stem, within
/// the limit, so the save has nothing to cut.
#[test]
fn long_track_names_that_differ_at_the_end_stay_unique_through_a_save() {
    let d = transit_test_support::TestDir::new("split-to-mono-unique");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Mics.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let max = transit_model::MAX_NAME_CHARS;
    let mut clips = Vec::new();
    for end in ["a", "b"] {
        let t = track(&mut e, "stereo");
        e.session_mut().track_mut(TrackId(t)).unwrap().name = format!("{}{end}", "T".repeat(max - 1));
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": t})).unwrap();
        clips.push(e.session().track(TrackId(t)).unwrap().clips()[0].id.0);
    }
    e.execute("clip.split_to_mono", &json!({"clips": clips})).unwrap();
    let names = |e: &Engine| e.session().tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    let before = names(&e);
    assert_eq!(before.len(), 6);
    assert!(before.iter().all(|n| n.chars().count() <= max), "every name is within the limit");
    let file = d.join("unique.transit");
    e.execute("session.save", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut f = Engine::default();
    f.execute("session.open", &json!({"path": file.to_string_lossy()})).unwrap();
    let after = names(&f);
    assert_eq!(after, before, "a reopen changes no name");
    assert_eq!(after.iter().collect::<std::collections::HashSet<_>>().len(), 6, "{} names, all different", after.len());
}

/// #771: the refusals the clip menu shows as the reason of its greyed row are the command's own:
/// `split_refusal` and `clip.split_to_mono` give the same words, and a refusal changes nothing.
#[test]
fn a_frozen_track_or_a_too_wide_clip_is_refused_in_the_words_the_menu_shows() {
    let mut e = Engine::default();
    let audio = |e: &mut Engine, name: &str, channels: usize| {
        let t = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": name})).unwrap()["tracks"][0].as_u64().unwrap();
        let s = e.session_mut();
        let buf = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1; 480]; channels] };
        let src = transit_engine::io::add_source(s, name, buf, None, transit_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        s.track_mut(TrackId(t)).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::audio(id, name, src, 0, 0, 480));
        (TrackId(t), src, id)
    };
    let (frozen_track, src, frozen) = audio(&mut e, "Frozen", 2);
    let (_, _, wide) = audio(&mut e, "Wide", 65);
    let (_, _, fine) = audio(&mut e, "Fine", 64);
    e.session_mut().track_mut(frozen_track).unwrap().freeze =
        Some(transit_model::Frozen { source: src, start: 0, length: 480, hash: 0, revision: 0, latency: 0 });
    let tracks = e.session().tracks.len();
    for (clip, why) in
        [(frozen, "Frozen is frozen: unfreeze it to split its clips"), (wide, "Wide has 65 channels: a clip splits into at most 64 tracks")]
    {
        assert_eq!(transit_engine::cmd::split_refusal(e.session(), &[clip]).as_deref(), Some(why));
        let r = e.execute("clip.split_to_mono", &json!({"clips": [clip.0]}));
        assert!(r.as_ref().is_err_and(|err| err.to_string().contains(why)), "{r:?}");
        assert_eq!(e.session().tracks.len(), tracks, "a refusal adds no track");
    }
    assert_eq!(transit_engine::cmd::split_refusal(e.session(), &[fine]), None, "64 channels split");
}

/// #771: a stereo clip and a wider clip selected together on one track do not share destination
/// tracks: the stereo clip's two tracks keep their hard left and right and render as the stereo
/// track did, the wider clip's tracks stay centered, all below the track, the stereo pair first.
#[test]
fn a_stereo_and_a_wider_clip_on_one_track_split_onto_separate_tracks() {
    let d = transit_test_support::TestDir::new("split-to-mono-mixed");
    std::fs::create_dir_all(&*d).unwrap();
    let (stereo, wide) = (d.join("Mics.wav"), d.join("Room.wav"));
    let sample = |i: u32, ch: u16| (((i * if ch == 0 { 37 } else { 91 }) % 2_000) as i16 - 1_000) * 16;
    std::fs::write(&stereo, pcm_wav(4_800, 2, sample)).unwrap();
    std::fs::write(&wide, pcm_wav(480, 4, |_, ch| 1_000 * (i16::try_from(ch).unwrap() + 1))).unwrap();
    let mut e = Engine::default();
    let mics = e.execute("track.new", &json!({"kind": "audio", "format": "stereo", "name": "Mics"})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("file.import_audio", &json!({"path": stereo.to_string_lossy(), "track": mics, "at": 960})).unwrap();
    e.execute("file.import_audio", &json!({"path": wide.to_string_lossy(), "track": mics, "at": 9_600})).unwrap();
    let ids: Vec<u64> = e.session().track(TrackId(mics)).unwrap().clips().iter().map(|c| c.id.0).collect();
    // The wide clip starts after the range rendered, so the comparison is of the stereo clip alone.
    let range = transit_time::Range::new(0, 6_400);
    let before = transit_mix::render_range(e.session(), range, 512);
    assert!(before.len() == 2 && before.iter().all(|ch| ch.iter().any(|v| *v != 0.0)), "the stereo clip sounds on both sides");
    let r = e.execute("clip.split_to_mono", &json!({"clips": ids})).unwrap();
    let names: Vec<String> = e.session().tracks.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, ["Mics", "Mics L", "Mics R", "Mics 1", "Mics 2", "Mics 3", "Mics 4"], "{names:?}");
    let pans: Vec<Vec<f32>> =
        r["tracks"].as_array().unwrap().iter().map(|t| e.session().track(TrackId(t.as_u64().unwrap())).unwrap().mixer.pan.clone()).collect();
    assert_eq!(pans[..2], [vec![-1.0], vec![1.0]], "the stereo clip keeps hard left and right");
    assert!(pans[2..].iter().all(|p| *p == [0.0]), "the wider clip stays centered: {pans:?}");
    let per_track: Vec<usize> = e.session().tracks[1..].iter().map(|t| t.clips().len()).collect();
    assert_eq!(per_track, [1, 1, 1, 1, 1, 1], "each new track holds one channel of one clip");
    let after = transit_mix::render_range(e.session(), range, 512);
    let diff = before.iter().flatten().zip(after.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(after == before, "the stereo clip renders exactly as before (largest difference {diff})");
}

/// #771: a stereo track whose panners were moved takes them to its two mono tracks. Swapped to hard
/// right and hard left the pans are exact under every pan law, so the split renders exactly as the
/// stereo track did.
#[test]
fn a_split_carries_a_stereo_tracks_swapped_panners_and_renders_as_it_did() {
    let d = transit_test_support::TestDir::new("split-to-mono-swapped");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Mics.wav");
    let sample = |i: u32, ch: u16| (((i * if ch == 0 { 37 } else { 91 }) % 2_000) as i16 - 1_000) * 16;
    std::fs::write(&wav, pcm_wav(4_800, 2, sample)).unwrap();
    let mut e = Engine::default();
    let mics = track(&mut e, "stereo");
    e.session_mut().track_mut(TrackId(mics)).unwrap().mixer.pan = vec![1.0, -1.0];
    e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": mics, "at": 960})).unwrap();
    let range = transit_time::Range::new(0, 6_400);
    let before = transit_mix::render_range(e.session(), range, 512);
    assert!(before.len() == 2 && before.iter().all(|ch| ch.iter().any(|v| *v != 0.0)), "the stereo track sounds on both sides");
    let cid = e.session().track(TrackId(mics)).unwrap().clips()[0].id.0;
    let r = e.execute("clip.split_to_mono", &json!({"clips": [cid]})).unwrap();
    let pans: Vec<Vec<f32>> = (0..2).map(|i| e.session().track(TrackId(r["tracks"][i].as_u64().unwrap())).unwrap().mixer.pan.clone()).collect();
    assert_eq!(pans, [vec![1.0], vec![-1.0]], "each side takes its panner");
    let after = transit_mix::render_range(e.session(), range, 512);
    let diff = before.iter().flatten().zip(after.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(after == before, "the split renders exactly as the stereo track did (largest difference {diff})");
}

/// #771: panners between hard left and right are carried as they are (exact under the Zero pan law;
/// under the others a mono track pans by that law's gains: the stated limit), and a stereo clip on a
/// mono track, which has one panner, keeps hard left and right.
#[test]
fn a_split_carries_a_stereo_tracks_moved_panners_and_a_mono_track_keeps_hard_sides() {
    let d = transit_test_support::TestDir::new("split-to-mono-moved");
    std::fs::create_dir_all(&*d).unwrap();
    let wav = d.join("Mics.wav");
    std::fs::write(&wav, stereo_wav(480, 16_384, -8_192)).unwrap();
    let mut e = Engine::default();
    let (wide, mono) = (track(&mut e, "stereo"), track(&mut e, "mono"));
    e.session_mut().track_mut(TrackId(wide)).unwrap().mixer.pan = vec![-0.5, 0.25];
    let mut clips = Vec::new();
    for t in [wide, mono] {
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": t})).unwrap();
        clips.push(e.session().track(TrackId(t)).unwrap().clips()[0].id.0);
    }
    let r = e.execute("clip.split_to_mono", &json!({"clips": clips})).unwrap();
    let pans: Vec<Vec<f32>> =
        r["tracks"].as_array().unwrap().iter().map(|t| e.session().track(TrackId(t.as_u64().unwrap())).unwrap().mixer.pan.clone()).collect();
    assert_eq!(pans, [vec![-0.5], vec![0.25], vec![-1.0], vec![1.0]]);
}
