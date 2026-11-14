//! A MIDI clip's left edge hides; it never destroys (ADR 0052 decision 17), activation (slice 6a):
//! a start trim keeps what it cuts off, a trim back brings it back bit for bit, and playback, the
//! external chase, bounce and copies sound nothing the left edge hides. The hidden events here are
//! made by the real trim command, except where a test reads a session file that already holds them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use serde_json::{Value, json};
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{Clip, ClipContent, ClipId, ClipLoop, EditMode, Session, TrackId};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (a quarter is 24,000 samples and `Q` ticks).
const BAR: Samples = 96_000;
const BEAT: Samples = 24_000;
/// Where the fixture clip starts, and where the trim moves its start: one and a half beats in, and
/// seven samples off the tick grid, so the trim back has to give back exactly the ticks it took.
const START: Samples = 2 * BAR;
const CUT: Samples = START + BEAT + BEAT / 2 + 7;

fn run(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute_with_mode(id, &p, EditMode::Slip).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn cc(tick: i64, value: i32) -> CtrlEvent {
    CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value }
}

/// C3 wholly before the cut; D3 starts before it and is still held after it (hidden whole, decision
/// 17.3); C4, E4 and G4 after it. Two controllers before the cut and one after.
fn fixture() -> Sequence {
    let notes = vec![note(48, 0, Q), note(50, Q / 2, 2 * Q), note(60, 2 * Q, Q), note(64, 4 * Q, Q), note(67, 6 * Q, Q)];
    Sequence { notes, ctrls: vec![cc(0, 10), cc(Q, 20), cc(3 * Q, 30)] }
}

/// An instrument track (its default instrument renders) holding one clip at `start`, two bars long.
fn engine_with(start: Samples, seq: Sequence, looping: Option<ClipLoop>) -> (Engine, TrackId, ClipId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    let s = e.session_mut();
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "m", start, 2 * BAR, seq);
    c.looping = looping;
    crate::edit::place_clip(s, t, c);
    (e, t, id)
}

fn clip(e: &Engine, id: ClipId) -> &Clip {
    e.session().find_clip(id).expect("clip").1
}

fn seq_of(e: &Engine, id: ClipId) -> &Sequence {
    match &clip(e, id).content {
        ClipContent::Midi { sequence } => sequence,
        _ => panic!("not a MIDI clip"),
    }
}

fn negatives(seq: &Sequence) -> (usize, usize) {
    (seq.notes.iter().filter(|n| n.start < 0).count(), seq.ctrls.iter().filter(|c| c.tick < 0).count())
}

/// Every note-on the track's clips play over the first six bars, as (sample, pitch).
fn ons(s: &Session, t: TrackId) -> Vec<(Samples, u8)> {
    transit_mix::clip_note_ons(s, t, 0, 6 * BAR as usize)
}

fn pitches(ons: &[(Samples, u8)]) -> Vec<u8> {
    ons.iter().map(|(_, p)| *p).collect()
}

fn render(s: &Session) -> Vec<Vec<f32>> {
    transit_mix::render_range(s, Range::new(0, 6 * BAR), 512)
}

fn trim(e: &mut Engine, id: ClipId, start: Samples) {
    run(e, "edit.trim_clip", json!({"clip": id.0, "start": start}));
}

/// Whether the clip holds an event its left edge hides, written out here so these tests read the
/// same against the base commit.
fn hides_something(c: &Clip) -> bool {
    let ClipContent::Midi { sequence } = &c.content else { return false };
    !c.looping.is_some_and(|l| l.is_valid()) && (negatives(sequence) != (0, 0))
}

fn needs_hidden(s: &Session) -> bool {
    s.needs().contains(&"midi_hidden_notes")
}

// ---- trim and trim back (decision 17.2) -----------------------------------------------------

#[test]
fn a_start_trim_then_a_trim_back_gives_back_every_note_and_controller_bit_for_bit() {
    let (mut e, t, id) = engine_with(START, fixture(), None);
    let original = clip(&e, id).clone();
    let before = ons(e.session(), t);
    assert_eq!(pitches(&before), vec![48, 50, 60, 64, 67]);
    trim(&mut e, id, CUT);
    assert_eq!(clip(&e, id).start, CUT);
    assert_eq!(seq_of(&e, id).notes.len(), 5, "the trim keeps the notes it cut off");
    assert_eq!(negatives(seq_of(&e, id)), (2, 2), "C3, D3 and two controllers are before the edge");
    // While trimmed, neither the note wholly before the edge nor the one held across it sounds.
    assert_eq!(pitches(&ons(e.session(), t)), vec![60, 64, 67]);
    trim(&mut e, id, START);
    assert_eq!(clip(&e, id), &original, "the clip, its sequence and its controllers are as before the trim");
    assert_eq!(ons(e.session(), t), before, "and it sounds as before");
}

