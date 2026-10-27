use crate::tests::*;

// ---- #63: fade and crossfade handles ----

#[test]
fn audio_clip_corners_are_fade_handles_clear_of_trim_loop_and_move() {
    use crate::arrange::{DragKind, zone};
    use egui::pos2;
    // A 200-point wide audio clip at x 100..300, y 50..110; the header strip is its top 14 points.
    let r = [100.0, 50.0, 300.0, 110.0];
    let (lo, hi) = (50.0 + 14.0 + 4.0, 50.0 + 14.0 + 13.0);
    assert_eq!(zone(r, pos2(112.0, lo), true), DragKind::FadeIn, "top left corner of the body");
    assert_eq!(zone(r, pos2(288.0, lo), true), DragKind::FadeOut, "top right corner of the body");
    assert_eq!(zone(r, pos2(112.0, hi), true), DragKind::FadeIn);
    assert_eq!(zone(r, pos2(112.0, 50.0 + 14.0 + 15.0), true), DragKind::Move, "below the fade zone");
    assert_eq!(zone(r, pos2(103.0, lo), true), DragKind::TrimStart, "the trim edge wins");
    assert_eq!(zone(r, pos2(297.0, lo), true), DragKind::TrimEnd, "the trim edge wins");
    assert_eq!(zone(r, pos2(290.0, 55.0), true), DragKind::Loop, "the loop zone is in the header strip");
    assert_eq!(zone(r, pos2(110.0, 55.0), true), DragKind::Move, "the header strip has no fade handle");
    assert_eq!(zone(r, pos2(150.0, lo), true), DragKind::Move, "the rest of the body moves");
    assert_eq!(zone(r, pos2(250.0, lo), true), DragKind::Move);
    assert_eq!(zone(r, pos2(112.0, lo), false), DragKind::Move, "only audio clips fade");
    // A narrow clip keeps a move area between the zones.
    let n = [0.0, 0.0, 30.0, 60.0];
    assert_eq!(zone(n, pos2(15.0, 20.0), true), DragKind::Move);
    assert_eq!(zone(n, pos2(8.0, 20.0), true), DragKind::FadeIn);
    assert_eq!(zone(n, pos2(22.0, 20.0), true), DragKind::FadeOut);
    // Too narrow for any: the edges trim, the middle moves.
    let tiny = [0.0, 0.0, 12.0, 60.0];
    assert_eq!(zone(tiny, pos2(6.0, 20.0), true), DragKind::Move);
}

#[test]
fn the_fade_curve_is_the_gain_function_of_its_shape() {
    use crate::fades::{Side, curve_points};
    use transit_model::{Fade, FadeShape};
    for shape in FadeShape::ALL {
        for curvature in [0.0, 0.6, -0.8] {
            let f = Fade { len: 1000, shape, curvature };
            for side in [Side::In, Side::Out] {
                let pts = curve_points(f, side, 10.0, 110.0, 20.0, 80.0, 16);
                assert_eq!(pts.len(), 17);
                for (i, p) in pts.iter().enumerate() {
                    let along = i as f32 / 16.0;
                    // Fade-ins rise from the clip start; fade-outs fall to the clip end.
                    let t = if side == Side::In { along } else { 1.0 - along };
                    let gain = (80.0 - p.y) / 60.0;
                    assert!((gain - f.gain(t)).abs() < 1e-5, "{shape:?} {curvature} {side:?} at {i}: {gain} vs {}", f.gain(t));
                    assert!((p.x - (10.0 + 100.0 * along)).abs() < 1e-4);
                }
            }
        }
    }
}

