//! #289: the real device card reflects the loaded host's editor capability.

use crate::tests::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use transit_dsp::{Plugin, PluginEditor};
use transit_playback::{Player, PluginTestHooks};
use transit_plugin_host::{Config, Host};

struct Fixture {
    inner: Box<dyn Plugin>,
    gui: bool,
}
impl Plugin for Fixture {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, max: usize, ch: usize) {
        self.inner.prepare(sr, max, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(Editor { open: false, gui: self.gui }))
    }
}
struct Editor {
    open: bool,
    gui: bool,
}
impl PluginEditor for Editor {
    fn has_gui(&self) -> bool {
        self.gui
    }
    fn open(&mut self) -> Result<(), String> {
        self.open = true;
        Ok(())
    }
    fn close(&mut self) {
        self.open = false;
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
}
fn app_with(gui: bool) -> (StudioApp, egui::Context, TrackId) {
    let engine = transit_engine::Engine::default();
    let player = Player::prepare_device_free(engine.session_arc()).unwrap();
    let now = Instant::now();
    player.set_plugin_test_hooks(PluginTestHooks {
        host: Host::new(Config { inline: true, ..Config::default() }),
        factory: Arc::new(move |spec: &transit_mix::InstanceSpec| {
            transit_mix::create_instance("gain", None, 48_000.0, 512, spec.ch)
                .map(|n| transit_mix::NewInstance { plugin: Box::new(Fixture { inner: n.plugin, gui }), ..n })
        }),
        deferred: Arc::new(|id: &str| id == "clap:test.editor289"),
        clock: Arc::new(move || now),
        slow_after: Duration::from_secs(10),
    });
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let r = app.run("track.new", json!({"kind": "audio"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.engine.session_mut().track_mut(t).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new("clap:test.editor289"));
    app.focus(t);
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    (app, egui::Context::default(), t)
}
fn frame(app: &mut StudioApp, ctx: &egui::Context, time: f64, hover: Option<egui::Pos2>) -> Vec<egui::epaint::ClippedShape> {
    app.player.as_ref().unwrap().step_device_free();
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
        time: Some(time),
        ..Default::default()
    };
    if let Some(pos) = hover {
        input.events.push(egui::Event::PointerMoved(pos));
    }
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    out.shapes
}
fn has_text(shapes: &[egui::epaint::ClippedShape], text: &str) -> bool {
    fn contains(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.text().contains(text),
            egui::Shape::Vec(v) => v.iter().any(|s| contains(s, text)),
            _ => false,
        }
    }
    shapes.iter().any(|s| contains(&s.shape, text))
}
#[test]
fn no_gui_disables_edit_and_explains_it_on_hover() {
    let (mut app, ctx, track) = app_with(false);
    for i in 0..8 {
        frame(&mut app, &ctx, f64::from(i), None);
    }
    assert!(app.player.as_ref().unwrap().live_name(track, 0).is_some(), "the host finished loading");
    assert!(app.player.as_ref().unwrap().has_editor(track, 0), "the headless controller is still adopted");
    assert!(!app.player.as_ref().unwrap().has_gui(track, 0));
    let card = &app.mixer.devices.cards[0];
    assert!(!card.editor_enabled, "a loaded no_gui plug-in must disable Edit");
    let r = card.editor_button;
    let pos = egui::pos2((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
    frame(&mut app, &ctx, 9.0, Some(pos));
    // PointerMoved resets egui's hover timer even at the same position. Leave the
    // pointer stationary while the injected clock advances past the tooltip delay.
    frame(&mut app, &ctx, 10.0, None);
    let shapes = frame(&mut app, &ctx, 11.0, None);
    assert!(has_text(&shapes, "This plug-in has no editor"));
}
#[test]
fn an_offered_editor_keeps_edit_enabled() {
    let (mut app, ctx, track) = app_with(true);
    for i in 0..8 {
        frame(&mut app, &ctx, f64::from(i), None);
    }
    assert!(app.player.as_ref().unwrap().has_editor(track, 0));
    assert!(app.mixer.devices.cards[0].editor_enabled);
}
