//! #421: pitch (scale) mode gestures. Shift keeps only its selection meaning, the row layout holds
//! still through a gesture, a dragged note sits on the row under the pointer, and the folded view
//! draws its rows. Every gesture goes through the control channel's input queue, one frame at a time.

use super::*;
use crate::Services;

struct Rig {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    clip: ClipId,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl Rig {
    fn new() -> Self {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let track = app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let clip = ClipId(app.run("midi.new_clip", json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap());
        assert!(app.open_keys(clip));
        // Short rows, so two octaves fit on the default pane.
        app.ui.keys.key_h = 8.0;
        let mut rig = Self { app, ctx: egui::Context::default(), time: 0.0, clip, shapes: Vec::new() };
        rig.frames(3);
        rig
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame_with(Vec::new());
        }
    }

    /// One frame with `extra` events after the queued input's (a key typed mid-gesture).
    fn frame_with(&mut self, extra: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let mut raw =
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))), time: Some(self.time), ..Default::default() };
        self.app.raw_input_hook(&mut raw);
        raw.events.extend(extra);
        let mut output = self.ctx.run_ui(raw, |ui| {
            self.app.logic(ui.ctx());
            self.app.ui(ui);
        });
        output.textures_delta.clear();
        self.shapes = output.shapes;
    }

    /// Queue a control request without stepping any frame.
    fn send(&mut self, method: &str, params: Value) {
        let (req, _) = crate::ControlRequest::new(method, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
    }

    /// Run a drag from `a` to `b` in `steps` pointer moves, then let the frames settle.
    fn drag(&mut self, a: (f32, f32), b: (f32, f32), mods: Value) {
        let mut p = json!({"x": a.0, "y": a.1, "to_x": b.0, "to_y": b.1, "steps": 4});
        if let (Some(p), Some(m)) = (p.as_object_mut(), mods.as_object()) {
            p.extend(m.clone());
        }
        self.send("ui.drag", p);
        self.frames(14);
    }

    /// A drag from `a` through `via` to `b`.
    fn drag_via(&mut self, a: (f32, f32), via: (f32, f32), b: (f32, f32)) {
        let (a, via, b) = (pos2(a.0, a.1), pos2(via.0, via.1), pos2(b.0, b.1));
        let button =
            |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        self.app.synthetic.extend([
            egui::Event::PointerMoved(a),
            button(a, true),
            egui::Event::PointerMoved(via),
            egui::Event::PointerMoved(b),
            button(b, false),
        ]);
        self.frames(14);
    }

    /// C minor; `enabled` is the Pitch (scale) switch, `fold` the Fold to scale option.
    fn mode(&mut self, enabled: bool, fold: bool) {
        self.app.keys_scale_set(json!({"enabled": enabled, "fold": fold, "root": 0, "scale": "minor"})).unwrap();
        self.app.keys.fit = true;
        self.frames(2);
    }

    fn add(&mut self, pitch: u8, start: i64) {
        self.app.run("midi.note_add", json!({"clip": self.clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": 240})).unwrap();
        self.frames(2);
    }

    fn pitches(&self) -> Vec<u8> {
        let mut p: Vec<u8> = self.app.keys_notes().iter().map(|n| n.pitch).collect();
        p.sort_unstable();
        p
    }

    /// The vertical center of `pitch`'s row (the keyboard column's visible rows are the row map).
    fn row_y(&self, pitch: u8) -> f32 {
        let (_, r) = self.app.keys.layout.pitch_rows.iter().find(|(p, _)| *p == pitch).unwrap_or_else(|| panic!("row {pitch} not visible"));
        (r[1] + r[3]) / 2.0
    }

    /// The painted note at `pitch` (the first one).
    fn note(&self, pitch: u8) -> R4 {
        self.app.keys.layout.notes.iter().find(|n| n.pitch == pitch).unwrap_or_else(|| panic!("no note at {pitch}")).rect
    }

    fn note_mid(&self, pitch: u8) -> (f32, f32) {
        let r = self.note(pitch);
        ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
    }

    /// Each visible row's pitch and top, in pitch order.
    fn row_tops(&self) -> Vec<(u8, f32)> {
        self.app.keys.layout.pitch_rows.iter().map(|(p, r)| (*p, r[1])).collect()
    }

    fn undo(&mut self) {
        self.app.run("edit.undo", json!({})).unwrap();
        self.frames(2);
    }
}

/// Hunt (a): with fold on, a Shift-click on a note must leave every row where it was from the
/// press frame on, and select the note under the pointer; a Shift-band must select exactly the
/// notes under the rectangle the user drew.
#[test]
fn hunt_a_shift_click_and_band_in_a_folded_roll_keep_every_row_in_place() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.add(63, 480);
    rig.add(67, 960);
    rig.mode(true, true);
    let before = rig.row_tops();
    let (x, y) = rig.note_mid(67);
    rig.app.keys.selection.clear();
    rig.send("ui.click", json!({"x": x, "y": y, "shift": true}));
    let mut seen = Vec::new();
    for frame in 0..6 {
        rig.frames(1);
        seen.push((frame, rig.row_tops() == before));
    }
    rig.frames(4);
    let clicked: Vec<u8> = rig.app.keys.selection.iter().map(|n| n.pitch).collect();
    // A Shift-band around the 63 only, drawn in the folded layout the user sees.
    rig.app.keys.selection.clear();
    rig.frames(2);
    let r = rig.note(63);
    let cy = (r[1] + r[3]) / 2.0;
    rig.drag((r[0] - 6.0, cy - 2.0), (r[2] + 6.0, cy + 2.0), json!({"shift": true}));
    let banded: Vec<u8> = rig.app.keys.selection.iter().map(|n| n.pitch).collect();
    assert_eq!(
        (seen.iter().all(|(_, same)| *same), clicked.as_slice(), banded.as_slice()),
        (true, [67].as_slice(), [63].as_slice()),
        "(rows unchanged on every frame, Shift-click selection, Shift-band selection); per frame: {seen:?}"
    );
    assert_eq!(rig.row_tops(), before);
}

/// Hunt (b): unfolded, constraint on, a note dragged up 1 to 12 semitone rows must stay on the
/// nearest scale row to the pointer (never more than half a gap away).
#[test]
fn hunt_b_an_unfolded_scale_drag_keeps_the_note_at_the_pointer() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.mode(true, false);
    let key = rig.app.keys_scale().key;
    let mut log = Vec::new();
    for k in 1..=12u8 {
        let grab = rig.note_mid(60);
        let row = 60 + k;
        rig.drag(grab, (grab.0, rig.row_y(row)), json!({}));
        let got = rig.pitches()[0];
        log.push((row, got));
        if got != 60 {
            rig.undo();
        }
    }
    // The nearest scale tone to each row; a row exactly between two tones keeps the note's side.
    let want: Vec<(u8, u8)> = (1..=12u8)
        .map(|k| {
            let row = 60 + k;
            let below = (0..=row).rev().find(|p| key.contains(*p)).unwrap();
            let above = (row..=127).find(|p| key.contains(*p)).unwrap();
            (row, if row - below <= above - row { below } else { above })
        })
        .collect();
    assert_eq!(log, want, "(pointer row, note row) after each drag");
}

/// Hunt (c): folded, the view paints a line on every visible row boundary, and a drag from row i
/// to row j lands on row j's pitch, accidental rows included.
#[test]
fn hunt_c_the_folded_view_draws_its_rows_and_a_drag_lands_on_the_pointer_row() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.add(61, 480);
    rig.mode(true, true);
    let grid = rig.app.keys.layout.grid;
    let boundaries: Vec<f32> =
        rig.app.keys.layout.pitch_rows.iter().map(|(_, r)| r[3]).filter(|y| *y > grid[1] + 1.0 && *y < grid[3] - 1.0).collect();
    let lined: Vec<bool> = boundaries
        .iter()
        .map(|y| {
            rig.shapes.iter().any(|s| match &s.shape {
                egui::Shape::LineSegment { points, .. } => {
                    (points[0].y - y).abs() < 0.6 && (points[1].y - y).abs() < 0.6 && points[0].x <= grid[0] + 1.0 && points[1].x >= grid[2] - 1.0
                }
                _ => false,
            })
        })
        .collect();
    // Row 60 up three visible rows (61, 62, 63): lands on 63.
    let grab = rig.note_mid(60);
    rig.drag(grab, (grab.0, rig.row_y(63)), json!({}));
    let three_up = rig.pitches();
    rig.undo();
    // The accidental C# up one row: lands on D.
    let grab = rig.note_mid(61);
    rig.drag(grab, (grab.0, rig.row_y(62)), json!({}));
    let one_up = rig.pitches();
    assert_eq!(
        (lined.iter().all(|l| *l), three_up.as_slice(), one_up.as_slice()),
        (true, [61, 63].as_slice(), [60, 62].as_slice()),
        "(every folded boundary lined, C three rows up, C# one row up); boundaries lined: {lined:?}"
    );
}

