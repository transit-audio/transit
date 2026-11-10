//! The Pads view's view bar and its Start note field (README, Sampler views; ADR 0041 amendment of
//! 2026-11-09; ADR 0051 decision 9): the engine's value through the one namer, a drag and the
//! wheel with the 0 to 64 bounds and the octave modifier (a drag one undo step), typing a name or
//! a number, the engine's refusal in the panel's note, a kit saved before the field, and the bar's
//! rects. Each with a negative control (recorded with the job).

use super::test_rig::{T, mid};
use crate::tokens::{self, layout as space, sampler as tk};
use serde_json::json;
use transit_midi::{MiddleC, note_name};
use transit_model::TrackId;

/// Layout rects are rounded to 0.01 each, so a difference of two can be 0.01 off.
const EPS: f32 = 0.02;

fn start(t: &T, tr: TrackId) -> super::bar::StartNoteLayout {
    t.panel(tr).bar.start
}

/// Opens the field's typing with a double-click, replaces its text with `text` and presses `key`.
fn type_into(t: &mut T, tr: TrackId, text: &str, key: &str) {
    let field = start(t, tr).field;
    t.click(mid(field), 2);
    assert!(start(t, tr).editing, "a double-click opens the typing");
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": text}));
    t.key(key);
}

#[test]
fn the_field_shows_the_engines_start_note_through_the_namer() {
    let (mut t, tr) = T::open();
    let s = start(&t, tr);
    assert_eq!(s.value, t.patch(tr).pad_start_note);
    assert_eq!((s.value, s.text.as_str()), (36, "C1"), "a new kit's start note, under the default Middle C (C3)");
    assert_eq!(s.text, note_name(36, MiddleC::C3));
    assert!(s.tooltip.ends_with("Bank A: C1–D#2"), "{}", s.tooltip);
    // The Middle C preference changes the name, never the value.
    t.app.ui.middle_c = MiddleC::C4;
    t.frames(3);
    let s = start(&t, tr);
    assert_eq!((s.value, s.text.as_str()), (36, "C2"));
    assert!(s.tooltip.ends_with("Bank A: C2–D#3"), "{}", s.tooltip);
    assert_eq!(t.patch(tr).pad_start_note, 36, "the document is untouched");
}

#[test]
fn a_drag_steps_the_start_note_in_one_undo_step_within_the_bounds() {
    let (mut t, tr) = T::kit();
    let before = t.undo_len();
    // Up 20 points: three semitones at 6 points each.
    let at = mid(start(&t, tr).field);
    t.drag(at, (at.0, at.1 - 20.0), false);
    assert_eq!(t.patch(tr).pad_start_note, 39);
    assert_eq!(t.patch(tr).pad_note(0), 39, "pad A1 follows");
    assert_eq!(start(&t, tr).text, "D#1");
    assert_eq!(t.undo_len(), before + 1, "one step for the gesture");
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert_eq!(t.patch(tr).pad_start_note, 36, "one undo takes the whole drag back");
    // Shift: an octave a step.
    t.drag(at, (at.0, at.1 - 8.0), true);
    assert_eq!(t.patch(tr).pad_start_note, 48);
    // Far down: held at 0.
    t.drag(at, (at.0, at.1 + 400.0), false);
    assert_eq!(t.patch(tr).pad_start_note, 0);
    assert_eq!(start(&t, tr).text, "C-2");
    // Far up with Shift: held at 64, the highest start note.
    t.drag(at, (at.0, at.1 - 60.0), true);
    assert_eq!(t.patch(tr).pad_start_note, transit_model::pad_notes::MAX_PAD_START);
    assert_eq!(t.undo_len(), before + 3, "a step per gesture (the undone one is gone)");
}

#[test]
fn ctrl_wheel_steps_a_semitone_a_notch_and_an_octave_with_shift() {
    let (mut t, tr) = T::kit();
    let at = mid(start(&t, tr).field);
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": 2.0, "unit": "line", "steps": 2, "ctrl": true}));
    assert_eq!(t.patch(tr).pad_start_note, 38);
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": -1.0, "unit": "line", "steps": 1, "ctrl": true, "shift": true}));
    assert_eq!(t.patch(tr).pad_start_note, 26);
    // A plain wheel is the window's, not the field's.
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": 1.0, "unit": "line", "steps": 1}));
    assert_eq!(t.patch(tr).pad_start_note, 26);
}

