//! Piano roll (slice 3) tests. Every gesture is driven the way a user drives it: synthetic
//! clicks, drags, wheel and keys through the control channel's input queue, one frame at a time.
//! No player, no plug-ins, no scan.

#[path = "keys/overlay_tests.rs"]
mod overlay_tests;
#[path = "keys/step_tests.rs"]
mod step_tests;
#[path = "keys/view_tests.rs"]
mod view_tests;

use crate::keys::Snap;
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_midi::Note;
use transit_model::{ClipContent, ClipId, TrackId};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    width: f32,
    painted_text: Vec<String>,
    track: TrackId,
    clip: ClipId,
}

impl T {
    /// An instrument track with a one-bar MIDI clip at bar 1, the mixer pane shown.
    fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
        let track = TrackId(r["tracks"][0].as_u64().unwrap());
        let c = app.run("midi.new_clip", json!({"tracks": [track.0], "start": 0, "end": 0})).unwrap();
        let clip = ClipId(c["clip"].as_u64().unwrap());
        app.ui.show_mixer = true;
        app.ui.show_devices = false;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, width: 1400.0, painted_text: Vec::new(), track, clip };
        t.frames(3);
        t
    }

    /// Opened by a double-click on the clip in the arrangement.
    fn open() -> T {
        let mut t = T::new();
        let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
        t.click_n(mid(r), 2, json!({}));
        assert!(t.app.keys.layout.visible, "double-click opened the piano roll");
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(self.width, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            self.painted_text = out
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if shape.clip_rect.intersects(text.galley.rect.translate(text.pos.to_vec2())) => {
                        Some(text.galley.text().to_owned())
                    }
                    _ => None,
                })
                .collect();
            out.textures_delta.clear();
        }
    }

    fn input(&mut self, method: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(30);
    }

    fn click_n(&mut self, at: (f32, f32), count: u64, mods: Value) {
        self.input("ui.click", merged(json!({"x": at.0, "y": at.1, "count": count}), mods));
    }

    fn click(&mut self, at: (f32, f32)) {
        self.click_n(at, 1, json!({}));
    }

    fn drag(&mut self, from: (f32, f32), to: (f32, f32), mods: Value) {
        self.input("ui.drag", merged(json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1}), mods));
    }

    fn key(&mut self, key: &str, mods: Value) {
        self.input("ui.key", merged(json!({"key": key}), mods));
    }

    /// The point on the grid at `tick` (from the clip start) in the middle of `pitch`'s row.
    fn at(&self, tick: f64, pitch: u8) -> (f32, f32) {
        let l = &self.app.keys.layout;
        let x = l.grid[0] + ((tick - l.scroll_ticks) * l.px_per_tick) as f32;
        let y = l.grid[1] + (l.top_pitch - f32::from(pitch)) * l.key_h + l.key_h / 2.0;
        (x, y)
    }

    fn notes(&self) -> Vec<Note> {
        match &self.app.engine.session().find_clip(self.clip).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.clone(),
            _ => Vec::new(),
        }
    }

    fn undo_len(&self) -> usize {
        self.app.engine.undo_history().len()
    }

    fn add(&mut self, pitch: u8, start: i64, length: i64) {
        self.app.run("midi.note_add", json!({"clip": self.clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": length})).unwrap();
        self.frames(2);
    }

    fn note_rect(&self, pitch: u8, start: i64) -> [f32; 4] {
        self.app.keys.layout.notes.iter().find(|n| n.pitch == pitch && n.start == start).map(|n| n.rect).unwrap()
    }

    fn selected(&self) -> usize {
        self.app.keys.layout.selected
    }
}

fn merged(mut a: Value, b: Value) -> Value {
    if let (Some(o), Some(m)) = (a.as_object_mut(), b.as_object()) {
        o.extend(m.clone());
    }
    a
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

#[test]
fn note_length_alt_resizes_both_edges_below_fixed_and_adaptive_grids_and_undoes() {
    for adaptive in [false, true] {
        for start_edge in [false, true] {
            for (alt, wanted) in [(true, 60), (true, 1), (false, 240)] {
                let mut t = T::open();
                (if adaptive { Snap::Adaptive } else { Snap::Sixteenth }).apply(&mut t.app.ui.keys.grid);
                t.add(60, 480, 960);
                let before = t.notes();
                let undo = t.undo_len();
                let r = t.note_rect(60, 480);
                let grab = (if start_edge { r[0] + 1.0 } else { r[2] - 1.0 }, mid(r).1);
                let dx = (960 - wanted) as f32 * t.app.keys.layout.px_per_tick as f32;
                t.drag(grab, (grab.0 + if start_edge { dx } else { -dx }, grab.1), json!({"alt": alt}));
                let note = t.notes()[0];
                let expected = if alt { wanted } else { t.app.keys.layout.grid_step };
                assert_eq!(note.length, expected, "adaptive={adaptive} start={start_edge} alt={alt}");
                assert_eq!(note.start, if start_edge { before[0].end() - expected } else { 480 });
                assert_eq!(note.velocity, before[0].velocity);
                assert_eq!(t.undo_len(), undo + 1);
                t.app.run("edit.undo", json!({})).unwrap();
                assert_eq!(t.notes(), before);
            }
        }
    }
}

#[test]
fn note_length_alt_shift_move_and_copy_bypass_snap_without_changing_velocity_or_length() {
    for copy in [false, true] {
        let mut t = T::open();
        t.add(60, 480, 480);
        let undo = t.undo_len();
        let grab = t.at(700.0, 60);
        let to = t.at(700.0 + 137.0, 62);
        t.drag(grab, to, json!({"alt": true, "shift": true, "cmd": copy}));
        let notes = t.notes();
        let moved = notes.iter().find(|n| n.pitch == 62).unwrap();
        assert_eq!((moved.start, moved.length, moved.velocity), (617, 480, 100));
        assert_eq!(notes.len(), if copy { 2 } else { 1 });
        assert_eq!(t.undo_len(), undo + 1);
        t.app.run("edit.undo", json!({})).unwrap();
        assert_eq!((t.notes().len(), t.notes()[0].start), (1, 480));
    }
}

#[test]
fn note_length_inspector_wraps_below_the_toolbar_in_a_narrow_window() {
    let mut t = T::open();
    t.width = 1024.0;
    t.add(60, 480, 480);
    t.click(mid(t.note_rect(60, 480)));
    let l = &t.app.keys.layout;
    assert!(l.note_length[1] > l.tab_keys[3]);
    assert!(l.note_length[3] < l.ruler[1]);
    t.click(mid(t.app.keys.layout.note_length));
    t.key("A", json!({"cmd": true}));
    t.input("ui.text", json!({"text": "37"}));
    t.key("Enter", json!({}));
    assert_eq!(t.notes()[0].length, 37);
}

#[test]
fn note_length_inspector_accepts_exact_ticks_rejects_invalid_and_undoes_once() {
    let mut t = T::open();
    t.add(60, 480, 480);
    t.add(64, 960, 240);
    t.click(mid(t.note_rect(60, 480)));
    for (text, expected, changed) in
        [("60", 60, true), ("1", 1, true), ("0", 1, false), ("-5", 1, false), ("9223372036854775807", 1, false), ("bad", 1, false)]
    {
        t.click(mid(t.note_rect(60, 480)));
        let before = t.notes();
        let undo = t.undo_len();
        t.click(mid(t.app.keys.layout.note_length));
        t.key("A", json!({"cmd": true}));
        t.key("Delete", json!({}));
        assert_eq!(t.notes(), before, "Delete in the inspector must not delete notes");
        t.input("ui.text", json!({"text": text}));
        assert_eq!(t.notes(), before, "typing is a draft until committed");
        assert_eq!(t.selected(), 1, "Ctrl+A in the inspector must not select every note");
        t.key("Enter", json!({}));
        assert_eq!(t.notes()[0].length, expected, "input {text}");
        assert_eq!(t.notes()[1], before[1], "unselected note is untouched");
        assert_eq!(t.undo_len(), undo + usize::from(changed));
        if changed {
            t.app.run("edit.undo", json!({})).unwrap();
            assert_eq!(t.notes(), before);
            t.app.run("edit.redo", json!({})).unwrap();
            t.frames(2);
        }
    }
}

#[test]
fn note_length_group_resize_and_snap_off_keep_one_tick_minimum_without_touching_step_length() {
    for start_edge in [false, true] {
        let mut t = T::open();
        Snap::Off.apply(&mut t.app.ui.keys.grid);
        t.add(60, 480, 960);
        t.add(64, 720, 480);
        t.app.keys.selection = t.notes();
        let before = t.notes();
        let step = t.app.keys_step();
        let undo = t.undo_len();
        let r = t.note_rect(60, 480);
        let grab = (if start_edge { r[0] + 1.0 } else { r[2] - 1.0 }, mid(r).1);
        let dx = 900.0 * t.app.keys.layout.px_per_tick as f32;
        t.drag(grab, (grab.0 + if start_edge { dx } else { -dx }, grab.1), json!({}));
        let after = t.notes();
        assert_eq!(after.iter().map(|n| n.length).collect::<Vec<_>>(), if start_edge { vec![1, 60] } else { vec![60, 1] });
        for note in &after {
            let original = before.iter().find(|n| n.pitch == note.pitch).unwrap();
            assert_eq!(if start_edge { note.end() } else { note.start }, if start_edge { original.end() } else { original.start });
        }
        assert_eq!(t.app.keys_step(), step);
        assert_eq!(t.undo_len(), undo + 1);
        t.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(t.notes(), before);
    }
}

#[test]
fn note_length_one_tick_notes_keep_both_edges_reachable_and_start_resize_stops_at_zero() {
    for start_edge in [false, true] {
        let mut t = T::open();
        t.add(60, 480, 1);
        let r = t.note_rect(60, 480);
        let grab = (if start_edge { r[0] + 0.25 } else { r[2] - 0.25 }, mid(r).1);
        let dx = 300.0 * t.app.keys.layout.px_per_tick as f32;
        t.drag(grab, (grab.0 + if start_edge { -dx } else { dx }, grab.1), json!({"alt":true}));
        assert_eq!((t.notes()[0].start, t.notes()[0].length), (if start_edge { 180 } else { 480 }, 301));
    }
    let mut t = T::open();
    t.add(60, 480, 480);
    let r = t.note_rect(60, 480);
    let grab = (r[0] + 1.0, mid(r).1);
    t.drag(grab, (grab.0 - 900.0 * t.app.keys.layout.px_per_tick as f32, grab.1), json!({"alt":true}));
    assert_eq!((t.notes()[0].start, t.notes()[0].length), (0, 960));
}

#[test]
fn note_length_preview_is_visible_before_release_and_matches_committed_ticks() {
    let mut t = T::open();
    t.add(60, 480, 960);
    let before = t.notes();
    let r = t.note_rect(60, 480);
    let grab = (r[2] - 1.0, mid(r).1);
    let to = (grab.0 - 900.0 * t.app.keys.layout.px_per_tick as f32, t.app.keys.layout.grid[1] + 1.0);
    let (req, _rx) = crate::ControlRequest::new("ui.drag", json!({"x":grab.0,"y":grab.1,"to_x":to.0,"to_y":to.1,"alt":true,"steps":12}));
    crate::control::handle_for_test(&mut t.app, &t.ctx, &req).unwrap();
    let mut lengths = Vec::new();
    let mut painted = false;
    for _ in 0..30 {
        t.frames(1);
        if !t.app.keys.layout.drag_lengths.is_empty() && t.notes() == before {
            lengths.extend_from_slice(&t.app.keys.layout.drag_lengths);
            painted |= t.painted_text.iter().any(|text| text == "60 ticks");
        }
    }
    assert!(lengths.contains(&60), "live preview must reach the exact length before commit: {lengths:?}");
    assert!(painted, "the length must reach the visible paint output before release");
    assert_eq!(t.notes()[0].length, 60);
    assert!(t.app.keys.layout.drag_lengths.is_empty(), "readout disappears on release");
}

#[test]
fn quantize_preview_cancel_apply_and_undo_preserve_unselected_notes() {
    let mut t = T::open();
    t.add(60, 100, 220);
    t.add(64, 600, 190);
    t.app.keys.selection = vec![t.notes()[0]];
    t.app.ui.keys.quantize.strength = 50;
    t.app.ui.keys.quantize.ends = true;
    let before = t.notes();
    let undo = t.undo_len();
    t.click(mid(t.app.keys.layout.quantize));
    assert!(t.app.keys.quantize_open);
    assert_eq!(t.notes(), before, "opening preview never edits the document");
    assert_eq!(t.undo_len(), undo);
    let expected = Note { start: 50, length: 230, ..before[0] };
    assert_eq!(t.app.keys.layout.quantize_preview, vec![expected]);
    t.click(mid(t.app.keys.layout.quantize_cancel));
    assert!(!t.app.keys.quantize_open);
    assert_eq!(t.notes(), before);
    assert_eq!(t.undo_len(), undo);
    t.click(mid(t.app.keys.layout.quantize));
    t.click(mid(t.app.keys.layout.quantize_apply));
    assert_eq!(t.notes(), vec![expected, before[1]]);
    assert_eq!(t.app.keys.selection, vec![expected]);
    assert_eq!(t.undo_len(), undo + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes(), before);
}

#[test]
fn quantize_preview_swing_and_zero_strength_match_apply_and_saved_preferences() {
    let mut t = T::open();
    t.add(60, 480, 120);
    t.add(64, 960, 120);
    t.app.keys.selection = vec![t.notes()[0]];
    t.app.ui.keys.quantize.swing = 100;
    t.click(mid(t.app.keys.layout.quantize));
    t.app.ui.keys.quantize.grid_ticks = 480;
    t.frames(2);
    let before = t.notes();
    assert_eq!(t.app.keys.layout.quantize_preview, vec![Note { start: 640, ..before[0] }]);
    t.app.ui.keys.quantize.strength = 0;
    t.frames(2);
    assert_eq!(t.app.keys.layout.quantize_preview, vec![before[0]]);
    let undo = t.undo_len();
    t.click(mid(t.app.keys.layout.quantize_apply));
    assert_eq!(t.notes(), before);
    assert_eq!(t.undo_len(), undo);
    t.app.ui.keys.quantize.strength = 100;
    t.click(mid(t.app.keys.layout.quantize));
    t.app.ui.keys.quantize.grid_ticks = 480;
    t.frames(2);
    t.click(mid(t.app.keys.layout.quantize_apply));
    assert_eq!(t.notes(), vec![Note { start: 640, ..before[0] }, before[1]]);
    let saved = serde_json::to_value(&t.app.ui.keys).unwrap();
    let restored: crate::keys::KeysPrefs = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.quantize.swing, 100);
    assert_eq!(restored.quantize.grid_ticks, 480);
    let legacy: crate::keys::KeysPrefs = serde_json::from_value(json!({})).unwrap();
    assert_eq!(legacy.quantize.strength, 100);
    assert_eq!(legacy.quantize.swing, 0);
    assert!(!legacy.quantize.ends);
}

#[test]
fn pitch_menu_transposes_only_selected_notes_and_undoes_once() {
    let mut t = T::open();
    t.add(60, 0, 240);
    t.add(126, 480, 240);
    t.add(80, 960, 240);
    let original = t.notes();
    t.app.keys.selection = original[..2].to_vec();
    t.frames(2);
    let undo = t.undo_len();
    t.click(mid(t.app.keys.layout.pitch));
    let item = t.app.keys.layout.pitch_menu.iter().find(|m| m.label == "Up an octave").cloned().unwrap();
    t.click(mid(item.rect));
    assert_eq!(t.notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [72, 127, 80]);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.app.keys.selection, t.notes()[..2]);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes(), original);
}