#[test]
fn undo_and_redo_of_a_start_trim_keep_the_hidden_notes() {
    let (mut e, _, id) = engine_with(START, fixture(), None);
    let original = clip(&e, id).clone();
    trim(&mut e, id, CUT);
    let trimmed = clip(&e, id).clone();
    assert!(e.undo());
    assert_eq!(clip(&e, id), &original);
    assert!(e.redo());
    assert_eq!(clip(&e, id), &trimmed);
    assert_eq!(negatives(seq_of(&e, id)), (2, 2), "the redone trim still holds what it hides");
    trim(&mut e, id, START);
    assert_eq!(clip(&e, id), &original, "a trim back after the redo restores the clip");
}

#[test]
fn a_session_saved_with_hidden_notes_and_loaded_again_keeps_them_and_lists_the_need() {
    let (mut e, _, id) = engine_with(START, fixture(), None);
    let original = clip(&e, id).clone();
    trim(&mut e, id, CUT);
    let trimmed = clip(&e, id).clone();
    let dir = transit_test_support::TestDir::new("hidden-edge-save");
    let path = dir.join("trimmed.transit").to_string_lossy().into_owned();
    crate::io::save_session(&mut e, &path).unwrap();
    let text = crate::io::read_text(&path).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["needs"].as_array().is_some_and(|a| a.iter().any(|x| x == "midi_hidden_notes")), "{}", v["needs"]);
    let mut back = Engine::default();
    let notes = crate::io::open_session(&mut back, &path).unwrap();
    assert!(notes.is_empty(), "this build knows the need: {notes:?}");
    assert_eq!(clip(&back, id), &trimmed, "the hidden notes and controllers survive the file");
    trim(&mut back, id, START);
    assert_eq!(clip(&back, id).content, original.content, "and come back with a trim back after loading");
}

// ---- what sounds (decision 17.3) ------------------------------------------------------------

#[test]
fn after_a_trim_playback_bounce_and_a_clip_copy_agree_on_what_sounds() {
    let (mut e, t, id) = engine_with(START, fixture(), None);
    trim(&mut e, id, CUT);
    // The same clip holding only what it shows: what the trimmed clip must sound like.
    let (mut reference, rt, rid) = engine_with(START, Sequence::default(), None);
    {
        let mut shown = clip(&e, id).clone();
        crate::edit::drop_left_hidden(&mut shown);
        shown.id = rid;
        *reference.session_mut().find_clip_mut(rid).unwrap() = shown;
    }
    let played = ons(e.session(), t);
    assert_eq!(pitches(&played), vec![60, 64, 67]);
    assert_eq!(played, ons(reference.session(), rt), "playback");
    let range = Range::new(0, 6 * BAR);
    let consolidated = transit_mix::sequence_as_played(e.session(), e.session().track(t).unwrap(), range).unwrap();
    assert_eq!(
        consolidated,
        transit_mix::sequence_as_played(reference.session(), reference.session().track(rt).unwrap(), range).unwrap(),
        "the MIDI bounce"
    );
    let printed = render(e.session());
    assert!(printed[0].iter().any(|x| x.abs() > 1e-4), "the bounce is not silent");
    assert_eq!(printed, render(reference.session()), "the audio bounce");
    // A copy of the trimmed clip, two bars later, sounds the same notes two bars later and carries
    // nothing hidden, so the session needs nothing for it.
    let out = run(&mut e, "edit.copy_clips", json!({"clips": [id.0], "by": 2 * BAR}));
    let copy = ClipId(out["clips"][0].as_u64().unwrap());
    assert!(!hides_something(clip(&e, copy)));
    let all = ons(e.session(), t);
    let copied: Vec<_> = all.iter().filter(|(at, _)| *at >= CUT + 2 * BAR).copied().collect();
    assert_eq!(copied, played.iter().map(|(at, p)| (at + 2 * BAR, *p)).collect::<Vec<_>>(), "the copy");
}

/// A session file whose unlooped clip holds notes before tick 0 (a start trim of a looped clip
/// followed by Clear Loop writes one; so can a hand edit).
fn loaded_with_hidden_notes() -> (Engine, TrackId, Samples) {
    let seq = Sequence { notes: vec![note(72, -2 * Q, Q), note(61, -Q, 2 * Q), note(60, 0, Q)], ctrls: Vec::new() };
    let (e, t, _) = engine_with(BAR, seq, None);
    let text = e.session().to_json().unwrap();
    (Engine::new(Session::from_json(&text).unwrap()), t, BAR)
}