#[test]
fn fade_gestures_map_to_commands() {
    use crate::fades::{Plan, Side, curvature_after, fade_len, plan, region_at};
    use egui::pos2;
    // Length: from the clip edge to the pointer, negative when outward.
    assert_eq!(fade_len(Side::In, 1000, 5000, 1800), 800);
    assert_eq!(fade_len(Side::Out, 1000, 5000, 4200), 800);
    assert_eq!(fade_len(Side::In, 1000, 5000, 900), -100);
    assert_eq!(fade_len(Side::Out, 1000, 5000, 5100), -100);
    // Up bends up, down bends down, clamped.
    assert!(curvature_after(0.0, -30.0) > 0.0 && curvature_after(0.0, 30.0) < 0.0);
    assert_eq!(curvature_after(0.9, -500.0), 1.0);
    assert_eq!(curvature_after(-0.9, 500.0), -1.0);
    // The fade region is the body strip under the header, as wide as the fade.
    let r = [100.0, 50.0, 300.0, 110.0];
    assert_eq!(region_at(r, 40.0, 0.0, pos2(120.0, 90.0)), Some(Side::In));
    assert_eq!(region_at(r, 40.0, 0.0, pos2(150.0, 90.0)), None);
    assert_eq!(region_at(r, 0.0, 40.0, pos2(290.0, 90.0)), Some(Side::Out));
    assert_eq!(region_at(r, 40.0, 40.0, pos2(120.0, 55.0)), None, "not in the header strip");
    // Plans on a real session.
    let r = fade_rig();
    let s = r.app.engine.session();
    assert_eq!(plan(s, r.clip, Side::In, 0, false), Plan::None);
    assert_eq!(plan(s, r.clip, Side::Out, -50, false), Plan::None, "outward with no neighbor");
    match plan(s, r.clip, Side::In, 3 * r.bar, false) {
        Plan::Set(v) => assert_eq!(v.len(), 1),
        p => panic!("{p:?}"),
    }
    match plan(s, r.clip, Side::In, 100 * r.bar, false) {
        Plan::Set(v) => assert_eq!(v[0].2.len, 4 * r.bar, "no longer than the clip"),
        p => panic!("{p:?}"),
    }
}

struct FadeRig {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    t: TrackId,
    clip: ClipId,
    bar: i64,
    beat: i64,
    px_bar: f32,
}

/// The demo's Hats audio clip, cut to four bars and with its fades cleared, in view.
fn fade_rig() -> FadeRig {
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    let (t, clip) = {
        let s = app.engine.session();
        let t = s.track_by_name("Hats").unwrap();
        (t.id, t.clips()[0].id)
    };
    let bar = {
        let s = app.engine.session();
        let sr = s.sample_rate;
        s.tempo.samples_at_bar_beat(transit_time::BarBeat { bar: 2, beat: 1, tick: 0 }, sr)
            - s.tempo.samples_at_bar_beat(transit_time::BarBeat { bar: 1, beat: 1, tick: 0 }, sr)
    };
    {
        let c = app.engine.session_mut().find_clip_mut(clip).unwrap();
        c.length = 4 * bar;
        c.fade_in = Default::default();
        c.fade_out = Default::default();
    }
    let ctx = egui::Context::default();
    let mut time = 0.0;
    app.ui.show_mixer = false;
    app.ui.show_devices = false;
    frames(&mut app, &ctx, 4, &mut time);
    let px_bar = (bar as f64 / app.arrange.layout.samples_per_px) as f32;
    FadeRig { app, ctx, time, t, clip, bar, beat: bar / 4, px_bar }
}

impl FadeRig {
    fn rect(&self, c: ClipId) -> [f32; 4] {
        self.app.arrange.layout.clips.iter().find(|x| x.id == c.0).unwrap().rect
    }
    /// A point in the body's top left (`left`) or right corner, in the fade handle zone.
    fn handle(&self, c: ClipId, left: bool) -> (f32, f32) {
        let r = self.rect(c);
        (if left { r[0] + 12.0 } else { r[2] - 12.0 }, r[1] + crate::theme::CLIP_NAME_H + 6.0)
    }
    fn go(&mut self, from: (f32, f32), to: (f32, f32), mods: Value) {
        drag(&mut self.app, from, to, mods);
        frames(&mut self.app, &self.ctx, 20, &mut self.time);
    }
    fn click(&mut self, at: (f32, f32), count: u64) {
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "count": count}));
        let _ = crate::control::handle_for_test(&mut self.app, &egui::Context::default(), &req);
        frames(&mut self.app, &self.ctx, 20, &mut self.time);
    }
    fn right_click(&mut self, at: (f32, f32)) {
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
        let _ = crate::control::handle_for_test(&mut self.app, &egui::Context::default(), &req);
        frames(&mut self.app, &self.ctx, 20, &mut self.time);
    }
    fn clip(&self, c: ClipId) -> transit_model::Clip {
        the_clip(&self.app, c)
    }
    fn item(&self, label: &str) -> (f32, f32) {
        let m = self.app.arrange.menu.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("no menu item {label}"));
        ((m.rect[0] + m.rect[2]) / 2.0, (m.rect[1] + m.rect[3]) / 2.0)
    }
    /// A second clip on the track: a copy of the first starting at `start`.
    fn add_copy(&mut self, start: i64) -> ClipId {
        let s = self.app.engine.session_mut();
        let mut c = s.find_clip(self.clip).unwrap().1.clone();
        c.id = s.new_clip_id();
        c.start = start;
        c.fade_in = Default::default();
        c.fade_out = Default::default();
        let id = c.id;
        let pl = s.track_mut(self.t).unwrap().playlist_mut().unwrap();
        pl.clips.push(c);
        pl.sort();
        frames(&mut self.app, &self.ctx, 4, &mut self.time);
        id
    }
}

