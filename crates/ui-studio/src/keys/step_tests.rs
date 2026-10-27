use super::*;

#[test]
fn step_hold_commits_the_preview_length_as_one_undo_step() {
    let mut t = T::open();
    t.step_on();
    let undo = t.undo_len();
    let note = |pitch, on| transit_playback::PlayedNote { pitch, velocity: if on { 90 } else { 0 }, on };
    t.app.step_midi_at(note(60, true), 0);
    t.app.step_midi_at(note(64, true), 20);
    t.app.step_midi_at(note(60, false), 600);
    assert!(t.notes().is_empty(), "preview must not edit the document");
    t.app.step_midi_at(note(64, false), 1050);
    let notes = t.notes();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|n| n.length == 3 * SIXTEENTH));
    assert_eq!(t.app.keys.step.cursor, 3 * SIXTEENTH);
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn step_pedal_down_and_up_drained_in_one_frame_still_insert_a_rest() {
    use transit_engine::step_input::StepBinding::Cc;
    use transit_playback::PlayedMidi;
    let mut t = T::open();
    t.step_on();
    t.app.live_played_midi(PlayedMidi::Control { cc: 64, value: 127 });
    t.app.live_played_midi(PlayedMidi::Control { cc: 64, value: 0 });
    t.app.step_tick_at(0);
    assert_eq!(t.app.keys.step.cursor, SIXTEENTH, "a tap drained with its release in one frame is one rest");
    t.app.step_message_at(Cc(64), 127, 10);
    t.app.step_message_at(Cc(64), 0, 10);
    assert_eq!(t.app.keys.step.cursor, 2 * SIXTEENTH, "same millisecond");
}

#[test]
fn step_events_are_timed_from_their_arrival_stamp_not_from_a_late_drain() {
    use transit_playback::{PlayedMidi, PlayedNote};
    let mut t = T::open();
    t.step_on();
    let note = |on| PlayedMidi::Note(PlayedNote { pitch: 60, velocity: if on { 90 } else { 0 }, on });
    // The UI did not look for 800 ms: the events carry when they arrived.
    t.app.live_played_midi_at(note(true), 0);
    t.app.live_played_midi_at(note(false), 100);
    assert_eq!(t.notes()[0].length, SIXTEENTH, "a 100 ms hold is one unit, not the 900 ms of the drain");
    t.app.live_played_midi_at(PlayedMidi::Control { cc: 64, value: 127 }, 200);
    t.app.live_played_midi_at(PlayedMidi::Control { cc: 64, value: 0 }, 230);
    t.app.step_tick_at(1000);
    assert_eq!(t.app.keys.step.cursor, 2 * SIXTEENTH, "the late tap is one rest, not a ramp's worth");
}

#[test]
fn clearing_live_input_discards_the_pending_step() {
    let mut t = T::open();
    t.step_on();
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 60, velocity: 90, on: true }, 0);
    t.app.live_clear();
    t.app.step_midi_at(transit_playback::PlayedNote { pitch: 60, velocity: 0, on: false }, 1000);
    assert!(t.notes().is_empty());
    assert_eq!(t.app.keys.step.cursor, 0);
}