#[test]
fn typing_a_name_or_a_number_sets_it_and_other_text_changes_nothing() {
    let (mut t, tr) = T::kit();
    let before = t.undo_len();
    type_into(&mut t, tr, "d1", "Enter");
    assert_eq!(t.patch(tr).pad_start_note, 38, "D1 under C3");
    assert!(!start(&t, tr).editing);
    assert_eq!(t.undo_len(), before + 1);
    type_into(&mut t, tr, "48", "Enter");
    assert_eq!(t.patch(tr).pad_start_note, 48, "a MIDI number");
    assert_eq!(t.undo_len(), before + 2);
    // Not a note, and a note above the highest start note: refused in place.
    for bad in ["H2", "70", "E3x"] {
        type_into(&mut t, tr, bad, "Enter");
        let s = start(&t, tr);
        assert_eq!(t.patch(tr).pad_start_note, 48, "{bad}");
        assert_eq!(t.undo_len(), before + 2, "{bad}: no step");
        assert!(s.editing && s.error, "{bad}: the field keeps the text with its error ring");
        let note = t.panel(tr).note.expect("the reason is shown").0;
        assert!(note.starts_with(&format!("No start note {bad}:")), "{note}");
        t.key("Escape");
        assert!(!start(&t, tr).editing && !start(&t, tr).error, "{bad}: Escape gives the typing up");
    }
    assert!(!t.app.builtin_editor.windows.is_empty(), "Escape while typing kept the window open");
    // Under C4 a typed name means the key the screen names.
    t.app.ui.middle_c = MiddleC::C4;
    t.frames(2);
    type_into(&mut t, tr, "C2", "Enter");
    assert_eq!(t.patch(tr).pad_start_note, 36);
}

#[test]
fn an_engine_refusal_shows_its_text_and_the_next_accepted_edit_clears_it() {
    let (mut t, tr) = T::kit();
    // Pad B1 stores 30: a start note of 30 would put pad A1 on it.
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 17, "note": 30}));
    let before = t.undo_len();
    type_into(&mut t, tr, "30", "Enter");
    let p = t.panel(tr);
    let (text, rect) = p.note.clone().expect("the refusal is shown, never silent");
    assert!(text.ends_with("Pad B1 plays F#0: a start note of F#0 would put pad A1 there."), "{text}");
    assert!(rect[1] >= p.panel("pads").unwrap()[3], "the panel's note row, under the hero row");
    assert_eq!((p.bar.start.value, p.bar.start.text.as_str()), (36, "C1"), "the field keeps the engine's value");
    assert_eq!(t.undo_len(), before, "no step");
    type_into(&mut t, tr, "40", "Enter");
    assert_eq!(t.patch(tr).pad_start_note, 40);
    assert!(t.panel(tr).note.is_none(), "the accepted edit clears the note");
}

#[test]
fn reset_in_the_right_click_menu_returns_to_c1_in_one_step() {
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_start", json!({"track": tr.0, "note": 50}));
    let before = t.undo_len();
    let field = start(&t, tr).field;
    t.send("ui.click", json!({"x": mid(field).0, "y": mid(field).1, "button": "right"}));
    let s = start(&t, tr);
    assert_eq!(s.reset_text, "Reset to C1");
    let row = s.reset_row.expect("the menu is open");
    t.click(mid(row), 1);
    assert_eq!(t.patch(tr).pad_start_note, transit_model::pad_notes::NEW_PAD_START);
    assert_eq!(t.undo_len(), before + 1, "one step");
    assert!(start(&t, tr).reset_row.is_none(), "the menu closed");
    // Under C4 the row names the same key.
    t.app.ui.middle_c = MiddleC::C4;
    t.frames(2);
    t.send("ui.click", json!({"x": mid(field).0, "y": mid(field).1, "button": "right"}));
    assert_eq!(start(&t, tr).reset_text, "Reset to C2");
}

#[test]
fn a_kit_saved_without_a_start_note_shows_sixty() {
    let (mut t, tr) = T::kit();
    let mut v = serde_json::to_value(t.patch(tr)).unwrap();
    assert!(v.as_object_mut().unwrap().remove("pad_start_note").is_some(), "a new kit writes its 36");
    let saved = transit_model::SamplerPatch::from_value(v);
    t.app.engine.session_mut().track_mut(tr).unwrap().instrument.as_mut().unwrap().sampler_patch = Some(saved);
    t.frames(3);
    let s = start(&t, tr);
    assert_eq!((s.value, s.text.as_str()), (60, "C3"));
}