#[test]
fn dragging_the_fade_in_handle_sets_the_length_snapped_in_one_undo_step() {
    let mut r = fade_rig();
    let h = r.handle(r.clip, true);
    let r0 = r.rect(r.clip);
    let before = undo_len(&mut r.app);
    r.go(h, (r0[0] + r.px_bar, h.1), json!({}));
    let c = r.clip(r.clip);
    assert_eq!(c.fade_in.len, r.bar, "one bar in");
    assert_eq!(c.fade_in.shape, transit_model::FadeShape::EqualPower);
    assert_eq!(c.fade_out.len, 0);
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.clip(r.clip).fade_in.len, 0);
    // Snap: a drop between beats lands on one; Shift leaves it at the pointer.
    r.go(h, (r0[0] + 0.37 * r.px_bar, h.1), json!({}));
    assert_eq!(r.clip(r.clip).fade_in.len % r.beat, 0, "snapped to the grid");
    r.app.run("edit.undo", json!({})).unwrap();
    r.go(h, (r0[0] + 0.37 * r.px_bar, h.1), json!({"shift": true}));
    let len = r.clip(r.clip).fade_in.len;
    assert!((len - (0.37 * r.bar as f64) as i64).abs() < r.beat / 8, "Shift: at the pointer, {len}");
    assert_ne!(len % r.beat, 0);
}

#[test]
fn dragging_the_fade_out_handle_sets_the_length_from_the_clip_end() {
    let mut r = fade_rig();
    let h = r.handle(r.clip, false);
    let r0 = r.rect(r.clip);
    let before = undo_len(&mut r.app);
    r.go(h, (r0[2] - r.px_bar, h.1), json!({}));
    let c = r.clip(r.clip);
    assert!((c.fade_out.len - r.bar).abs() <= 2, "{}", c.fade_out.len);
    assert_eq!(c.fade_in.len, 0);
    assert_eq!(undo_len(&mut r.app), before + 1);
    assert!(r.app.arrange.gesture.is_none(), "the gesture ended");
}

#[test]
fn a_fade_drag_that_changes_nothing_adds_no_undo_step() {
    let mut r = fade_rig();
    let h = r.handle(r.clip, true);
    let before = undo_len(&mut r.app);
    // Left of the clip start on a clip with no neighbor: nothing to fade or cross into.
    r.go(h, (h.0 - 30.0, h.1), json!({}));
    assert_eq!(r.clip(r.clip).fade_in.len, 0);
    assert_eq!(undo_len(&mut r.app), before);
}

#[test]
fn alt_drag_on_a_fade_bends_its_curvature_and_a_click_opens_the_shape_menu() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "linear"})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let r0 = r.rect(r.clip);
    let inside = (r0[0] + r.px_bar * 0.5, r0[1] + 40.0);
    let before = undo_len(&mut r.app);
    r.go(inside, (inside.0, inside.1 - 40.0), json!({"alt": true}));
    let c = r.clip(r.clip);
    assert!(c.fade_in.curvature > 0.2, "dragged up: {}", c.fade_in.curvature);
    assert_eq!(c.fade_in.len, r.bar, "the length is untouched");
    assert_eq!(c.fade_in.shape, transit_model::FadeShape::Linear);
    assert_eq!(c.start, 0, "the clip did not move");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    // A click on the fade opens the five shapes; picking S-Curve switches it.
    r.click(inside, 1);
    for shape in transit_model::FadeShape::ALL {
        assert!(r.app.arrange.menu.iter().any(|m| m.label == shape.label()), "{shape:?} in the menu");
    }
    let pick = r.item("S-Curve");
    r.click(pick, 1);
    let c = r.clip(r.clip);
    assert_eq!(c.fade_in.shape, transit_model::FadeShape::SCurve);
    assert_eq!(c.fade_in.len, r.bar);
    assert!(c.fade_in.curvature > 0.2, "the bend stays");
    assert!(r.app.arrange.fade_menu.is_none(), "the menu closed");
}