#[test]
fn step_pedal_rests_repeat_and_sustain_chord_ties_are_undoable() {
    use transit_engine::step_input::StepBinding::{Cc, Note};
    let mut t = T::open();
    t.step_on();
    t.app.step_message_at(Cc(64), 127, 0);
    t.app.step_message_at(Cc(64), 0, 100);
    assert_eq!(t.app.keys.step.cursor, SIXTEENTH);
    t.app.step_message_at(Cc(64), 127, 200);
    t.app.step_tick_at(700);
    assert_eq!(t.app.keys.step.cursor, 3 * SIXTEENTH);
    t.app.step_tick_at(1100);
    assert_eq!(t.app.keys.step.cursor, 4 * SIXTEENTH);
    t.app.step_message_at(Cc(64), 0, 1101);
    assert!(t.notes().is_empty());
    t.app.step_message_at(Note(60), 90, 1200);
    t.app.step_message_at(Note(64), 80, 1210);
    t.app.step_message_at(Note(60), 0, 1300);
    t.app.step_message_at(Note(64), 0, 1310);
    let undo = t.undo_len();
    t.app.step_message_at(Cc(64), 127, 1400);
    t.app.step_message_at(Note(60), 90, 1410);
    t.app.step_message_at(Note(64), 80, 1420);
    t.app.step_message_at(Note(60), 0, 1500);
    t.app.step_message_at(Note(64), 0, 1510);
    t.app.step_message_at(Cc(64), 0, 1550);
    assert_eq!(t.notes().len(), 2, "tie extends, never duplicates the chord");
    assert!(t.notes().iter().all(|n| n.length == 2 * SIXTEENTH && n.start == 4 * SIXTEENTH));
    assert_eq!(t.app.keys.step.cursor, 6 * SIXTEENTH);
    assert_eq!(t.undo_len(), undo + 1);
    t.app.step_back();
    assert!(t.notes().iter().all(|n| n.length == SIXTEENTH));
    assert_eq!(t.app.keys.step.cursor, 5 * SIXTEENTH);
    t.app.step_back();
    assert!(t.notes().is_empty(), "after taking back the tie, take back the original chord");
}

#[test]
fn step_learn_consumes_the_learning_press_and_back_key_and_persists_binding() {
    use transit_engine::step_input::{StepBinding, StepControl};
    let mut t = T::open();
    t.step_on();
    t.tap(60, 90);
    t.app.keys.step.input_cancel_for_learn(StepControl::Back);
    t.app.step_message_at(StepBinding::Note(21), 100, 0);
    t.app.step_message_at(StepBinding::Note(21), 0, 50);
    assert_eq!(t.notes().len(), 1, "learning must not execute the new action");
    t.app.step_message_at(StepBinding::Note(21), 100, 100);
    t.app.step_message_at(StepBinding::Note(21), 0, 150);
    assert!(t.notes().is_empty());
    assert_eq!(t.app.keys.step.cursor, 0);
    let saved = serde_json::to_value(&t.app.ui.keys).unwrap();
    let back: crate::keys::KeysPrefs = serde_json::from_value(saved).unwrap();
    assert_eq!(back.step_bindings.back, Some(StepBinding::Note(21)));
    let old: crate::keys::KeysPrefs = serde_json::from_value(json!({})).unwrap();
    assert_eq!(old.step_bindings.tie_rest, Some(StepBinding::Cc(64)));
}

#[test]
fn played_midi_tap_controls_reach_the_step_editor_and_reset_discards_a_hold() {
    use transit_playback::{PlayedMidi, PlayedNote};
    let mut t = T::open();
    t.step_on();
    t.app.live_played_midi(PlayedMidi::Control { cc: 64, value: 127 });
    t.app.live_played_midi(PlayedMidi::Control { cc: 64, value: 0 });
    assert_eq!(t.app.keys.step.cursor, SIXTEENTH);
    t.app.live_played_midi(PlayedMidi::Note(PlayedNote { pitch: 60, velocity: 100, on: true }));
    t.app.live_played_midi(PlayedMidi::Reset);
    t.app.live_played_midi(PlayedMidi::Note(PlayedNote { pitch: 60, velocity: 0, on: false }));
    assert!(t.notes().is_empty());
    assert!(t.app.live_keys().is_empty());
}

