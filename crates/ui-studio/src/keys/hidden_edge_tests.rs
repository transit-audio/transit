//! A MIDI clip's left edge hides (ADR 0052 decision 17), piano roll preparation (slice 5): the roll
//! draws, hit-tests, selects, copies and edits no note or controller an unlooped clip stores before
//! tick 0, and every command it sends addresses the sequence index of the note it means. Nothing
//! in a session made by transit holds such an event yet, so every clip here gets its hidden
//! events written into the document directly.

use super::*;
use transit_midi::{CtrlEvent, CtrlKind};

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn cc1(tick: i64, value: i32) -> CtrlEvent {
    CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value }
}

/// Write `notes` and `ctrls` into `clip` as stored, sorted the way the engine keeps a sequence.
fn store(t: &mut T, clip: ClipId, notes: &[Note], ctrls: &[CtrlEvent]) {
    let c = t.app.engine.session_mut().find_clip_mut(clip).unwrap();
    let ClipContent::Midi { sequence } = &mut c.content else { panic!("not a MIDI clip") };
    sequence.notes.extend_from_slice(notes);
    sequence.ctrls.extend_from_slice(ctrls);
    sequence.sort();
    t.frames(3);
}

fn ctrls_of(t: &T, clip: ClipId) -> Vec<CtrlEvent> {
    match &t.app.engine.session().find_clip(clip).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.ctrls.clone(),
        _ => Vec::new(),
    }
}

/// Every note as (pitch, start), in sequence order.
fn spots(notes: &[Note]) -> Vec<(u8, i64)> {
    notes.iter().map(|n| (n.pitch, n.start)).collect()
}

/// The two hidden notes: one wholly before the left edge, one that starts before it and is still
/// held after it (hidden whole, decision 17.3).
const HIDDEN: [(u8, i64, i64); 2] = [(72, -470, 240), (67, -240, 480)];

fn hidden() -> Vec<Note> {
    HIDDEN.iter().map(|&(p, s, l)| note(p, s, l)).collect()
}

/// The open one-bar clip holding two visible notes and both hidden notes.
fn open_with_hidden(visible: &[(u8, i64)]) -> T {
    let mut t = T::open();
    let clip = t.clip;
    let mut notes = hidden();
    notes.extend(visible.iter().map(|&(p, s)| note(p, s, 480)));
    store(&mut t, clip, &notes, &[]);
    t
}

fn hidden_untouched(t: &T) {
    let n = t.notes();
    for h in hidden() {
        assert!(n.contains(&h), "the hidden note {h:?} is unchanged: {n:?}");
    }
}

#[test]
fn the_roll_draws_hit_tests_and_selects_no_note_before_the_left_edge() {
    let mut t = open_with_hidden(&[(60, 0), (64, 480)]);
    let drawn: Vec<_> = t.app.keys.layout.notes.iter().map(|n| (n.pitch, n.start, n.index)).collect();
    assert_eq!(drawn, vec![(60, 0, 0), (64, 480, 1)], "only the visible notes are drawn, indexed by what is drawn");
    // Where the held hidden note's tail would be drawn (pitch 67, ticks 0..240), a click hits nothing.
    t.click(t.at(120.0, 67));
    assert_eq!(t.selected(), 0, "the hidden note is not hit");
    assert!(t.app.keys.selection.is_empty());
    t.key("A", json!({"cmd": true}));
    assert_eq!(spots(&t.app.keys.selection), vec![(60, 0), (64, 480)], "Select All takes the visible notes only");
    assert_eq!(t.selected(), 2);
}

#[test]
fn a_looped_clips_negative_tick_note_is_loop_material_and_stays_in_the_roll() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.loop_to_bars(0, 2);
    let clip = t.clip;
    store(&mut t, clip, &[note(72, -480, 240)], &[]);
    let drawn: Vec<_> = t.app.keys.layout.notes.iter().map(|n| (n.pitch, n.start)).collect();
    assert_eq!(drawn, vec![(72, -480), (60, 0)], "a valid loop's negative ticks are not hidden");
}

