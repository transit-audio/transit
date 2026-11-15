//! Stored-selection readers and the pre-count zone (ADR 0052 decision 7, slice 7). Each reader
//! takes a range in a zone as it is, and floors a stored range that starts before the session
//! start (its zone gone) at the session start. 120 bpm, 4/4, 48 kHz: a bar is 96,000 samples.

#![allow(clippy::unwrap_used)]

use crate::Engine;
use serde_json::{Value, json};
use transit_midi::Sequence;
use transit_model::{ChannelFormat, Clip, Session, TrackId, TrackKind};
use transit_time::{Range, Samples};

const BAR: Samples = 96_000;

/// An engine with a MIDI track; a one-bar clip at `zone` (before bar 1) when given, and one at bar
/// 1 either way.
fn engine(zone: Option<Samples>) -> (Engine, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    for at in zone.into_iter().chain([0]) {
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", at, BAR, Sequence::default()));
    }
    (Engine::new(s), t)
}

fn run(e: &mut Engine, cmd: &str, p: Value) -> Value {
    e.execute(cmd, &p).unwrap()
}

fn select(e: &mut Engine, r: Range) {
    e.session_mut().edit.selection = r;
}

#[test]
fn select_all_starts_at_the_session_start() {
    let (mut e, _) = engine(Some(-BAR));
    run(&mut e, "edit.select_all", json!({}));
    assert_eq!(e.session().edit.selection, Range::new(-BAR, BAR), "the zone is part of Select All");
}

#[test]
fn zoom_to_selection_and_zoom_toggle_scroll_to_the_selection_read_from_the_session_start() {
    for cmd in ["view.zoom_to_selection", "view.zoom_toggle"] {
        let (mut e, _) = engine(None);
        select(&mut e, Range::new(-BAR, BAR));
        run(&mut e, cmd, json!({}));
        assert_eq!(e.session().edit.zoom.scroll, 0, "{cmd}: the zone has gone, the view starts at bar 1");
        let (mut e, _) = engine(Some(-BAR));
        select(&mut e, Range::new(-BAR / 2, BAR));
        run(&mut e, cmd, json!({}));
        assert_eq!(e.session().edit.zoom.scroll, -BAR / 2, "{cmd}: a selection in the zone");
    }
}

#[test]
fn cycle_to_selection_takes_the_selection_from_the_session_start() {
    let (mut e, _) = engine(None);
    select(&mut e, Range::new(-BAR, BAR));
    run(&mut e, "transport.cycle_to_selection", json!({}));
    assert_eq!(e.session().edit.loop_range, Range::new(0, BAR));
}

#[test]
fn fast_forward_moves_from_the_insertion_point_read_from_the_session_start() {
    let (mut e, _) = engine(None);
    select(&mut e, Range::point(-BAR));
    assert_eq!(run(&mut e, "transport.fast_forward", json!({"seconds": 1})), json!({"at": 48_000}));
    let (mut e, _) = engine(Some(-BAR));
    select(&mut e, Range::point(-BAR));
    assert_eq!(run(&mut e, "transport.fast_forward", json!({"seconds": 1})), json!({"at": -BAR + 48_000}), "inside a zone");
}

/// A range command without `start` and `end` takes the selection from the session start.
#[test]
fn a_range_command_defaults_to_the_selection_from_the_session_start() {
    let (mut e, t) = engine(None);
    select(&mut e, Range::new(-BAR, BAR));
    run(&mut e, "edit.duplicate", json!({"tracks": [t.0]}));
    assert_eq!(e.session().edit.selection, Range::new(BAR, 2 * BAR), "duplicated bar 1 only");
}

#[test]
fn current_timecode_reads_the_insertion_point_in_the_zone() {
    let (mut e, _) = engine(Some(-BAR));
    select(&mut e, Range::point(-BAR));
    let r = run(&mut e, "setup.current_timecode", json!({"timecode": "01:00:00:00"}));
    assert_eq!(r["at"], -BAR);
    let abs = transit_time::Timecode::parse("01:00:00:00").unwrap().to_samples(e.session().frame_rate, e.session().sample_rate);
    assert_eq!(e.session().timecode_start, abs + BAR, "the zone's first bar reads 01:00:00:00");
}

#[test]
fn play_timeline_plays_the_selection_from_the_session_start() {
    let (mut e, _) = engine(None);
    select(&mut e, Range::new(-BAR, BAR));
    assert_eq!(run(&mut e, "edit.play_timeline", json!({})), json!({"start": 0, "end": BAR}));
}

/// A recording placed without the host's own loop wraps at the cycle range Play loops, from the
/// session start.
#[test]
fn a_take_placed_under_a_stored_cycle_before_the_session_start_wraps_at_the_floored_loop() {
    use crate::midi_take::{Msg, RecEvent, place_midi_recording};
    let mut e = Engine::default();
    let t = TrackId(run(&mut e, "track.new", json!({"kind": "midi"}))["tracks"][0].as_u64().unwrap());
    e.session_mut().edit.loop_playback = true;
    e.session_mut().edit.loop_range = Range::new(-BAR, BAR);
    let ev = |offset, msg| RecEvent { track: t, channel: 0, offset, msg };
    let events = [
        ev(1_000, Msg::NoteOn { pitch: 60, velocity: 90 }),
        ev(2_000, Msg::NoteOff { pitch: 60, velocity: 0 }),
        ev(BAR + 1_000, Msg::NoteOn { pitch: 62, velocity: 90 }),
        ev(BAR + 2_000, Msg::NoteOff { pitch: 62, velocity: 0 }),
    ];
    let plan = crate::io::Recording { count_in: Some(0), ..crate::io::Recording::new(0) };
    let ids = place_midi_recording(&mut e, &plan, &events, 0);
    assert_eq!(ids.len(), 2, "a take per lap");
    let range = |id| e.session().track(t).unwrap().playlists.iter().flat_map(|p| &p.clips).find(|c| c.id == id).unwrap().range();
    assert_eq!((range(ids[0]), range(ids[1])), (Range::new(0, BAR), Range::new(0, BAR)), "both laps lie over the loop Play loops");
}
