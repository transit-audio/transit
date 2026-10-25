use super::*;
use crate::Services;
use soundcraft_midi::scale::{KeyScale, Scale, ScaleMode};

struct Harness {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    clip: ClipId,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl Harness {
    fn new() -> Self {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        let track = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let clip = ClipId(app.run("midi.new_clip", json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap());
        assert!(app.open_keys(clip));
        let mut h = Self { app, ctx: egui::Context::default(), time: 0.0, clip, shapes: Vec::new() };
        h.frames(3);
        h
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let mut output = self.ctx.run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }
    }

    fn input(&mut self, command: &str, params: Value) {
        let (req, _) = crate::ControlRequest::new(command, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
        self.frames(30);
    }

    fn click(&mut self, rect: R4) {
        self.input("ui.click", json!({"x": (rect[0]+rect[2])/2.0, "y": (rect[1]+rect[3])/2.0}));
    }

    fn minor(&mut self, fold: bool) {
        self.app.keys_scale_set(json!({"enabled": true, "fold": fold, "root": 0, "scale": "minor"})).unwrap();
        self.frames(2);
    }

    fn add(&mut self, pitch: u8, start: i64) {
        self.app.run("midi.note_add", json!({"clip": self.clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": 240})).unwrap();
        self.frames(2);
    }

    fn at(&self, pitch: u8, tick: f64) -> (f32, f32) {
        let l = &self.app.keys.layout;
        (l.grid[0] + ((tick - l.scroll_ticks) * l.px_per_tick) as f32, l.grid[1] + (l.top_pitch - f32::from(pitch) + 0.5) * l.key_h)
    }
}

#[test]
fn scale_controls_toggle_through_ui_and_undo_without_changing_notes() {
    let mut h = Harness::new();
    h.add(61, 0);
    let before = h.app.keys_notes();
    h.click(h.app.keys.layout.pitch);
    let toggle = h.app.keys.layout.pitch_menu.iter().find(|m| m.label == "scale_mode").unwrap().rect;
    h.click(toggle);
    assert!(h.app.keys_scale().enabled);
    assert_eq!(h.app.keys_notes(), before);
    h.click(h.app.keys.layout.pitch);
    let fold = h.app.keys.layout.pitch_menu.iter().find(|m| m.label == "scale_fold").unwrap().rect;
    h.click(fold);
    assert!(h.app.keys_scale().fold);
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(!h.app.keys_scale().fold);
    assert!(h.app.keys_scale().enabled);
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(!h.app.keys_scale().enabled);
    assert_eq!(h.app.keys_notes(), before);
    h.app.engine.session_mut().find_clip_mut(h.clip).unwrap().scale_mode.key.root = 255;
    h.frames(2);
    h.click(h.app.keys.layout.pitch);
    assert_eq!(h.app.keys_scale().key.root, 255, "display normalization must not rewrite saved data merely by opening the menu");
}

#[test]
fn scale_draw_drag_override_and_transpose_use_real_handlers() {
    let mut h = Harness::new();
    h.minor(false);
    let (x, y) = h.at(64, 0.0);
    h.input("ui.click", json!({"x": x+4.0, "y": y, "count": 2}));
    assert_eq!(h.app.keys_notes()[0].pitch, 63);
    let n = h.app.keys.layout.notes[0].rect;
    let (x, y) = ((n[0] + n[2]) / 2.0, (n[1] + n[3]) / 2.0);
    h.input("ui.drag", json!({"x": x, "y": y, "to_x": x, "to_y": y-h.app.ui.keys.key_h}));
    assert_eq!(h.app.keys_notes()[0].pitch, 65, "one row moves one scale degree");
    h.app.run("edit.undo", json!({})).unwrap();
    h.frames(2);
    let n = h.app.keys.layout.notes[0].rect;
    let (x, y) = ((n[0] + n[2]) / 2.0, (n[1] + n[3]) / 2.0);
    h.input("ui.drag", json!({"x": x, "y": y, "to_x": x, "to_y": y-h.app.ui.keys.key_h, "shift": true}));
    assert_eq!(h.app.keys_notes()[0].pitch, 64, "Shift permits the accidental");
    h.app.keys_transpose(1).unwrap();
    assert_eq!(h.app.keys_notes()[0].pitch, 66, "an accidental keeps its offset above the tone: E to F# in C minor");
}

#[test]
fn folded_scale_keeps_accidentals_and_live_input_inline() {
    let mut h = Harness::new();
    h.add(60, 0);
    h.add(63, 480);
    h.add(61, 960);
    h.minor(true);
    let row = |app: &StudioApp, pitch| app.keys.layout.notes.iter().find(|n| n.pitch == pitch).unwrap().rect[1];
    assert!((row(&h.app, 60) - row(&h.app, 61) - h.app.ui.keys.key_h).abs() < 0.1);
    assert!((row(&h.app, 61) - row(&h.app, 63) - h.app.ui.keys.key_h * 2.0).abs() < 0.1);
    let mode = h.app.keys_scale();
    let rows = scale::Rows::new(mode, &h.app.keys_notes(), &h.app.keys.live, false);
    assert!(!rows.contains(64));
    h.app.live_midi(soundcraft_playback::PlayedNote { pitch: 64, velocity: 97, on: true });
    assert_eq!(h.app.live_keys(), [(64, 97)]);
    h.frames(1);
    let (_, rect) = h.app.keys.layout.pitch_rows.iter().find(|(p, _)| *p == 64).unwrap();
    let center = pos2((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    let color = TOKENS.loop_region.gamma_multiply(0.6 + 0.4 * 97.0 / 127.0);
    assert!(
        h.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.rect.contains(center) && r.fill == color)),
        "a held accidental keeps its distinct color in the painted keyboard"
    );
    let rows = scale::Rows::new(mode, &h.app.keys_notes(), &h.app.keys.live, false);
    assert!(rows.contains(64));
    assert_eq!(rows.pitch(rows.rank(64.0)), 64.0);
    h.app.live_midi(soundcraft_playback::PlayedNote { pitch: 64, velocity: 0, on: false });
    assert!(h.app.live_keys().is_empty());
    assert_eq!(h.app.keys_notes().len(), 3);
    let empty = scale::Rows::new(mode, &[], &[0; 128], false);
    assert_eq!((60..72).filter(|&p| empty.contains(p)).collect::<Vec<_>>(), [60, 62, 63, 65, 67, 68, 70]);
}

#[test]
fn scale_degrees_are_bounded_and_chromatic_defaults_are_unchanged() {
    let key = KeyScale { root: 0, scale: Scale::Minor };
    assert_eq!(key.transpose(63, 1), 65);
    assert_eq!(key.transpose(60, 7), 72);
    assert_eq!(key.transpose(60, i64::MAX), 127);
    assert_eq!(key.transpose(60, i64::MIN), 0);
    let rows = scale::Rows::new(ScaleMode::default(), &[], &[0; 128], false);
    for p in 0u8..=127 {
        assert_eq!(rows.pitch(rows.rank(f32::from(p))), f32::from(p));
    }
}

#[test]
fn step_entry_snaps_only_the_edit_and_chromatic_override_keeps_accidentals() {
    let mut h = Harness::new();
    h.minor(false);
    h.app.step_toggle();
    let note = |on| soundcraft_playback::PlayedNote { pitch: 64, velocity: if on { 97 } else { 0 }, on };
    h.app.live_midi(note(true));
    assert_eq!(h.app.live_keys(), [(64, 97)]);
    h.app.live_midi(note(false));
    assert_eq!(h.app.keys_notes()[0].pitch, 63);
    h.time += 1.0 / 60.0;
    let mut output = h.ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
            time: Some(h.time),
            events: vec![egui::Event::ModifiersChanged(egui::Modifiers { shift: true, ..Default::default() })],
            ..Default::default()
        },
        |ui| {
            h.app.logic(ui.ctx());
            if ui.ctx().current_pass_index() == 0 {
                assert!(h.app.keys.step.chromatic, "current-frame Shift reaches step entry before MIDI drains");
                h.app.live_midi(note(true));
                h.app.live_midi(note(false));
            }
            h.app.ui(ui);
        },
    );
    output.textures_delta.clear();
    assert_eq!(h.app.keys_notes()[1].pitch, 64);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.keys_notes().len(), 1);
}

#[test]
fn scale_chord_move_does_not_resolve_a_previously_moved_note_twice() {
    let mut h = Harness::new();
    h.minor(false);
    h.add(60, 0);
    h.add(62, 0);
    h.app.keys.selection = h.app.keys_notes();
    h.app.keys_move(0, 1).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [62, 63]);
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [60, 62]);
}

#[test]
fn scale_group_move_keeps_accidental_chord_distinct_and_is_one_undo() {
    let mut h = Harness::new();
    h.minor(false);
    h.add(60, 0);
    h.add(61, 0);
    let before: Vec<_> = h.app.keys_notes().iter().map(|n| n.pitch).collect();
    h.app.keys.selection = h.app.keys_notes();
    h.app.keys_move(0, 1).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [62, 63], "C# keeps its offset: D#, not D");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), before, "one undo restores the chord");
}

#[test]
fn scale_group_at_top_of_range_is_refused_as_a_group() {
    let mut h = Harness::new();
    h.app.keys_scale_set(json!({"enabled": true, "root": 0, "scale": "major"})).unwrap();
    for p in [124, 125, 127] {
        h.add(p, 0);
    }
    h.app.keys.selection = h.app.keys_notes();
    h.app.keys_move(0, 1).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [124, 125, 127], "no note folds onto another");
    h.app.keys_move(0, -1).unwrap();
    assert_eq!(h.app.keys_notes().iter().map(|n| n.pitch).collect::<Vec<_>>(), [122, 124, 125], "downward the whole group moves one degree");
}
