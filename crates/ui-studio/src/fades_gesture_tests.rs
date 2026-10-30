use crate::tests::*;

// ---- #63: fade and crossfade handles ----

#[test]
fn hit_zone_table_of_a_40_point_and_a_200_point_clip() {
    use crate::arrange::{DragKind, zone};
    use DragKind::{FadeIn as FI, FadeOut as FO, Loop, Move, TrimEnd as TE, TrimStart as TS};
    use egui::pos2;
    // The clip body starts under the 14 point header strip (y 64 for a clip at y 50), 46 points
    // tall. Trim edges are 6 points (a quarter of a narrow clip at most); the fade corner is a
    // triangle as tall as the body and 36 points wide, or half the room between the edges.
    let wide = [100.0, 50.0, 300.0, 110.0];
    let narrow = [100.0, 50.0, 140.0, 110.0];
    let cases: &[(&str, [f32; 4], (f32, f32), DragKind)] = &[
        // Corner points of the 200 point clip.
        ("wide: top left of the clip", wide, (100.0, 50.0), TS),
        ("wide: bottom left of the clip", wide, (100.0, 110.0), TS),
        ("wide: top right of the clip", wide, (300.0, 50.0), TE),
        ("wide: bottom right of the clip", wide, (300.0, 110.0), TE),
        ("wide: top left of the body, inside the trim edge", wide, (107.0, 65.0), FI),
        ("wide: top right of the body, inside the trim edge", wide, (293.0, 65.0), FO),
        ("wide: the corner triangle's far end on the top edge", wide, (141.0, 65.0), FI),
        ("wide: past the corner triangle on the top edge", wide, (143.0, 65.0), Move),
        ("wide: the corner triangle's low tip on the inner edge", wide, (107.0, 105.0), FI),
        ("wide: below the hypotenuse is the body", wide, (125.0, 100.0), Move),
        ("wide: bottom left of the body, outside the triangle", wide, (115.0, 108.0), Move),
        ("wide: the middle of the body", wide, (200.0, 85.0), Move),
        ("wide: the header strip is not a fade", wide, (112.0, 57.0), Move),
        ("wide: the loop corner is in the header strip", wide, (290.0, 57.0), Loop),
        ("wide: trim wins over the corner", wide, (103.0, 70.0), TS),
        ("wide: the right triangle mirrors the left", wide, (259.0, 65.0), FO),
        ("wide: ... and past it", wide, (257.0, 65.0), Move),
        // Corner points of the 40 point clip: the triangles are 14 wide and meet at the top.
        ("narrow: top left of the clip", narrow, (100.0, 50.0), TS),
        ("narrow: bottom right of the clip", narrow, (140.0, 110.0), TE),
        ("narrow: top left of the body", narrow, (107.0, 65.0), FI),
        ("narrow: top right of the body", narrow, (133.0, 65.0), FO),
        ("narrow: just left of the middle on the top edge", narrow, (119.0, 64.5), FI),
        ("narrow: just right of the middle on the top edge", narrow, (121.0, 64.5), FO),
        ("narrow: the middle is a move zone below the triangles", narrow, (120.0, 90.0), Move),
        ("narrow: the loop corner survives", narrow, (130.0, 57.0), Loop),
        ("narrow: the header strip has no fade", narrow, (110.0, 57.0), Move),
    ];
    for (what, rect, at, want) in cases {
        assert_eq!(zone(*rect, pos2(at.0, at.1), true), *want, "{what} at {at:?}");
    }
    // Only audio clips fade.
    assert_eq!(zone(wide, pos2(107.0, 65.0), false), Move, "a MIDI clip has no fade corner");
    // A clip too narrow for a fade target (under twice the floor plus the edges) keeps trim and move.
    let tiny = [0.0, 0.0, 12.0, 60.0];
    assert_eq!(zone(tiny, pos2(6.0, 20.0), true), Move);
    assert_eq!(zone(tiny, pos2(2.0, 20.0), true), TS);
    // Narrow clips lose the middle move zone before the fade zones: shrinking a clip never
    // shrinks a fade corner below half of what lies between its trim edges.
    for w in [24.0_f32, 30.0, 40.0, 60.0] {
        let r = [0.0, 0.0, w, 60.0];
        let edge = 6.0_f32.min(w / 4.0);
        assert_eq!(zone(r, pos2(edge + 1.0, 15.0), true), FI, "{w} wide keeps a left fade corner");
        assert_eq!(zone(r, pos2(w - edge - 1.0, 15.0), true), FO, "{w} wide keeps a right fade corner");
    }
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
    /// The `i`th of 25 points of the painted curve of a clip's fade on `side` ("in" or "out").
    fn curve_point(&self, c: ClipId, side: &str, i: usize) -> (f32, f32) {
        let p = self.app.arrange.layout.fades.iter().find(|p| p.clip == c.0 && p.kind == side).unwrap_or_else(|| panic!("no {side} fade painted"));
        (p.curve[i][0], p.curve[i][1])
    }
    /// The painted fades of `kind` on the rig's clip (the demo has other fades on other tracks).
    fn paints(&self, kind: &str) -> Vec<crate::fades::Paint> {
        self.app.arrange.layout.fades.iter().filter(|p| p.kind == kind && (p.clip == self.clip.0 || kind == "cross")).cloned().collect()
    }
    fn hover(&mut self, at: (f32, f32)) {
        let (req, _rx) = crate::ControlRequest::new("ui.move", json!({"x": at.0, "y": at.1}));
        let _ = crate::control::handle_for_test(&mut self.app, &egui::Context::default(), &req);
        frames(&mut self.app, &self.ctx, 4, &mut self.time);
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
fn dragging_the_curve_line_bends_it_and_a_click_on_the_fade_opens_the_shape_menu() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "linear"})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let r0 = r.rect(r.clip);
    let on_curve = r.curve_point(r.clip, "in", 12);
    let before = undo_len(&mut r.app);
    // No modifier: up bends the fade up.
    r.go(on_curve, (on_curve.0, on_curve.1 - 40.0), json!({}));
    let c = r.clip(r.clip);
    assert!(c.fade_in.curvature > 0.2, "dragged up: {}", c.fade_in.curvature);
    assert_eq!(c.fade_in.len, r.bar, "the length is untouched");
    assert_eq!(c.fade_in.shape, transit_model::FadeShape::Linear);
    assert_eq!(c.start, 0, "the clip did not move");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    // A click on the fade's area, off the curve, opens the five shapes; picking S-Curve switches it.
    let inside = (r0[0] + r.px_bar * 0.5, r0[1] + crate::theme::CLIP_NAME_H + 6.0);
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