#[test]
fn step_editor_stops_inserting_rests_once_the_tapped_pedal_up_arrives() {
    use transit_engine::step_input::StepBinding::Cc;
    let mut t = T::open();
    t.step_on();
    t.app.step_message_at(Cc(64), 127, 0);
    t.app.step_tick_at(2000);
    let held = t.app.keys.step.cursor;
    assert!(held > SIXTEENTH, "a held pedal keeps inserting rests");
    // The lane shed the notes in between; the tap still delivers the pedal-up (#74).
    t.app.step_message_at(Cc(64), 0, 2010);
    let released = t.app.keys.step.cursor;
    t.app.step_tick_at(5000);
    t.app.step_tick_at(9000);
    assert_eq!(t.app.keys.step.cursor, released, "no held pedal: no more rests");
    assert!(t.notes().is_empty());
}

#[test]
fn a_reset_releases_a_held_pedal_and_stops_the_rests() {
    use transit_engine::step_input::StepBinding::Cc;
    use transit_playback::PlayedMidi;
    let mut t = T::open();
    t.step_on();
    t.app.step_message_at(Cc(64), 127, 0);
    t.app.step_tick_at(2000);
    let held = t.app.keys.step.cursor;
    assert!(held > SIXTEENTH);
    t.app.live_played_midi(PlayedMidi::Reset);
    t.app.step_tick_at(5000);
    t.app.step_tick_at(9000);
    assert_eq!(t.app.keys.step.cursor, held, "a Reset drops the pedal: no more rests");
}

#[test]
fn step_toggles_from_the_toolbar_and_the_key_and_shows_a_cursor() {
    let mut t = T::open();
    assert!(!t.app.keys.layout.step_on);
    t.click(mid(t.app.keys.layout.step));
    assert!(t.app.keys.layout.step_on && t.app.keys.layout.step_cursor == 0 && t.app.keys.layout.step_ticks == SIXTEENTH);
    t.key("S", json!({}));
    assert!(!t.app.keys.layout.step_on, "S toggles it off");
    t.key("S", json!({}));
    assert!(t.app.keys.layout.step_on);
}

#[test]
fn a_played_note_enters_at_the_cursor_with_the_step_length_as_one_undo_step() {
    let mut t = T::open();
    t.step_on();
    let undo = t.undo_len();
    t.played(60, 90, true);
    assert!(t.notes().is_empty(), "entered when the key is released");
    t.played(60, 0, false);
    let n = t.notes();
    assert_eq!((n.len(), n[0].pitch, n[0].start, n[0].length, n[0].velocity), (1, 60, 0, SIXTEENTH, 90));
    assert_eq!(t.app.keys.layout.step_cursor, SIXTEENTH);
    assert_eq!(t.undo_len(), undo + 1);
    t.tap(62, 70);
    assert_eq!(t.notes().iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(60, 0), (62, SIXTEENTH)]);
    assert_eq!(t.undo_len(), undo + 2);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes().len(), 1, "one step undone");
}

#[test]
fn keys_held_together_enter_one_chord_step() {
    let mut t = T::open();
    t.step_on();
    let undo = t.undo_len();
    t.played(60, 80, true);
    t.played(64, 100, true);
    t.played(67, 90, true);
    t.played(60, 0, false);
    assert!(t.notes().is_empty(), "a chord must wait for its last held key");
    t.played(64, 0, false);
    assert!(t.notes().is_empty(), "lifting a second finger must not commit the chord");
    t.played(67, 0, false);
    let n = t.notes();
    assert_eq!(n.len(), 3);
    assert!(n.iter().all(|x| x.start == 0 && x.length == SIXTEENTH));
    assert_eq!(n.iter().map(|x| (x.pitch, x.velocity)).collect::<Vec<_>>(), vec![(60, 80), (64, 100), (67, 90)]);
    assert_eq!((t.app.keys.layout.step_cursor, t.undo_len()), (SIXTEENTH, undo + 1));
}

