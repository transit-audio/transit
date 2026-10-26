#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The MIDI Mappings window (issue #141): a table in a scroll area inside bounded window sizes,
//! and plug-in parameters named by the parameter table instead of an index.
use super::*;
use crate::midi_learn;

fn new_app(w: f32, h: f32) -> (StudioApp, egui::Context, TrackId) {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Gtr"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    let ctx = egui::Context::default();
    frames(&mut app, &ctx, 3, (w, h));
    (app, ctx, t)
}

fn frames(app: &mut StudioApp, ctx: &egui::Context, n: usize, (w, h): (f32, f32)) {
    for i in 0..n {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            time: Some(f64::from(u32::try_from(i).unwrap()) / 60.0 + 1.0),
            ..Default::default()
        };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
}

fn map(app: &mut StudioApp, port: &str, target: &str, cc: u8) {
    app.run("midi.map", json!({"source": {"port": port, "channel": 1, "kind": "cc", "number": cc}, "target": target})).unwrap();
}

fn open(app: &mut StudioApp, ctx: &egui::Context, screen: (f32, f32)) -> [f32; 4] {
    app.run("window.midi_mappings", json!({})).unwrap();
    frames(app, ctx, 6, screen);
    app.midi_learn.window
}

#[test]
fn an_oversized_stored_size_opens_within_the_bounds() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    for i in 0..30 {
        map(&mut app, "KeyStep", &format!("track:{}:volume", t.0), 10 + i);
    }
    map(&mut app, "KeyStep", &format!("track:{}:pan", t.0), 50);
    app.ui.midi_mappings.size = [4000.0, 3000.0];
    let w = open(&mut app, &ctx, screen);
    let (min, max) = midi_learn::window_bounds(egui::vec2(screen.0, screen.1));
    let (rw, rh) = (w[2] - w[0], w[3] - w[1]);
    assert!(rw >= min.x - 1.0 && rw <= max.x + 1.0, "width {rw} within {}..{}", min.x, max.x);
    assert!(rh >= min.y - 1.0 && rh <= max.y + 1.0, "height {rh} within {}..{}", min.y, max.y);
    assert!(w[3] <= screen.1, "the window stays on the screen");
    let saved = app.ui.midi_mappings.size;
    assert!(saved[0] <= max.x + 1.0 && saved[1] <= max.y + 1.0, "the saved size is clamped: {saved:?}");
    // A tiny stored size opens at the minimum.
    app.ui.midi_mappings.open = false;
    frames(&mut app, &ctx, 2, screen);
    app.ui.midi_mappings.size = [10.0, 10.0];
    let ctx = egui::Context::default();
    let w = open(&mut app, &ctx, screen);
    assert!(w[2] - w[0] >= min.x - 1.0 && w[3] - w[1] >= min.y - 1.0, "{w:?} at least {min:?}");
}

#[test]
fn the_target_cell_names_a_plugin_parameter_by_its_table() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    app.add_device(t, "eq_7band").unwrap();
    map(&mut app, "KeyStep", &format!("track:{}:plugin:a:mid_gain", t.0), 74);
    map(&mut app, "KeyStep", &format!("track:{}:volume", t.0), 7);
    open(&mut app, &ctx, screen);
    let info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
    let name = info.param("mid_gain").unwrap().name;
    let targets: Vec<String> = midi_learn::rows(&app).into_iter().map(|r| r.target).collect();
    assert!(targets.contains(&format!("Gtr: {} > {name}", info.name)), "{targets:?}");
    assert!(targets.contains(&"Gtr: Volume".to_string()), "mixer controls keep their name: {targets:?}");
}

#[test]
fn the_target_cell_falls_back_to_the_index_while_the_table_is_not_cached() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    if app.run("mix.insert", json!({"track": t.0, "slot": 0, "plugin": "vst3:not-cached"})).is_err() {
        return;
    }
    map(&mut app, "KeyStep", &format!("track:{}:plugin:a:40", t.0), 74);
    open(&mut app, &ctx, screen);
    let targets: Vec<String> = midi_learn::rows(&app).into_iter().map(|r| r.target).collect();
    assert_eq!(targets, vec!["Gtr: vst3:not-cached > parameter 40".to_string()]);
}

#[test]
fn a_long_port_name_keeps_learn_and_delete_inside_the_window() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    map(&mut app, &"Very Long Controller Port Name ".repeat(6), &format!("track:{}:volume", t.0), 7);
    app.ui.midi_mappings.size = [0.0, 0.0];
    let w = open(&mut app, &ctx, screen);
    let win = egui::Rect::from_min_max(egui::pos2(w[0], w[1]), egui::pos2(w[2], w[3]));
    let (_, max) = midi_learn::window_bounds(egui::vec2(screen.0, screen.1));
    assert!(win.width() <= max.x + 1.0, "the long name does not widen the window: {}", win.width());
    // The buttons are the last cells of the row: find them by their labels in the rendered shapes.
    let mut learn = None;
    let mut delete = None;
    ctx.output_mut(|_| {});
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(screen.0, screen.1))),
        time: Some(99.0),
        ..Default::default()
    };
    let out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    for clipped in &out.shapes {
        collect_text(&clipped.shape, &mut |text, rect| match text {
            "Learn" => learn = Some(rect),
            "Delete" => delete = Some(rect),
            _ => {}
        });
    }
    let (learn, delete) = (learn.expect("a Learn button"), delete.expect("a Delete button"));
    assert!(win.contains_rect(learn) && win.contains_rect(delete), "{learn:?} {delete:?} inside {win:?}");
}

fn collect_text(shape: &egui::epaint::Shape, f: &mut dyn FnMut(&str, egui::Rect)) {
    match shape {
        egui::epaint::Shape::Text(t) => f(t.galley.text(), t.visual_bounding_rect()),
        egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, f)),
        _ => {}
    }
}