/// Every piano roll gesture dispatches the same commands as before #421 (Pitch off): a scripted
/// session of select, move, resize, velocity drag, duplicate drag and band select, its undo
/// history and its notes compared with the record taken on the build before the change.
#[test]
fn a_scripted_session_dispatches_the_same_commands_as_before() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.add(64, 480);
    rig.frames(2);
    let base = rig.app.engine.undo_history().len();
    let (x, y) = rig.note_mid(60);
    rig.send("ui.click", json!({"x": x, "y": y}));
    rig.frames(8);
    rig.drag((x, y), (x + 60.0, rig.row_y(62)), json!({}));
    let r = rig.note(62);
    rig.drag((r[2] - 2.0, (r[1] + r[3]) / 2.0), (r[2] + 60.0, (r[1] + r[3]) / 2.0), json!({}));
    let (x, y) = rig.note_mid(64);
    rig.drag((x, y), (x, y - 30.0), json!({"alt": true}));
    let (x, y) = rig.note_mid(64);
    rig.drag((x, y), (x + 120.0, rig.row_y(67)), json!({"cmd": true}));
    let g = rig.app.keys.layout.grid;
    rig.drag((g[0] + 2.0, g[1] + 2.0), (g[2] - 2.0, g[3] - 2.0), json!({}));
    let history = rig.app.engine.undo_history()[base..].to_vec();
    let notes: Vec<(u8, i64, i64, u8)> = rig.app.keys_notes().iter().map(|n| (n.pitch, n.start, n.length, n.velocity)).collect();
    // Recorded on the build before #421 (085c0438).
    assert_eq!(history, ["Move Notes", "Edit Note", "Edit Note", "Add Note"]);
    assert_eq!(notes, [(64, 480, 240, 120), (62, 960, 1200, 100), (67, 2400, 240, 120)]);
    assert_eq!(rig.app.keys.selection.len(), 3, "the band selects every note");
}