#[test]
fn rest_advances_and_backspace_takes_the_last_step_back() {
    let mut t = T::open();
    t.step_on();
    t.tap(60, 100);
    t.key("Period", json!({}));
    assert_eq!((t.app.keys.layout.step_cursor, t.notes().len()), (2 * SIXTEENTH, 1), "rest: cursor moves, no note");
    t.tap(62, 100);
    assert_eq!(t.notes().iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 2 * SIXTEENTH]);
    let undo = t.undo_len();
    t.key("Backspace", json!({}));
    assert_eq!(
        t.notes().iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![60],
        "the last step's note is gone: {} {:?}",
        t.app.status,
        t.app.keys.step
    );
    assert_eq!(t.app.keys.layout.step_cursor, 2 * SIXTEENTH);
    assert_eq!(t.undo_len(), undo + 1, "undoable");
    t.key("Backspace", json!({}));
    assert_eq!((t.app.keys.layout.step_cursor, t.notes().len()), (SIXTEENTH, 1), "the rest is taken back, no note lost");
    t.key("Backspace", json!({}));
    assert!(t.notes().is_empty());
    assert_eq!(t.app.keys.layout.step_cursor, 0);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes().len(), 1, "Backspace undone");
}

#[test]
fn number_keys_set_the_step_length() {
    let mut t = T::open();
    t.step_on();
    let q = transit_time::TICKS_PER_QUARTER;
    for (key, ticks) in [("1", q * 4), ("2", q * 2), ("3", q), ("4", q / 2), ("5", q / 4), ("6", q / 8)] {
        t.key(key, json!({}));
        assert_eq!(t.app.keys.layout.step_ticks, ticks, "key {key}");
    }
    t.key("3", json!({}));
    t.tap(60, 100);
    assert_eq!((t.notes()[0].length, t.app.keys.layout.step_cursor), (q, q));
}

#[test]
fn entering_past_the_clip_end_extends_the_clip() {
    let mut t = T::open();
    t.step_on();
    let q = transit_time::TICKS_PER_QUARTER;
    t.key("1", json!({}));
    assert_eq!(t.clip_ticks(), 4 * q);
    t.tap(60, 100);
    t.tap(62, 100);
    assert_eq!(t.clip_ticks(), 8 * q, "a whole-note step past the bar grows the clip to the step boundary");
    assert_eq!(t.notes().iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 4 * q]);
}

#[test]
fn entering_stops_at_the_next_clip_and_says_so() {
    let mut t = T::open();
    let (end, _) = {
        let s = t.app.engine.session();
        let c = s.find_clip(t.clip).unwrap().1;
        (c.end(), c.start)
    };
    t.app.run("midi.new_clip", json!({"tracks": [t.track.0], "start": end, "end": end + 48_000})).unwrap();
    t.frames(2);
    t.step_on();
    let q = transit_time::TICKS_PER_QUARTER;
    t.key("1", json!({}));
    t.tap(60, 100);
    assert_eq!(t.notes().len(), 1);
    let undo = t.undo_len();
    t.tap(62, 100);
    assert_eq!((t.notes().len(), t.undo_len(), t.clip_ticks()), (1, undo, 4 * q), "nothing entered, clip not overlapped");
    assert!(t.app.status.contains("next clip"), "{}", t.app.status);
}

#[test]
fn midi_enters_nothing_while_step_is_off() {
    let mut t = T::open();
    t.tap(60, 100);
    assert!(t.notes().is_empty());
    t.step_on();
    t.key("S", json!({}));
    t.tap(60, 100);
    assert!(t.notes().is_empty(), "off again");
}

// Live MIDI keys: the keyboard column accents the keys being played.

#[test]
fn a_note_on_holds_the_key_and_the_note_off_lets_go() {
    let mut t = T::open();
    assert!(t.app.keys.layout.live.is_empty());
    t.played(64, 100, true);
    assert_eq!(t.app.keys.layout.live, vec![(64, 100)]);
    t.played(64, 0, false);
    assert!(t.app.keys.layout.live.is_empty());
}