#[test]
fn select_all_quantize_duplicate_transpose_and_delete_leave_hidden_notes_untouched() {
    let mut t = open_with_hidden(&[(60, 10), (64, 490)]);
    // Ctrl+A then Ctrl+U: the visible notes snap; the hidden ones stay where they are.
    t.key("A", json!({"cmd": true}));
    t.key("U", json!({"cmd": true}));
    assert_eq!(t.app.engine.undo_history().last().map(String::as_str), Some("Quantize..."), "{}", t.app.status);
    hidden_untouched(&t);
    assert!(t.notes().iter().any(|n| (n.pitch, n.start) == (60, 0)) && t.notes().iter().any(|n| (n.pitch, n.start) == (64, 480)), "{:?}", t.notes());
    // Quantize with nothing selected: every visible note, still no hidden one.
    t.app.run("midi.notes_move", json!({"clip": t.clip.0, "indices": [2, 3], "ticks": 30})).unwrap();
    t.frames(2);
    t.click(t.at(1500.0, 62));
    assert!(t.app.keys.selection.is_empty());
    t.key("U", json!({"cmd": true}));
    assert_eq!(spots(&t.notes()[2..]), vec![(60, 0), (64, 480)], "{}", t.app.status);
    hidden_untouched(&t);
    // Ctrl+A then Ctrl+D: copies of the visible notes only, so no audible copy of a hidden one.
    t.key("A", json!({"cmd": true}));
    t.key("D", json!({"cmd": true}));
    let n = t.notes();
    assert_eq!(n.len(), 6, "two hidden, two visible, two copies: {n:?}");
    assert_eq!(n.iter().filter(|n| n.pitch == 72 || n.pitch == 67).count(), 2, "no copy of a hidden note: {n:?}");
    hidden_untouched(&t);
    // Ctrl+Up on one note (`midi.notes_transpose`), then on everything (`event.transpose`): the
    // hidden notes keep their pitch.
    t.click(mid(t.note_rect(60, 0)));
    t.key("ArrowUp", json!({"cmd": true}));
    assert_eq!(spots(&t.notes()[2..]), vec![(72, 0), (64, 480), (60, 960), (64, 1440)], "{}", t.app.status);
    hidden_untouched(&t);
    t.key("A", json!({"cmd": true}));
    t.key("ArrowUp", json!({"cmd": true}));
    assert_eq!(spots(&t.notes()[2..]), vec![(84, 0), (76, 480), (72, 960), (76, 1440)], "{}", t.app.status);
    hidden_untouched(&t);
    // Ctrl+A then Delete: every visible note goes, the hidden ones stay.
    t.key("A", json!({"cmd": true}));
    t.key("Delete", json!({}));
    let mut left = t.notes();
    left.sort_by_key(|n| n.start);
    assert_eq!(left, hidden(), "only the hidden notes are left");
}

#[test]
fn an_edit_of_a_visible_note_after_a_hidden_one_addresses_its_own_engine_index() {
    // Sequence: the two hidden notes at indices 0 and 1, then C4 at 2 and E4 at 3.
    let mut t = open_with_hidden(&[(60, 0), (64, 480)]);
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (60, 0), (64, 480)]);
    // A velocity edit of E4 (view index 1, sequence index 3).
    let e4 = t.notes()[3];
    t.app.keys_edit(&[(e4, Note { velocity: 30, ..e4 })]).unwrap();
    t.frames(2);
    assert_eq!(t.notes().iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![100, 100, 100, 30], "E4 was edited, nothing else");
    // Up a semitone on C4 (view index 0, sequence index 2).
    t.click(mid(t.note_rect(60, 0)));
    t.key("ArrowUp", json!({}));
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (61, 0), (64, 480)], "{}", t.app.status);
    // Delete E4.
    t.click(mid(t.note_rect(64, 480)));
    t.key("Delete", json!({}));
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (61, 0)]);
    // Constrain to the clip's key (C major): C#4 goes down to C4.
    t.click(mid(t.note_rect(61, 0)));
    t.app.keys_constrain().unwrap();
    t.frames(2);
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (60, 0)], "{}", t.app.status);
}

