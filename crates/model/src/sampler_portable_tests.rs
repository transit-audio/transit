#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::sampler_patch::{
    AutoSource, Detector, Division, Extra, Fades, GroupEntry, LoopMode, Marker, Markers, Membership, Ownership, PadEntry, PatchState, PatchV1,
    PlayMode, RoundRobin, SliceBinding, SliceLoop, SliceMode, SliceSet, SourceRef, ZoneEntry,
};
use crate::{SourceAudio, SourceId};
use std::sync::Arc;
use transit_audio_io::AudioBuffer;

/// A patch with every field away from its default: pads with stored notes, names, choke, mute and
/// solo; zones with every playback field, fades, membership, crossfade, labels; groups in both
/// round-robin modes; a loop with both marker sets; slice-bound zones; controls; the seed; a
/// nondefault pad start note; unknown fields at the top and in entries.
fn full(a: SourceId, b: SourceId) -> PatchV1 {
    let mut extra = Extra::new();
    extra.insert("future".into(), json!({"x": [1, 2]}));
    let zone = ZoneEntry {
        id: 7,
        source: 1,
        start: 10,
        end: 4000,
        loop_mode: LoopMode::PingPong,
        loop_start: 100,
        loop_end: 3000,
        crossfade: 450,
        keys: [36, 72],
        velocities: [20, 110],
        root: 48,
        transpose: -12.0,
        cents: 25.0,
        reverse: true,
        mode: PlayMode::OneShot,
        key_tracking: false,
        attack_ms: 5.0,
        decay_ms: 200.0,
        sustain: 0.5,
        release_ms: 80.0,
        declick_ms: 2.0,
        velocity_sensitivity: 0.25,
        fades: Fades::default(),
        membership: None,
        slice: Some(SliceBinding { set: SliceSet::Manual, id: 9, extra: extra.clone() }),
        label: Some("snare".into()),
        extra: extra.clone(),
    };
    let layered = ZoneEntry {
        id: 200,
        membership: Some(Membership { group: 3, layer: 2, alternative: 0, extra: extra.clone() }),
        fades: Fades { key_lo: 3, key_hi: 4, vel_lo: 5, vel_hi: 6, extra: extra.clone() },
        ..ZoneEntry::whole(200, 0, 512)
    };
    let sequential = ZoneEntry { id: 201, membership: Some(Membership { group: 4, ..Membership::default() }), ..ZoneEntry::whole(201, 0, 512) };
    let marker = |id, frame, label: Option<&str>| Marker { id, frame, label: label.map(str::to_string), extra: extra.clone() };
    PatchV1 {
        schema: SCHEMA,
        seed: 0xDEAD_BEEF,
        sources: vec![
            SourceRef { source: a, ownership: Ownership::Managed, extra: Extra::new() },
            SourceRef { source: b, ownership: Ownership::Referenced, extra: extra.clone() },
        ],
        zones: vec![zone, layered, sequential],
        pad_start_note: 50,
        pads: vec![PadEntry {
            index: 5,
            zone: Some(7),
            note: Some(40),
            mute: true,
            solo: true,
            choke: 16,
            name: Some("Snare".into()),
            extra: extra.clone(),
        }],
        groups: vec![
            GroupEntry { id: 3, round_robin: RoundRobin::Random, extra: extra.clone() },
            GroupEntry { id: 4, round_robin: RoundRobin::Sequential, extra: Extra::new() },
        ],
        slicing: Some(SliceLoop {
            source: 1,
            start: 0,
            end: 4000,
            mode: SliceMode::Manual,
            snap: false,
            detector: Detector { source: AutoSource::Transients, sensitivity: 0.8, extra: extra.clone() },
            division: Division::ThirtySecond,
            bars: Some(2),
            grid_bpm: Some(120.5),
            root: Some(36),
            beat_length: Some(8.0),
            auto: Some(Markers { markers: vec![marker(1, 0, Some("one")), marker(2, 2000, None)], next_id: 5, extra: extra.clone() }),
            manual: Some(Markers { markers: vec![marker(8, 0, None), marker(9, 1000, Some("ghost"))], next_id: 12, extra: extra.clone() }),
            markers: None,
            extra: extra.clone(),
        }),
        controls: [("zone008_level".to_string(), -6.0), ("zone008_mod1_amount".to_string(), 25.0)].into_iter().collect(),
        extra,
    }
}

fn ready(p: PatchV1) -> SamplerPatch {
    SamplerPatch { state: PatchState::Ready(Box::new(p)) }
}