// ---- #182: Logic's look and feel ----

#[test]
fn a_fade_is_painted_as_a_dimmed_area_with_its_curve_and_the_hover_shows_the_handle_dot() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "linear"})).unwrap();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "out", "length": r.bar / 2})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let r0 = r.rect(r.clip);
    let fi = r.paints("in");
    assert_eq!(fi.len(), 1);
    let (a, curve) = (fi[0].area, &fi[0].curve);
    assert!((a[0] - r0[0]).abs() < 0.01 && (a[2] - a[0] - r.px_bar).abs() < 1.0, "the area is the fade's extent: {a:?}");
    assert!(fi[0].dim > 0.0 && fi[0].dim < 1.0, "the waveform is dimmed, not hidden: {}", fi[0].dim);
    assert_eq!(curve.len(), 25);
    assert!((curve[0][1] - a[3]).abs() < 0.01, "a fade-in starts at silence (the bottom)");
    assert!((curve[24][1] - a[1]).abs() < 0.01, "and ends at unity (the top)");
    let fo = r.paints("out");
    assert!((fo[0].curve[0][1] - fo[0].area[1]).abs() < 0.01, "a fade-out starts at unity");
    assert!((fo[0].area[2] - r0[2]).abs() < 0.01, "and runs to the clip end");
    assert!(fi[0].dot.is_none() && fo[0].dot.is_none(), "no dot without the pointer");
    // Over the fade-in's area: the dot sits at the fade end on the clip top edge.
    r.hover((r0[0] + r.px_bar * 0.5, r0[1] + crate::theme::CLIP_NAME_H + 6.0));
    let fi = r.paints("in");
    let dot = fi[0].dot.expect("a dot on the hovered fade");
    assert!((dot[0] - (r0[0] + r.px_bar)).abs() < 1.0 && (dot[1] - fi[0].area[1]).abs() < 0.01, "{dot:?}");
    assert!(r.paints("out")[0].dot.is_none(), "only the hovered fade");
    // Over the empty corner of a clip with no fade there: a dot at the clip corner.
    r.app.run("edit.fades_delete", json!({"tracks": [r.t.0], "start": 0, "end": 5 * r.bar})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    r.hover((r0[0] + 12.0, r0[1] + crate::theme::CLIP_NAME_H + 6.0));
    let h = r.paints("handle");
    assert_eq!(h.len(), 1);
    assert!((h[0].dot.unwrap()[0] - r0[0]).abs() < 0.01);
    // Away from the clip: no dot.
    r.hover((r0[0] + 3.0 * r.px_bar, r0[3] + 200.0));
    assert!(r.paints("handle").is_empty() && r.paints("in").is_empty());
}

