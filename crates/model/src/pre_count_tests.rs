//! The pre-count zone in the model (ADR 0052 decisions 2 and 13): the derived session start and
//! the `pre_count` need.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{ChannelFormat, Clip, ClipId, Playlist, Session, TrackKind};
use transit_midi::{Note, Sequence};

/// The `KNOWN_NEEDS` of the last release before the pre-count zone.
const PREVIOUS_RELEASE: [&str; 5] = ["clip_loop", "clip_scale_mode", "device_pages", "sampler_patch", "sampler_slicing_modes"];

fn note(start: i64) -> Note {
    Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start, length: 480 }
}

/// A MIDI track holding one clip at `start` with two notes.
fn session_with_clip_at(start: i64) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let mut seq = Sequence::default();
    seq.notes.push(note(0));
    seq.notes.push(note(1920));
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(10), "m", start, 96_000, seq));
    s
}

#[test]
fn the_session_start_is_the_earliest_clip_start_and_never_after_bar_1() {
    assert_eq!(Session::default().start(), 0, "an empty session");
    assert_eq!(session_with_clip_at(48_000).start(), 0, "no clip before bar 1");
    assert_eq!(session_with_clip_at(0).start(), 0);
    assert_eq!(session_with_clip_at(-96_000).start(), -96_000);
    // The earliest of several, on several tracks.
    let mut s = session_with_clip_at(-12_000);
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(11), "n", -40_000, 1_000, Sequence::default()));
    assert_eq!(s.start(), -40_000);
}

#[test]
fn a_clip_on_an_inactive_playlist_counts_so_a_take_choice_never_moves_the_start() {
    let mut s = session_with_clip_at(0);
    let t = s.tracks[0].id;
    let tr = s.track_mut(t).unwrap();
    tr.playlists.push(Playlist { name: "take 2".into(), clips: vec![Clip::midi(ClipId(12), "p", -24_000, 48_000, Sequence::default())] });
    assert_eq!(tr.active_playlist, 0, "the zone clip is on the inactive playlist");
    assert_eq!(s.start(), -24_000);
    s.track_mut(t).unwrap().active_playlist = 1;
    assert_eq!(s.start(), -24_000, "choosing the other take keeps the session start");
}

#[test]
fn the_pre_count_need_is_listed_exactly_when_the_session_start_is_before_0() {
    for (start, listed) in [(48_000, false), (0, false), (-1, true), (-96_000, true)] {
        let s = session_with_clip_at(start);
        assert_eq!(s.needs().contains(&"pre_count"), listed, "clip at {start}");
        let v: serde_json::Value = serde_json::from_str(&s.to_json().unwrap()).unwrap();
        let written = v.get("needs").and_then(|n| n.as_array()).is_some_and(|a| a.iter().any(|x| x == "pre_count"));
        assert_eq!(written, listed, "clip at {start}: the saved file");
    }
}

#[test]
fn a_session_with_a_zone_round_trips_and_a_build_without_the_entry_warns() {
    let s = session_with_clip_at(-96_000);
    let text = s.to_json().unwrap();
    let back = Session::from_json(&text).unwrap();
    assert!(back.load_notes.is_empty(), "this build knows the need: {:?}", back.load_notes);
    assert_eq!(back.tracks[0].clips(), s.tracks[0].clips(), "the clip start and its notes are unchanged");
    assert_eq!(back.start(), -96_000);
    // A build with the marker and without this entry warns on load (ADR 0023).
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let old = crate::unknown_needs(&v, &PREVIOUS_RELEASE);
    assert_eq!(old, vec!["pre_count".to_string()]);
    let note = crate::unknown_needs_note(&old).unwrap();
    assert!(note.contains("pre_count") && note.contains("saving it from here can drop them"), "{note}");
}