#[test]
fn pitch_menu_constrains_selection_and_key_preferences_round_trip() {
    use transit_midi::scale::{KeyScale, Scale};
    let mut t = T::open();
    t.add(64, 0, 240);
    t.add(69, 480, 240);
    t.add(71, 960, 240);
    let original = t.notes();
    t.app.keys.selection = original[..2].to_vec();
    t.app.ui.keys.key_scale = KeyScale { root: 0, scale: Scale::Minor };
    t.app.run("midi.scale_mode", json!({"clip": t.clip.0, "root": 0, "scale": "minor"})).unwrap();
    let saved = serde_json::to_vec(&t.app.ui).unwrap();
    let restored: crate::UiState = serde_json::from_slice(&saved).unwrap();
    assert_eq!(restored.keys.key_scale, t.app.ui.keys.key_scale);
    let legacy: crate::keys::KeysPrefs = serde_json::from_value(json!({})).unwrap();
    assert_eq!(legacy.key_scale, KeyScale::default());
    t.frames(2);
    let undo = t.undo_len();
    t.click(mid(t.app.keys.layout.pitch));
    let item = t.app.keys.layout.pitch_menu.iter().find(|m| m.label == "constrain").cloned().unwrap();
    t.click(mid(item.rect));
    assert_eq!(t.notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [63, 68, 71]);
    assert_eq!(t.app.keys.selection, t.notes()[..2]);
    assert_eq!(t.undo_len(), undo + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes(), original);
    t.app.keys.selection.clear();
    t.app.keys_constrain().unwrap();
    assert_eq!(t.notes(), original, "empty selection does not change the clip");
}

