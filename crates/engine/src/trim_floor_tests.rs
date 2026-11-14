//! The hidden-content trim floor (ADR 0052 decision 7, slice 6b): a MIDI clip's start trim reaches
//! back to the earlier of the session start and the clip's own hidden content, never further; a
//! looped clip's negative ticks are loop material, so its floor is the session start; an audio
//! clip's floor stays at bar 1. Every trim here goes through the real `edit.trim_clip` command.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use crate::edit::start_trim_floor;
use serde_json::{Value, json};
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, ClipLoop, EditMode, Session, TrackId, TrackKind};
use transit_time::{Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (a quarter is 24,000 samples and `Q` ticks).
const BAR: Samples = 96_000;
const BEAT: Samples = 24_000;
/// Ticks in a bar of 4/4.
const BAR_TICKS: i64 = 4 * Q;
/// Far enough left that only the floor stops a trim back.
const FAR: Samples = -10 * BAR;

fn run(e: &mut Engine, mode: EditMode, id: &str, p: Value) -> Value {
    e.execute_with_mode(id, &p, mode).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn trim(e: &mut Engine, id: ClipId, start: Samples) {
    run(e, EditMode::Slip, "edit.trim_clip", json!({"clip": id.0, "start": start}));
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn cc(tick: i64, value: i32) -> CtrlEvent {
    CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(64), value }
}

/// A count-in take as recording makes it: its earliest event is a pedal press at the clip start
/// (tick 0), before the first note; notes before and after bar 1.
fn take() -> Sequence {
    Sequence {
        notes: vec![note(48, Q / 4, Q), note(52, 2 * Q, Q), note(60, BAR_TICKS, Q), note(64, BAR_TICKS + 2 * Q, Q)],
        ctrls: vec![cc(0, 127), cc(BAR_TICKS + Q, 0)],
    }
}

/// A session with one MIDI track per entry of `clips`, each holding one MIDI clip `(start, length,
/// sequence)`; tracks after the first only set the session start.
fn engine(clips: Vec<(Samples, Samples, Sequence)>) -> (Engine, Vec<TrackId>, Vec<ClipId>) {
    let mut s = Session::default();
    let (mut tracks, mut ids) = (Vec::new(), Vec::new());
    for (start, length, seq) in clips {
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", start, length, seq));
        tracks.push(t);
        ids.push(id);
    }
    (Engine::new(s), tracks, ids)
}

fn clip(e: &Engine, id: ClipId) -> &Clip {
    e.session().find_clip(id).expect("clip").1
}

fn seq_of(c: &Clip) -> &Sequence {
    match &c.content {
        ClipContent::Midi { sequence } => sequence,
        _ => panic!("not a MIDI clip"),
    }
}

/// `seq` with every event moved later by `ticks`.
fn later(seq: &Sequence, ticks: i64) -> Sequence {
    let mut s = seq.clone();
    s.notes.iter_mut().for_each(|n| n.start += ticks);
    s.ctrls.iter_mut().for_each(|c| c.tick += ticks);
    s
}

// ---- the hidden-content floor -------------------------------------------------------------------

#[test]
fn a_count_in_take_trimmed_to_bar_1_drags_back_to_its_earliest_event_and_the_zone_returns() {
    let (mut e, _, ids) = engine(vec![(-BAR, 3 * BAR, take())]);
    let id = ids[0];
    let original = clip(&e, id).clone();
    assert_eq!(e.session().start(), -BAR);
    trim(&mut e, id, 0);
    assert_eq!(e.session().start(), 0, "trimmed to bar 1, the zone is gone");
    assert_eq!(start_trim_floor(e.session(), clip(&e, id)), -BAR, "the pedal press at the count-in start");
    trim(&mut e, id, FAR);
    assert_eq!(clip(&e, id).start, -BAR, "the trim back stops at the earliest retained event, not at bar 1");
    assert_eq!(clip(&e, id), &original, "and gives back the take bit for bit");
    assert_eq!(e.session().start(), -BAR, "the zone returns");
}

#[test]
fn with_a_longer_zone_the_trim_back_reaches_the_session_start_and_keeps_every_event_in_place() {
    // Another track's clip sets the session start two bars before the take's earliest event.
    let (mut e, _, ids) = engine(vec![(-BAR, 3 * BAR, take()), (-3 * BAR, BAR, Sequence::default())]);
    let id = ids[0];
    let original = clip(&e, id).clone();
    trim(&mut e, id, 0);
    assert_eq!(start_trim_floor(e.session(), clip(&e, id)), -3 * BAR, "the earlier of the session start and the hidden content");
    trim(&mut e, id, FAR);
    let c = clip(&e, id);
    assert_eq!((c.start, c.end()), (-3 * BAR, original.end()), "stops at the session start");
    assert_eq!(seq_of(c), &later(seq_of(&original), 2 * BAR_TICKS), "every event where it was played, two bars into the clip");
}

#[test]
fn without_hidden_content_a_trim_back_stops_at_the_session_start_with_a_zone_and_at_bar_1_without() {
    let one = || Sequence { notes: vec![note(60, 0, Q)], ctrls: Vec::new() };
    // No zone: bar 1, as before this slice.
    let (mut e, _, ids) = engine(vec![(2 * BAR, BAR, one())]);
    trim(&mut e, ids[0], FAR);
    assert_eq!(clip(&e, ids[0]).start, 0);
    // A clip inside a zone set by another track: the zone's start, never bar 1 (which would trim
    // the clip's start later instead of earlier).
    let (mut e, _, ids) = engine(vec![(-BAR, BAR, one()), (-3 * BAR, BAR, Sequence::default())]);
    trim(&mut e, ids[0], FAR);
    let c = clip(&e, ids[0]);
    assert_eq!((c.start, c.end()), (-3 * BAR, 0));
    assert_eq!(seq_of(c).notes[0].start, 2 * BAR_TICKS, "the note stays where it was");
}

#[test]
fn across_a_tempo_change_the_floor_is_found_in_ticks() {
    // ADR 0052 decision 7's case: 120 BPM in bar 1, 60 BPM from bar 2. A clip from tick -960 (sample
    // -24,000) trimmed to bar 3 (sample 288,000, tick 7,680): its hidden note has relative tick
    // -8,640 and its position is sample -24,000, not 288,000 + tick_to_samples(-8,640) = 72,000.
    let mut s = Session::default();
    s.tempo.set_tempo(BAR_TICKS, 60.0).unwrap();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    crate::edit::place_clip(
        &mut s,
        t,
        Clip::midi(id, "m", -BEAT, 360_000, Sequence { notes: vec![note(60, 0, Q), note(64, 9_600, Q)], ctrls: Vec::new() }),
    );
    let mut e = Engine::new(s);
    let original = clip(&e, id).clone();
    trim(&mut e, id, 288_000);
    assert_eq!(seq_of(clip(&e, id)).notes[0].start, -8_640);
    assert_eq!(start_trim_floor(e.session(), clip(&e, id)), -24_000);
    trim(&mut e, id, FAR);
    assert_eq!(clip(&e, id), &original, "back at sample -24,000 with every note in place");
}

#[test]
fn a_looped_clips_negative_ticks_are_loop_material_so_its_floor_is_the_session_start() {
    let seq = Sequence { notes: vec![note(55, -Q, Q), note(60, 0, Q)], ctrls: Vec::new() };
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "m", 0, 2 * BAR, seq);
    c.looping = Some(ClipLoop { start: 0, end: BAR });
    crate::edit::place_clip(&mut s, t, c);
    let mut e = Engine::new(s);
    let original = clip(&e, id).clone();
    assert_eq!(start_trim_floor(e.session(), &original), 0, "not the material's position, a beat before bar 1");
    trim(&mut e, id, FAR);
    assert_eq!(clip(&e, id), &original, "the start stays at bar 1");
}

#[test]
fn an_audio_clips_start_trim_floor_stays_at_bar_1_with_a_zone() {
    let (e, ..) = engine(vec![(-2 * BAR, BAR, Sequence::default())]);
    let audio = Clip::audio(ClipId(999), "a", transit_model::SourceId(1), 0, BAR, BAR);
    assert_eq!(e.session().start(), -2 * BAR);
    assert_eq!(start_trim_floor(e.session(), &audio), 0, "ADR 0052 decision 11");
}

#[test]
fn a_neighbor_on_the_track_still_stops_the_trim_back() {
    let (mut e, tracks, ids) = engine(vec![(-BAR, 3 * BAR, take())]);
    let id = ids[0];
    trim(&mut e, id, 0);
    // A clip on the same track ending half a bar before the take's hidden content stops it there.
    let s = e.session_mut();
    let n = s.new_clip_id();
    crate::edit::place_clip(s, tracks[0], Clip::midi(n, "n", -3 * BAR, BAR + BAR / 2, Sequence::default()));
    assert_eq!(start_trim_floor(e.session(), clip(&e, id)), -3 * BAR, "the zone reaches the neighbor's start");
    trim(&mut e, id, FAR);
    assert_eq!(clip(&e, id).start, -3 * BAR + BAR + BAR / 2);
}

// ---- edit modes -------------------------------------------------------------------------------

#[test]
fn slip_grid_relative_grid_and_spot_all_stop_at_the_floor() {
    for mode in [EditMode::Slip, EditMode::Grid, EditMode::GridRelative, EditMode::Spot] {
        let (mut e, _, ids) = engine(vec![(-BAR, 3 * BAR, take())]);
        let original = clip(&e, ids[0]).clone();
        run(&mut e, mode, "edit.trim_clip", json!({"clip": ids[0].0, "start": 0}));
        run(&mut e, mode, "edit.trim_clip", json!({"clip": ids[0].0, "start": FAR}));
        assert_eq!(clip(&e, ids[0]), &original, "{mode:?}");
    }
}

#[test]
fn in_shuffle_a_trim_back_reveals_exactly_the_hidden_content_and_no_empty_space() {
    // Shuffle keeps the start where it is and slides the material under it: a trim back reveals
    // the hidden events and stops at the earliest one, so the clip is as it was.
    let (mut e, _, ids) = engine(vec![(0, 2 * BAR, take())]);
    let id = ids[0];
    let original = clip(&e, id).clone();
    run(&mut e, EditMode::Shuffle, "edit.trim_clip", json!({"clip": id.0, "start": BEAT}));
    let c = clip(&e, id);
    assert_eq!((c.start, c.length), (0, 2 * BAR - BEAT));
    assert_eq!(seq_of(c).ctrls[0].tick, -Q, "the pedal press is hidden a beat before the edge");
    assert_eq!(start_trim_floor(e.session(), c), -BEAT);
    run(&mut e, EditMode::Shuffle, "edit.trim_clip", json!({"clip": id.0, "start": FAR}));
    assert_eq!(clip(&e, id), &original);
}

// ---- undo, redo and the file ------------------------------------------------------------------

#[test]
fn undo_redo_and_a_saved_file_keep_what_a_trim_to_the_floor_restores() {
    let (mut e, _, ids) = engine(vec![(-BAR, 3 * BAR, take())]);
    let id = ids[0];
    let original = clip(&e, id).clone();
    trim(&mut e, id, 0);
    let trimmed = clip(&e, id).clone();
    trim(&mut e, id, FAR);
    assert!(e.undo());
    assert_eq!(clip(&e, id), &trimmed);
    assert_eq!(e.session().start(), 0);
    assert!(e.redo());
    assert_eq!(clip(&e, id), &original);
    assert_eq!(e.session().start(), -BAR, "redo brings the zone back");
    // The trimmed state through a session file: the floor is the same after loading.
    assert!(e.undo());
    let dir = transit_test_support::TestDir::new("trim-floor-save");
    let path = dir.join("trimmed.transit").to_string_lossy().into_owned();
    crate::io::save_session(&mut e, &path).unwrap();
    let mut back = Engine::default();
    let notes = crate::io::open_session(&mut back, &path).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(clip(&back, id), &trimmed);
    trim(&mut back, id, FAR);
    assert_eq!(clip(&back, id), &original, "a trim back after loading reaches the same floor");
}