#[test]
fn a_chord_holds_every_key_and_a_stop_lets_go_of_all() {
    let mut t = T::open();
    for p in [60, 64, 67] {
        t.played(p, 90, true);
    }
    assert_eq!(t.app.keys.layout.live.iter().map(|k| k.0).collect::<Vec<_>>(), vec![60, 64, 67]);
    t.played(64, 0, false);
    assert_eq!(t.app.keys.layout.live.iter().map(|k| k.0).collect::<Vec<_>>(), vec![60, 67]);
    t.app.stop_play();
    t.frames(2);
    assert!(t.app.keys.layout.live.is_empty(), "Stop clears them");
}

#[test]
fn step_mode_still_enters_the_note_and_highlights_the_key() {
    let mut t = T::open();
    t.step_on();
    t.played(60, 90, true);
    assert_eq!(t.app.keys.layout.live, vec![(60, 90)]);
    t.played(60, 0, false);
    assert!(t.app.keys.layout.live.is_empty());
    let n = t.notes();
    assert_eq!((n.len(), n[0].pitch), (1, 60), "the step was entered");
}

#[test]
fn reselecting_a_midi_input_lets_go_of_held_keys() {
    let mut t = T::open();
    t.played(60, 90, true);
    t.app.run("midi.input", json!({"track": t.track.0, "channel": 3})).unwrap();
    t.frames(2);
    assert!(t.app.keys.layout.live.is_empty());
}

#[test]
fn step_entry_default_length_follows_the_adaptive_grid() {
    let mut t = T::open();
    let q = transit_time::TICKS_PER_QUARTER;
    t.zoom_to(200.0);
    t.step_on();
    assert_eq!(t.app.keys.layout.step_ticks, q / 16);
    t.tap(60, 100);
    assert_eq!((t.notes()[0].length, t.app.keys.layout.step_cursor), (q / 16, q / 16));
    // A length key still wins over the grid.
    t.key("3", json!({}));
    t.zoom_to(400.0);
    assert_eq!(t.app.keys.layout.step_ticks, q);
}

#[test]
fn left_and_right_move_the_step_cursor_without_editing_and_home_and_end_jump() {
    let mut t = T::open();
    t.step_on();
    t.tap(60, 100);
    t.tap(62, 100);
    t.tap(64, 100);
    assert_eq!(t.app.keys.layout.step_cursor, 3 * SIXTEENTH);
    let (undo, notes) = (t.undo_len(), t.notes());
    t.key("ArrowLeft", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 2 * SIXTEENTH);
    t.key("ArrowLeft", json!({}));
    t.key("ArrowRight", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 2 * SIXTEENTH);
    assert_eq!((t.undo_len(), t.notes()), (undo, notes), "moving the cursor edits nothing, and does not nudge notes");
    t.key("Home", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 0);
    t.key("ArrowLeft", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 0, "stops at the clip start");
    t.key("End", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 3 * SIXTEENTH, "the end of the entered material");
    t.key("ArrowRight", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 4 * SIXTEENTH);
    // Entering at a moved cursor, then Backspace still takes the last entered step back.
    t.key("ArrowLeft", json!({}));
    t.key("ArrowLeft", json!({}));
    t.tap(65, 100);
    assert_eq!(t.notes().iter().filter(|n| n.pitch == 65).map(|n| n.start).collect::<Vec<_>>(), vec![2 * SIXTEENTH]);
    t.key("Backspace", json!({}));
    assert!(t.notes().iter().all(|n| n.pitch != 65));
    assert_eq!(t.app.keys.layout.step_cursor, 2 * SIXTEENTH);
    // A rest counts as material for End.
    t.key("End", json!({}));
    t.key("Period", json!({}));
    t.key("Home", json!({}));
    t.key("End", json!({}));
    assert_eq!(t.app.keys.layout.step_cursor, 4 * SIXTEENTH);
}