#[test]
fn a_double_click_on_the_grid_adds_a_snapped_note_and_a_double_click_on_it_deletes_it() {
    let mut t = T::open();
    let undo = t.undo_len();
    // Just after the first sixteenth: the note goes in that cell, a sixteenth long, velocity 100.
    t.click_n(t.at(300.0, 62), 2, json!({}));
    let notes = t.notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!((notes[0].pitch, notes[0].start, notes[0].length, notes[0].velocity), (62, 240, 240, 100));
    assert_eq!(t.undo_len(), undo + 1, "one undo entry");
    assert_eq!(t.app.keys.layout.last_audition, Some((62, 100)), "placing auditions it");
    assert_eq!(t.selected(), 1, "the new note is selected");
    t.click_n(mid(t.note_rect(62, 240)), 2, json!({}));
    assert!(t.notes().is_empty(), "double-click on a note deletes it");
    assert_eq!(t.undo_len(), undo + 2);
}

#[test]
fn click_shift_click_rubber_band_ctrl_a_and_delete_select_and_remove() {
    let mut t = T::open();
    t.add(60, 0, 240);
    t.add(64, 480, 240);
    t.add(67, 960, 240);
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.selected(), 1);
    t.click_n(mid(t.note_rect(64, 480)), 1, json!({"shift": true}));
    assert_eq!(t.selected(), 2, "Shift-click adds");
    t.click(t.at(1800.0, 62));
    assert_eq!(t.selected(), 0, "a click on empty grid clears the selection");
    // Rubber band over the first two notes, from empty space.
    let from = t.at(1.0, 66);
    let to = t.at(700.0, 58);
    t.drag(from, to, json!({}));
    assert_eq!(t.selected(), 2, "rubber band");
    let undo = t.undo_len();
    t.key("Delete", json!({}));
    let left: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    assert_eq!(left, vec![67], "Delete removes the selection");
    assert_eq!(t.undo_len(), undo + 1);
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 1, "Ctrl+A");
    t.key("Backspace", json!({}));
    assert!(t.notes().is_empty());
    assert_eq!(t.app.arrange.layout.clips.len(), 1, "the arrangement's Delete did not run: the clip stays");
}