/// Decision 2: from the press to the release of a drag the rows hold still, even when a held MIDI
/// key would show a folded-away row and the dragged accidental leaves its row; after the release
/// the layout settles.
#[test]
fn the_row_map_holds_from_press_to_release_of_a_drag() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.add(61, 480);
    rig.mode(true, true);
    let before = rig.row_tops();
    let grab = rig.note_mid(61);
    let to = (grab.0, rig.row_y(62));
    rig.send("ui.drag", json!({"x": grab.0, "y": grab.1, "to_x": to.0, "to_y": to.1, "steps": 6}));
    let mut held = Vec::new();
    for frame in 0..9 {
        if frame == 3 {
            // E, out of C minor: folding would show its row while the key is down.
            rig.app.live_midi(transit_playback::PlayedNote { pitch: 64, velocity: 90, on: true });
        }
        rig.frames(1);
        held.push(rig.row_tops() == before);
    }
    assert!(held.iter().all(|h| *h), "rows unchanged on every frame from press to release: {held:?}");
    rig.frames(3);
    assert_eq!(rig.pitches(), [60, 62]);
    let after: Vec<u8> = rig.row_tops().iter().map(|(p, _)| *p).collect();
    assert!(after.contains(&64) && !after.contains(&61), "after the release the held key's row shows and the vacated row folds away");
    rig.app.live_midi(transit_playback::PlayedNote { pitch: 64, velocity: 0, on: false });
}

