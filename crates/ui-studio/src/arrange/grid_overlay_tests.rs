use super::*;
use crate::tests::{app, json};
use crate::theme::Tokens;
use egui::epaint::ClippedShape;

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

/// The grid's stroke colors in either layer: the opaque tokens beneath the clips and the
/// translucent overlay strokes above them.
fn grid_colors() -> Vec<Color32> {
    let kinds = [Kind::Bar { labeled: true }, Kind::Beat, Kind::Sub];
    let mut v = vec![TOKENS.grid_bar, TOKENS.grid_beat, TOKENS.grid_sub];
    for k in kinds {
        let a = body_alpha(overlay_color(k).a());
        v.extend([overlay_color(k), Color32::from_black_alpha(a), Color32::from_white_alpha(a)]);
    }
    v
}

/// One frame's shapes, in paint order, flattened.
fn flat(shapes: &[ClippedShape]) -> Vec<Shape> {
    let mut out = Vec::new();
    shapes.iter().for_each(|s| flatten(&s.shape, &mut out));
    out.into_iter().cloned().collect()
}

fn flatten<'a>(shape: &'a Shape, out: &mut Vec<&'a Shape>) {
    if let Shape::Vec(v) = shape {
        v.iter().for_each(|s| flatten(s, out));
    } else {
        out.push(shape);
    }
}

/// A studio with one four-bar audio clip, framed.
fn with_clip() -> Ui {
    let mut ui = Ui::new();
    let v = ui.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let track = TrackId(v["tracks"][0].as_u64().unwrap());
    transit_engine::io::import_audio_bytes(&mut ui.app.engine, "grid.wav", &wav(), None, Some(track), 0).unwrap();
    ui.frames(3);
    ui
}

/// What a frame painted: the lane grid segments (index in paint order, x) and the clip's rect
/// shapes (indexes in paint order).
struct Painted {
    grid: Vec<(usize, f32)>,
    clip: Vec<usize>,
    all: Vec<String>,
}

fn painted(ui: &mut Ui) -> Painted {
    ui.frames(2);
    let l = &ui.app.arrange.layout;
    let tl = l.timeline;
    let clip = l.clips[0].rect;
    let shapes = flat(&ui.shapes);
    let mut grid = Vec::new();
    let mut clip_ix = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        match s {
            Shape::LineSegment { points, stroke }
                if points[0].x == points[1].x
                    && grid_colors().contains(&stroke.color)
                    && points[0].y >= tl[1] - 0.5
                    && points[1].y <= tl[3] + 0.5 =>
            {
                grid.push((i, points[0].x));
            }
            Shape::Rect(r)
                if (r.rect.min.x - clip[0]).abs() < 0.5 && (r.rect.max.x - clip[2]).abs() < 0.5 && (r.rect.min.y - clip[1]).abs() < 0.5 =>
            {
                clip_ix.push(i)
            }
            _ => {}
        }
    }
    Painted { grid, clip: clip_ix, all: shapes.iter().map(|s| format!("{s:?}")).collect() }
}

fn xs(p: &Painted) -> Vec<f32> {
    let mut x: Vec<f32> = p.grid.iter().map(|g| g.1).collect();
    x.sort_by(f32::total_cmp);
    // Split segments share an x; the exact segment counts guard against a second stroke.
    x.dedup();
    x
}

#[test]
fn off_paints_the_grid_once_beneath_every_clip_shape_and_the_frame_is_stable() {
    let mut ui = with_clip();
    let off = painted(&mut ui);
    assert!(!off.clip.is_empty(), "the clip's shapes were found");
    assert!(off.grid.len() >= 8, "the lane grid is painted: {}", off.grid.len());
    let last_grid = off.grid.iter().map(|g| g.0).max().unwrap();
    assert!(last_grid < *off.clip.iter().min().unwrap(), "grid index {last_grid} before clip index {:?}", off.clip);
    // On and back off: the paint is the same as it was.
    ui.app.run("view.grid_over_clips", json!({"value": true})).unwrap();
    let _ = painted(&mut ui);
    ui.app.run("view.grid_over_clips", json!({"value": false})).unwrap();
    assert_eq!(painted(&mut ui).all, off.all, "option off paints byte-identically");
}