#[test]
fn dragging_moves_snapped_the_edge_resizes_and_ctrl_drag_copies_a_note_or_the_selection() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let undo = t.undo_len();
    // Move: about two sixteenths and a bit later, two keys up.
    let grab = t.at(120.0, 60);
    let to = t.at(120.0 + 510.0, 62);
    t.drag(grab, to, json!({}));
    let n = t.notes();
    assert_eq!((n.len(), n[0].start, n[0].pitch, n[0].length), (1, 480, 62, 480), "{n:?}");
    assert_eq!(t.undo_len(), undo + 1, "a move is one undo entry");
    // Resize from the right edge by a sixteenth and a bit.
    let r = t.note_rect(62, 480);
    let px = 260.0 * t.app.keys.layout.px_per_tick as f32;
    t.drag((r[2] - 2.0, (r[1] + r[3]) / 2.0), (r[2] - 2.0 + px, (r[1] + r[3]) / 2.0), json!({}));
    let n = t.notes();
    assert_eq!((n[0].start, n[0].length), (480, 720), "{n:?}");
    assert_eq!(t.undo_len(), undo + 2);
    // Ctrl+drag copies one bar later; the original stays.
    let grab = t.at(600.0, 62);
    let to = t.at(600.0 + 3840.0, 62);
    t.drag(grab, to, json!({"cmd": true}));
    let n = t.notes();
    assert_eq!(n.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>(), vec![(480, 62), (4320, 62)], "{n:?}");
    assert_eq!(t.undo_len(), undo + 3, "a copy is one undo entry");
    // Highlight both and Ctrl+drag the selection down an octave: two more notes in one step.
    let a = t.at(300.0, 64);
    let b = t.at(5400.0, 61);
    t.drag(a, b, json!({}));
    assert_eq!(t.selected(), 2);
    let grab = t.at(600.0, 62);
    let to = t.at(600.0, 50);
    t.drag(grab, to, json!({"cmd": true}));
    let mut pitches: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    pitches.sort();
    assert_eq!(pitches, vec![50, 50, 62, 62]);
    assert_eq!(t.undo_len(), undo + 4, "copying a selection is one undo entry");
    assert_eq!(t.selected(), 2, "the copies are selected");
    assert!(t.app.keys.layout.notes.iter().filter(|n| n.selected).all(|n| n.pitch == 50));
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.notes().len(), 2, "undo removes both copies");
}

#[test]
fn alt_drag_and_the_velocity_lane_set_velocity_in_one_step() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let undo = t.undo_len();
    let grab = t.at(200.0, 60);
    t.drag(grab, (grab.0, grab.1 - 30.0), json!({"alt": true}));
    let n = t.notes();
    assert_eq!((n[0].velocity, n[0].start, n[0].pitch), (120, 0, 60), "Alt+drag up 30 points: +20");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.app.ui.keys.velocity, 100, "editing a note leaves the new-note velocity alone");
    // The velocity lane: drag the stem to about a quarter height.
    let l = t.app.keys.layout.clone();
    let stem = l.notes[0].vel_rect;
    let y = l.velocity[3] - (l.velocity[3] - l.velocity[1] - 6.0) * 32.0 / 127.0;
    t.drag((stem[0] + 1.0, stem[1] + 2.0), (stem[0] + 1.0, y), json!({}));
    let v = t.notes()[0].velocity;
    assert!((30..=34).contains(&v), "lane drag set {v}");
    assert_eq!(t.undo_len(), undo + 2);
    // A click in the lane sets it at that height.
    let y = l.velocity[3] - (l.velocity[3] - l.velocity[1] - 6.0) * 100.0 / 127.0;
    t.click((stem[0] + 1.0, y));
    let v = t.notes()[0].velocity;
    assert!((98..=102).contains(&v), "lane click set {v}");
    // A new note uses the independent toolbar preference.
    t.click_n(t.at(1000.0, 64), 2, json!({}));
    let new = t.notes().into_iter().find(|n| n.pitch == 64).unwrap();
    assert_eq!(new.velocity, 100);
}