#[test]
fn right_click_on_a_fade_offers_the_shapes_and_double_click_resets_it() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "logarithmic", "curvature": 0.5})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let r0 = r.rect(r.clip);
    let inside = (r0[0] + r.px_bar * 0.5, r0[1] + 40.0);
    r.right_click(inside);
    let pick = r.item("Exponential");
    r.click(pick, 1);
    assert_eq!(r.clip(r.clip).fade_in.shape, transit_model::FadeShape::Exponential);
    // Long enough after the last click that egui does not count a triple click.
    frames(&mut r.app, &r.ctx, 60, &mut r.time);
    let len0 = r.clip(r.clip).fade_in.len;
    let before = undo_len(&mut r.app);
    r.click(inside, 2);
    let c = r.clip(r.clip);
    assert_eq!((c.fade_in.shape, c.fade_in.curvature, c.fade_in.len), (transit_model::FadeShape::EqualPower, 0.0, len0), "reset keeps the length");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
}

#[test]
fn the_crossfade_menu_entry_fades_both_clips_over_their_overlap() {
    let mut r = fade_rig();
    // A: 0..4 bars; B starts at 3 bars + 3 beats: one beat of overlap.
    let b = r.add_copy(3 * r.bar + 3 * r.beat);
    let overlap = r.beat;
    let rb = r.rect(b);
    let at = (rb[0] + 3.0, rb[1] + 8.0);
    let before = undo_len(&mut r.app);
    r.right_click(at);
    let pick = r.item("Crossfade");
    r.click(pick, 1);
    assert!((r.clip(r.clip).fade_out.len - overlap).abs() <= 2, "{}", r.clip(r.clip).fade_out.len);
    assert_eq!(r.clip(b).fade_in.len, r.clip(r.clip).fade_out.len);
    assert_eq!(r.clip(b).fade_in.shape, transit_model::FadeShape::EqualPower);
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    // No overlap, no entry.
    assert!(crate::fades::overlap(r.app.engine.session(), b).is_some());
    r.app.engine.session_mut().find_clip_mut(b).unwrap().start = 8 * r.bar;
    assert!(crate::fades::overlap(r.app.engine.session(), b).is_none());
}

#[test]
fn dragging_a_crossfade_edge_adjusts_both_sides_and_alt_one() {
    let mut r = fade_rig();
    let b = r.add_copy(3 * r.bar);
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "out", "length": r.bar})).unwrap();
    r.app.run("edit.fades_create", json!({"clip": b.0, "side": "in", "length": r.bar})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let rb = r.rect(b);
    let h = r.handle(b, true);
    let before = undo_len(&mut r.app);
    // B's fade-in handle half a bar in: both sides of the crossfade follow.
    r.go(h, (rb[0] + r.px_bar / 2.0, h.1), json!({}));
    assert!((r.clip(b).fade_in.len - r.bar / 2).abs() <= 2);
    assert_eq!(r.clip(r.clip).fade_out.len, r.clip(b).fade_in.len, "the other side moved too");
    assert_eq!(undo_len(&mut r.app), before + 1);
    // The plan: Alt keeps to one side.
    let s = r.app.engine.session();
    match crate::fades::plan(s, b, crate::fades::Side::In, r.bar / 4, true) {
        crate::fades::Plan::Set(v) => assert_eq!(v.len(), 1, "Alt: one side"),
        p => panic!("{p:?}"),
    }
    match crate::fades::plan(s, b, crate::fades::Side::In, r.bar / 4, false) {
        crate::fades::Plan::Set(v) => {
            assert_eq!(v.len(), 2);
            assert_eq!((v[1].0, v[1].1), (r.clip, crate::fades::Side::Out));
        }
        p => panic!("{p:?}"),
    }
}

#[test]
fn dragging_a_fade_past_the_boundary_into_an_abutting_neighbor_makes_a_crossfade() {
    let mut r = fade_rig();
    let b = r.add_copy(4 * r.bar);
    let ra = r.rect(r.clip);
    let h = r.handle(r.clip, false);
    let before = undo_len(&mut r.app);
    // A's fade-out handle a quarter bar past A's end, into B.
    r.go(h, (ra[2] + r.px_bar / 4.0, h.1), json!({}));
    assert!(r.clip(r.clip).fade_out.len > 0, "A fades out");
    assert!(r.clip(b).fade_in.len > 0, "B fades in");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
}

#[test]
fn dragging_a_fade_to_nothing_removes_it_and_keeps_the_other_side() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar})).unwrap();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "out", "length": r.bar})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let r0 = r.rect(r.clip);
    let h = r.handle(r.clip, true);
    let before = undo_len(&mut r.app);
    // The handle sits inside the fade: drag it back to the clip start.
    r.go(h, (r0[0] - 4.0, h.1), json!({}));
    let c = r.clip(r.clip);
    assert_eq!(c.fade_in.len, 0, "the fade-in is gone");
    assert_eq!(c.fade_out.len, r.bar, "the fade-out stays");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.clip(r.clip).fade_in.len, r.bar);
}
