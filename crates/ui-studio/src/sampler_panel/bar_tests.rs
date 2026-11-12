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
    let before = t.undo_len();
    let at = mid(start(&t, tr).field);
    // Three notches in consecutive frames (the rig feeds one event a frame): one turn, one step.
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": 3.0, "unit": "line", "steps": 3, "ctrl": true}));
    assert_eq!(t.patch(tr).pad_start_note, 39);
    assert_eq!(t.undo_len(), before + 1, "one undo step for the turn");
    // A later turn (frames without a notch between) is its own step; Shift moves by octaves.
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": -1.0, "unit": "line", "steps": 1, "ctrl": true, "shift": true}));
    assert_eq!(t.patch(tr).pad_start_note, 27);
    assert_eq!(t.undo_len(), before + 2);
    // A plain wheel is the window's, not the field's.
    t.send("ui.scroll", json!({"x": at.0, "y": at.1, "dy": 1.0, "unit": "line", "steps": 1}));
    assert_eq!(t.patch(tr).pad_start_note, 27);
    assert_eq!(t.undo_len(), before + 2);
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
        // The bar: a 28 px row, the window padding below the body's top, a panel gap above the hero
        // row, the hero row's edges. The body's top is the reference, not the title bar's
        // bottom: egui puts the body's child on the physical pixel grid, so at 1.5 the title bar
        // ends at 53.0 (79.5 px) and the body starts at 53.33 (80 px), for every row of the body.
        let origin = p.body_origin;
        assert!(origin[1] >= title[3] - EPS && origin[1] - title[3] < 1.0 / ppp, "{ppp}: {origin:?} {title:?}");
        assert!((origin[1] * ppp - (origin[1] * ppp).round()).abs() < EPS * ppp, "{ppp}: on the pixel grid");
        assert_eq!(tk::VIEW_BAR_HEIGHT, 28.0, "the board's bar");
        assert!((bar[3] - bar[1] - tk::VIEW_BAR_HEIGHT).abs() < EPS, "{ppp}");
        // The pad grid's top: the window padding, the 28 px bar and a panel gap under the body's
        // top (4 lower than under the 24 px bar before it).
        assert!((pads[1] - origin[1] - (space::WINDOW_PADDING + 28.0 + space::PANEL_GAP)).abs() < EPS, "{ppp}: {pads:?}");
        assert!((bar[1] - origin[1] - space::WINDOW_PADDING).abs() < EPS, "{ppp}");
        assert!((bar[0] - origin[0] - space::WINDOW_PADDING).abs() < EPS, "{ppp}");
        assert!((pads[1] - bar[3] - space::PANEL_GAP).abs() < EPS, "{ppp}");
        assert!((bar[0] - pads[0]).abs() < EPS && (bar[2] - strip[2]).abs() < EPS, "{ppp}");
        // The field: 48 by the dense 24, centered in the bar, one bar gap before Load kit;
        // the label 6 before it.
        assert_eq!(tk::START_NOTE_HEIGHT, tk::VIEW_BAR_CONTROL_HEIGHT, "the field takes the bar's control height");
        assert!((field[2] - field[0] - tk::START_NOTE_WIDTH).abs() < EPS && (field[3] - field[1] - tk::VIEW_BAR_CONTROL_HEIGHT).abs() < EPS);
        assert_eq!(super::view_bar_tests::hundredths(p.bar.load_kit.unwrap()[0]) - super::view_bar_tests::hundredths(field[2]), 1200, "{ppp}");
        assert!((field[1] - bar[1] - 2.0).abs() < EPS && (bar[3] - field[3] - 2.0).abs() < EPS, "{ppp}: {bar:?} {field:?}");
        assert!((field[0] - label[2] - tk::START_NOTE_LABEL_GAP).abs() < EPS, "{ppp}: {label:?} {field:?}");
        // The summary follows the switch by one bar gap, clear of the label.
        assert_eq!(super::view_bar_tests::hundredths(summary[0]) - super::view_bar_tests::hundredths(p.bar.switch.as_ref().unwrap().track[2]), 1200);
        assert!(summary[1] >= bar[1] - EPS && summary[3] <= bar[3] + EPS);
        assert!(summary[2] + tk::VIEW_BAR_GAP <= label[0], "{summary:?} {label:?}");
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
    // The summary and the Start note item (its label and field) stand at least the bar gap apart.
    let gap = p.bar.start.label_rect[0] - (p.bar.summary_rect[0] + widest);
    eprintln!("widest summary {widest}, gap to the label {gap}");
    assert!(gap >= tk::VIEW_BAR_GAP, "{widest} {gap} {:?}", p.bar);
}