/// Decision 3: the dragged note sits on the row under the pointer over a sweep of rows, for one
/// note and for a chord with an accidental, folded and unfolded, Scale mode on and off.
#[test]
fn a_dragged_note_sits_on_the_row_under_the_pointer() {
    let mut failures = Vec::new();
    for (enabled, fold) in [(false, false), (true, false), (true, true), (false, true)] {
        for chord in [false, true] {
            let mut rig = Rig::new();
            rig.add(60, 0);
            if chord {
                rig.add(64, 480);
            }
            rig.mode(enabled, fold);
            let mode = rig.app.keys_scale();
            let start = rig.pitches();
            let rows = scale::Rows::new(mode, &rig.app.keys_notes(), &[0; 128]);
            for target in 52u8..=74 {
                if !rows.contains(target) || rig.app.keys.layout.pitch_rows.iter().all(|(p, _)| *p != target) {
                    continue;
                }
                rig.app.keys.selection = rig.app.keys_notes();
                let grab = rig.note_mid(60);
                rig.drag(grab, (grab.0, rig.row_y(target)), json!({}));
                let got = rig.pitches();
                // The oracle: the anchor's expected row, the others by the rule stated in `rows::drag_pitches`.
                let mut want: Vec<u8> = if rows.folded() {
                    let dr = rows.rank(f32::from(target)) - rows.rank(60.0);
                    start.iter().map(|p| rows.pitch(rows.rank(f32::from(*p)) + dr) as u8).collect()
                } else if !mode.enabled {
                    start.iter().map(|p| p + target - 60).collect()
                } else {
                    let below = (0..=target).rev().find(|p| mode.key.contains(*p)).unwrap();
                    let above = (target..=127).find(|p| mode.key.contains(*p)).unwrap();
                    // A row exactly between two scale rows keeps the note on its own side.
                    let near = if target - below < above - target || (target - below == above - target && below >= 60) { below } else { above };
                    let tones: Vec<u8> = (0..=127).filter(|p| mode.key.contains(*p)).collect();
                    let d = tones.iter().position(|t| *t == near).unwrap() as i64 - tones.iter().position(|t| *t == 60).unwrap() as i64;
                    mode.key.transpose_group(&start, 60, d)
                };
                want.sort_unstable();
                if got != want {
                    failures.push(format!("enabled {enabled} fold {fold} chord {chord}: pointer row {target}, notes {got:?}, want {want:?}"));
                }
                if got != start {
                    rig.undo();
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Decision 3, unfolded with Scale mode on: a quarter-row sweep of the pointer. The note moves
/// exactly when the pointer passes the midpoint between two scale rows and never sits farther
/// from the pointer than that.
#[test]
fn an_unfolded_scale_drag_moves_at_the_midpoints() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.mode(true, false);
    let key = rig.app.keys_scale().key;
    let kh = rig.app.keys.layout.key_h;
    let mut log = Vec::new();
    for k in 1..=24 {
        let grab = rig.note_mid(60);
        // The pointer `k / 4` rows above the row's center, plus a hair so no point is exactly a midpoint.
        let q = 60.0 + k as f32 / 4.0 + 0.01;
        // Up four rows first, so even the shortest sweep step is a drag and never a click.
        rig.drag_via(grab, (grab.0, grab.1 - 4.0 * kh), (grab.0, grab.1 - (q - 60.0) * kh));
        let got = rig.pitches()[0];
        let want = (0..=127u8).filter(|p| key.contains(*p)).min_by(|a, b| (f32::from(*a) - q).abs().total_cmp(&(f32::from(*b) - q).abs())).unwrap();
        log.push((q, got, want));
        if got != 60 {
            rig.undo();
        }
    }
    let wrong: Vec<_> = log.iter().filter(|(_, got, want)| got != want).collect();
    assert!(wrong.is_empty(), "(pointer, note, nearest scale row): {wrong:?}");
}

/// The horizontal lines across the whole grid: (y, ink).
fn grid_lines(rig: &Rig) -> Vec<(f32, egui::Color32)> {
    let grid = rig.app.keys.layout.grid;
    rig.shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::LineSegment { points, stroke }
                if points[0].y == points[1].y
                    && points[0].x <= grid[0] + 1.0
                    && points[1].x >= grid[2] - 1.0
                    && points[0].y > grid[1]
                    && points[0].y < grid[3] =>
            {
                Some((points[0].y, stroke.color))
            }
            _ => None,
        })
        .collect()
}

/// Decision 4: the folded view's row lines take the keyboard column's row ink with the octave
/// line under the lowest row of each octave; the unfolded grid keeps only its octave lines.
#[test]
fn the_folded_row_lines_take_the_row_ink_and_the_unfolded_grid_is_unchanged() {
    let mut rig = Rig::new();
    rig.add(60, 0);
    rig.mode(true, true);
    let grid = rig.app.keys.layout.grid;
    let inside = |r: &R4| r[3] > grid[1] + 1.0 && r[3] < grid[3] - 1.0;
    let painted = grid_lines(&rig);
    let rows = rig.app.keys.layout.pitch_rows.clone();
    for (i, (pitch, r)) in rows.iter().enumerate() {
        if !inside(r) {
            continue;
        }
        let below = i.checked_sub(1).map(|b| rows[b].0);
        let octave = below.is_none_or(|b| b / 12 != pitch / 12);
        let ink = if octave { TOKENS.grid_bar } else { TOKENS.line.gamma_multiply(0.5) };
        assert!(painted.iter().any(|(y, c)| (y - r[3]).abs() < 0.6 && *c == ink), "row {pitch}: a line in {ink:?} under it; painted {painted:?}");
    }
    rig.mode(true, false);
    let painted = grid_lines(&rig);
    for (pitch, r) in rig.app.keys.layout.pitch_rows.iter().filter(|(_, r)| inside(r)) {
        let lined = painted.iter().any(|(y, c)| (y - r[3]).abs() < 0.6 && *c == TOKENS.grid_bar);
        assert_eq!(lined, pitch % 12 == 0, "unfolded row {pitch}: the octave line under C only");
        assert!(!painted.iter().any(|(y, c)| (y - r[3]).abs() < 0.6 && *c == TOKENS.line.gamma_multiply(0.5)), "no row line unfolded");
    }
}

/// Decisions 3 and 4: a double-click on a row enters that row's pitch, folded and unfolded; only an
/// out-of-scale row of the unfolded view with Scale mode on gives the nearest scale tone.
#[test]
fn entry_creates_the_note_on_the_row_clicked() {
    let mut failures = Vec::new();
    for (enabled, fold) in [(false, false), (true, false), (true, true)] {
        let mut rig = Rig::new();
        rig.add(61, 960);
        rig.mode(enabled, fold);
        let key = rig.app.keys_scale().key;
        let x = rig.app.keys.layout.grid[0] + 2.0;
        let rows: Vec<u8> = rig.app.keys.layout.pitch_rows.iter().map(|(p, _)| *p).filter(|p| (52..=74).contains(p)).collect();
        for pitch in rows {
            let y = rig.row_y(pitch);
            rig.send("ui.click", json!({"x": x, "y": y, "count": 2}));
            rig.frames(10);
            let added: Vec<u8> = rig.app.keys_notes().iter().filter(|n| n.start == 0).map(|n| n.pitch).collect();
            let want = if enabled && !fold { key.nearest(pitch) } else { pitch };
            if added != [want] {
                failures.push(format!("enabled {enabled} fold {fold}: row {pitch} entered {added:?}, want {want}"));
            }
            if !added.is_empty() {
                rig.undo();
            }
            rig.frames(30);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