fn source(id: u64, path: &str, frames: u64) -> Source {
    Source {
        id: SourceId(id),
        name: Path::new(path).file_stem().and_then(|x| x.to_str()).unwrap_or("x").to_string(),
        path: path.to_string(),
        channels: 1,
        frames,
        sample_rate: 44_100,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    }
}

fn with_audio(s: &mut Session, src: Source) {
    let audio = AudioBuffer { sample_rate: src.sample_rate, channels: vec![vec![0.0; src.frames as usize]] };
    s.pool.insert(src.id, Arc::new(SourceAudio::new(audio)));
    s.sources.push(src);
}

/// An absolute scratch folder in this platform's form (nothing is created in it).
fn scratch() -> PathBuf {
    // The guard drops at once: the path is unique and nothing is ever created at it.
    transit_test_support::TestDir::new("portable-paths").path().to_path_buf()
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn a_kit_with_every_field_round_trips_except_for_the_source_ids() {
    let base = scratch();
    let (kits, session_folder) = (base.join("Kits").join("808"), base.join("Song"));
    let mut s = Session::default();
    with_audio(&mut s, source(11, "Audio Files/kick.wav", 4000));
    with_audio(&mut s, source(12, &text(&kits.join("One").join("Snare.wav")), 4000));
    let original = full(SourceId(11), SourceId(12));
    assert_eq!(original.validate(), Ok(()));
    let saved = to_portable(&ready(original.clone()), &s, Some(&session_folder), &kits).unwrap();
    // No session id is written; every entry carries its file and the session's facts.
    for e in saved.kit["sources"].as_array().unwrap() {
        assert!(e.get("source").is_none() && e.get("ownership").is_none(), "{e}");
        assert_eq!(e["file"]["frames"], 4000);
        assert_eq!(e["file"]["sample_rate"], 44_100);
        assert_eq!(e["file"]["format"], "Wav");
    }
    assert_eq!(saved.kit["sources"][1]["future"], json!({"x": [1, 2]}), "unknown fields in entries are kept");
    assert_eq!(saved.kit["sources"][1]["file"]["path"], "One/Snare.wav");
    assert_eq!(saved.kit["sources"][1]["file"]["name"], "Snare");
    assert_eq!(saved.kit["sources"][0]["file"]["path"], text(&session_folder.join("Audio Files").join("kick.wav")));
    assert_eq!(saved.files.iter().map(|f| (f.relative, f.managed, f.missing)).collect::<Vec<_>>(), [(false, true, false), (true, false, false)]);
    // Through the file text and back.
    let file_text = serde_json::to_string_pretty(&file(saved.kit)).unwrap();
    let back: Value = serde_json::from_str(&file_text).unwrap();
    assert_eq!((back["format"].as_str(), back["version"].as_u64()), (Some(FORMAT), Some(VERSION)));
    let (kit, entries) = from_portable(back["kit"].clone()).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[1], Entry::File(f) if f.path == "One/Snare.wav" && f.frames == 4000 && f.channels == 1));
    let recalled = with_sources(kit, &[SourceId(101), SourceId(102)]);
    let mut expected = original;
    expected.sources[0].source = SourceId(101);
    expected.sources[1].source = SourceId(102);
    for r in &mut expected.sources {
        r.ownership = Ownership::Referenced;
    }
    assert_eq!(recalled.ready(), Some(&expected), "{:?}", recalled.state);
}