#[test]
fn on_paints_the_same_single_grid_above_every_clip_shape() {
    let mut ui = with_clip();
    let off = painted(&mut ui);
    ui.app.run("view.grid_over_clips", json!({"value": true})).unwrap();
    let on = painted(&mut ui);
    let first_grid = on.grid.iter().map(|g| g.0).min().unwrap();
    let last_clip = *on.clip.iter().max().unwrap();
    assert!(first_grid > last_clip, "first grid segment index {first_grid} after the last clip shape {last_clip}");
    let l = &ui.app.arrange.layout.clips[0];
    let (clip, strip, body) = (l.rect, l.strip, l.body);
    let crossing = off.grid.iter().filter(|g| g.1 >= clip[0] && g.1 <= clip[2]).count();
    // A line over the clip is lane, strip, body, lane; the strip and body merge when they share a direction.
    let extra = if bright(strip) == bright(body) { 2 } else { 3 };
    assert_eq!(on.grid.len(), off.grid.len() + extra * crossing, "each line crossing the clip is split, nothing else is stroked twice");
    assert_eq!(xs(&on), xs(&off), "the same x positions");
    assert_eq!(on.all.len(), off.all.len() + extra * crossing, "nothing else was added or dropped");
}

#[test]
fn toggling_the_command_flips_the_layer_and_leaves_the_segment_set() {
    let mut ui = with_clip();
    let before = painted(&mut ui);
    for _ in 0..2 {
        ui.app.run("view.grid_over_clips", json!({})).unwrap();
        let on = painted(&mut ui);
        assert!(on.grid.iter().all(|g| g.0 > *on.clip.iter().max().unwrap()), "above the clip when on");
        ui.app.run("view.grid_over_clips", json!({})).unwrap();
        let off = painted(&mut ui);
        assert!(off.grid.iter().all(|g| g.0 < *off.clip.iter().min().unwrap()), "beneath the clip when off");
        assert_eq!(xs(&on), xs(&off));
        assert_eq!(xs(&off), xs(&before));
    }
}