#[test]
fn new_note_velocity_field_survives_note_edits_and_drives_drawing_and_audition() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    // Edit the actual toolbar field, rather than assigning the preference in the test.
    t.click(mid(t.app.keys.layout.new_note_velocity));
    t.key("A", json!({"cmd": true}));
    t.input("ui.text", json!({"text": "87"}));
    t.key("Enter", json!({}));
    assert_eq!(t.app.ui.keys.velocity, 87);
    assert_eq!(t.undo_len(), undo);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);

    let grab = t.at(200.0, 60);
    t.drag(grab, (grab.0, grab.1 + 148.5), json!({"alt": true}));
    assert_eq!(t.notes()[0].velocity, 1, "reproduce the near-silent edited note");
    assert_eq!(t.app.ui.keys.velocity, 87, "Alt+drag must not change the field");
    // Selecting the quiet note auditions at the field's velocity without adopting its velocity.
    t.click(mid(t.note_rect(60, 0)));
    assert_eq!(t.app.keys.last_audition, Some((60, 87)));
    assert_eq!(t.app.ui.keys.velocity, 87);
    // Both velocity-lane dragging and clicking leave the preference alone, too.
    let l = t.app.keys.layout.clone();
    let stem = l.notes[0].vel_rect;
    let y = l.velocity[3] - (l.velocity[3] - l.velocity[1] - 6.0) * 32.0 / 127.0;
    t.drag((stem[0] + 1.0, stem[3] - 1.0), (stem[0] + 1.0, y), json!({}));
    assert!((30..=34).contains(&t.notes()[0].velocity));
    assert_eq!(t.app.ui.keys.velocity, 87);
    t.click((stem[0] + 1.0, l.velocity[3] - 0.1));
    assert_eq!(t.notes()[0].velocity, 1);
    assert_eq!(t.app.ui.keys.velocity, 87);

    t.click_n(t.at(1000.0, 64), 2, json!({}));
    assert_eq!(t.notes().iter().find(|n| n.pitch == 64).unwrap().velocity, 87);
    assert_eq!(t.app.keys.last_audition, Some((64, 87)));
    // Moving an existing quiet note auditions its new pitch at the same toolbar velocity.
    let grab = t.at(200.0, 60);
    t.drag(grab, (grab.0, grab.1 - t.app.ui.keys.key_h), json!({}));
    assert_eq!(t.app.keys.last_audition, Some((61, 87)));
    assert_eq!(t.notes().iter().find(|n| n.pitch == 61).unwrap().velocity, 1);
    let kb = t.app.keys.layout.keyboard;
    t.click(((kb[0] + kb[2]) / 2.0, t.at(0.0, 65).1));
    assert_eq!(t.app.keys.last_audition, Some((65, 87)));
}

#[test]
fn velocity_eyedropper_requires_one_selected_note_and_is_ui_state_only() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.add(64, 960, 480);
    t.app.run("midi.note_edit", json!({"clip": t.clip.0, "index": 0, "velocity": 37})).unwrap();
    t.frames(2);
    t.click(mid(t.app.keys.layout.use_selected_velocity));
    assert_eq!(t.app.ui.keys.velocity, 100, "no selection: disabled");
    t.click(mid(t.note_rect(60, 0)));
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    assert_eq!(t.app.ui.keys.velocity, 100, "selection alone never copies");
    t.click(mid(t.app.keys.layout.use_selected_velocity));
    assert_eq!(t.app.ui.keys.velocity, 37, "explicit eyedropper copies the selected note");
    assert_eq!(t.undo_len(), undo);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    t.app.run("ui.set", json!({"keys": {"velocity": 87}})).unwrap();
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 2);
    t.click(mid(t.app.keys.layout.use_selected_velocity));
    assert_eq!(t.app.ui.keys.velocity, 87, "multiple notes: disabled");
}

#[test]
fn new_note_velocity_defaults_bounds_and_control_updates_preserve_other_tool_settings() {
    let mut t = T::open();
    let old: crate::UiState = serde_json::from_value(json!({"keys": {"snap": "1/8"}})).unwrap();
    assert_eq!(old.keys.velocity, 100, "old ui.json without velocity gets the default");
    t.app.run("ui.set", json!({"keys": {"grid": {"mode": {"Fixed": 480}}, "audition": false}})).unwrap();
    for (value, expected) in [(-10, 1), (0, 1), (1, 1), (87, 87), (127, 127), (128, 127), (300, 127)] {
        t.app.run("ui.set", json!({"keys": {"velocity": value}})).unwrap();
        assert_eq!(t.app.ui.keys.velocity, expected);
        assert_eq!(Snap::of(&t.app.ui.keys.grid), Some(Snap::Eighth));
        assert!(!t.app.ui.keys.audition);
        assert!(t.app.keys_shown());
        assert_eq!(t.app.inspect(&t.ctx)["ui"]["keys"]["velocity"], json!(expected));
        let saved = serde_json::to_vec(&t.app.ui).unwrap();
        let restored: crate::UiState = serde_json::from_slice(&saved).unwrap();
        assert_eq!(restored.keys.velocity, expected);
    }
    for value in [json!("bad"), json!(null), json!(1.5)] {
        assert!(t.app.run("ui.set", json!({"keys": {"velocity": value}})).is_err());
        assert_eq!(t.app.ui.keys.velocity, 127, "invalid updates are atomic");
    }
}

#[test]
fn ctrl_d_arrows_ctrl_up_and_ctrl_u_edit_the_selection() {
    let mut t = T::open();
    t.add(60, 0, 240);
    t.click(mid(t.note_rect(60, 0)));
    let undo = t.undo_len();
    t.key("D", json!({"cmd": true}));
    let n = t.notes();
    assert_eq!(n.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 240], "Ctrl+D: one grid length after itself");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.app.keys.selection, vec![n[1]], "the copy is selected");
    assert_eq!(t.app.arrange.layout.clips.len(), 1, "the arrangement's Ctrl+D did not run");
    t.key("ArrowRight", json!({}));
    t.key("ArrowUp", json!({}));
    t.key("ArrowUp", json!({"shift": true}));
    let moved = t.app.keys.selection[0];
    assert_eq!((moved.start, moved.pitch), (480, 73), "right a sixteenth, up one, up an octave");
    assert_eq!(t.undo_len(), undo + 4, "each nudge is one step");
    // Ctrl+Up with everything selected: event.transpose on the clip.
    t.key("A", json!({"cmd": true}));
    t.key("ArrowUp", json!({"cmd": true}));
    let mut p: Vec<u8> = t.notes().iter().map(|n| n.pitch).collect();
    p.sort();
    assert_eq!(p, vec![72, 85]);
    assert_eq!(t.app.engine.undo_history().last().map(String::as_str), Some("Transpose..."));
    assert_eq!(t.selected(), 2, "the selection is read back after the edit");
    // Ctrl+U on one off-grid note of three: only that one moves.
    t.add(62, 1000, 200);
    t.click(mid(t.note_rect(62, 1000)));
    t.key("U", json!({"cmd": true}));
    let n = t.notes();
    assert!(n.iter().any(|n| n.pitch == 62 && n.start == 960), "{n:?}");
    assert!(n.iter().any(|n| n.pitch == 85 && n.start == 480), "the other notes stay");
    // The Quantize button with everything selected runs event.quantize.
    t.app.run("midi.notes_move", json!({"clip": t.clip.0, "indices": [0, 1, 2], "ticks": 30})).unwrap();
    t.frames(2);
    t.key("A", json!({"cmd": true}));
    t.click(mid(t.app.keys.layout.quantize));
    t.click(mid(t.app.keys.layout.quantize_apply));
    assert_eq!(t.app.engine.undo_history().last().map(String::as_str), Some("Quantize..."));
    assert!(t.notes().iter().all(|n| n.start % 240 == 0), "{:?}", t.notes());
}

