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
    let (min, max) = midi_learn::window_bounds(egui::vec2(screen.0, screen.1), midi_learn::stretch_widths(&ctx, &midi_learn::rows(&app)));
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
    // A plug-in the registries do not know: no parameter table is cached for it (K81).
    app.engine.session_mut().track_mut(t).unwrap().mixer.inserts[0] = Some(soundcraft_model::Insert::new("vst3:not-cached"));
    map(&mut app, "KeyStep", &format!("track:{}:plugin:a:40", t.0), 74);
    open(&mut app, &ctx, screen);
    let targets: Vec<String> = midi_learn::rows(&app).into_iter().map(|r| r.target).collect();
    assert_eq!(targets, vec!["Gtr: vst3:not-cached > parameter 40".to_string()]);
}

/// One frame with `events`, returning the text shapes drawn as (text, rect).
fn frame_with(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
    *time += 1.0 / 60.0;
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))),
        time: Some(*time),
        events,
        ..Default::default()
    };
    app.raw_input_hook(&mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    let mut texts = Vec::new();
    for c in &out.shapes {
        collect_text(&c.shape, &mut |t, r| texts.push((t.to_string(), r)));
    }
    texts
}

fn range_app() -> (StudioApp, egui::Context, f64, egui::Pos2) {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    map(&mut app, "KeyStep", &format!("track:{}:volume", t.0), 7);
    open(&mut app, &ctx, screen);
    let mut time = 10.0;
    let texts = frame_with(&mut app, &ctx, &mut time, vec![]);
    let low = texts
        .iter()
        .find(|(t, _)| t == "0.0")
        .unwrap_or_else(|| panic!("the low Range field in {:?}", texts.iter().map(|x| x.0.clone()).collect::<Vec<_>>()))
        .1;
    (app, ctx, time, low.center())
}

fn range_of(app: &StudioApp) -> [f32; 2] {
    let m = app.engine.session().midi_mappings.iter().next().unwrap().clone();
    [m.min, m.max]
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
}

#[test]
fn dragging_the_low_range_field_over_several_frames_commits_the_range() {
    let (mut app, ctx, mut time, at) = range_app();
    frame_with(&mut app, &ctx, &mut time, vec![egui::Event::PointerMoved(at)]);
    frame_with(&mut app, &ctx, &mut time, vec![button(at, true)]);
    for i in 1..=6 {
        let p = at + egui::vec2(5.0 * i as f32, 0.0);
        frame_with(&mut app, &ctx, &mut time, vec![egui::Event::PointerMoved(p)]);
    }
    assert_eq!(range_of(&app), [0.0, 1.0], "nothing commits mid-drag");
    let end = at + egui::vec2(30.0, 0.0);
    frame_with(&mut app, &ctx, &mut time, vec![button(end, false)]);
    frame_with(&mut app, &ctx, &mut time, vec![]);
    let [lo, hi] = range_of(&app);
    assert!(lo > 0.1 && hi == 1.0, "the drag moved the low end: {lo} {hi}");
}

#[test]
fn typing_a_range_value_and_pressing_enter_commits_it() {
    let (mut app, ctx, mut time, at) = range_app();
    frame_with(&mut app, &ctx, &mut time, vec![egui::Event::PointerMoved(at)]);
    frame_with(&mut app, &ctx, &mut time, vec![button(at, true)]);
    frame_with(&mut app, &ctx, &mut time, vec![button(at, false)]);
    frame_with(&mut app, &ctx, &mut time, vec![]);
    frame_with(&mut app, &ctx, &mut time, vec![egui::Event::Text("50".into())]);
    let key = |pressed| egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame_with(&mut app, &ctx, &mut time, vec![key(true)]);
    frame_with(&mut app, &ctx, &mut time, vec![key(false)]);
    frame_with(&mut app, &ctx, &mut time, vec![]);
    assert_eq!(range_of(&app), [0.5, 1.0]);
}

#[test]
fn a_long_port_name_keeps_learn_and_delete_inside_the_window() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    map(&mut app, &"Very Long Controller Port Name ".repeat(6), &format!("track:{}:volume", t.0), 7);
    app.ui.midi_mappings.size = [0.0, 0.0];
    let w = open(&mut app, &ctx, screen);
    let win = egui::Rect::from_min_max(egui::pos2(w[0], w[1]), egui::pos2(w[2], w[3]));
    let (_, max) = midi_learn::window_bounds(egui::vec2(screen.0, screen.1), midi_learn::stretch_widths(&ctx, &midi_learn::rows(&app)));
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

/// The text shapes of one frame as (text, rect, clip rect).
fn texts_clipped(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64) -> Vec<(String, egui::Rect, egui::Rect)> {
    *time += 1.0 / 60.0;
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0))),
        time: Some(*time),
        ..Default::default()
    };
    app.raw_input_hook(&mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    let mut v = Vec::new();
    for c in &out.shapes {
        collect_text(&c.shape, &mut |t, r| v.push((t.to_string(), r, c.clip_rect)));
    }
    v
}

#[test]
fn both_range_fields_and_the_buttons_are_drawn_whole_at_the_default_size() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    map(&mut app, "Arturia KeyLab 49 MkII", &format!("track:{}:volume", t.0), 7);
    app.run("midi.map", json!({"source": {"port": "Pads", "channel": "any", "kind": "cc", "number": 9}, "target": format!("track:{}:pan", t.0), "range": [0.15, 0.75]})).unwrap();
    open(&mut app, &ctx, screen);
    let mut time = 10.0;
    let texts = texts_clipped(&mut app, &ctx, &mut time);
    let whole = |text: &str| {
        let hits: Vec<_> = texts.iter().filter(|(t, ..)| t == text).collect();
        assert!(!hits.is_empty(), "{text:?} is drawn in {:?}", texts.iter().map(|x| x.0.clone()).collect::<Vec<_>>());
        for (t, r, clip) in hits {
            assert!(clip.expand(0.5).contains_rect(*r), "{t:?} {r:?} is clipped to {clip:?}");
        }
    };
    for text in ["0.0", "100.0", "15.0", "75.0", "any", "Learn", "Delete"] {
        whole(text);
    }
    assert_eq!(texts.iter().filter(|(t, ..)| t == "%").count(), 4, "four percent suffixes");
    for (t, r, clip) in texts.iter().filter(|(t, ..)| t == "%") {
        assert!(clip.expand(0.5).contains_rect(*r), "{t:?} {r:?} is clipped to {clip:?}");
    }
}

#[test]
fn typical_names_show_untruncated_at_the_default_size() {
    let screen = (1920.0, 1080.0);
    let (mut app, ctx, t) = new_app(screen.0, screen.1);
    app.run("track.rename", json!({"track": t.0, "name": "Lead: Subtractive Synth"})).ok();
    app.add_device(t, "eq_7band").unwrap();
    map(&mut app, "Arturia KeyLab 49 MkII", &format!("track:{}:plugin:a:mid_gain", t.0), 74);
    open(&mut app, &ctx, screen);
    let target = midi_learn::rows(&app)[0].target.clone();
    let mut time = 10.0;
    let texts = texts_clipped(&mut app, &ctx, &mut time);
    for want in ["Arturia KeyLab 49 MkII", target.as_str()] {
        assert!(
            texts.iter().any(|(t, ..)| t == want),
            "{want:?} is drawn whole (not truncated) in {:?}",
            texts.iter().map(|x| x.0.clone()).collect::<Vec<_>>()
        );
    }
}