#[test]
fn hidden_grid_lines_keep_the_grid_beneath_the_clips_even_when_the_option_is_on() {
    let mut ui = with_clip();
    ui.app.run("view.grid_over_clips", json!({"value": true})).unwrap();
    ui.app.run("view.grid_lines", json!({"value": false})).unwrap();
    let p = painted(&mut ui);
    assert!(p.grid.iter().all(|g| g.0 < *p.clip.iter().min().unwrap()), "beneath, as with the option off");
    ui.app.run("view.grid_lines", json!({"value": true})).unwrap();
    let p = painted(&mut ui);
    assert!(p.grid.iter().all(|g| g.0 > *p.clip.iter().max().unwrap()), "above once lines are shown");
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

/// A frame-stepped UI harness for the menu: synthetic clicks through the control channel.
struct Ui {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    shapes: Vec<ClippedShape>,
}

impl Ui {
    fn new() -> Ui {
        let mut ui = Ui { app: app(), ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() };
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
            self.shapes = out.shapes;
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

const KINDS: [Kind; 3] = [Kind::Bar { labeled: true }, Kind::Beat, Kind::Sub];

/// Straight-alpha blend of an overlay stroke over an opaque body, per channel, rounded: the
/// stroke is premultiplied (`Color32`), so out = premultiplied rgb + body * (255 - a) / 255.
fn over(stroke: Color32, body: [u8; 3]) -> [i32; 3] {
    let a = i32::from(stroke.a());
    let s = [stroke.r(), stroke.g(), stroke.b()];
    std::array::from_fn(|i| (f64::from(s[i]) + f64::from(body[i]) * f64::from(255 - a) / 255.0).round() as i32)
}

fn both_themes(f: impl Fn(&Tokens)) {
    for t in [&crate::theme::DARK, &crate::theme::LIGHT] {
        f(t);
    }
}

fn tok(t: &Tokens, kind: Kind) -> Color32 {
    match kind {
        Kind::Bar { labeled: true } => t.grid_bar,
        Kind::Bar { labeled: false } | Kind::Beat => t.grid_beat,
        Kind::Sub => t.grid_sub,
    }
}

/// How far, per channel, the gray overlay over an empty lane may land from the grid token it stands
/// for. The overlay is gray (one alpha for all channels, the mean of the three distances) and the
/// tokens lean blue, so a channel can land a few steps off. The allowance is fixed, never derived
/// from the token under test:
/// - dark: 3, and the token's lean (how far its channel distances stray from their mean) at most 2,
///   as before this branch (the tokens sit 25, 25, 27 above the lane);
/// - light: 4. The designed light bar line `#CDD1D6` sits 38, 35, 32 below the lane `#F3F4F6`, so
///   its blue channel lands 4 off: one step past the old bound of 3, which is the whole reason the
///   light allowance is 4 (#326).
const OVERLAY_BOUND_DARK: i32 = 3;
const OVERLAY_BOUND_LIGHT: i32 = 4;
/// The dark set's fixed cap on a grid token's lean.
const DARK_LEAN_CAP: f32 = 2.0;

/// The overlay for `token` over an empty `lane`, checked against the fixed `bound` (and the fixed
/// lean cap, where there is one): `Err` says what missed.
fn overlay_reproduces(token: Color32, lane: Color32, bound: i32, lean_cap: Option<f32>) -> Result<(), String> {
    if let Some(cap) = lean_cap {
        let d: Vec<f32> =
            [(token.r(), lane.r()), (token.g(), lane.g()), (token.b(), lane.b())].iter().map(|(a, b)| f32::from(*a) - f32::from(*b)).collect();
        let mean = d.iter().sum::<f32>() / 3.0;
        let lean = d.iter().map(|x| (x - mean).abs()).fold(0.0, f32::max);
        if lean > cap {
            return Err(format!("lean {lean} over the cap {cap}"));
        }
    }
    let o = translucent(token, lane, 1.0);
    let got = over(o, [lane.r(), lane.g(), lane.b()]);
    let want = [token.r(), token.g(), token.b()].map(i32::from);
    for i in 0..3 {
        if (got[i] - want[i]).abs() > bound {
            return Err(format!("channel {i}: {got:?} vs {want:?} (overlay {o:?}, bound {bound})"));
        }
    }
    Ok(())
}

#[test]
fn the_overlay_over_an_empty_lane_reproduces_the_token_in_both_themes() {
    both_themes(|t| {
        let (bound, cap) = if t.light_clips { (OVERLAY_BOUND_LIGHT, None) } else { (OVERLAY_BOUND_DARK, Some(DARK_LEAN_CAP)) };
        for k in KINDS {
            if let Err(e) = overlay_reproduces(tok(t, k), t.lane, bound, cap) {
                panic!("{k:?}: {e}");
            }
        }
    });
    // Negative controls: a token a gray cannot stand for fails. A graphite-hue light bar line
    // (#C8CFD8, 6 off in one channel) misses the light bound of 4; a bluer dark one (#30353B, a lean
    // of 2.7) misses the dark lean cap.
    let (d, l) = (&crate::theme::DARK, &crate::theme::LIGHT);
    assert!(overlay_reproduces(crate::tokens::hex(0xC8CFD8), l.lane, OVERLAY_BOUND_LIGHT, None).is_err(), "light #C8CFD8");
    assert!(overlay_reproduces(crate::tokens::hex(0x30353B), d.lane, OVERLAY_BOUND_DARK, Some(DARK_LEAN_CAP)).is_err(), "dark #30353B");
    // And the designed light bar line needs the 4: it misses the old 3.
    assert!(overlay_reproduces(l.grid_bar, l.lane, OVERLAY_BOUND_DARK, None).is_err(), "the designed light bar line misses 3");
}

#[test]
fn the_overlay_tints_a_bright_body_proportionally_never_with_an_opaque_line() {
    both_themes(|t| {
        for body in [[0x5a, 0x9a, 0xd8], [0xc8, 0xd4, 0x5a]] {
            for k in KINDS {
                let o = translucent(tok(t, k), t.lane, 1.0);
                assert!(o.a() < 255 && o.a() > 0, "{k:?} is translucent: {o:?}");
                let got = over(o, body);
                let a = f64::from(o.a()) / 255.0;
                for i in 0..3 {
                    // Dark theme tints toward white, light toward black: the shift is the alpha
                    // times the headroom of that channel, so never more than the alpha of range.
                    let headroom = if o.r() > 0 { 255 - i32::from(body[i]) } else { i32::from(body[i]) };
                    let shift = (got[i] - i32::from(body[i])).abs();
                    assert!(f64::from(shift) <= a * f64::from(headroom) + 1.0, "{k:?} body {body:?} channel {i}: shift {shift}");
                    assert!(f64::from(shift) <= 0.20 * 255.0, "no channel moves more than 20 percent of its range: {shift}");
                }
            }
        }
    });
}

#[test]
fn the_overlay_is_an_extreme_toward_the_tokens_side_of_the_lane() {
    let dark = translucent(crate::theme::DARK.grid_bar, crate::theme::DARK.lane, 1.0);
    assert_eq!((dark.r(), dark.g(), dark.b()), (dark.a(), dark.a(), dark.a()), "white at alpha (premultiplied)");
    let light = translucent(crate::theme::LIGHT.grid_bar, crate::theme::LIGHT.lane, 1.0);
    assert_eq!((light.r(), light.g(), light.b()), (0, 0, 0), "black");
    assert!(translucent(crate::theme::DARK.grid_bar, crate::theme::DARK.lane, 2.0).a() > dark.a(), "gain scales alpha");
}

/// The colors of the full-height-or-split vertical grid strokes of a frame.
fn grid_stroke_colors(ui: &Ui) -> Vec<Color32> {
    let tl = ui.app.arrange.layout.timeline;
    flat(&ui.shapes)
        .iter()
        .filter_map(|s| match s {
            Shape::LineSegment { points, stroke }
                if points[0].x == points[1].x
                    && grid_colors().contains(&stroke.color)
                    && points[0].y >= tl[1] - 0.5
                    && points[1].y <= tl[3] + 0.5 =>
            {
                Some(stroke.color)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_lane_pass_strokes_the_opaque_tokens_and_the_overlay_pass_translucent_ones() {
    let mut ui = with_clip();
    ui.frames(2);
    let off = grid_stroke_colors(&ui);
    assert!(!off.is_empty() && off.iter().all(|c| c.a() == 255), "beneath the clips every stroke is an opaque token: {off:?}");
    ui.app.run("view.grid_over_clips", json!({"value": true})).unwrap();
    ui.frames(2);
    let on = grid_stroke_colors(&ui);
    assert!(!on.is_empty() && on.iter().all(|c| c.a() < 255), "above the clips every stroke is translucent: {on:?}");
}

fn body(x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) -> Body {
    Body { rect: Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), color: c }
}

#[test]
fn a_bar_line_darkens_a_bright_body_and_lightens_the_lane_by_the_token_fraction() {
    both_themes(|t| {
        // The dark theme's lane lightens; its bright-body stroke is black at the same alpha.
        let tl = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
        let line = Line { x: 50.0, kind: Kind::Bar { labeled: true }, bar: 1 };
        let bright_body = Color32::from_rgb(0xc8, 0xd4, 0x5a);
        let lane = translucent(t.grid_bar, t.lane, 1.0);
        let on_body = segments(&line, tl, &[body(0.0, 30.0, 100.0, 60.0, bright_body)])[1].2;
        let a = f64::from(body_alpha(lane.a())) / 255.0;
        if lane.r() == 0 {
            return; // the light theme: the lane stroke is black already; the dark theme covers the split
        }
        let b = [0xc8u8, 0xd4, 0x5a];
        let dark = over(on_body, b);
        for i in 0..3 {
            let want = (f64::from(b[i]) * (1.0 - a)).round() as i32;
            assert!(dark[i] < i32::from(b[i]) && (dark[i] - want).abs() <= 1, "over the body channel {i}: {dark:?}, want {want}");
        }
        let l = [t.lane.r(), t.lane.g(), t.lane.b()];
        let lit = over(lane, l);
        let token = [t.grid_bar.r(), t.grid_bar.g(), t.grid_bar.b()].map(i32::from);
        for i in 0..3 {
            assert!((lit[i] - token[i]).abs() <= 3, "over the lane channel {i}: {lit:?} vs {token:?}");
        }
        // Dark theme: the lane is lightened (token above lane); light theme: darkened.
        let dir = (lit[0] - i32::from(l[0])).signum();
        assert_eq!(dir, if t.lane.r() < 128 { 1 } else { -1 }, "the lane's own direction is kept");
        if t.lane.r() < 128 {
            assert!(lit[0] > i32::from(l[0]), "lighter than the lane");
        }
    });
}

#[test]
fn a_dark_body_is_lightened_and_a_line_splits_at_each_body_edge() {
    let tl = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
    let line = Line { x: 50.0, kind: Kind::Beat, bar: 1 };
    let a = body_alpha(overlay_color(Kind::Beat).a());
    let segs = segments(
        &line,
        tl,
        &[body(0.0, 20.0, 100.0, 40.0, Color32::from_rgb(0xc8, 0xd4, 0x5a)), body(0.0, 60.0, 100.0, 80.0, Color32::from_rgb(0x20, 0x30, 0x80))],
    );
    let ys: Vec<(f32, f32)> = segs.iter().map(|s| (s.0, s.1)).collect();
    assert_eq!(ys, vec![(0.0, 20.0), (20.0, 40.0), (40.0, 60.0), (60.0, 80.0), (80.0, 100.0)], "split at every body edge");
    assert_eq!(segs[1].2, Color32::from_black_alpha(a), "bright body: black");
    assert_eq!(segs[3].2, Color32::from_white_alpha(a), "dark body: white");
    assert_eq!(segs[0].2, overlay_color(Kind::Beat), "lane: the derived overlay color");
    assert_ne!(segs[0].2, segs[1].2, "two segments, two stroke colors");
    // A line outside every body is one segment, and a body edge off the line does not split it.
    assert_eq!(segments(&Line { x: 150.0, ..line }, tl, &[body(0.0, 20.0, 100.0, 40.0, Color32::WHITE)]).len(), 1);
}

#[test]
fn no_body_stroke_exceeds_the_alpha_cap_in_either_theme() {
    let cap = (OVERLAY_BODY_ALPHA_MAX * 255.0).round() as u8;
    both_themes(|t| {
        for k in KINDS {
            let lane = translucent(tok(t, k), t.lane, 1.0).a();
            assert!(body_alpha(lane) <= cap, "{k:?}: {} over the cap {cap}", body_alpha(lane));
        }
    });
    // The light theme's bar (alpha 49 at gain 1, 147 at 3.0) is what the cap exists for.
    assert_eq!(body_alpha(49), cap);
    assert_eq!(body_alpha(29), 87, "the dark bar is under the cap and keeps the full gain");
}

#[test]
fn a_clips_strip_and_body_are_stroked_by_their_own_colors() {
    let tl = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0));
    let line = Line { x: 50.0, kind: Kind::Bar { labeled: true }, bar: 1 };
    // The demo blue: a bright body (0x5a9ad8) under a darker strip (0x41709d reads dark).
    let bodies =
        [body(0.0, 20.0, 100.0, 34.0, Color32::from_rgb(0x41, 0x70, 0x9d)), body(0.0, 34.0, 100.0, 60.0, Color32::from_rgb(0x5a, 0x9a, 0xd8))];
    let segs = segments(&line, tl, &bodies);
    assert_eq!(segs.len(), 4, "lane, strip, body, lane: {segs:?}");
    let a = body_alpha(overlay_color(line.kind).a());
    assert_eq!(segs[1].2, Color32::from_white_alpha(a), "the dark strip is lightened");
    assert_eq!(segs[2].2, Color32::from_black_alpha(a), "the bright body is darkened");
    // The same direction merges into one segment.
    let same = [body(0.0, 20.0, 100.0, 34.0, Color32::BLACK), body(0.0, 34.0, 100.0, 60.0, Color32::BLACK)];
    assert_eq!(segments(&line, tl, &same).len(), 3, "strip and body merge");
}

/// [`segments_into`] on a sorted copy of `bodies`.
fn segments(l: &Line, tl: Rect, bodies: &[Body]) -> Vec<(f32, f32, Color32)> {
    let mut sorted = bodies.to_vec();
    sorted.sort_by(|a, b| a.rect.min.y.total_cmp(&b.rect.min.y));
    let mut out = Vec::new();
    segments_into(l, tl, &sorted, &mut out);
    out
}