#[test]
fn bending_by_the_curve_line_is_monotonic_within_range_and_alt_halves_the_step() {
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "linear"})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let on = r.curve_point(r.clip, "in", 12);
    let mut last = 0.0f32;
    let mut seen = Vec::new();
    for up in [8.0f32, 12.0, 20.0, 40.0, 80.0, 400.0] {
        r.go(on, (on.0, on.1 - up), json!({}));
        let k = r.clip(r.clip).fade_in.curvature;
        assert!((-1.0..=1.0).contains(&k), "within the documented -1..1: {k}");
        assert!(k >= last, "monotonic: {k} after {last} at {up} up");
        last = k;
        seen.push(k);
        r.app.run("edit.undo", json!({})).unwrap();
        frames(&mut r.app, &r.ctx, 4, &mut r.time);
    }
    assert_eq!(seen[5], 1.0, "clamped at the top of the range");
    assert!(seen[0] > 0.0, "even a small drag bends");
    // Down bends the other way, and the range is symmetric.
    r.go(on, (on.0, on.1 + 400.0), json!({}));
    assert_eq!(r.clip(r.clip).fade_in.curvature, -1.0);
    r.app.run("edit.undo", json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    // Alt is fine adjust: half the step for the same travel.
    r.go(on, (on.0, on.1 - 40.0), json!({}));
    let full = r.clip(r.clip).fade_in.curvature;
    r.app.run("edit.undo", json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    r.go(on, (on.0, on.1 - 40.0), json!({"alt": true}));
    let fine = r.clip(r.clip).fade_in.curvature;
    assert!((fine - full / 2.0).abs() < 0.02, "Alt halves the step: {fine} vs {full}");
    assert_eq!(r.clip(r.clip).start, 0, "Alt on a fade does not move the clip");
    assert_eq!(r.app.engine.session().track(r.t).unwrap().clips().len(), 1, "and does not copy it");
}

#[test]
fn dragging_a_fade_handle_past_the_clip_edge_into_an_overlapping_neighbor_makes_the_menus_crossfade() {
    // B starts a bar before A ends. B's fade-in handle pulled back past B's start crosses into A.
    let make = || {
        let mut r = fade_rig();
        let b = r.add_copy(3 * r.bar);
        (r, b)
    };
    let (mut r, b) = make();
    let rb = r.rect(b);
    let h = r.handle(b, true);
    let before = undo_len(&mut r.app);
    r.go(h, (rb[0] - 10.0, h.1), json!({}));
    let (a_out, b_in) = (r.clip(r.clip).fade_out, r.clip(b).fade_in);
    assert_eq!(a_out.len, r.bar, "the whole overlap");
    assert_eq!(b_in.len, r.bar);
    assert_eq!((a_out.shape, b_in.shape), (transit_model::FadeShape::EqualPower, transit_model::FadeShape::EqualPower));
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    // The same state as the context menu's Crossfade.
    let (mut m, mb) = make();
    let rmb = m.rect(mb);
    m.right_click((rmb[0] + 3.0, rmb[1] + 8.0));
    let pick = m.item("Crossfade");
    m.click(pick, 1);
    assert_eq!((m.clip(m.clip).fade_out, m.clip(mb).fade_in), (a_out, b_in), "drag and menu agree");
    // One undo takes it back.
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!((r.clip(r.clip).fade_out.len, r.clip(b).fade_in.len), (0, 0));
    // The crossfade is drawn as an X over the overlap.
    r.app.run("edit.redo", json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let x = r.paints("cross");
    assert_eq!(x.len(), 1);
    let ra = r.rect(r.clip);
    assert!((x[0].area[0] - rb[0]).abs() < 1.0 && (x[0].area[2] - ra[2]).abs() < 1.0, "over the overlap: {:?}", x[0].area);
    assert!(x[0].curve[0][1] < x[0].curve[24][1], "the outgoing curve falls");
    assert!(x[0].curve2[0][1] > x[0].curve2[24][1], "the incoming curve rises");
}

#[test]
fn dragging_a_fade_out_handle_past_the_end_into_an_abutting_neighbor_is_one_undo_step() {
    let mut r = fade_rig();
    let b = r.add_copy(4 * r.bar);
    let ra = r.rect(r.clip);
    let h = r.handle(r.clip, false);
    let before = undo_len(&mut r.app);
    r.go(h, (ra[2] + r.px_bar / 2.0, h.1), json!({}));
    assert!(r.clip(r.clip).fade_out.len > 0 && r.clip(b).fade_in.len > 0, "both clips fade");
    assert_eq!(undo_len(&mut r.app), before + 1);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!((r.clip(r.clip).fade_out.len, r.clip(b).fade_in.len), (0, 0));
}

#[test]
fn the_two_ends_of_a_crossfade_drag_independently_once_their_lengths_differ() {
    let mut r = fade_rig();
    let b = r.add_copy(3 * r.bar);
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "out", "length": r.bar / 2})).unwrap();
    r.app.run("edit.fades_create", json!({"clip": b.0, "side": "in", "length": r.bar})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let (ra, rb) = (r.rect(r.clip), r.rect(b));
    let top = rb[1] + crate::theme::CLIP_NAME_H + 2.5;
    let tol = r.bar / 30;
    let near = move |got: i64, want: i64| (got - want).abs() < tol;
    // B's end of the X: its fade-in handle, a bar in, dragged to a quarter (Shift: no snap).
    r.go((rb[0] + 24.0, top), (rb[0] + r.px_bar / 4.0, top), json!({"shift": true}));
    assert!(near(r.clip(b).fade_in.len, r.bar / 4), "{}", r.clip(b).fade_in.len);
    assert_eq!(r.clip(r.clip).fade_out.len, r.bar / 2, "A's end stayed");
    // A's end: its fade-out area reaches under B; press where B has no fade, near the top.
    r.go((rb[0] + 48.0, top), (ra[2] - 20.0, top), json!({"shift": true}));
    assert!(near(r.clip(r.clip).fade_out.len, (20.0 * r.bar as f64 / f64::from(r.px_bar)) as i64), "{}", r.clip(r.clip).fade_out.len);
    assert!(near(r.clip(b).fade_in.len, r.bar / 4), "B's end stayed: {}", r.clip(b).fade_in.len);
    assert_eq!(r.clip(b).start, 3 * r.bar, "nothing moved");
}