impl T {
    /// One frame with the given focus state; returns the repaint delay egui was asked for.
    fn frame_with(&mut self, focused: Option<bool>) -> std::time::Duration {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
            time: Some(self.time),
            ..Default::default()
        };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().focused = focused;
        self.app.raw_input_hook(&mut raw);
        let app = &mut self.app;
        let out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay)
    }
}

#[test]
fn an_audition_releases_by_itself_with_the_mouse_still_and_on_focus_loss_or_a_tab_switch() {
    let mut t = T::open();
    t.app.audition(60, t.time);
    let delay = t.frame_with(None);
    // timing: value the repaint delay the app requests (a computed interval), not a measured duration
    assert!(delay <= std::time::Duration::from_millis(50), "a repaint is requested while a note is held: {delay:?}");
    assert_eq!(t.app.keys.layout.sounding, vec![60]);
    // No input at all: frames spaced like the repaint requests release it after 150 ms.
    for _ in 0..3 {
        t.time += 0.05;
        t.frame_with(None);
    }
    assert!(t.app.keys.layout.sounding.is_empty(), "released without any event");
    // Focus loss releases at once.
    t.app.audition(61, t.time);
    t.frame_with(None);
    assert_eq!(t.app.keys.layout.sounding, vec![61]);
    t.frame_with(Some(false));
    assert!(t.app.keys.layout.sounding.is_empty(), "released on focus loss");
    // A tab switch (pane hidden) releases too.
    t.app.audition(62, t.time);
    t.frame_with(None);
    t.app.ui.keys.show = false;
    t.frame_with(None);
    assert!(t.app.keys.held_pitches().is_empty(), "released when the pane is hidden");
    let delay = t.frame_with(None);
    assert!(delay > std::time::Duration::from_millis(50), "no repaint once nothing is held");
}

#[test]
fn notes_past_the_clip_end_are_refused_on_add_and_flagged_when_moved_or_loaded() {
    let mut t = T::open();
    let len = t.app.keys.layout.clip_ticks;
    assert!(len > 0);
    t.add(60, 0, 240);
    // A double-click past the end adds nothing and says why.
    let undo = t.undo_len();
    t.click_n(t.at(len as f64 + 480.0, 64), 2, json!({}));
    assert_eq!(t.notes().len(), 1, "no note added past the end");
    assert_eq!(t.undo_len(), undo);
    assert_eq!(t.app.status, "Outside the clip: drag the clip end to extend");
    assert!(t.app.keys.layout.notes.iter().all(|n| !n.outside));
    // Moving a note past the end is allowed; it is flagged (drawn dimmed and hatched).
    t.app.keys.selection = t.notes();
    t.app.keys_move(len + 480, 0).unwrap();
    t.frames(2);
    assert_eq!(t.notes()[0].start, len + 480);
    assert!(t.app.keys.layout.notes[0].outside, "{:?}", t.app.keys.layout.notes);
    // A note already past the end (a loaded session) is flagged the same way.
    t.add(65, len + 10, 120);
    let n = t.app.keys.layout.notes.iter().find(|n| n.pitch == 65).unwrap();
    assert!(n.outside);
}

#[test]
fn an_empty_instrument_lane_shows_the_create_clip_hint_until_a_clip_exists() {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let track = TrackId(r["tracks"][0].as_u64().unwrap());
    let mut t = T { app, ctx: egui::Context::default(), time: 0.0, width: 1400.0, painted_text: Vec::new(), track, clip: ClipId(0) };
    t.frames(3);
    assert_eq!(t.app.keys.layout.hint_tracks, vec![track.0], "hint on the empty instrument lane");
    let lane = t.app.arrange.layout.tracks[0].lane;
    t.click_n(mid(lane), 2, json!({}));
    assert!(t.app.keys.layout.hint_tracks.is_empty(), "gone once a clip exists");
}

fn track_order(t: &T) -> Vec<TrackId> {
    t.app.engine.session().tracks.iter().map(|x| x.id).collect()
}

#[test]
fn alt_down_moves_the_track_while_the_piano_roll_is_active_and_leaves_the_notes() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let second = TrackId(t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.frames(2);
    t.app.run("edit.select", json!({"tracks": [t.track.0]})).unwrap();
    t.key("ArrowDown", json!({}));
    let moved = t.notes();
    t.app.run("edit.select", json!({"tracks": [t.track.0]})).unwrap();
    assert!(t.app.keys.active && t.app.keys_shown(), "the piano roll has the keyboard");
    let before = track_order(&t);
    let at = before.iter().position(|x| *x == t.track).unwrap();
    t.key("ArrowDown", json!({"alt": true}));
    let after = track_order(&t);
    assert_eq!(after.iter().position(|x| *x == t.track).unwrap(), at + 1, "Alt+Down moved the track below {second:?}");
    assert_eq!(t.notes(), moved, "and the notes did not move");
}

#[test]
fn ctrl_alt_c_is_not_copy_but_ctrl_c_is() {
    let mut t = T::new();
    t.app.click_clip(t.clip, false);
    t.frames(2);
    t.key("C", json!({"cmd": true, "alt": true}));
    assert!(t.app.copied.is_empty(), "Ctrl+Alt+C is not Copy");
    t.key("C", json!({"cmd": true}));
    assert!(!t.app.copied.is_empty(), "Ctrl+C copies");
}

#[test]
fn ctrl_a_selects_every_clip_in_the_arrangement_and_delete_removes_clips_never_tracks() {
    let mut t = T::new();
    let extra = TrackId(t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap());
    let c2 = t.app.run("midi.new_clip", json!({"tracks": [extra.0], "start": 0, "end": 0})).unwrap()["clip"].as_u64().unwrap();
    t.frames(2);
    t.app.run("edit.select_none", json!({})).unwrap();
    t.app.keys.active = false;
    t.key("A", json!({"cmd": true}));
    let mut sel: Vec<u64> = t.app.engine.session().edit.selected_clips.iter().map(|c| c.0).collect();
    sel.sort();
    assert_eq!(sel, vec![t.clip.0.min(c2), t.clip.0.max(c2)], "both clips");
    let tracks = track_order(&t).len();
    t.key("Delete", json!({}));
    assert!(t.app.engine.session().find_clip(t.clip).is_none() && t.app.engine.session().find_clip(ClipId(c2)).is_none(), "clips gone");
    assert_eq!(track_order(&t).len(), tracks, "tracks stay");
    // Nothing selected but tracks (no header pick): Delete does nothing.
    t.app.run("edit.select_all", json!({})).unwrap();
    t.key("Delete", json!({}));
    assert_eq!(track_order(&t).len(), tracks);
}