#[test]
fn the_step_cursor_stays_visible_when_it_jumps() {
    let mut t = T::open();
    t.step_on();
    t.zoom_to(400.0);
    let q = transit_time::TICKS_PER_QUARTER;
    t.key("1", json!({}));
    for pitch in 60..70 {
        t.tap(pitch, 100);
    }
    t.key("Home", json!({}));
    t.frames(2);
    let l = &t.app.keys.layout;
    assert_eq!((l.step_cursor, l.scroll_ticks), (0, 0.0));
    t.key("End", json!({}));
    t.frames(2);
    let l = &t.app.keys.layout;
    let visible = f64::from(l.grid[2] - l.grid[0]) / l.px_per_tick;
    assert_eq!(l.step_cursor, 10 * 4 * q);
    assert!(l.scroll_ticks <= l.step_cursor as f64 && (l.step_cursor as f64) < l.scroll_ticks + visible, "{} {visible}", l.scroll_ticks);
}

#[test]
fn fixed_step_velocity_overrides_the_played_velocity_and_played_follows_the_key() {
    let mut t = T::open();
    assert!(!t.app.ui.keys.step_fixed && t.app.ui.keys.step_velocity == 100, "default: Played, 100 when fixed");
    t.step_on();
    t.tap(60, 37);
    assert_eq!(t.notes()[0].velocity, 37, "Played");
    // Click the toolbar control: Fixed. Drag it to change the value.
    t.click(mid(t.app.keys.layout.step_velocity));
    assert!(t.app.ui.keys.step_fixed);
    let m = mid(t.app.keys.layout.step_velocity);
    t.drag(m, (m.0 - 40.0, m.1), json!({}));
    assert!(t.app.ui.keys.step_fixed && t.app.ui.keys.step_velocity < 100, "dragging left lowers it: {}", t.app.ui.keys.step_velocity);
    t.app.ui.keys.step_velocity = 80;
    t.tap(62, 127);
    t.tap(64, 5);
    let v: Vec<(u8, u8)> = t.notes().iter().map(|n| (n.pitch, n.velocity)).collect();
    assert_eq!(v, vec![(60, 37), (62, 80), (64, 80)], "Fixed 80 whatever the key did");
    // A chord is fixed note by note.
    t.played(65, 20, true);
    t.played(67, 120, true);
    t.played(65, 0, false);
    assert!(!t.notes().iter().any(|n| n.pitch == 65 || n.pitch == 67), "the chord is pending until its last release");
    t.played(67, 0, false);
    assert_eq!(t.notes().iter().filter(|n| n.pitch == 65 || n.pitch == 67).count(), 2);
    assert!(t.notes().iter().filter(|n| n.pitch == 65 || n.pitch == 67).all(|n| n.velocity == 80));
    // Back to Played.
    t.click(mid(t.app.keys.layout.step_velocity));
    assert!(!t.app.ui.keys.step_fixed);
    t.tap(69, 99);
    assert_eq!(t.notes().iter().find(|n| n.pitch == 69).unwrap().velocity, 99);
    // Saved with the other piano-roll preferences.
    t.app.ui.keys.step_fixed = true;
    t.app.ui.keys.step_velocity = 64;
    let saved = serde_json::to_vec(&t.app.ui).unwrap();
    let back: crate::UiState = serde_json::from_slice(&saved).unwrap();
    assert_eq!((back.keys.step_fixed, back.keys.step_velocity), (true, 64));
    let old: crate::UiState = serde_json::from_value(json!({"keys": {"snap": "1/8"}})).unwrap();
    assert_eq!((old.keys.step_fixed, old.keys.step_velocity, old.keys.grid), (false, 100, crate::grid::GridView::default()));
}