// ---- #182: the clip inspector's fade section ----

impl FadeRig {
    fn select(&mut self, c: ClipId) {
        self.app.click_clip(c, false);
        frames(&mut self.app, &self.ctx, 4, &mut self.time);
    }
    fn field(&self, f: crate::clip_inspector::Field) -> crate::clip_inspector::FieldView {
        self.app.inspector.fields.iter().find(|v| v.field == f).cloned().unwrap_or_else(|| panic!("no {f:?} field shown"))
    }
    fn center(rect: [f32; 4]) -> (f32, f32) {
        ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0)
    }
    /// Type `text` into a field the way a user does: click it, select all, type, Return.
    fn type_into(&mut self, f: crate::clip_inspector::Field, text: &str) {
        let at = Self::center(self.field(f).rect);
        self.click(at, 1);
        for (req, args) in [("ui.key", json!({"key": "A", "cmd": true})), ("ui.text", json!({"text": text})), ("ui.key", json!({"key": "Enter"}))] {
            let (req, _rx) = crate::ControlRequest::new(req, args);
            let _ = crate::control::handle_for_test(&mut self.app, &egui::Context::default(), &req);
            frames(&mut self.app, &self.ctx, 6, &mut self.time);
        }
    }
    fn shown(&self, len: i64) -> String {
        let s = self.app.engine.session();
        crate::clip_inspector::format_len(s, len, s.edit.main_counter)
    }
}