#[test]
fn ctrl_a_in_the_piano_roll_selects_notes_not_clips() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.app.run("edit.select_none", json!({})).unwrap();
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 1, "the note");
    assert!(t.app.engine.session().edit.selected_clips.is_empty(), "no clip selected by the arrangement's Ctrl+A");
}

#[test]
fn ctrl_t_adds_an_audio_track_and_ctrl_shift_t_an_instrument_track_and_neither_splits() {
    use transit_model::TrackKind;
    let mut t = T::new();
    t.app.keys.active = false;
    let undo = t.undo_len();
    let n = track_order(&t).len();
    t.key("T", json!({"cmd": true}));
    let kinds: Vec<TrackKind> = t.app.engine.session().tracks.iter().map(|x| x.kind).collect();
    assert_eq!(kinds.len(), n + 1);
    assert_eq!(t.app.engine.session().track(t.app.cursor_track().unwrap()).unwrap().kind, TrackKind::Audio);
    t.key("T", json!({"cmd": true, "shift": true}));
    assert_eq!(track_order(&t).len(), n + 2);
    assert!(t.app.engine.session().track(t.app.cursor_track().unwrap()).unwrap().kind.is_midi());
    assert_eq!(t.undo_len(), undo + 2, "two tracks, no split");
    t.key("E", json!({"cmd": true}));
    assert_eq!(track_order(&t).len(), n + 2, "Ctrl+E still splits only");
}

// ---- Shift+wheel and horizontal-wheel panning (arrangement and piano roll) ----

/// Fraction of the visible width one wheel notch pans.
const NOTCH: f64 = 0.10;

impl T {
    fn notch(&mut self, at: (f32, f32), extra: Value) {
        // A real keyboard reports Shift as held, which synthetic wheel events alone do not.
        let shift = extra.get("shift").and_then(Value::as_bool).unwrap_or(false);
        if shift {
            self.app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        }
        self.input("ui.scroll", merged(json!({"x": at.0, "y": at.1, "unit": "line", "steps": 1}), extra));
        if shift {
            self.app.synthetic.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
            self.frames(2);
        }
    }
}

fn arrange_view(t: &T) -> (f64, f64) {
    let l = &t.app.arrange.layout;
    (l.scroll as f64, f64::from(l.timeline[2] - l.timeline[0]) * l.samples_per_px)
}

fn pan_cases(mut scroll: impl FnMut(&mut T, Value) -> (f64, f64)) {
    // (extra wheel params, direction sign): wheel down and a right tilt move later in time.
    let cases =
        [(json!({"dy": -1, "shift": true}), 1.0), (json!({"dy": 1, "shift": true}), -1.0), (json!({"dx": -1}), 1.0), (json!({"dx": 1}), -1.0)];
    for ppp in [1.0f32, 1.5] {
        for (i, (extra, sign)) in cases.iter().enumerate() {
            let mut t = T::open();
            t.ctx.set_pixels_per_point(ppp);
            let (moved, visible) = scroll(&mut t, extra.clone());
            let want = sign * NOTCH * visible;
            assert!((moved - want).abs() <= 0.02 * visible, "case {i} ppp {ppp}: moved {moved}, want {want} (visible {visible})");
        }
    }
}

// Step entry. There is no MIDI device in the harness: `StudioApp::live_midi` is the entry point
// `step_poll` feeds from `Player::take_played_notes` (the ingress tap), so these tests call it.

impl T {
    fn step_on(&mut self) {
        self.key("S", json!({}));
        assert!(self.app.keys.layout.step_on, "Step is on");
    }

    fn played(&mut self, pitch: u8, velocity: u8, on: bool) {
        self.app.live_midi(transit_playback::PlayedNote { pitch, velocity, on });
        self.frames(2);
    }

    /// A note played and released.
    fn tap(&mut self, pitch: u8, velocity: u8) {
        self.played(pitch, velocity, true);
        self.played(pitch, 0, false);
    }

    fn clip_ticks(&self) -> i64 {
        self.app.keys.layout.clip_ticks
    }
}

const SIXTEENTH: i64 = transit_time::TICKS_PER_QUARTER / 4;

// The piano roll shares the arrangement's grid machinery (`crate::grid`) with a view of its own.

impl T {
    fn zoom_to(&mut self, px_per_quarter: f32) {
        self.app.ui.keys.px_per_quarter = px_per_quarter;
        self.frames(2);
    }
}

#[test]
fn zooming_in_subdivides_the_piano_roll_grid_down_to_a_128th_and_lines_and_snap_agree() {
    let mut t = T::open();
    let q = transit_time::TICKS_PER_QUARTER;
    // 60 points per quarter: a 1/16 (15 points), as before.
    assert_eq!(t.app.keys.layout.grid_step, q / 4);
    for (px, step) in [(12.0, q), (24.0, q / 2), (60.0, q / 4), (100.0, q / 8), (200.0, q / 16), (400.0, q / 32), (800.0, q / 32)] {
        t.zoom_to(px);
        let l = &t.app.keys.layout;
        assert_eq!(l.grid_step, step, "{px} points per quarter");
        assert_eq!(l.grid_ticks, Some(step), "snap uses the step the lines are drawn at");
        assert!(step as f64 * l.px_per_tick >= crate::grid::MIN_SPACING_PX - 1e-6, "{px}: spacing");
    }
    assert!(t.app.keys.layout.grid_text.contains("1/128"), "{}", t.app.keys.layout.grid_text);
    // A double-click at the finest zoom adds a note one 128th long.
    t.zoom_to(400.0);
    t.click_n(t.at(95.0, 62), 2, json!({}));
    let n = t.notes();
    assert_eq!((n[0].start, n[0].length), (90, 30), "{n:?}");
    // The arrangement's own grid is untouched.
    assert_eq!(t.app.arrange.grid, crate::grid::GridView::default());
}

