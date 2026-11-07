//! The harness of the Compressor panel's tests: the app driven the way a user drives it, synthetic
//! input one frame at a time (no player, no sleeps, no wall clock: each frame advances a fixed
//! 1/60 s).

use super::CompressorLayout;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

pub struct T {
    pub app: StudioApp,
    pub ctx: egui::Context,
    pub time: f64,
    pub shapes: Vec<egui::epaint::ClippedShape>,
    /// The screen the frames run on (1400 by 900 unless a test sets another).
    pub screen: egui::Vec2,
}

pub fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

pub fn info() -> &'static transit_dsp::PluginInfo {
    transit_dsp::plugin_info(super::COMPRESSOR).unwrap()
}

/// The Compressor's slot on its track.
pub const SLOT: usize = 0;

impl T {
    pub fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new(), screen: egui::vec2(1400.0, 900.0) };
        t.frames(3);
        t
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            self.shapes = out.shapes;
            out.textures_delta.clear();
        }
    }

    pub fn send(&mut self, id: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(id, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(12);
    }

    pub fn click(&mut self, at: (f32, f32), count: u32) {
        self.send("ui.click", json!({"x": at.0, "y": at.1, "count": count}));
    }

    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32), shift: bool) {
        self.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1, "shift": shift}));
    }

    pub fn key(&mut self, key: &str) {
        self.send("ui.key", json!({"key": key}));
    }

    pub fn run(&mut self, id: &str, p: Value) {
        self.app.run(id, p).unwrap();
        self.frames(3);
    }

    /// Sets a parameter of the Compressor on `t` through the command a control would send.
    pub fn set(&mut self, t: TrackId, id: &str, v: f32) {
        self.run("mix.insert_param", json!({"track": t.0, "slot": SLOT, "param": id, "value": v}));
    }

    /// An audio track with a Compressor in its first slot, focused.
    pub fn compressor_track(&mut self) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": "audio"})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.add_device(t, super::COMPRESSOR).unwrap();
        self.app.focus(t);
        self.frames(4);
        t
    }

    /// A track with the Compressor, its editor open (the device card's editor button).
    pub fn open() -> (T, TrackId) {
        let mut t = T::new();
        let tr = t.compressor_track();
        t.open_card(0);
        (t, tr)
    }

    /// Clicks the focused track's editor button on card `card`.
    pub fn open_card(&mut self, card: usize) {
        let at = mid(self.app.mixer.devices.cards[card].editor_button);
        self.click(at, 1);
    }

    pub fn undo_len(&mut self) -> usize {
        self.app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
    }

    pub fn window(&self, t: TrackId) -> EditorLayout {
        self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).cloned().expect("an editor window is open")
    }

    pub fn panel(&self, t: TrackId) -> CompressorLayout {
        *self.window(t).compressor.expect("the Compressor's panel")
    }

    pub fn row(&self, t: TrackId, param: &str) -> RowLayout {
        self.window(t).rows.into_iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a row for `{param}`"))
    }

    pub fn param(&self, t: TrackId, id: &str) -> f32 {
        let ins = self.app.engine.session().track(t).unwrap().mixer.inserts[SLOT].clone().unwrap();
        ins.params.get(id).copied().unwrap_or_else(|| info().param(id).unwrap().default)
    }

    /// The centers of every painted text reading `want`.
    pub fn painted_all(&self, want: &str) -> Vec<(f32, f32)> {
        fn walk(shape: &egui::Shape, want: &str, out: &mut Vec<(f32, f32)>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == want => out.push((t.pos.x + t.galley.size().x / 2.0, t.pos.y + t.galley.size().y / 2.0)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        self.shapes.iter().for_each(|s| walk(&s.shape, want, &mut found));
        found
    }

    /// Opens the Detector section.
    pub fn expand(&mut self, tr: TrackId) {
        if self.panel(tr).detector_body.is_none() {
            let b = self.panel(tr).detector_bar.unwrap().button;
            self.click(mid(b), 1);
        }
    }
}
