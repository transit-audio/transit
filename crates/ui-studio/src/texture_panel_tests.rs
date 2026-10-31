//! The Texture insert's panel (#241), driven the way a user drives it: it is in the insert picker,
//! its fixed window lists every parameter with the credit under it, Freeze latches on a click and
//! Seed holds only while the pointer holds it down.

use crate::{Services, StudioApp};
use serde_json::json;
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl T {
    fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
        t
    }

    /// Runs `n` frames; the first one carries `events`.
    fn frames_with(&mut self, n: usize, mut events: Vec<egui::Event>) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                events: std::mem::take(&mut events),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn frames(&mut self, n: usize) {
        self.frames_with(n, Vec::new());
    }

    fn click(&mut self, at: (f32, f32)) {
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "count": 1}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(12);
    }

    fn pointer(&mut self, at: (f32, f32), pressed: bool) {
        let pos = egui::pos2(at.0, at.1);
        let button = egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        self.frames_with(4, vec![egui::Event::PointerMoved(pos), button]);
    }

    fn window(&self, t: TrackId) -> crate::builtin_editor::EditorLayout {
        self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).cloned().expect("an editor window is open")
    }

    fn control(&self, t: TrackId, param: &str) -> (f32, f32) {
        let r = self.window(t).rows.into_iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a row for `{param}`")).control;
        ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
    }

    fn param(&self, t: TrackId, id: &str) -> f32 {
        let ins = self.app.engine.session().track(t).unwrap().mixer.inserts[0].clone().unwrap();
        ins.params.get(id).copied().unwrap_or_else(|| transit_dsp::plugin_info("texture").unwrap().param(id).unwrap().default)
    }
}

#[test]
fn the_texture_panel_lists_every_control_credits_the_original_and_plays_freeze_and_seed() {
    let mut t = T::new();
    let picker = t.app.builtin_plugins();
    assert!(picker.iter().any(|p| p.id == "texture" && p.name == "Texture" && !p.instrument), "the Texture is an insert effect");
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let tr = TrackId(r["tracks"][0].as_u64().unwrap());
    t.app.focus(tr);
    t.frames(4);
    t.app.add_device(tr, "texture").unwrap();
    t.frames(4);
    let at = t.app.mixer.devices.cards[0].editor_button;
    t.click(((at[0] + at[2]) / 2.0, (at[1] + at[3]) / 2.0));
    let w = t.window(tr);
    assert_eq!(w.title.rsplit(" · ").next(), Some("Texture"));
    assert_eq!(w.credit, crate::texture_panel::CREDIT);
    assert!(w.credit.contains("Émilie Gillet"));
    let mut got: Vec<&str> = w.rows.iter().map(|r| r.param.as_str()).collect();
    got.sort_unstable();
    let mut want: Vec<&str> = transit_dsp::plugin_info("texture").unwrap().params.iter().map(|p| p.id).collect();
    want.sort_unstable();
    assert_eq!(got, want, "every parameter has a control");
    assert!(w.preset != [0.0; 4], "the preset dropdown");
    // A fixed window that fits a 1280 x 720 screen (#232).
    let (ww, wh) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    println!("Texture window {ww} x {wh}");
    assert!(ww > 300.0 && ww < 1000.0 && wh > 200.0 && wh < 640.0, "{ww} x {wh}");
    assert_eq!(t.window(tr).rows.iter().find(|r| r.param == "quality").unwrap().value, "Digital", "the module's default quality");

    // Freeze latches on a click.
    let freeze = t.control(tr, "freeze");
    t.click(freeze);
    assert_eq!(t.param(tr, "freeze"), 1.0);
    // Seed is on while held and off once let go.
    let seed = t.control(tr, "seed");
    t.pointer(seed, true);
    assert_eq!(t.param(tr, "seed"), 1.0, "held down");
    t.pointer(seed, false);
    assert_eq!(t.param(tr, "seed"), 0.0, "let go");
}