#[test]
fn step_back_takes_back_its_own_step_in_a_clip_holding_hidden_notes() {
    let mut t = T::open();
    let clip = t.clip;
    store(&mut t, clip, &hidden(), &[]);
    t.step_on();
    t.tap(60, 100);
    t.tap(62, 100);
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (60, 0), (62, SIXTEENTH)]);
    t.key("Backspace", json!({}));
    assert_eq!(spots(&t.notes()), vec![(72, -470), (67, -240), (60, 0)], "the last step's note is gone: {}", t.app.status);
    t.key("Backspace", json!({}));
    assert_eq!(t.notes(), hidden(), "{}", t.app.status);
}

/// The point in the controller lane at `tick` (from the clip start) and value `v`.
fn lane_at(t: &T, tick: i64, v: i32) -> (f32, f32) {
    let l = &t.app.keys.layout;
    let x = l.velocity[0] + ((tick as f64 - l.scroll_ticks) * l.px_per_tick) as f32;
    (x, l.velocity[3] - 4.0 - (l.velocity[3] - l.velocity[1] - 8.0) * v as f32 / 127.0)
}

#[test]
fn the_cc_lane_shows_and_edits_only_points_after_the_left_edge() {
    let mut t = T::open();
    let clip = t.clip;
    store(&mut t, clip, &[], &[cc1(-10, 5), cc1(480, 90)]);
    t.app.keys.cc.controller = Some(1);
    t.frames(3);
    let points: Vec<usize> = t.app.keys.layout.cc.points.iter().map(|(i, _)| *i).collect();
    assert_eq!(points, vec![1], "the hidden point is not drawn; the visible one carries its sequence index");
    // Drag the visible point to tick 960 at value 40.
    let (from, to) = (lane_at(&t, 480, 90), lane_at(&t, 960, 40));
    t.drag(from, to, json!({}));
    let c = ctrls_of(&t, clip);
    assert_eq!(c[0], cc1(-10, 5), "the hidden point is unchanged");
    assert_eq!((c.len(), c[1].tick), (2, 960), "the visible point moved: {c:?} {}", t.app.status);
    assert!((c[1].value - 40).abs() <= 1);
    // Right-click it: it goes, the hidden point stays.
    let p = lane_at(&t, 960, c[1].value);
    t.input("ui.click", json!({"x": p.0, "y": p.1, "button": "secondary"}));
    assert_eq!(ctrls_of(&t, clip), vec![cc1(-10, 5)], "{}", t.app.status);
}

#[test]
fn several_clips_move_their_visible_notes_and_leave_a_hidden_one_in_the_second_clip() {
    let mut t = T::new();
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tb = TrackId(r["tracks"][0].as_u64().unwrap());
    let c = t.app.run("midi.new_clip", json!({"tracks": [tb.0], "start": 96_000, "end": 192_000})).unwrap();
    let b = ClipId(c["clip"].as_u64().unwrap());
    t.add(60, 0, 480);
    store(&mut t, b, &[note(72, -480, 240), note(64, 0, 480)], &[]);
    t.app.run("edit.select", json!({"clips": [t.clip.0, b.0]})).unwrap();
    t.frames(3);
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    assert_eq!(t.app.keys.layout.clips.len(), 2, "both clips are open");
    let drawn: Vec<_> = t.app.keys.layout.notes.iter().map(|n| (n.pitch, n.start)).collect();
    assert_eq!(drawn, vec![(60, 0), (64, 3840)], "B's hidden note is not drawn");
    t.key("A", json!({"cmd": true}));
    t.key("ArrowUp", json!({}));
    let of = |t: &T, clip: ClipId| match &t.app.engine.session().find_clip(clip).unwrap().1.content {
        ClipContent::Midi { sequence } => spots(&sequence.notes),
        _ => Vec::new(),
    };
    assert_eq!(of(&t, t.clip), vec![(61, 0)]);
    assert_eq!(of(&t, b), vec![(72, -480), (65, 0)], "B's visible note moved, its hidden note did not: {}", t.app.status);
}