#[test]
fn windows_paths_relative_only_in_or_below_the_preset_folder() {
    assert_eq!(relative(r"C:\Kits\808\Kick.wav", r"c:\kits\808").as_deref(), Some("Kick.wav"), "drive paths ignore case");
    assert_eq!(relative(r"C:\Kits\808\One\Kick.wav", r"C:\Kits\808\").as_deref(), Some("One/Kick.wav"), "written with /");
    assert_eq!(relative(r"C:/Kits/808/Kick.wav", r"C:\Kits\808").as_deref(), Some("Kick.wav"), "both separators");
    assert_eq!(relative(r"C:\Kits\909\Kick.wav", r"C:\Kits\808"), None);
    assert_eq!(relative(r"D:\Kits\808\Kick.wav", r"C:\Kits\808"), None, "another drive");
    assert_eq!(relative(r"C:\Kits\8080\Kick.wav", r"C:\Kits\808"), None, "whole components");
    assert_eq!(relative(r"C:\Kits\808", r"C:\Kits\808"), None, "the folder itself");
    assert_eq!(relative(r"C:\Kits\x\..\808\Kick.wav", r"C:\Kits\808").as_deref(), Some("Kick.wav"), "folded lexically");
    // The extended-length prefix goes before the relative test, on either side.
    assert_eq!(relative(r"\\?\C:\Kits\808\Kick.wav", r"C:\Kits\808").as_deref(), Some("Kick.wav"));
    assert_eq!(relative(r"C:\Kits\808\Kick.wav", r"\\?\C:\Kits\808").as_deref(), Some("Kick.wav"));
    assert_eq!(strip_verbatim(r"\\?\C:\Kits\Kick.wav"), r"C:\Kits\Kick.wav");
    assert_eq!(strip_verbatim(r"\\?\UNC\nas\samples\Kick.wav"), r"\\nas\samples\Kick.wav");
    assert_eq!(strip_verbatim("/home/me/Kick.wav"), "/home/me/Kick.wav");
    // Shares: relative on the same share (host and share ignore case), never across shares.
    assert_eq!(relative(r"\\nas\samples\kits\k.wav", r"\\NAS\Samples\kits").as_deref(), Some("k.wav"));
    assert_eq!(relative(r"\\?\UNC\nas\samples\kits\k.wav", r"\\nas\samples\kits").as_deref(), Some("k.wav"));
    assert_eq!(relative(r"\\nas\other\kits\k.wav", r"\\nas\samples\kits"), None);
    assert!(other_share(r"\\nas\samples\k.wav", r"C:\Users\me\presets"));
    assert!(other_share(r"\\nas\samples\k.wav", r"\\nas\other"));
    assert!(!other_share(r"\\nas\samples\k.wav", r"\\NAS\samples\kits"));
    assert!(!other_share(r"C:\k.wav", r"\\nas\samples"));
    for p in [r"C:\x", r"\\?\C:\x", r"\\nas\s\x", "/x"] {
        assert!(is_absolute(p), "{p}");
    }
    for p in ["x", r"Kits\x", r"\x", "C:x", r"\\.\COM1\x", ""] {
        assert!(!is_absolute(p), "{p}");
    }
}

#[test]
fn same_path_and_join_follow_the_relative_rules() {
    assert!(same_path(r"\\?\C:\Kits\Kick.wav", r"c:\kits\KICK.wav"));
    assert!(same_path("/a/b/Kick.wav", "/a/x/../b/Kick.wav"));
    assert!(!same_path("/a/b/Kick.wav", "/a/b/kick.wav"), "case matters on a Unix path");
    assert!(!same_path(r"C:\a\Kick.wav", r"D:\a\Kick.wav"));
    assert!(!same_path("Kick.wav", "Kick.wav"), "only absolute paths");
    assert_eq!(join(r"\\?\C:\Kits\808\", "One/Kick.wav").as_deref(), Some(r"C:\Kits\808\One\Kick.wav"));
    assert_eq!(join("/kits/808", "One/Kick.wav").as_deref(), Some("/kits/808/One/Kick.wav"));
    assert_eq!(join("kits", "Kick.wav"), None);
}

#[test]
fn unix_paths_relative_only_in_or_below_the_preset_folder_and_case_matters() {
    assert_eq!(relative("/home/me/Kits/808/Kick.wav", "/home/me/Kits/808").as_deref(), Some("Kick.wav"));
    assert_eq!(relative("/home/me/Kits/808/a/b/Kick.wav", "/home/me/Kits/808/").as_deref(), Some("a/b/Kick.wav"));
    assert_eq!(relative("/home/me/Kits/808/Kick.wav", "/home/me/kits/808"), None, "case matters on a Unix path");
    assert_eq!(relative("/home/me/Library/Kick.wav", "/home/me/Kits/808"), None);
    assert_eq!(relative("/home/me/Kits/808/Kick.wav", r"C:\home\me\Kits\808"), None, "another root");
}

#[test]
fn save_writes_relative_below_the_folder_and_absolute_elsewhere_without_the_prefix() {
    let base = scratch();
    let kits = base.join("Kits").join("808");
    let elsewhere = base.join("Library").join("Kick 01.wav");
    let mut s = Session::default();
    with_audio(&mut s, source(1, &text(&kits.join("Kick.wav")), 10));
    with_audio(&mut s, source(2, &text(&elsewhere), 10));
    // A Load kit source stored from canonicalization, with the extended-length prefix.
    let prefixed = if cfg!(windows) { format!(r"\\?\{}", text(&kits.join("Snare.wav"))) } else { text(&kits.join("Snare.wav")) };
    with_audio(&mut s, source(3, &prefixed, 10));
    let mut p = PatchV1::default();
    for (z, id) in [1u64, 2, 3].iter().enumerate() {
        let i = p.source_index(SourceId(*id), Ownership::Referenced) as u16;
        p.zones.push(ZoneEntry::whole(z as u16, i, 10));
    }
    let saved = to_portable(&ready(p), &s, None, &kits).unwrap();
    let paths: Vec<&str> = saved.kit["sources"].as_array().unwrap().iter().map(|e| e["file"]["path"].as_str().unwrap()).collect();
    assert_eq!(paths[0], "Kick.wav");
    assert_eq!(paths[1], text(&elsewhere), "absolute, as the session holds it");
    assert_eq!(paths[2], "Snare.wav", "prefix removed before the relative test");
    assert!(paths.iter().all(|p| !p.contains(r"\\?\")));
    assert_eq!(saved.files.iter().map(|f| f.relative).collect::<Vec<_>>(), [true, false, true]);
    // A prefixed sample outside the folder is written absolute without the prefix.
    let out = SourceId(4);
    let mut s2 = s.clone();
    with_audio(&mut s2, source(4, r"\\?\D:\Library\Hat.wav", 10));
    let mut p2 = PatchV1::default();
    let i = p2.source_index(out, Ownership::Referenced) as u16;
    p2.zones.push(ZoneEntry::whole(0, i, 10));
    let saved = to_portable(&ready(p2), &s2, None, &kits).unwrap();
    assert_eq!(saved.kit["sources"][0]["file"]["path"], r"D:\Library\Hat.wav");
}

#[test]
fn a_moved_kit_folder_keeps_relative_paths_free_of_the_old_folder() {
    let base = scratch();
    let kits = base.join("Kits").join("808");
    let mut s = Session::default();
    with_audio(&mut s, source(1, &text(&kits.join("Drums").join("Kick.wav")), 10));
    let mut p = PatchV1::default();
    let i = p.source_index(SourceId(1), Ownership::Referenced) as u16;
    p.zones.push(ZoneEntry::whole(0, i, 10));
    let saved = to_portable(&ready(p), &s, None, &kits).unwrap();
    let written = saved.kit["sources"][0]["file"]["path"].as_str().unwrap().to_string();
    assert_eq!(written, "Drums/Kick.wav");
    // Wherever the folder moves, the path names the same file below it.
    let moved = base.join("Elsewhere").join("808 copy");
    let found = written.split('/').fold(moved.clone(), |p, c| p.join(c));
    assert_eq!(found, moved.join("Drums").join("Kick.wav"));
}

#[test]
fn a_source_with_no_file_of_its_own_is_refused_naming_the_pad_else_the_zone_else_the_loop() {
    let base = scratch();
    let mut s = Session::default();
    let mut unsaved = source(1, "Audio Files/Hit.wav", 10);
    unsaved.unsaved = true;
    with_audio(&mut s, unsaved);
    let mut p = PatchV1::default();
    let i = p.source_index(SourceId(1), Ownership::Managed) as u16;
    p.zones.push(ZoneEntry::whole(4, i, 10));
    p.set_pad(PadEntry { zone: Some(4), ..p.pad(2) });
    let err = to_portable(&ready(p.clone()), &s, Some(&base), &base).unwrap_err();
    assert_eq!(err.text(), "Pad A3's sample \"Hit.wav\" has no file yet: save the session, or load the sample onto the pad from a file.");
    p.set_pad(PadEntry { zone: None, ..p.pad(2) });
    assert_eq!(
        to_portable(&ready(p.clone()), &s, Some(&base), &base).unwrap_err(),
        SaveRefusal::NoFile { user: User::Zone(4), name: "Hit.wav".into() }
    );
    p.zones.clear();
    p.slicing = Some(SliceLoop { source: i, end: 10, ..SliceLoop::default() });
    assert_eq!(to_portable(&ready(p.clone()), &s, Some(&base), &base).unwrap_err(), SaveRefusal::NoFile { user: User::Loop, name: "Hit.wav".into() });
    // A managed file with no session folder has no file either; nor does an id with no record.
    s.sources[0].unsaved = false;
    assert!(to_portable(&ready(p.clone()), &s, None, &base).is_err());
    assert!(to_portable(&ready(p.clone()), &s, Some(&base), &base).is_ok());
    p.sources[0].source = SourceId(0);
    assert_eq!(to_portable(&ready(p), &s, Some(&base), &base).unwrap_err(), SaveRefusal::NoFile { user: User::Loop, name: "source 0".into() });
}

#[test]
fn a_missing_source_is_saved_with_its_stored_path_and_listed() {
    let base = scratch();
    let mut s = Session::default();
    s.sources.push(source(1, &text(&base.join("Gone.wav")), 10));
    let mut p = PatchV1::default();
    let i = p.source_index(SourceId(1), Ownership::Referenced) as u16;
    p.zones.push(ZoneEntry::whole(0, i, 10));
    let saved = to_portable(&ready(p), &s, None, &base.join("Kits")).unwrap();
    assert!(saved.files[0].missing);
    assert_eq!(saved.kit["sources"][0]["file"]["path"], text(&base.join("Gone.wav")));
}

#[test]
fn an_opaque_patch_is_rewritten_without_session_ids() {
    let base = scratch();
    let mut s = Session::default();
    with_audio(&mut s, source(5, &text(&base.join("Kick.wav")), 10));
    let raw = json!({"schema": 2, "sources": [{"source": 5, "ownership": "referenced", "x": 1}], "new_thing": true});
    let opaque = SamplerPatch::from_value(raw);
    assert!(matches!(opaque.state, PatchState::Unknown { schema: 2, .. }));
    let saved = to_portable(&opaque, &s, None, &base).unwrap();
    assert_eq!(
        saved.kit,
        json!({"schema": 2, "sources": [{"file": {"path": "Kick.wav", "name": "Kick", "frames": 10, "channels": 1, "sample_rate": 44100, "format": "Wav"}, "x": 1}], "new_thing": true})
    );
}

#[test]
fn an_unknown_newer_schema_is_refused_and_stays_opaque() {
    let kit = json!({"schema": 2, "sources": [{"file": {"path": "a.wav"}}], "zones": "anything"});
    let refusal = from_portable(kit).unwrap_err();
    assert_eq!(refusal, ReadRefusal::NewerSchema(2));
    assert_eq!(refusal.text(), "saved by a newer transit (Sampler kit schema 2)");
    assert!(matches!(from_portable(json!({"schema": 0})), Err(ReadRefusal::Invalid(_))));
    assert!(matches!(from_portable(json!({"schema": "1"})), Err(ReadRefusal::Invalid(_))));
}

#[test]
fn source_entries_need_exactly_one_of_file_and_source() {
    let file = json!({"path": "a.wav", "name": "a", "frames": 1, "channels": 1, "sample_rate": 48000, "format": "Wav"});
    let both = json!({"sources": [{"file": file, "source": 3}]});
    let neither = json!({"sources": [{"x": 1}]});
    let malformed = json!({"sources": [{"file": {"path": 7}}]});
    let long = json!({"sources": [{"file": {"path": "a".repeat(MAX_LABEL_BYTES + 1), "name": "a", "frames": 1, "channels": 1, "sample_rate": 48000, "format": "Wav"}}]});
    for bad in [both, neither, malformed, long, json!([]), json!({"sources": {}})] {
        assert!(matches!(from_portable(bad.clone()), Err(ReadRefusal::Invalid(_))), "{bad}");
    }
    let (kit, entries) = from_portable(json!({"sources": [{"source": 3}, {"file": file}]})).unwrap();
    assert_eq!(entries[0], Entry::Session(SourceId(3)));
    assert!(matches!(&entries[1], Entry::File(f) if f.path == "a.wav"));
    assert!(kit["sources"][1].get("file").is_none(), "the file objects are taken out");
    // A file object without its facts loads with the defaults (0 frames at 0 Hz: no file matches).
    let (_, entries) = from_portable(json!({"sources": [{"file": {"path": "a.wav"}}]})).unwrap();
    assert_eq!(entries[0], Entry::File(FileRef { path: "a.wav".into(), ..FileRef::default() }));
}

#[test]
fn the_extension_is_appended_never_replacing_another() {
    assert_eq!(kit_file_path(Path::new("808 v1.5")), PathBuf::from("808 v1.5.tkit"));
    assert_ne!(kit_file_path(Path::new("Kit.1")), kit_file_path(Path::new("Kit.2")));
    assert_eq!(kit_file_path(Path::new("Kit.1")), PathBuf::from("Kit.1.tkit"));
    assert_eq!(kit_file_path(Path::new("808.tkit")), PathBuf::from("808.tkit"));
    assert_eq!(kit_file_path(Path::new("808.TKit")), PathBuf::from("808.TKit"));
    assert_eq!(kit_file_path(Path::new("808")), PathBuf::from("808.tkit"));
}