#[test]
fn the_inspector_shows_one_selected_audio_clip_and_its_lengths_in_the_session_time_format() {
    use crate::clip_inspector::Field;
    let mut r = fade_rig();
    assert!(r.app.inspector.clip.is_none(), "nothing selected, no inspector");
    r.select(r.clip);
    assert_eq!(r.app.inspector.clip, Some(r.clip));
    assert_eq!(r.field(Field::FadeIn).text, r.shown(0));
    assert_eq!(r.field(Field::FadeOut).text, r.shown(0));
    assert_eq!(r.field(Field::FadeIn).curve, Some(transit_model::FadeShape::EqualPower.label()));
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "out", "length": r.bar + r.beat})).unwrap();
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    assert_eq!(r.field(Field::FadeOut).text, r.shown(r.bar + r.beat));
    // The text follows the session's main counter.
    for fmt in transit_time::TimeFormat::ALL {
        r.app.engine.session_mut().edit.main_counter = fmt;
        frames(&mut r.app, &r.ctx, 4, &mut r.time);
        assert_eq!(r.field(Field::FadeOut).text, r.shown(r.bar + r.beat), "{fmt:?}");
    }
    r.app.engine.session_mut().edit.main_counter = transit_time::TimeFormat::BarsBeats;
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    assert_eq!(r.field(Field::FadeOut).text, "1| 1| 000", "a bar and a beat is one bar, one beat");
    // Two clips selected, or a clip of another kind: no inspector.
    let b = r.add_copy(8 * r.bar);
    r.app.click_clip(b, true);
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    assert!(r.app.inspector.clip.is_none());
}

#[test]
fn lengths_survive_a_format_round_trip_in_every_time_format() {
    let r = fade_rig();
    let s = r.app.engine.session();
    for fmt in transit_time::TimeFormat::ALL {
        for len in [r.beat, r.bar, 3 * r.beat, 2 * r.bar + r.beat] {
            let text = crate::clip_inspector::format_len(s, len, fmt);
            let back = crate::clip_inspector::parse_len(s, &text, fmt).unwrap_or_else(|e| panic!("{fmt:?} {text}: {e}"));
            let slack = match fmt {
                transit_time::TimeFormat::Samples | transit_time::TimeFormat::BarsBeats => 1,
                transit_time::TimeFormat::MinSecs => i64::from(s.sample_rate.hz()) / 1000,
                _ => i64::from(s.sample_rate.hz()) / 20,
            };
            assert!((back - len).abs() <= slack, "{fmt:?} {text}: {back} vs {len}");
        }
    }
    assert!(crate::clip_inspector::parse_len(s, "soon", transit_time::TimeFormat::Samples).is_err());
    assert!(crate::clip_inspector::parse_len(s, "1| 2", transit_time::TimeFormat::BarsBeats).is_err());
}

#[test]
fn typing_a_fade_length_changes_the_clip_and_dragging_the_handle_changes_the_field() {
    use crate::clip_inspector::Field;
    let mut r = fade_rig();
    r.select(r.clip);
    let before = undo_len(&mut r.app);
    // Type a length: the clip shows it.
    let text = r.shown(r.bar);
    r.type_into(Field::FadeIn, &text);
    let c = r.clip(r.clip);
    assert!((c.fade_in.len - r.bar).abs() <= 2, "typed {text}: {}", c.fade_in.len);
    assert_eq!(c.fade_out.len, 0);
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    assert_eq!(r.field(Field::FadeIn).text, r.shown(c.fade_in.len), "the field reads it back");
    assert!(r.paints("in").len() == 1, "and the clip draws it");
    // Drag the handle: the field shows it.
    let r0 = r.rect(r.clip);
    let h = (r0[0] + 12.0, r0[1] + crate::theme::CLIP_NAME_H + 2.5);
    r.go(h, (r0[0] + r.px_bar / 2.0, h.1), json!({}));
    let len = r.clip(r.clip).fade_in.len;
    assert!((len - r.bar / 2).abs() <= r.beat, "dragged to half a bar: {len}");
    assert_eq!(r.field(Field::FadeIn).text, r.shown(len), "the field follows the handle");
    // Fade Out the same way, and zero removes the fade.
    let text = r.shown(r.beat * 2);
    r.type_into(Field::FadeOut, &text);
    assert!((r.clip(r.clip).fade_out.len - 2 * r.beat).abs() <= 2);
    let zero = r.shown(0);
    r.type_into(Field::FadeOut, &zero);
    assert_eq!(r.clip(r.clip).fade_out.len, 0, "zero removes the fade");
    assert_eq!(r.clip(r.clip).fade_in.len, len, "and keeps the other");
    // Text that is not a length changes nothing and says why.
    let n = undo_len(&mut r.app);
    r.type_into(Field::FadeIn, "soon");
    assert_eq!(r.clip(r.clip).fade_in.len, len);
    assert_eq!(undo_len(&mut r.app), n);
    assert!(r.app.status.contains("Fade In") && r.app.status.contains("soon"), "{}", r.app.status);
}