#[test]
fn the_bar_sits_under_the_title_bar_at_the_readmes_sizes_at_both_zooms() {
    let mut seen = Vec::new();
    for ppp in [1.0f32, 1.5] {
        let (mut t, tr) = T::kit();
        t.ctx.set_pixels_per_point(ppp);
        t.frames(4);
        let p = t.panel(tr);
        let (bar, summary, label, field) = (p.bar.rect, p.bar.summary_rect, p.bar.start.label_rect, p.bar.start.field);
        let (pads, strip, title) = (p.panel("pads").unwrap(), p.panel("strip").unwrap(), p.title.bar);
        eprintln!("ppp {ppp}: window {:?} title {title:?} bar {bar:?} summary {summary:?} label {label:?} field {field:?} pads {pads:?}", p.window);
        // The bar: the view switch's 24, the window padding below the body's top, a panel gap above
        // the hero row, the hero row's edges. The body's top is the reference, not the title bar's
        // bottom: egui puts the body's child on the physical pixel grid, so at 1.5 the title bar
        // ends at 53.0 (79.5 px) and the body starts at 53.33 (80 px), for every row of the body.
        let origin = p.body_origin;
        assert!(origin[1] >= title[3] - EPS && origin[1] - title[3] < 1.0 / ppp, "{ppp}: {origin:?} {title:?}");
        assert!((origin[1] * ppp - (origin[1] * ppp).round()).abs() < EPS * ppp, "{ppp}: on the pixel grid");
        assert!((bar[3] - bar[1] - tk::VIEW_BAR_HEIGHT).abs() < EPS, "{ppp}");
        assert!((bar[1] - origin[1] - space::WINDOW_PADDING).abs() < EPS, "{ppp}");
        assert!((bar[0] - origin[0] - space::WINDOW_PADDING).abs() < EPS, "{ppp}");
        assert!((pads[1] - bar[3] - space::PANEL_GAP).abs() < EPS, "{ppp}");
        assert!((bar[0] - pads[0]).abs() < EPS && (bar[2] - strip[2]).abs() < EPS, "{ppp}");
        // The field: 48 by 24, centred in the bar, at its right edge; the label 6 before it.
        assert!((field[2] - field[0] - tk::START_NOTE_WIDTH).abs() < EPS && (field[3] - field[1] - tk::START_NOTE_HEIGHT).abs() < EPS);
        assert!((field[2] - bar[2]).abs() < EPS && ((field[1] + field[3]) / 2.0 - (bar[1] + bar[3]) / 2.0).abs() < EPS);
        assert!((field[0] - label[2] - tk::START_NOTE_LABEL_GAP).abs() < 0.5, "{ppp}: {label:?} {field:?}");
        // The summary at the left, clear of the label.
        assert!((summary[0] - bar[0]).abs() < EPS && summary[1] >= bar[1] - EPS && summary[3] <= bar[3] + EPS);
        assert!(summary[2] + tokens::SPACE[2] <= label[0], "{summary:?} {label:?}");
        assert_eq!(p.bar.summary, "Bank A · 3 of 16 loaded · no choke groups");
        // The pad grid keeps its size.
        assert!((p.grid[2] - p.grid[0] - 236.0).abs() < EPS);
        seen.push(((bar[3] - bar[1]).round(), (field[2] - field[0]).round(), (field[3] - field[1]).round()));
    }
    assert_eq!(seen[0], seen[1], "the same sizes in points at 1.0 and 1.5");
}

#[test]
fn the_widest_summary_clears_the_field_at_the_minimum_width() {
    let (mut t, tr, _) = super::width_tests::open_at(super::MIN_WIDTH, None);
    for (i, g) in [(0u8, 1), (1, 2), (2, 2)] {
        t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": i + 1, "choke": g}));
    }
    let p = t.panel(tr);
    assert_eq!(p.bar.summary, "Bank A · 3 of 16 loaded · 2 choke groups", "groups in use, counted once");
    let painter = egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
    let widest = crate::widgets::role_size(&painter, &tokens::READOUT_DISPLAY, "Bank D · 16 of 16 loaded · 16 choke groups").x;
    assert!(p.bar.summary_rect[0] + widest + tokens::SPACE[2] <= p.bar.start.label_rect[0], "{widest} {:?}", p.bar);
}
