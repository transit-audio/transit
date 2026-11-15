//! A clip whose stretch is outside what a clip can play (ADR 0056 decision 5.2, #604) is clamped and
//! said on every way in besides Open: Import Session Data passes on the donor's line for each track it
//! takes, and a clip group import says it for each clip.

#![allow(clippy::unwrap_used, clippy::panic)]
use crate::Engine;
use serde_json::json;
use transit_model::{ChannelFormat, Clip, ClipContent, Session, SourceId, TrackKind};

const SAID: &str = "its stretch 0.025 was outside what a clip can play";

#[test]
fn import_session_data_says_a_donor_clips_clamped_stretch_for_the_tracks_it_takes() {
    let dir = transit_test_support::TestDir::new("stretch-import");
    let _ = std::fs::create_dir_all(&dir);
    let mut donor = Session::default();
    for name in ["Fast", "Other"] {
        let t = donor.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        let id = donor.new_clip_id();
        let mut c = Clip::audio(id, format!("{name} clip"), SourceId(77), 0, 0, 4_800);
        c.stretch = 1.0 / 40.0;
        donor.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    }
    let path = dir.join("donor.transit").to_string_lossy().into_owned();
    std::fs::write(&path, donor.to_json().unwrap()).unwrap();
    let mut e = Engine::default();
    let mark = e.message_pushes();
    e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Fast"]})).unwrap();
    let said = e.messages_since(mark).to_vec();
    assert_eq!(said.iter().filter(|m| m.contains(SAID)).count(), 1, "the taken track's line only: {said:?}");
    assert!(said.iter().any(|m| m.starts_with("track \"Fast\", clip \"Fast clip\": ") && m.contains(SAID)), "{said:?}");
    let fast = e.session().tracks.iter().find(|t| t.name.starts_with("Fast")).unwrap();
    assert_eq!(fast.clips()[0].stretch, 1.0 / 32.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_clip_group_import_says_a_clips_clamped_stretch() {
    let dir = transit_test_support::TestDir::new("stretch-group");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("fast.tclips").to_string_lossy().into_owned();
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
    let src = crate::io::add_source(s, "Fast", audio, None, transit_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "Fast", src, 0, 0, 100);
    c.stretch = 1.0 / 40.0;
    crate::edit::place_clip(s, t, c);
    assert_eq!(e.execute("file.export_clip_groups", &json!({"path": path, "clips": [id.0]})).unwrap()["clips"], 1);
    let mark = e.message_pushes();
    let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": 48_000})).unwrap();
    let said = e.messages_since(mark).to_vec();
    assert!(said.iter().any(|m| m.starts_with("clip \"Fast\": ") && m.contains(SAID)), "{said:?}");
    let placed = transit_model::ClipId(r["clips"][0].as_u64().unwrap());
    let (_, c) = e.session().find_clip(placed).unwrap();
    assert!(matches!(c.content, ClipContent::Audio { .. }));
    assert_eq!(c.stretch, 1.0 / 32.0);
    let _ = std::fs::remove_dir_all(&dir);
}
