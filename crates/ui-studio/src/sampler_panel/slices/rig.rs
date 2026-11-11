//! Shared helpers of the Slices view's tests (on the panel's rig, `super::super::test_rig`).

use super::super::test_rig::{T, mid};
use super::BenchLayout;
use super::panels::PanelsLayout;
use serde_json::json;
use transit_model::TrackId;

/// A kit with a one-second loop at 44.1 kHz (the session runs at 48 kHz), Manual markers at a
/// quarter, a half and three quarters, the four slices on pads A1 to A4, the Slices view showing.
pub fn sliced() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    let path = t.sample_at("Loop.wav", 44_100, 44_100);
    t.run("mix.sampler_slice_loop", json!({"track": tr.0, "path": path.to_string_lossy()}));
    for frame in [11_025, 22_050, 33_075] {
        t.run("mix.sampler_slice_marker", json!({"track": tr.0, "op": "add", "frame": frame, "snap": false}));
    }
    t.run("mix.sampler_slices_to_pads", json!({"track": tr.0}));
    show_slices(&mut t, tr);
    (t, tr)
}

/// The same loop in Auto, its Auto set at the same three markers (S included: four slices), Manual
/// at S only, nothing on pads.
pub fn auto() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    let path = t.sample_at("Loop.wav", 44_100, 44_100);
    t.run("mix.sampler_slice_loop", json!({"track": tr.0, "path": path.to_string_lossy()}));
    detect(&mut t, tr, json!({"source": "grid", "division": "1/4", "grid_bpm": 240.0, "at": 0}));
    let c = wait_candidate(&mut t, tr);
    t.run("mix.sampler_slice_replace", json!({"track": tr.0, "candidate_id": c["candidate_id"]}));
    show_slices(&mut t, tr);
    (t, tr)
}

/// Runs Detect directly (fixture setup, not through the panel).
pub fn detect(t: &mut T, tr: TrackId, mut p: serde_json::Value) {
    p["track"] = json!(tr.0);
    t.app.engine.execute("mix.sampler_slice_detect", &p).unwrap();
}

/// The track's candidate once its worker finished: a handshake on the engine's own state (the
/// worker runs a few hundred frames of analysis; nothing here measures time).
pub fn wait_candidate(t: &mut T, tr: TrackId) -> serde_json::Value {
    for _ in 0..50_000_000u64 {
        let c = t.app.engine.execute("mix.sampler_slice_candidate", &json!({"track": tr.0})).unwrap()["candidate"].clone();
        if c["state"] != "pending" {
            return c;
        }
        std::thread::yield_now();
    }
    panic!("the detection never finished");
}

pub fn show_slices(t: &mut T, tr: TrackId) {
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[2]), 1);
}

pub fn bench(t: &T, tr: TrackId) -> BenchLayout {
    t.panel(tr).slices.expect("the Slices view's bench")
}

pub fn panels(t: &T, tr: TrackId) -> PanelsLayout {
    t.panel(tr).slice_panels.expect("the Slices view's panels")
}

/// The instance's Slices view state.
pub fn view(t: &T) -> super::SlicesView {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().slices.clone()
}

/// The commands the view sent since `from` (an index into its log).
pub fn sent(t: &T, from: usize) -> Vec<String> {
    view(t).sent.into_iter().skip(from).collect()
}

/// The showing set's marker frames as the engine stores them.
pub fn frames(t: &T, tr: TrackId) -> Vec<u64> {
    let p = t.patch(tr);
    let l = p.slicing.unwrap();
    let set = if l.mode == transit_model::sampler_patch::SliceMode::Auto { l.auto } else { l.manual };
    set.map(|m| m.markers.iter().map(|x| x.frame).collect()).unwrap_or_default()
}

pub fn h(x: f32) -> i32 {
    (x * 100.0).round() as i32
}

/// The point on the bench (under the flag strip, above the handles) over `frame` of the
/// one-second 44.1 kHz loop at 1×.
pub fn at_frame(b: &BenchLayout, frame: u64) -> (f32, f32) {
    let (left, right) = (b.well[0] + 1.0, b.well[2] - 1.0);
    let x = left + (frame as f64 / 44_100.0) as f32 * (right - left);
    (x, (b.strip[3] + b.well[3]) / 2.0 - 20.0)
}

/// Sends `events` one per step (each step runs a few frames).
pub fn steps(t: &mut T, events: Vec<egui::Event>) {
    for e in events {
        t.input(vec![e]);
    }
}

pub fn press(at: (f32, f32), pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos: egui::pos2(at.0, at.1), button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

pub fn moved(at: (f32, f32)) -> egui::Event {
    egui::Event::PointerMoved(egui::pos2(at.0, at.1))
}

pub fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
}