#[test]
fn a_loaded_clip_with_notes_before_its_left_edge_plays_chases_and_bounces_none_of_them() {
    let (e, t, at) = loaded_with_hidden_notes();
    let s = e.session();
    assert_eq!(ons(s, t), vec![(at, 60)], "only the visible note starts, at the clip's first tick");
    // Half a beat in, the held note's span (when it was clamped to the first tick) still covers
    // the position: the chase holds only the visible note.
    let mut external = s.track(t).unwrap().clone();
    external.midi_output = transit_model::MidiOutput::External {
        port: transit_model::MidiOutPort::default(),
        channel: transit_model::MidiOutChannel::AsRecorded,
        offset_ms: 0.0,
    };
    let mut held = Vec::new();
    transit_mix::external_notes_across(s, &external, at + BEAT / 2, |_, pitch| held.push(pitch));
    assert_eq!(held, vec![60], "the external chase");
    let played = transit_mix::sequence_as_played(s, s.track(t).unwrap(), Range::new(0, 4 * BAR)).unwrap();
    assert_eq!(played.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60], "the MIDI bounce");
    let (reference, _, _) = engine_with(at, Sequence { notes: vec![note(60, 0, Q)], ctrls: Vec::new() }, None);
    assert_eq!(render(s), render(reference.session()), "the audio bounce");
}

#[test]
fn a_visible_note_at_tick_0_under_a_negative_delay_still_sounds_at_tick_0() {
    let (mut e, t, id) = engine_with(START, fixture(), None);
    trim(&mut e, id, START + 2 * BEAT);
    // C3 and the held D3 are hidden; C4 now sits at tick 0, E4 and G4 two and four beats later.
    run(&mut e, "event.midi_rtp", json!({"tracks": [t.0], "delay": -120}));
    let start = clip(&e, id).start;
    let got = ons(e.session(), t);
    assert_eq!(got.first(), Some(&(start, 60)), "clamped to the clip's first tick: {got:?}");
    assert_eq!(pitches(&got), vec![60, 64, 67]);
}

// ---- the need (decision 17.8) and Clear Loop ------------------------------------------------

#[test]
fn the_hidden_notes_need_is_listed_exactly_while_hidden_content_exists() {
    let (mut e, _, id) = engine_with(START, fixture(), None);
    assert!(!needs_hidden(e.session()), "nothing hidden");
    trim(&mut e, id, CUT);
    assert!(needs_hidden(e.session()), "the trim hides two notes and two controllers");
    trim(&mut e, id, START);
    assert!(!needs_hidden(e.session()), "the trim back shows them again");
    // Only a controller hidden.
    let ctrl_only = Sequence { notes: vec![note(60, 4 * Q, Q)], ctrls: vec![cc(0, 1)] };
    let (mut e, _, id) = engine_with(START, ctrl_only, None);
    trim(&mut e, id, START + BEAT);
    assert_eq!(negatives(seq_of(&e, id)), (0, 1));
    assert!(needs_hidden(e.session()), "a hidden controller counts");
}

#[test]
fn clear_loop_on_a_start_trimmed_looped_clip_hides_its_loop_material_and_lists_the_need() {
    // A one-bar loop; the start trimmed two beats in moves the first two beats' notes to negative
    // ticks, where later cycles play them.
    let seq = Sequence { notes: vec![note(48, 0, Q / 2), note(55, Q, Q / 2), note(60, 2 * Q, Q / 2), note(64, 3 * Q, Q / 2)], ctrls: Vec::new() };
    let (mut e, t, id) = engine_with(START, seq, Some(ClipLoop { start: 0, end: BAR }));
    trim(&mut e, id, START + 2 * BEAT);
    assert_eq!(negatives(seq_of(&e, id)), (2, 0));
    assert!(!needs_hidden(e.session()), "a valid loop's negative ticks are loop material, not hidden");
    assert!(pitches(&ons(e.session(), t)).contains(&48), "a later cycle plays them");
    run(&mut e, "clip.clear_loop", json!({"clips": [id.0]}));
    assert!(clip(&e, id).looping.is_none());
    let start = clip(&e, id).start;
    assert_eq!(ons(e.session(), t), vec![(start, 60), (start + BEAT, 64)], "they are silent, not stacked on the first tick");
    assert!(needs_hidden(e.session()), "Clear Loop leaves them before the edge of an unlooped clip");
}

// ---- split aligned with the left-edge rule ---------------------------------------------------

#[test]
fn a_split_of_a_clip_with_an_invalid_loop_is_a_partition() {
    let (mut e, _, id) = engine_with(START, fixture(), None);
    // A loop shorter than `MIN_CYCLE` is no loop: playback and the left-edge rule treat the clip as unlooped.
    e.session_mut().find_clip_mut(id).unwrap().looping = Some(ClipLoop { start: 0, end: 10 });
    let s = e.session();
    let (left, right) = crate::edit::split_clip(clip(&e, id), CUT, ClipId(9_999), crate::edit::TimeCtx::of(s)).unwrap();
    let (ClipContent::Midi { sequence: l }, ClipContent::Midi { sequence: r }) = (&left.content, &right.content) else { panic!() };
    assert_eq!(negatives(r), (0, 0), "the right half carries nothing the left half holds");
    assert_eq!(l.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![48, 50], "the left half keeps its own side");
    assert_eq!(r.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
}
