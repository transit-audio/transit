//! A MIDI clip's left edge hides (ADR 0052 decision 17), engine preparation (slice 4): the walkers
//! that act on what a clip holds leave alone, or never carry, an event that an unlooped clip
//! stores before tick 0. Nothing in a session made by transit holds such an event yet, so every
//! clip here is built with negative ticks directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use crate::midi_take::Built;
use serde_json::{Value, json};
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, ClipLoop, EditMode, Session, TrackId, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (a quarter is 24,000 samples and `Q` ticks).
const BAR: Samples = 96_000;
const BEAT: Samples = 24_000;

fn run(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute_with_mode(id, &p, EditMode::Slip).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn refused(e: &mut Engine, id: &str, p: Value) -> String {
    match e.execute_with_mode(id, &p, EditMode::Slip) {
        Ok(v) => panic!("{id} {p} was accepted: {v}"),
        Err(err) => err.to_string(),
    }
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn cc(tick: i64, value: i32) -> CtrlEvent {
    CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value }
}

/// A MIDI track holding one clip at `start`, `length` long, with `seq` as stored.
fn session_with(start: Samples, length: Samples, seq: Sequence, looping: Option<ClipLoop>) -> (Session, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "m", start, length, seq);
    c.looping = looping;
    crate::edit::place_clip(&mut s, t, c);
    (s, t, id)
}

/// A hidden note (pitch 61) and a hidden controller before the left edge, two visible notes and a
/// visible controller after it.
fn trimmed() -> Sequence {
    let mut seq = Sequence { notes: vec![note(61, -Q, Q / 2), note(60, 0, Q), note(64, Q, Q)], ctrls: vec![cc(-10, 5), cc(10, 90)] };
    seq.sort();
    seq
}

fn seq_of(s: &Session, id: ClipId) -> &Sequence {
    match &s.find_clip(id).expect("clip").1.content {
        ClipContent::Midi { sequence } => sequence,
        _ => panic!("not a MIDI clip"),
    }
}

fn negatives(seq: &Sequence) -> (usize, usize) {
    (seq.notes.iter().filter(|n| n.start < 0).count(), seq.ctrls.iter().filter(|c| c.tick < 0).count())
}

/// The left-edge rule, written out here so these tests read the same against the base commit.
fn hid(c: &Clip, tick: i64) -> bool {
    tick < 0 && !c.looping.is_some_and(|l| l.is_valid())
}

fn hidden_note(s: &Session, id: ClipId) -> Note {
    *seq_of(s, id).notes.iter().find(|n| n.pitch == 61).expect("the hidden note is still stored")
}

// ---- copies (decision 17.10) ----------------------------------------------------------------

#[test]
fn a_range_copy_over_a_whole_clip_carries_nothing_its_left_edge_hides_and_neither_does_its_paste() {
    let (s, t, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let parts = crate::edit::copy_range(&s, t, Range::new(0, 4 * BAR));
    assert_eq!(parts.len(), 1);
    let ClipContent::Midi { sequence } = &parts[0].content else { panic!() };
    assert_eq!(negatives(sequence), (0, 0), "the copy carries no hidden event");
    assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(60, 0), (64, Q)]);
    assert_eq!(sequence.ctrls, vec![cc(10, 90)]);

    let mut e = Engine::new(s);
    run(&mut e, "edit.copy", json!({"tracks": [t.0], "start": 0, "end": 4 * BAR}));
    run(&mut e, "edit.paste", json!({"tracks": [t.0], "at": 8 * BAR}));
    let copy = e.session().track(t).unwrap().clips().iter().find(|c| c.start == 8 * BAR + BAR).map(|c| c.id).expect("the pasted clip");
    assert_eq!(negatives(seq_of(e.session(), copy)), (0, 0), "the paste carries no hidden event");
    assert_eq!(negatives(seq_of(e.session(), id)), (1, 1), "the original keeps what it hides");
}

#[test]
fn copying_bars_5_and_6_of_a_16_bar_clip_carries_no_notes_before_its_start() {
    let mut seq = Sequence::default();
    for bar in 0..16 {
        seq.notes.push(note(60 + bar as u8, bar * 4 * Q, Q));
    }
    let (s, t, _) = session_with(0, 16 * BAR, seq, None);
    let parts = crate::edit::copy_range(&s, t, Range::new(4 * BAR, 6 * BAR));
    let ClipContent::Midi { sequence } = &parts[0].content else { panic!() };
    assert_eq!(negatives(sequence), (0, 0));
    // Bars 7 to 16 stay in the copy past its end, hidden by its right edge, as today.
    assert_eq!(sequence.notes.first().map(|n| (n.pitch, n.start)), Some((64, 0)));
    assert_eq!(sequence.notes.len(), 12);
    assert_eq!(parts[0].length, 2 * BAR);
}

