//! The Modal Voice instrument's panel (#198 slice 2), driven the way a user drives it: it is in the
//! instrument picker after the Resonator (the Macro Oscillator stays the default), its window lists
//! every parameter with the gate and rate lines and the credit, the shaper and matrix show behind
//! the addition line and the expanded window fits 800 pt at 100% and 125%, and every factory
//! preset sounds on a fresh instance.

use crate::{Services, StudioApp};
use serde_json::json;
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    /// The screen in points.
    screen: egui::Vec2,
}

impl T {
    fn new(scale: f32) -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(scale);
        // A 1920 x 1080 screen in points at this scale.
        let mut t = T { app, ctx, time: 0.0, screen: egui::vec2(1920.0 / scale, 1080.0 / scale) };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
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
            out.textures_delta.clear();
        }
    }

    fn click(&mut self, at: (f32, f32)) {
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "count": 1}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(12);
    }

    fn window(&self, t: TrackId) -> crate::builtin_editor::EditorLayout {
        self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).cloned().expect("an editor window is open")
    }

    /// An instrument track playing the Modal Voice, its editor open.
    fn open(&mut self) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": "instrument"})).unwrap();
        let tr = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(tr);
        self.frames(4);
        self.app.run("mix.instrument", json!({"track": tr.0, "plugin": "modal_voice"})).unwrap();
        self.frames(4);
        let at = self.app.mixer.devices.cards[0].editor_button;
        self.click(((at[0] + at[2]) / 2.0, (at[1] + at[3]) / 2.0));
        tr
    }
}

fn sorted(mut v: Vec<&str>) -> Vec<&str> {
    v.sort_unstable();
    v
}

#[test]
fn the_modal_voice_is_an_instrument_after_the_texture_synth_and_its_panel_lists_every_control() {
    let mut t = T::new(1.0);
    let picker = t.app.builtin_plugins();
    let at = |id: &str| picker.iter().position(|p| p.id == id);
    assert!(picker.iter().any(|p| p.id == "modal_voice" && p.name == "Modal Voice" && p.instrument), "an instrument in the picker");
    assert_eq!(at("modal_voice"), at("texture_synth").map(|i| i + 1), "listed after the Texture Synth");
    assert_eq!(transit_model::DEFAULT_INSTRUMENT, "macro_oscillator", "the Macro Oscillator stays the default");
    let tr = t.open();
    let w = t.window(tr);
    assert_eq!(w.title.rsplit(" · ").next(), Some("Modal Voice"));
    assert_eq!(w.credit, crate::modal_voice_panel::CREDIT);
    assert!(w.credit.contains("Émilie Gillet"));
    assert_eq!(w.addition, crate::panel::MACRO_OSC_ADDITION, "the shaper and the matrix are labeled as transit's addition");
    assert!(w.rows.iter().all(|r| r.param != "shaper"), "not in use: the shaper and the matrix are hidden");
    let r = w.addition_rect;
    t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0));
    let w = t.window(tr);
    let got = sorted(w.rows.iter().map(|r| r.param.as_str()).collect());
    let want = sorted(transit_dsp::plugin_info("modal_voice").unwrap().params.iter().map(|p| p.id).collect());
    assert_eq!(got, want, "every parameter has a control");
    let value = |id: &str| w.rows.iter().find(|r| r.param == id).unwrap().value.clone();
    assert_eq!(value("model"), "Modal");
    assert_eq!(value("strike_level"), "80%");
    assert_eq!(value("space"), "50%");
    t.app.run("mix.instrument_param", json!({"track": tr.0, "param": "mod2_dest", "value": 15.0})).unwrap();
    t.frames(2);
    let w = t.window(tr);
    assert_eq!(w.rows.iter().find(|r| r.param == "mod2_dest").unwrap().value, "Strength", "the matrix names the modal voice's destinations");
}

/// With the shaper and the matrix shown, the window fits a 1080p screen at 100% and at 125%
/// scaling with its title bar: at most 800 pt tall and 1300 pt wide (the #243 budget). Collapsed
/// it is the module's own panel.
#[test]
fn the_expanded_modal_voice_window_fits_1080p_at_100_and_125_percent() {
    for scale in [1.0f32, 1.25] {
        let mut t = T::new(scale);
        let tr = t.open();
        let c = t.window(tr).rect;
        let (cw, ch) = (c[2] - c[0], c[3] - c[1]);
        println!("@{scale}: collapsed {cw} x {ch} pt");
        assert!(cw < 900.0 && ch < 500.0, "@{scale}: collapsed {cw} x {ch} pt");
        let r = t.window(tr).addition_rect;
        t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0));
        let w = t.window(tr);
        assert!(w.rows.iter().any(|r| r.param == "mod16_amount") && w.rows.iter().any(|r| r.param == "lfo2_amount"), "@{scale}: all of it shows");
        let (width, height) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
        println!("@{scale}: expanded {width} x {height} pt");
        assert!(height <= 800.0 && width <= 1300.0, "@{scale}: expanded {width} x {height} pt");
        let (right, bottom) = w.rows.iter().fold((0.0f32, 0.0f32), |(r, b), x| (r.max(x.control[2]), b.max(x.control[3])));
        assert!(w.rect[2] >= right && w.rect[3] >= bottom, "@{scale}: the window covers its controls");
    }
}

/// Every factory preset sounds on a fresh instance (recalled before the first block, a note held
/// for the second), and the presets cover the three models.
#[test]
fn every_modal_voice_preset_sounds_on_a_fresh_instance() {
    let info = transit_dsp::plugin_info("modal_voice").unwrap();
    let factory = crate::presets::factory_for("modal_voice");
    assert_eq!(factory.len(), 6);
    let mut models: Vec<u32> = factory.iter().map(|(_, p)| p.params.get("model").copied().unwrap_or(0.0) as u32).collect();
    models.sort_unstable();
    models.dedup();
    assert_eq!(models, [0, 1, 2], "the presets cover the modal, string and chord-string models");
    for (name, preset) in factory {
        let mut p = transit_dsp::create("modal_voice").unwrap();
        let sr = 48_000usize;
        p.prepare(sr as f32, 512, 2);
        for (id, v) in crate::presets::recall_values(info, &preset) {
            assert!(p.set_param(&id, v), "{name}: {id}");
        }
        p.note_on(0, 57, 100);
        let mut io = vec![vec![0.0f32; 512]; 2];
        let mut sum = 0.0f64;
        let mut count = 0usize;
        for _ in 0..(sr / 512) {
            p.process(&mut io, 512);
            sum += io.iter().flatten().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
            count += 1024;
        }
        let rms = (sum / count as f64).sqrt();
        println!("{name}: RMS {rms:.4} over the first second");
        assert!(rms > 0.005, "{name} is silent on a fresh instance (RMS {rms})");
        assert!(io.iter().flatten().all(|v| v.is_finite()));
    }
}