#[test]
fn step_combined_pedal_worked_example_writes_exact_grid_positions() {
    use transit_engine::step_input::StepBinding::{Cc, Note};
    let mut t = T::open();
    t.step_on();
    for (pitch, start) in [(60, 0), (62, 3000)] {
        t.app.step_message_at(Note(pitch), 90, start);
        t.app.step_message_at(Cc(64), 127, start + 10);
        t.app.step_message_at(Cc(64), 0, start + 20);
        t.app.step_message_at(Note(pitch), 0, start + 2000);
    }
    t.app.step_message_at(Cc(64), 127, 6000);
    t.app.step_message_at(Cc(64), 0, 6100);
    for (pitch, start) in [(64, 6200), (65, 6400), (67, 6600), (69, 6800)] {
        t.app.step_message_at(Note(pitch), 90, start);
        t.app.step_message_at(Note(pitch), 0, start + 100);
    }
    let notes: Vec<_> = t.notes().iter().map(|n| (n.pitch, n.start / SIXTEENTH, n.length / SIXTEENTH)).collect();
    assert_eq!(notes, [(60, 0, 2), (62, 2, 2), (64, 5, 1), (65, 6, 1), (67, 7, 1), (69, 8, 1)]);
    assert_eq!(t.app.keys.step.cursor, 9 * SIXTEENTH);
}

#[test]
fn step_keyboard_combined_enter_and_opt_in_space_are_scoped() {
    use transit_engine::step_input::StepBinding::Note;
    let mut t = T::open();
    t.step_on();
    t.app.ui.keys.step_hold.enabled = false;
    t.app.step_message_at(Note(60), 90, 0);
    t.key("Enter", json!({}));
    t.app.step_message_at(Note(60), 0, 100);
    assert_eq!(t.notes()[0].length, 2 * SIXTEENTH);
    t.key("Enter", json!({}));
    assert_eq!(t.app.keys.step.cursor, 3 * SIXTEENTH);
    t.app.ui.keys.step_tie_rest_space = true;
    t.key("Space", json!({}));
    assert_eq!(t.app.keys.step.cursor, 4 * SIXTEENTH);
    t.app.keys.active = false;
    t.key("Space", json!({}));
    assert!(t.app.is_playing(), "Space still reaches transport outside the piano roll");
    assert_eq!(t.app.keys.step.cursor, 4 * SIXTEENTH, "outside piano roll Space cannot enter rest");
    t.app.keys.active = true;
    t.app.ui.keys.step_tie_rest_space = false;
    t.key("Space", json!({}));
    assert_eq!(t.app.keys.step.cursor, 4 * SIXTEENTH, "Space defaults to transport");
    assert!(!t.app.is_playing());
    t.app.step_keyboard_control(true);
    t.app.keys.active = false;
    t.frames(1);
    t.app.step_tick_at(u64::MAX);
    assert_eq!(t.app.keys.step.cursor, 4 * SIXTEENTH, "focus loss cancels a held keyboard control");
}

#[test]
fn step_hold_preference_and_combined_learning_persist() {
    use transit_engine::step_input::{StepBinding, StepControl};
    let mut t = T::open();
    t.step_on();
    t.app.ui.keys.step_hold.enabled = false;
    t.app.ui.keys.step_tie_rest_space = true;
    t.app.keys.step.input_cancel_for_learn(StepControl::TieRest);
    t.app.step_message_at(StepBinding::Note(21), 100, 0);
    t.app.step_message_at(StepBinding::Note(21), 0, 50);
    assert_eq!(t.app.keys.step.cursor, 0);
    t.app.step_message_at(StepBinding::Note(60), 90, 100);
    t.app.step_message_at(StepBinding::Note(60), 0, 10_000);
    assert_eq!(t.notes()[0].length, SIXTEENTH);
    let saved = serde_json::to_value(&t.app.ui.keys).unwrap();
    let prefs: crate::keys::KeysPrefs = serde_json::from_value(saved).unwrap();
    assert!(!prefs.step_hold.enabled);
    assert!(prefs.step_tie_rest_space);
    assert_eq!(prefs.step_bindings.tie_rest, Some(StepBinding::Note(21)));
    let old: crate::keys::KeysPrefs = serde_json::from_value(json!({"step_hold":{"threshold_ms":500,"floor_ms":80}})).unwrap();
    assert!(old.step_hold.enabled);
    let old: crate::keys::KeysPrefs = serde_json::from_value(json!({"step_bindings":{"rest":null,"back":{"Cc":64},"tie":null}})).unwrap();
    assert_eq!(old.step_bindings.back, Some(StepBinding::Cc(64)));
    assert_eq!(old.step_bindings.tie_rest, None, "new default must not shadow old learned bindings");
}

