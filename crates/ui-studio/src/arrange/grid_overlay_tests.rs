use super::*;
use crate::tests::{app, json};
use egui::epaint::ClippedShape;

/// One audio clip of four bars on an audio track at 1000 samples per point (a bar is 96 points at
/// 120 BPM in 4/4, a beat 24), and the map and rect it is drawn with.
struct Scene {
    app: StudioApp,
    track: TrackId,
    clip: Clip,
    map: Map,
    rect: Rect,
}

fn scene() -> Scene {
    let mut app = app();
    let v = app.run("track.new", json!({"kind": "audio"})).unwrap();
    let track = TrackId(v["tracks"][0].as_u64().unwrap());
    transit_engine::io::import_audio_bytes(&mut app.engine, "grid.wav", &wav(), None, Some(track), 0).unwrap();
    let mut clip = app.engine.session().track(track).unwrap().clips()[0].clone();
    let bar = bar_at(app.engine.session(), 2);
    clip.start = 0;
    clip.length = 4 * bar;
    let map = Map { x0: 20.0, scroll: 0, spp: 1000.0, grid: Default::default() };
    let rect = Rect::from_min_max(pos2(map.x(0), 20.0), pos2(map.x(4 * bar), 20.0 + 88.0));
    Scene { app, track, clip, map, rect }
}

fn wav() -> Vec<u8> {
    let frames = 4096u32;
    let size = frames * 2;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + size).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(48_000u32.to_le_bytes());
    b.extend(96_000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(size.to_le_bytes());
    for i in 0..frames {
        b.extend((if i % 2 == 0 { 16000i16 } else { -16000i16 }).to_le_bytes());
    }
    b
}

/// The shapes `draw_clip` paints for the scene's clip with the current session state.
fn paint(sc: &Scene) -> Vec<ClippedShape> {
    let s = sc.app.engine.session();
    let track = s.track(sc.track).unwrap();
    let tl = Rect::from_min_max(pos2(20.0, 0.0), pos2(1000.0, 400.0));
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(egui::RawInput { time: Some(0.02), ..Default::default() }, |ui| {
        super::super::clips::draw_clip(ui.painter(), s, sc.map, tl, track, &sc.clip, &[], sc.rect, false, 1.0, false);
    });
    out.textures_delta.clear();
    out.shapes
}

fn flatten<'a>(shape: &'a Shape, out: &mut Vec<&'a Shape>) {
    if let Shape::Vec(v) = shape {
        v.iter().for_each(|s| flatten(s, out));
    } else {
        out.push(shape);
    }
}

fn is_vertical(s: &Shape) -> bool {
    matches!(s, Shape::LineSegment { points, .. } if points[0].x == points[1].x)
}

/// Every vertical line segment: (x, top, bottom, stroke alpha).
fn verticals(shapes: &[ClippedShape]) -> Vec<(f32, f32, f32, u8)> {
    let mut flat = Vec::new();
    shapes.iter().for_each(|s| flatten(&s.shape, &mut flat));
    flat.into_iter()
        .filter_map(|s| match s {
            Shape::LineSegment { points, stroke } if is_vertical(s) => Some((points[0].x, points[0].y, points[1].y, stroke.color.a())),
            _ => None,
        })
        .collect()
}

/// Where the lane grid puts the bar and beat lines inside the clip, computed from the tempo map
/// directly: (x, is a bar line).
fn expected(sc: &Scene) -> Vec<(f32, bool)> {
    let s = sc.app.engine.session();
    let mut xs = Vec::new();
    for bar in 1..=5 {
        for beat in 1..=4 {
            let at = s.tempo.samples_at_bar_beat(BarBeat { bar, beat, tick: 0 }, s.sample_rate);
            xs.push((sc.map.x(at), beat == 1));
        }
    }
    xs.retain(|(x, _)| *x >= sc.rect.min.x && *x <= sc.rect.max.x);
    xs
}

fn set(sc: &mut Scene, command: &str, value: bool) {
    sc.app.run(command, json!({"value": value})).unwrap();
}

#[test]
fn the_command_toggles_sets_and_leaves_no_undo_entry() {
    let mut a = app();
    let undo = format!("{:?}", a.engine.undo_label());
    assert!(!a.engine.session().edit.grid_over_clips, "off by default");
    assert_eq!(a.run("view.grid_over_clips", json!({})).unwrap(), json!({"value": true}));
    assert_eq!(a.run("view.grid_over_clips", json!({})).unwrap(), json!({"value": false}));
    assert_eq!(a.run("view.grid_over_clips", json!({"value": true})).unwrap(), json!({"value": true}));
    assert_eq!(a.run("view.grid_over_clips", json!({"value": true})).unwrap(), json!({"value": true}), "explicit true stays true");
    assert_eq!(a.run("view.grid_over_clips", json!({"value": false})).unwrap(), json!({"value": false}));
    assert_eq!(a.run("view.grid_over_clips", json!({"value": false})).unwrap(), json!({"value": false}), "explicit false stays false");
    assert_eq!(format!("{:?}", a.engine.undo_label()), undo, "a view command is not an undo step");
}