/// Every drawn shape, nested ones included.
fn flat(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Shape> {
    fn walk(s: &egui::Shape, out: &mut Vec<egui::Shape>) {
        match s {
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            x => out.push(x.clone()),
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

/// The field's face as last painted: its one `surface.inset` rect on the field's own bounds.
fn face(t: &T, tr: TrackId) -> egui::epaint::RectShape {
    let f = start(t, tr).field;
    let fill = crate::theme::TOKENS.surface_inset;
    let faces: Vec<_> = flat(&t.shapes)
        .into_iter()
        .filter_map(|s| match s {
            egui::Shape::Rect(r)
                if r.fill == fill
                    && (r.rect.min.x - f[0]).abs() < EPS
                    && (r.rect.min.y - f[1]).abs() < EPS
                    && (r.rect.max.x - f[2]).abs() < EPS
                    && (r.rect.max.y - f[3]).abs() < EPS =>
            {
                Some(r)
            }
            _ => None,
        })
        .collect();
    assert_eq!(faces.len(), 1, "one face on the field's bounds: {faces:?}");
    faces[0].clone()
}

#[test]
fn each_field_state_picks_its_edge_or_ring() {
    use super::bar::{FieldState as S, field_stroke};
    let t = &crate::theme::TOKENS;
    assert_eq!(field_stroke(S::Rest), egui::Stroke::new(1.0, t.line));
    assert_eq!(field_stroke(S::Hover), egui::Stroke::new(1.0, t.line_strong));
    assert_eq!(field_stroke(S::Dragging), egui::Stroke::new(1.5, t.signal));
    assert_eq!(field_stroke(S::Typing), egui::Stroke::new(1.5, t.signal));
    assert_eq!(field_stroke(S::Error), egui::Stroke::new(1.5, t.meter_hot));
    // The error wins over every other state, dragging over typing, both over hover.
    assert_eq!(S::of(true, true, true, true), S::Error);
    assert_eq!(S::of(false, false, false, true), S::Error, "focused or not");
    assert_eq!(S::of(true, true, false, false), S::Dragging);
    assert_eq!(S::of(true, false, true, false), S::Typing);
    assert_eq!(S::of(true, false, false, false), S::Hover);
    assert_eq!(S::of(false, false, false, false), S::Rest);
}

#[test]
fn the_field_draws_each_state_on_its_own_edge() {
    let (mut t, tr) = T::kit();
    let tok = &crate::theme::TOKENS;
    let check = |t: &T, state: super::bar::FieldState, width: f32, color: egui::Color32| {
        let s = start(t, tr);
        assert_eq!(s.state, state);
        let r = face(t, tr);
        assert_eq!((r.stroke.width, r.stroke.color), (width, color), "{state:?}");
        assert_eq!(r.stroke_kind, egui::StrokeKind::Inside, "{state:?}: inside the field's bounds, offset 0");
    };
    use super::bar::FieldState as S;
    // Rest: the pointer on the summary.
    t.point(mid(t.panel(tr).bar.summary_rect));
    check(&t, S::Rest, 1.0, tok.line);
    // Hover.
    let at = mid(start(&t, tr).field);
    t.point(at);
    check(&t, S::Hover, 1.0, tok.line_strong);
    // Dragging: held down and moved, not yet released.
    let press = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let a = egui::pos2(at.0, at.1);
    t.input(vec![egui::Event::PointerMoved(a), press(a, true)]);
    t.input(vec![egui::Event::PointerMoved(a - egui::vec2(0.0, 8.0))]);
    check(&t, S::Dragging, 1.5, tok.signal);
    t.input(vec![press(a - egui::vec2(0.0, 8.0), false)]);
    assert_eq!(t.patch(tr).pad_start_note, 37, "the held drag stepped once, as a drag does");
    // Typing.
    t.click(at, 2);
    check(&t, S::Typing, 1.5, tok.signal);
    // Error: text that is not a note, still typing.
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": "H2"}));
    t.key("Enter");
    check(&t, S::Error, 1.5, tok.meter_hot);
    // Error, focused or not: the pointer away keeps it.
    t.point(mid(t.panel(tr).bar.summary_rect));
    check(&t, S::Error, 1.5, tok.meter_hot);
    t.key("Escape");
    check(&t, S::Rest, 1.0, tok.line);
}

#[test]
fn the_note_name_sits_inside_the_field_padding() {
    let (mut t, tr) = T::kit();
    let [v, h] = tokens::chrome::FIELD_PADDING;
    assert_eq!((v, h), (0.0, 8.0));
    // Shown: centred, at least 8 a side.
    for note in [0u8, 1, 36, 61, 64] {
        t.run("mix.sampler_pad_start", json!({"track": tr.0, "note": note}));
        let s = start(&t, tr);
        let (f, r) = (s.field, s.text_rect);
        assert!(r[0] - f[0] >= h - EPS && f[2] - r[2] >= h - EPS, "{}: {r:?} in {f:?}", s.text);
        assert!(((r[0] + r[2]) / 2.0 - (f[0] + f[2]) / 2.0).abs() < EPS, "{}: centred", s.text);
        assert!(((r[1] + r[3]) / 2.0 - (f[1] + f[3]) / 2.0).abs() < 0.5, "{}: centred on the height", s.text);
    }
    // Typing: the box is the field less 8 a side, the full height (0 vertical).
    t.click(mid(start(&t, tr).field), 2);
    let s = start(&t, tr);
    let (f, r) = (s.field, s.text_rect);
    assert!(s.editing);
    assert!((r[0] - f[0] - h).abs() < EPS && (f[2] - r[2] - h).abs() < EPS, "{r:?} in {f:?}");
    assert!((r[1] - f[1] - v).abs() < EPS && (f[3] - r[3] - v).abs() < EPS, "{r:?} in {f:?}");
    // Every name of the range, under both Middle C settings, fits the field less its padding.
    let painter = egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING);
    for mc in [MiddleC::C3, MiddleC::C4] {
        for n in 0..=transit_model::pad_notes::MAX_PAD_START {
            let w = crate::widgets::role_size(&painter, &tokens::VALUE, &note_name(n, mc)).x;
            assert!(w <= tk::START_NOTE_WIDTH - 2.0 * h, "{}: {w}", note_name(n, mc));
        }
    }
}

#[test]
fn the_start_note_token_range_is_the_models() {
    assert_eq!(tk::START_NOTE_RANGE, [0, transit_model::pad_notes::MAX_PAD_START]);
}

/// Opens the typing, replaces the text with `text` and clicks far from the window (the
/// arrangement), without Enter.
fn type_and_click_away(t: &mut T, tr: TrackId, text: &str) {
    let field = start(t, tr).field;
    t.click(mid(field), 2);
    assert!(start(t, tr).editing, "a double-click opens the typing");
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": text}));
    t.click((5.0, 890.0), 1);
}

#[test]
fn a_click_elsewhere_commits_the_typing_and_hands_the_keyboard_back() {
    // #484: the field asked for the keyboard every frame, so a click elsewhere never ended the
    // typing and the app's shortcuts stayed dead until Enter or Escape.
    let (mut t, tr) = T::kit();
    let before = t.undo_len();
    type_and_click_away(&mut t, tr, "d1");
    assert_eq!(t.patch(tr).pad_start_note, 38, "the typed note is committed");
    assert_eq!(t.undo_len(), before + 1, "one step");
    assert!(!start(&t, tr).editing, "the typing ended");
    assert!(!t.ctx.egui_wants_keyboard_input(), "the keyboard is the app's again");
    // Text typed after the click goes nowhere near the field.
    t.send("ui.text", json!({"text": "7"}));
    assert_eq!(t.patch(tr).pad_start_note, 38);
    assert!(!start(&t, tr).editing);
    // Refused text on a click elsewhere: nothing changes, the note says why, and the typing ends.
    type_and_click_away(&mut t, tr, "H2");
    assert_eq!(t.patch(tr).pad_start_note, 38);
    assert_eq!(t.undo_len(), before + 1, "no step");
    let s = start(&t, tr);
    assert!(!s.editing && !s.error, "the typing ended without an error ring");
    let note = t.panel(tr).note.expect("the reason is shown").0;
    assert!(note.starts_with("No start note H2:"), "{note}");
    assert!(!t.ctx.egui_wants_keyboard_input());
}