#[test]
fn step_shortcut_selector_clicks_rebind_and_the_table_displays_the_choice() {
    let ctx = egui::Context::default();
    let mut space = false;
    let mut rect = egui::Rect::NOTHING;
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
            rect = crate::shortcuts::step_tie_rest_selector(ui, &mut space).rect;
        });
        output.textures_delta.clear();
        (space, rect.center())
    };
    let (_, at) = frame(Vec::new());
    let click = |at| {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE },
        ]
    };
    let (space, at) = frame(click(at));
    assert!(space, "clicking the selector opts in to Space");
    let (space, _) = frame(click(at));
    assert!(!space, "a second click restores Enter");
    let mut t = T::open();
    t.app.ui.show_shortcuts = true;
    t.app.ui.shortcuts_filter = "Step Tie/Rest".into();
    t.frames(2);
    assert_eq!(t.app.shortcuts_window.rows, ["Step entry · Step Tie/Rest · Enter"]);
    t.app.ui.keys.step_tie_rest_space = true;
    t.frames(2);
    assert_eq!(t.app.shortcuts_window.rows, ["Step entry · Step Tie/Rest · Space"]);
}

#[test]
fn step_entry_in_a_looped_clip_wraps_at_the_loop_end() {
    let mut t = T::open();
    let q = transit_time::TICKS_PER_QUARTER;
    t.loop_to_bars(0, 4);
    t.step_on();
    t.app.step_set_len(q);
    for p in [60, 62, 64, 65, 67] {
        t.tap(p, 100);
    }
    let mut starts: Vec<i64> = t.notes().iter().map(|n| n.start).collect();
    starts.sort_unstable();
    assert_eq!(starts, vec![0, 0, q, 2 * q, 3 * q], "the fifth step wraps to loop.start");
    assert!(t.app.keys.layout.notes.iter().all(|n| !n.outside), "no silent note past the loop end");
    assert_eq!(t.clip_ticks(), 4 * q, "the span is still the cycle");
}

#[test]
fn step_on_polls_at_30_ms_when_idle_and_full_rate_while_holding() {
    use transit_engine::step_input::StepBinding::Cc;
    let ms = std::time::Duration::from_millis;
    let mut t = T::open();
    // Step off: the existing idle policy, no 30 ms poll.
    let off = t.frame_with(None);
    assert!(off > ms(100), "idle policy with Step off: {off:?}");
    t.step_on();
    // timing: value the repaint delay the app requests (a computed interval), not a measured duration
    let idle = t.frame_with(None);
    // egui deducts one predicted frame (16.7 ms at 60 Hz) from the requested 30 ms.
    assert!((ms(10)..=ms(30)).contains(&idle), "Step on and idle polls at about 30 ms, not per frame: {idle:?}");
    // A held key is a pending chord: full rate.
    t.app.live_midi(transit_playback::PlayedNote { pitch: 60, velocity: 90, on: true });
    let held = t.frame_with(None);
    assert!(held < idle && held <= ms(1), "a held key repaints at full rate: {held:?}");
    t.app.live_midi(transit_playback::PlayedNote { pitch: 60, velocity: 0, on: false });
    t.frames(2);
    // A held pedal: full rate.
    t.app.step_message_at(Cc(64), 127, 0);
    let pedal = t.frame_with(None);
    assert!(pedal < idle && pedal <= ms(1), "a held pedal repaints at full rate: {pedal:?}");
    t.app.step_message_at(Cc(64), 0, 10);
}