#[test]
fn ctrl_1_to_4_drive_the_piano_roll_grid() {
    let mut t = T::open();
    let q = transit_time::TICKS_PER_QUARTER;
    let arrangement = t.app.arrange.grid;
    t.key("1", json!({"cmd": true}));
    assert_eq!(t.app.ui.keys.grid.mode, crate::grid::GridMode::Fixed(q / 8), "finer than the 1/16 in force");
    assert_eq!(t.app.keys.layout.grid_step, q / 8);
    t.key("2", json!({"cmd": true}));
    t.key("2", json!({"cmd": true}));
    assert_eq!(t.app.keys.layout.grid_step, q / 2);
    t.key("3", json!({"cmd": true}));
    assert_eq!(t.app.keys.layout.grid_step, q / 3, "a 1/8 triplet");
    t.key("3", json!({"cmd": true}));
    t.key("4", json!({"cmd": true}));
    assert!(!t.app.ui.keys.grid.snap_enabled && t.app.keys.layout.grid_ticks.is_none());
    assert_eq!(t.app.keys.layout.grid_text, "Snap off");
    assert_eq!(t.app.arrange.grid, arrangement, "Ctrl+1 to 4 in the piano roll leave the arrangement alone");
    t.key("4", json!({"cmd": true}));
    assert!(t.app.ui.keys.grid.snap_enabled);
    // Step entry's plain digits still set lengths (a different, exact key).
    t.step_on();
    t.key("5", json!({}));
    assert_eq!(t.app.keys.layout.step_ticks, q / 4);
}

#[test]
fn ctrl_4_turns_snap_off_so_a_dragged_note_lands_unsnapped() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.key("4", json!({"cmd": true}));
    let to = t.at(120.0 + 510.0, 62);
    t.drag(t.at(120.0, 60), to, json!({}));
    let n = t.notes();
    assert_eq!(n.len(), 1);
    assert_ne!(n[0].start % 240, 0, "unsnapped: {n:?}");
    assert!((n[0].start - 510).abs() <= 16, "{n:?}");
    // The edge resizes freely too.
    let r = t.note_rect(62, n[0].start);
    let px = 100.0 * t.app.keys.layout.px_per_tick as f32;
    t.drag((r[2] - 2.0, (r[1] + r[3]) / 2.0), (r[2] - 2.0 + px, (r[1] + r[3]) / 2.0), json!({}));
    assert_ne!(t.notes()[0].length % 240, 0, "{:?}", t.notes());
}

#[test]
fn the_snap_menu_pins_fixed_entries_and_adaptive_follows_the_zoom_again() {
    let mut t = T::open();
    let q = transit_time::TICKS_PER_QUARTER;
    t.zoom_to(400.0);
    assert_eq!(t.app.keys.layout.grid_step, q / 32, "adaptive");
    let pick = |t: &mut T, label: &str| {
        t.click(mid(t.app.keys.layout.snap));
        let item = t.app.keys.layout.snap_menu.iter().find(|m| m.label == label).cloned().unwrap_or_else(|| panic!("{label} listed"));
        t.click(mid(item.rect));
    };
    pick(&mut t, "1/16");
    assert_eq!(t.app.ui.keys.grid.mode, crate::grid::GridMode::Fixed(q / 4));
    assert_eq!(t.app.keys.layout.grid_step, q / 4, "pinned: the zoom no longer matters");
    t.zoom_to(12.0);
    assert_eq!(t.app.keys.layout.grid_step, q / 4);
    pick(&mut t, "1/8T");
    assert_eq!(t.app.keys.layout.grid_step, q / 3);
    pick(&mut t, "Off");
    assert!(t.app.keys.layout.grid_ticks.is_none());
    pick(&mut t, "Adaptive");
    assert_eq!(t.app.ui.keys.grid, crate::grid::GridView::default());
    assert_eq!(t.app.keys.layout.grid_step, q, "adaptive at 12 points per quarter");
}

#[test]
fn modifier_table_alt_body_is_velocity_alt_shift_body_is_free_move_shift_body_snaps() {
    let mut t = T::open();
    t.add(60, 480, 480);
    let grab = t.at(700.0, 60);
    let to = (grab.0 + 137.0, grab.1 - 20.0);
    // Alt alone on the body: velocity, never position.
    t.drag(grab, to, json!({"alt": true}));
    let n = t.notes();
    assert_eq!((n[0].start, n[0].pitch, n[0].length), (480, 60, 480), "Alt-body does not move");
    assert_ne!(n[0].velocity, 100, "Alt-body changes velocity");
    t.app.run("edit.undo", json!({})).unwrap();
    // Alt+Shift on the body: free move, velocity untouched.
    let grab = t.at(700.0, 60);
    t.drag(grab, t.at(700.0 + 137.0, 60), json!({"alt": true, "shift": true}));
    let n = t.notes();
    assert_eq!((n[0].start, n[0].velocity), (617, 100), "Alt+Shift-body moves without snap");
    t.app.run("edit.undo", json!({})).unwrap();
    // Shift alone (no scale mode): plain snapped move.
    let grab = t.at(700.0, 60);
    t.drag(grab, t.at(700.0 + 137.0, 60), json!({"shift": true}));
    assert_ne!(t.notes()[0].start, 617, "Shift-body snaps");
    assert_eq!(t.notes()[0].velocity, 100);
}

impl T {
    /// Loop the open one-bar clip over `[start_ticks, bar)` and lengthen it to `bars` bars (ADR 0019).
    fn loop_to_bars(&mut self, start_ticks: i64, bars: i64) -> i64 {
        let bar = self.clip_ticks();
        let (a, b, to) = {
            let s = self.app.engine.session();
            let sr = s.sample_rate;
            (s.tempo.tick_to_samples(start_ticks, sr), s.tempo.tick_to_samples(bar, sr), s.tempo.tick_to_samples(bars * bar, sr))
        };
        self.app.run("clip.set_loop", json!({"clip": self.clip.0, "start": a, "end": b})).unwrap();
        self.app.run("clip.loop_to", json!({"clip": self.clip.0, "to": to})).unwrap();
        self.frames(5);
        bar
    }
}