#[test]
fn a_whole_clip_copy_carries_nothing_the_left_edge_hides() {
    let (s, t, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let mut e = Engine::new(s);
    let out = run(&mut e, "edit.copy_clips", json!({"clips": [id.0], "by": 4 * BAR}));
    let copy = ClipId(out["clips"][0].as_u64().unwrap());
    assert_eq!(e.session().find_clip(copy).unwrap().0, t);
    assert_eq!(negatives(seq_of(e.session(), copy)), (0, 0), "the copy carries no hidden event");
    assert_eq!(seq_of(e.session(), copy).notes.len(), 2);
    assert_eq!(negatives(seq_of(e.session(), id)), (1, 1), "the original keeps what it hides");
}

#[test]
fn a_looped_clip_copy_keeps_its_negative_ticks_for_later_cycles() {
    let lp = Some(ClipLoop { start: 0, end: BAR });
    let (s, _, id) = session_with(BAR, 4 * BAR, trimmed(), lp);
    let mut e = Engine::new(s);
    let out = run(&mut e, "edit.copy_clips", json!({"clips": [id.0], "by": 8 * BAR}));
    let copy = ClipId(out["clips"][0].as_u64().unwrap());
    assert_eq!(negatives(seq_of(e.session(), copy)), (1, 1));
}

#[test]
fn extract_midi_copies_carry_nothing_the_left_edge_hides() {
    let (s, t, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let mut e = Engine::new(s);
    let out = run(&mut e, "track.extract_midi", json!({"tracks": [t.0]}));
    let nt = TrackId(out["tracks"][0]["track"].as_u64().unwrap());
    let copy = e.session().track(nt).unwrap().clips()[0].id;
    assert_eq!(negatives(seq_of(e.session(), copy)), (0, 0));
    assert_eq!(negatives(seq_of(e.session(), id)), (1, 1));
}

// ---- the bake (decision 17.9) ---------------------------------------------------------------

#[test]
fn the_bake_leaves_a_hidden_note_as_it_is_and_clamps_a_visible_note_at_tick_0_as_playback_does() {
    let (s, t, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let mut e = Engine::new(s);
    run(&mut e, "event.midi_rtp", json!({"tracks": [t.0], "delay": -120, "velocity": 10}));
    let out = run(&mut e, "track.write_midi_rtp", json!({"tracks": [t.0]}));
    assert_eq!(out["notes"], 2, "only the visible notes are baked");
    assert_eq!(hidden_note(e.session(), id), note(61, -Q, Q / 2), "the hidden note is unchanged");
    let visible: Vec<_> = seq_of(e.session(), id).notes.iter().filter(|n| n.pitch != 61).map(|n| (n.pitch, n.start, n.velocity)).collect();
    // The note at tick 0 under Delay -120 still sounds at tick 0, as playback clamps it.
    assert_eq!(visible, vec![(60, 0, 110), (64, Q - 120, 110)]);
}

#[test]
fn the_bake_of_a_looped_clip_sounds_as_the_clip_did_before_it() {
    let seq = Sequence { notes: vec![note(60, 0, Q / 2), note(62, Q, Q / 2), note(64, 2 * Q, Q / 2)], ctrls: Vec::new() };
    let (s, t, id) = session_with(BAR, 4 * BAR, seq, Some(ClipLoop { start: 0, end: BAR }));
    let mut e = Engine::new(s);
    run(&mut e, "event.midi_rtp", json!({"tracks": [t.0], "delay": -120}));
    let before = transit_mix::clip_note_ons(e.session(), t, 0, 6 * BAR as usize);
    run(&mut e, "track.write_midi_rtp", json!({"tracks": [t.0]}));
    let after = transit_mix::clip_note_ons(e.session(), t, 0, 6 * BAR as usize);
    assert!(!before.is_empty());
    assert_eq!(after, before, "a valid looped clip bakes as playback plays it: no clamp at tick 0");
    assert_eq!(seq_of(e.session(), id).notes[0].start, -120);
}

// ---- note and controller commands -----------------------------------------------------------

#[test]
fn note_commands_never_address_a_hidden_note() {
    let (s, _, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let mut e = Engine::new(s);
    // Index 0 is the hidden note (sorted first by its negative start).
    assert_eq!(seq_of(e.session(), id).notes[0].pitch, 61);
    let c = id.0;
    // Ignored like an index out of range where the command ignores one.
    run(&mut e, "midi.notes_move", json!({"clip": c, "indices": [0, 1], "ticks": -10, "semitones": 1}));
    assert_eq!(hidden_note(e.session(), id), note(61, -Q, Q / 2), "notes_move leaves the hidden note alone");
    assert!(seq_of(e.session(), id).notes.iter().any(|n| (n.pitch, n.start) == (61, 0)), "the visible note moved up a semitone and clamped at 0");
    run(&mut e, "midi.note_delete", json!({"clip": c, "indices": [0]}));
    assert_eq!(hidden_note(e.session(), id), note(61, -Q, Q / 2), "note_delete leaves the hidden note alone");
    // Refused like an index out of range where the command refuses one.
    refused(&mut e, "midi.note_edit", json!({"clip": c, "index": 0, "velocity": 20}));
    refused(&mut e, "midi.notes_transpose", json!({"clip": c, "indices": [0], "semitones": 2}));
    refused(&mut e, "midi.notes_constrain", json!({"clip": c, "indices": [0], "root": 0, "scale": "major"}));
    refused(&mut e, "midi.notes_move", json!({"clip": c, "indices": [0], "pitches": [70], "ticks": 2 * Q}));
    refused(&mut e, "event.quantize", json!({"clips": [c], "indices": [0], "grid": "1/4"}));
    assert_eq!(hidden_note(e.session(), id), note(61, -Q, Q / 2));
    assert_eq!(seq_of(e.session(), id).notes.len(), 3);
}

#[test]
fn a_looped_clips_negative_note_is_loop_material_and_stays_addressable() {
    let (s, _, id) = session_with(BAR, 4 * BAR, trimmed(), Some(ClipLoop { start: 0, end: BAR }));
    let mut e = Engine::new(s);
    run(&mut e, "midi.notes_transpose", json!({"clip": id.0, "indices": [0], "semitones": 2}));
    assert_eq!(seq_of(e.session(), id).notes[0], note(63, -Q, Q / 2));
}

#[test]
fn cc_commands_never_address_a_hidden_point() {
    let (s, _, id) = session_with(BAR, 2 * BAR, trimmed(), None);
    let mut e = Engine::new(s);
    assert_eq!(seq_of(e.session(), id).ctrls[0], cc(-10, 5));
    refused(&mut e, "midi.cc_delete", json!({"clip": id.0, "index": 0}));
    refused(&mut e, "midi.cc_edit", json!({"clip": id.0, "index": 0, "tick": 5, "value": 9}));
    assert_eq!(seq_of(e.session(), id).ctrls, vec![cc(-10, 5), cc(10, 90)]);
    // The visible point is still addressed.
    run(&mut e, "midi.cc_edit", json!({"clip": id.0, "index": 1, "value": 9}));
    assert_eq!(seq_of(e.session(), id).ctrls, vec![cc(-10, 5), cc(10, 9)]);
}

#[test]
fn whole_clip_note_operations_leave_hidden_notes_untouched() {
    // Off the quarter grid, so a quantize would move it; one beat from the next note, so legato would lengthen it.
    let mut seq = trimmed();
    seq.notes[0] = note(61, -Q - 50, Q / 2);
    let (s, _, id) = session_with(BAR, 2 * BAR, seq, None);
    let mut e = Engine::new(s);
    let c = id.0;
    let hidden = note(61, -Q - 50, Q / 2);
    let out = run(&mut e, "event.quantize", json!({"clips": [c], "grid": "1/4"}));
    assert_eq!(out["notes"], 2, "only the visible notes are counted");
    assert_eq!(hidden_note(e.session(), id), hidden, "quantize");
    run(&mut e, "event.transpose", json!({"clips": [c], "semitones": 2}));
    assert_eq!(hidden_note(e.session(), id), hidden, "transpose");
    run(&mut e, "event.change_velocity", json!({"clips": [c], "set": 30}));
    assert_eq!(hidden_note(e.session(), id), hidden, "velocity");
    run(&mut e, "event.change_duration", json!({"clips": [c], "legato": 0}));
    assert_eq!(hidden_note(e.session(), id), hidden, "legato");
    let visible: Vec<_> = seq_of(e.session(), id).notes.iter().filter(|n| n.start >= 0).map(|n| (n.pitch, n.velocity)).collect();
    assert_eq!(visible, vec![(62, 30), (66, 30)], "the visible notes were changed");
}

#[test]
fn a_step_input_tie_does_not_extend_a_hidden_note() {
    let seq = Sequence { notes: vec![note(60, -Q, 2 * Q)], ctrls: Vec::new() };
    let (s, t, id) = session_with(0, 2 * BAR, seq, None);
    let mut e = Engine::new(s);
    refused(&mut e, "event.step_input", json!({"track": t.0, "at": Q, "pitches": [60], "tie": true, "duration": Q}));
    assert_eq!(seq_of(e.session(), id).notes, vec![note(60, -Q, 2 * Q)]);
}

#[test]
fn extract_chords_does_not_hear_a_hidden_note() {
    // A C major bar at bar 2; a D minor triad the left edge hides fills bar 1.
    let notes =
        vec![note(62, -4 * Q, 4 * Q), note(65, -4 * Q, 4 * Q), note(69, -4 * Q, 4 * Q), note(60, 0, 4 * Q), note(64, 0, 4 * Q), note(67, 0, 4 * Q)];
    let mut seq = Sequence { notes, ctrls: Vec::new() };
    seq.sort();
    let (s, t, _) = session_with(BAR, BAR, seq, None);
    let mut e = Engine::new(s);
    let out = run(&mut e, "event.extract_chords", json!({"tracks": [t.0], "start": 0, "end": 2 * BAR}));
    let bars: Vec<_> = out["chords"].as_array().unwrap().iter().map(|c| c["bar"].as_i64().unwrap()).collect();
    assert_eq!(bars, vec![2], "only the visible bar has a chord: {out}");
}

#[test]
fn tab_to_transients_does_not_stop_at_a_hidden_note() {
    // The hidden note ends one beat into the clip; the next visible note starts two beats in.
    let seq = Sequence { notes: vec![note(61, -Q, 2 * Q), note(60, 2 * Q, Q)], ctrls: Vec::new() };
    let (mut s, t, _) = session_with(BAR, BAR, seq, None);
    s.edit.tab_to_transient = true;
    s.edit.selected_tracks = vec![t];
    s.edit.selection = Range::point(BAR + 1);
    let mut e = Engine::new(s);
    run(&mut e, "edit.tab", json!({}));
    assert_eq!(e.session().edit.selection.start, BAR + 2 * BEAT);
}

// ---- walkers that already skip, or deliberately keep, hidden events --------------------------

#[test]
fn the_score_leaves_out_a_hidden_note() {
    let (s, _, _) = session_with(BAR, BAR, trimmed(), None);
    let parts = crate::score::layout(&s, &[]);
    let pitches: Vec<u8> = parts[0].measures.iter().flat_map(|m| m.items.iter()).flat_map(|i| i.pitches.iter().copied()).collect();
    assert!(pitches.contains(&60) && pitches.contains(&64));
    assert!(!pitches.contains(&61), "the hidden note is not engraved: {pitches:?}");
}

#[test]
fn an_overdub_that_moves_the_start_earlier_sounds_the_hidden_notes_it_passes() {
    // An existing clip at bar 3 hides a note one beat before its edge and one two bars before it.
    let seq = Sequence { notes: vec![note(50, -8 * Q, Q), note(61, -Q, Q / 2), note(60, 0, Q)], ctrls: Vec::new() };
    let (mut s, t, id) = session_with(2 * BAR, BAR, seq, None);
    // A take one bar earlier merges into it.
    let built = Built { track: t, pass: 0, range: Range::new(BAR, 2 * BAR), sequence: Sequence { notes: vec![note(72, 0, Q)], ctrls: Vec::new() } };
    let (tempo, sr) = (s.tempo.clone(), s.sample_rate);
    crate::midi_take::merge(&mut s, id, &built, &tempo, sr);
    let c = s.find_clip(id).unwrap().1;
    assert_eq!(c.start, BAR);
    let got: Vec<_> = seq_of(&s, id).notes.iter().map(|n| (n.pitch, n.start, hid(c, n.start))).collect();
    // The note a beat before the old edge now sounds three beats into the clip; the one two bars
    // before it is still before the new edge.
    assert_eq!(got, vec![(50, -4 * Q, true), (72, 0, false), (61, 3 * Q, false), (60, 4 * Q, false)]);
}

#[test]
fn an_incoming_count_in_controller_is_hidden_by_the_final_edge_only() {
    // The take starts where the clip starts: its count-in controller stays before the edge.
    let seq = Sequence { notes: vec![note(60, 0, Q)], ctrls: Vec::new() };
    let take = |start: Samples| Built {
        track: TrackId(0),
        pass: 0,
        range: Range::new(start, start + BAR),
        sequence: Sequence { notes: vec![note(72, 0, Q)], ctrls: vec![cc(-Q, 64)] },
    };
    let (mut s, _, id) = session_with(BAR, BAR, seq.clone(), None);
    let (tempo, sr) = (s.tempo.clone(), s.sample_rate);
    crate::midi_take::merge(&mut s, id, &take(BAR), &tempo, sr);
    let c = s.find_clip(id).unwrap().1;
    assert_eq!(seq_of(&s, id).ctrls, vec![cc(-Q, 64)]);
    assert!(hid(c, -Q));

    // The take starts two bars after an existing clip: rebased into it, its controller sounds.
    let (mut s, _, id) = session_with(BAR, 3 * BAR, seq, None);
    crate::midi_take::merge(&mut s, id, &take(3 * BAR), &tempo, sr);
    let c = s.find_clip(id).unwrap().1;
    assert_eq!(c.start, BAR);
    assert_eq!(seq_of(&s, id).ctrls, vec![cc(7 * Q, 64)]);
    assert!(!hid(c, 7 * Q));
}
