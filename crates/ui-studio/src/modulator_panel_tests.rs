//! The Cross Modulator insert's panel (#200), driven the way a user drives it: it is in the insert
//! picker, its fixed window lists every parameter with the key picker, the algorithm line and the
//! credit, a choice and a toggle set their parameters, and every factory preset sounds.

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

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
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

    fn control(&self, t: TrackId, param: &str) -> (f32, f32) {
        let r = self.window(t).rows.into_iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a row for `{param}`")).control;
        ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
    }

    fn param(&self, t: TrackId, id: &str) -> f32 {
        let ins = self.app.engine.session().track(t).unwrap().mixer.inserts[0].clone().unwrap();
        ins.params.get(id).copied().unwrap_or_else(|| transit_dsp::plugin_info("cross_modulator").unwrap().param(id).unwrap().default)
    }
}

#[test]
fn the_cross_modulator_panel_lists_every_control_and_credits_the_original() {
    let mut t = T::new();
    let picker = t.app.builtin_plugins();
    assert!(picker.iter().any(|p| p.id == "cross_modulator" && p.name == "Cross Modulator" && !p.instrument), "an insert effect");
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let tr = TrackId(r["tracks"][0].as_u64().unwrap());
    t.app.focus(tr);
    t.frames(4);
    t.app.add_device(tr, "cross_modulator").unwrap();
    t.frames(4);
    let at = t.app.mixer.devices.cards[0].editor_button;
    t.click(((at[0] + at[2]) / 2.0, (at[1] + at[3]) / 2.0));
    let w = t.window(tr);
    assert_eq!(w.title.rsplit(" · ").next(), Some("Cross Modulator"));
    assert_eq!(w.credit, crate::modulator_panel::CREDIT);
    assert!(w.credit.contains("Émilie Gillet"));
    let mut got: Vec<&str> = w.rows.iter().map(|r| r.param.as_str()).collect();
    got.sort_unstable();
    let mut want: Vec<&str> = transit_dsp::plugin_info("cross_modulator").unwrap().params.iter().map(|p| p.id).collect();
    want.sort_unstable();
    assert_eq!(got, want, "every parameter has a control");
    let (ww, wh) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    println!("Cross Modulator window {ww} x {wh}");
    assert!(ww > 300.0 && ww < 1000.0 && wh > 150.0 && wh < 640.0, "a fixed window that fits 1280 x 720 (#232): {ww} x {wh}");
    assert_eq!(t.window(tr).rows.iter().find(|r| r.param == "carrier").unwrap().value, "Auto");

    // Swap latches on a click.
    let swap = t.control(tr, "swap");
    t.click(swap);
    assert_eq!(t.param(tr, "swap"), 1.0);
}

#[test]
fn the_algorithm_line_names_where_the_knob_sits() {
    use crate::modulator_panel::algorithm_line;
    assert_eq!(algorithm_line(0.0), "Crossfade");
    assert_eq!(algorithm_line(2.0), "Diode Ring");
    assert_eq!(algorithm_line(2.5), "Diode Ring → Digital Ring 50%");
    assert_eq!(algorithm_line(5.0), "Comparator");
    assert_eq!(algorithm_line(5.4), "Comparator → Vocoder 50%");
    assert_eq!(algorithm_line(6.0), "Vocoder");
    assert_eq!(algorithm_line(7.0), "Vocoder, release 50%");
    assert_eq!(algorithm_line(8.0), "Vocoder (freeze)");
    assert_eq!(algorithm_line(f32::NAN), "Crossfade");
}

/// Every factory preset sounds on a fresh instance with no key (the internal oscillator carries),
/// recalled before the first block.
#[test]
fn every_cross_modulator_preset_sounds_on_a_fresh_instance() {
    let info = transit_dsp::plugin_info("cross_modulator").unwrap();
    let factory = crate::presets::factory_for("cross_modulator");
    assert_eq!(factory.len(), 3);
    for (name, preset) in factory {
        let mut p = transit_dsp::create("cross_modulator").unwrap();
        let sr = 48_000usize;
        p.prepare(sr as f32, 512, 2);
        for (id, v) in crate::presets::recall_values(info, &preset) {
            assert!(p.set_param(&id, v), "{name}: {id}");
        }
        let mut io = vec![vec![0.0f32; 512]; 2];
        let mut tail = 0.0f64;
        let mut count = 0usize;
        for k in 0..(2 * sr / 512) {
            let (left, right) = io.split_at_mut(1);
            for (n, (l, r)) in left[0].iter_mut().zip(right[0].iter_mut()).enumerate() {
                let t = (k * 512 + n) as f32 / sr as f32;
                *l = 0.3 * (std::f32::consts::TAU * 220.0 * t).sin() * (1.0 + (std::f32::consts::TAU * 3.0 * t).sin()) / 2.0;
                *r = *l;
            }
            p.process(&mut io, 512);
            if k * 512 >= sr {
                tail += io[0].iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
                count += 512;
            }
        }
        let rms = (tail / count as f64).sqrt();
        println!("{name}: RMS {rms:.4} over the last second");
        assert!(rms > 0.01, "{name} is silent on a fresh instance (RMS {rms})");
    }
}