#[test]
fn the_curve_popup_sets_the_shape_and_keeps_length_and_bend() {
    use crate::clip_inspector::Field;
    let mut r = fade_rig();
    r.app.run("edit.fades_create", json!({"clip": r.clip.0, "side": "in", "length": r.bar, "shape": "linear", "curvature": 0.4})).unwrap();
    r.select(r.clip);
    assert_eq!(r.field(Field::FadeIn).curve, Some("Linear"));
    // The popup's choices are the five shapes; picking one goes through the same path as the
    // fade's shape menu.
    crate::fades::set_shape(&mut r.app, r.clip, crate::fades::Side::In, transit_model::FadeShape::SCurve);
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    let c = r.clip(r.clip);
    assert_eq!((c.fade_in.shape, c.fade_in.len), (transit_model::FadeShape::SCurve, r.bar));
    assert!((c.fade_in.curvature - 0.4).abs() < 1e-6);
    assert_eq!(r.field(Field::FadeIn).curve, Some(transit_model::FadeShape::SCurve.label()));
}

#[test]
fn the_crossfade_field_sets_both_ends_and_shows_custom_when_they_differ() {
    use crate::clip_inspector::{CUSTOM, Field};
    let mut r = fade_rig();
    let b = r.add_copy(3 * r.bar);
    r.select(b);
    assert_eq!(r.field(Field::Crossfade).text, r.shown(0), "an overlap with no fades");
    let before = undo_len(&mut r.app);
    let text = r.shown(r.bar / 2);
    r.type_into(Field::Crossfade, &text);
    let (a_out, b_in) = (r.clip(r.clip).fade_out, r.clip(b).fade_in);
    assert!((a_out.len - r.bar / 2).abs() <= 2 && a_out.len == b_in.len, "both ends: {} {}", a_out.len, b_in.len);
    assert_eq!((a_out.shape, b_in.shape), (transit_model::FadeShape::EqualPower, transit_model::FadeShape::EqualPower));
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    assert_eq!(r.field(Field::Crossfade).text, r.shown(a_out.len));
    // Typing one end's length leaves the other: the crossfade reads Custom.
    let text = r.shown(r.beat);
    r.type_into(Field::FadeIn, &text);
    assert!((r.clip(b).fade_in.len - r.beat).abs() <= 2);
    assert_eq!(r.clip(r.clip).fade_out.len, a_out.len, "the other end stayed");
    assert_eq!(r.field(Field::Crossfade).text, CUSTOM);
    // A clip that overlaps nothing has no crossfade to edit.
    r.app.engine.session_mut().find_clip_mut(b).unwrap().start = 8 * r.bar;
    frames(&mut r.app, &r.ctx, 4, &mut r.time);
    assert_eq!(r.field(Field::Crossfade).text, "-");
}

#[test]
fn the_fade_to_cursor_keys_fade_the_selected_clips_to_the_cursor_in_one_undo_step() {
    let mut r = fade_rig();
    let b = r.add_copy(8 * r.bar);
    r.app.click_clip(r.clip, false);
    r.app.click_clip(b, true);
    r.app.set_cursor(r.bar, Some(r.t));
    let before = undo_len(&mut r.app);
    let key = |r: &mut FadeRig, k: &str| {
        let (req, _rx) = crate::ControlRequest::new("ui.key", json!({"key": k, "alt": true}));
        let _ = crate::control::handle_for_test(&mut r.app, &egui::Context::default(), &req);
        frames(&mut r.app, &r.ctx, 4, &mut r.time);
    };
    key(&mut r, "[");
    assert_eq!(r.clip(r.clip).fade_in.len, r.bar, "from the clip start to the cursor");
    assert_eq!(r.clip(b).fade_in.len, 0, "the cursor is not inside the other clip");
    assert_eq!(undo_len(&mut r.app), before + 1);
    key(&mut r, "]");
    assert_eq!(r.clip(r.clip).fade_out.len, 3 * r.bar, "from the cursor to the clip end");
    assert_eq!(undo_len(&mut r.app), before + 2);
    // Every row has a key, a note and no clash.
    for action in [crate::shortcuts::Action::FadeInToCursor, crate::shortcuts::Action::FadeOutFromCursor] {
        let row = crate::shortcuts::rows(action).next().expect("a row");
        assert!(row.note.contains("Logic") || row.note.contains("Ours"), "the convention is cited: {}", row.note);
    }
}