#[test]
fn the_flag_survives_a_session_round_trip_and_an_absent_field_loads_false() {
    let mut a = app();
    assert!(!a.engine.session().to_json().unwrap().contains("grid_over_clips"), "off is not written");
    a.run("view.grid_over_clips", json!({"value": true})).unwrap();
    let text = a.engine.session().to_json().unwrap();
    assert!(transit_model::Session::from_json(&text).unwrap().edit.grid_over_clips);
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["edit"].as_object_mut().unwrap().remove("grid_over_clips").is_some(), "the field is saved");
    let old = transit_model::Session::from_json(&v.to_string()).unwrap();
    assert!(!old.edit.grid_over_clips, "an older session opens with the overlay off");
    assert!(old.edit.grid_lines, "its neighbor keeps its own default");
}

#[test]
fn off_draws_no_grid_line_through_the_clip_and_on_draws_the_lane_grid_x_positions() {
    let mut sc = scene();
    assert!(sc.app.engine.session().edit.grid_lines);
    let off = paint(&sc);
    assert!(verticals(&off).is_empty(), "no vertical line over the clip with the option off: {:?}", verticals(&off));
    set(&mut sc, "view.grid_over_clips", true);
    let on = paint(&sc);
    let mut lines = verticals(&on);
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let body_top = sc.rect.min.y + CLIP_NAME_H;
    let want = expected(&sc);
    assert_eq!(lines.len(), want.len(), "{lines:?} vs {want:?}");
    let s = sc.app.engine.session();
    let ink = theme::text_on(theme::clip_colors(super::super::clips::clip_base_color(&sc.clip, s.track(sc.track).unwrap()), false).0);
    for ((x, top, bottom, alpha), (wx, bar)) in lines.iter().zip(&want) {
        assert!((x - wx).abs() < 0.01, "{x} vs {wx}");
        assert!(*x >= sc.rect.min.x && *x <= sc.rect.max.x, "inside the clip: {x}");
        assert!(*top >= body_top && *bottom <= sc.rect.max.y, "inside the body, not the name strip: {top}..{bottom}");
        let nominal = if *bar { BAR_ALPHA } else { BEAT_ALPHA };
        assert_eq!(*alpha, ink.gamma_multiply(nominal).a(), "x={x} bar={bar}");
    }
    // Everything else painted is exactly what the option-off paint drew.
    let mut flat = Vec::new();
    on.iter().for_each(|s| flatten(&s.shape, &mut flat));
    let rest: Vec<String> = flat.iter().filter(|s| !is_vertical(s)).map(|s| format!("{s:?}")).collect();
    let mut base = Vec::new();
    off.iter().for_each(|s| flatten(&s.shape, &mut base));
    assert_eq!(rest, base.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>(), "the paint with the option off is unchanged");
}

#[test]
fn hidden_grid_lines_hide_the_overlay_even_when_the_option_is_on() {
    let mut sc = scene();
    set(&mut sc, "view.grid_over_clips", true);
    sc.app.run("view.grid_lines", json!({"value": false})).unwrap();
    assert!(verticals(&paint(&sc)).is_empty());
    sc.app.run("view.grid_lines", json!({"value": true})).unwrap();
    assert!(!verticals(&paint(&sc)).is_empty());
}

#[test]
fn the_overlay_stays_inside_a_clip_that_is_scrolled_partly_off_screen() {
    let mut sc = scene();
    set(&mut sc, "view.grid_over_clips", true);
    // The timeline begins two bars in: only the part of the clip inside it is stroked.
    let bar = bar_at(sc.app.engine.session(), 2);
    sc.map.scroll = 2 * bar;
    sc.rect = Rect::from_min_max(pos2(sc.map.x(0), 20.0), pos2(sc.map.x(4 * bar), 108.0));
    let lines = verticals(&paint(&sc));
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|l| l.0 >= 20.0 && l.0 <= sc.rect.max.x), "{lines:?}");
}

/// A frame-stepped UI harness for the menu: synthetic clicks through the control channel.
struct Ui {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl Ui {
    fn new() -> Ui {
        let mut ui = Ui { app: app(), ctx: egui::Context::default(), time: 0.0 };
        ui.frames(3);
        ui
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))),
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

    fn menu(&mut self, name: &str) {
        let v = self.app.inspect(&self.ctx);
        let rect = v["bar_menu"].as_array().and_then(|a| a.iter().find(|i| i["label"] == name).map(|i| i["rect"].clone())).unwrap_or_default();
        let r: [f32; 4] = serde_json::from_value(rect).unwrap_or_else(|_| panic!("no menu item {name}"));
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(30);
    }
}

#[test]
fn inspect_reports_the_flag_and_the_view_menu_item_dispatches_the_command() {
    let mut t = Ui::new();
    assert_eq!(t.app.inspect(&t.ctx)["layout"]["grid_over_clips"], json!(false));
    t.menu("View");
    t.menu("Grid Over Clips");
    assert!(t.app.engine.session().edit.grid_over_clips);
    t.frames(2);
    assert_eq!(t.app.inspect(&t.ctx)["layout"]["grid_over_clips"], json!(true));
    assert!(t.app.engine.journal.iter().any(|(id, _)| id == "view.grid_over_clips"));
    t.menu("View");
    t.menu("Grid Over Clips");
    assert!(!t.app.engine.session().edit.grid_over_clips, "the item toggles back");
}
