use super::*;
use crate::edit_window::test_support::*;
use egui_kittest::Harness;

#[test]
fn fade_drag_ranges_handle_empty_short_and_saturated_clips() {
    let mut c = Clip::audio(ClipId(1), "test", transit_model::SourceId(1), 0, 100, 1);
    for side in [true, false] {
        for to in [i64::MIN, 100, 101, i64::MAX] {
            assert_eq!(fade_drag_range(&c, side, to), Some(Range::new(100, 101)));
        }
        c.length = 0;
        assert_eq!(fade_drag_range(&c, side, 100), None);
        c.length = -1;
        assert_eq!(fade_drag_range(&c, side, 100), None);
        c.length = 1;
    }
    c.start = i64::MAX - 1;
    c.length = 10;
    for side in [true, false] {
        assert_eq!(fade_drag_range(&c, side, i64::MAX), Some(Range::new(i64::MAX - 1, i64::MAX)));
        c.start = i64::MAX;
        assert_eq!(fade_drag_range(&c, side, i64::MAX), None);
        c.start = i64::MAX - 1;
    }
}

fn fade_shot(h: &mut Harness<SoundApp>, name: &str) {
    if let Some(dir) = std::env::var_os("SOUNDCRAFT_FADE_SHOTS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn fade_curves(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<(egui::epaint::PathShape, Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Path(path)
                if path.stroke == egui::epaint::PathStroke::new(1.2, Color32::from_rgba_unmultiplied(255, 255, 255, 90))
                    && path.points.iter().all(|point| clip_rect.contains(*point)) =>
            {
                Some((path.clone(), shape.clip_rect))
            }
            _ => None,
        })
        .collect()
}

fn waveform_geometry(h: &Harness<SoundApp>, clip_rect: Rect) -> Vec<Vec<Pos2>> {
    h.output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            // The waveform's columns snap to physical pixels and fade out over half of one.
            Shape::Mesh(mesh) if !mesh.vertices.is_empty() && mesh.vertices.iter().all(|vertex| clip_rect.expand(1.0).contains(vertex.pos)) => {
                Some(mesh.vertices.iter().map(|vertex| vertex.pos).collect())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn waveforms_follow_fade_drags_before_release_and_match_the_committed_waveform() {
    for channels in [1, 2] {
        for fade_in in [true, false] {
            for bend in [false, true] {
                for mode in ["peak", "power", "rectified", "outlines"] {
                    let mut h = smart_demo();
                    let tl = timeline(&h);
                    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
                    let neighbour = h
                        .state()
                        .engine
                        .session()
                        .track(tid)
                        .unwrap()
                        .clips()
                        .iter()
                        .find(|c| c.id != clip.id && c.start >= clip.end())
                        .unwrap()
                        .clone();
                    let source = clip.source().unwrap();
                    if channels == 2 {
                        let audio = h.state().engine.session().pool.get(source).unwrap().clone();
                        let left = audio.buffer.channels[0].clone();
                        let right = left.iter().map(|sample| sample * 0.5).collect();
                        let audio = transit_model::SourceAudio::new(transit_audio_io::AudioBuffer {
                            sample_rate: audio.buffer.sample_rate,
                            channels: vec![left, right],
                        });
                        let s = h.state_mut().engine.session_mut();
                        s.pool.insert(source, std::sync::Arc::new(audio));
                        s.track_mut(tid).unwrap().format = transit_model::ChannelFormat::Stereo;
                    }
                    if mode == "power" {
                        h.state_mut().engine.execute("view.waveform_power", &json!({})).unwrap();
                    }
                    h.state_mut().engine.execute("view.waveform_rectified", &json!({"value": mode == "rectified"})).unwrap();
                    h.state_mut().engine.execute("view.waveform_outlines", &json!({"value": mode == "outlines"})).unwrap();
                    h.state_mut().engine.execute("view.waveform_zoom", &json!({"value": 1.4})).unwrap();
                    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                    c.gain_db = -3.0;
                    c.gain_env = vec![(0, -3.0), (c.length, 3.0)];
                    let target = transit_model::Fade {
                        len: if bend { c.length * 2 / 3 } else { c.length / 4 },
                        shape: transit_model::FadeShape::SCurve,
                        curvature: 0.0,
                    };
                    let other = transit_model::Fade { len: c.length / 8, shape: transit_model::FadeShape::Linear, curvature: 0.2 };
                    (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
                    h.step();
                    let before = h.state().engine.session().clone();
                    let undo_len = h.state().engine.undo_history().len();
                    let audio = before.pool.get(source).unwrap().clone();
                    let (xs, xe) = (x_of(&before, tl, clip.start), x_of(&before, tl, clip.end()));
                    let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)).intersect(tl);
                    let neighbour_rect = Rect::from_min_max(
                        pos2(x_of(&before, tl, neighbour.start), lane.min.y),
                        pos2(x_of(&before, tl, neighbour.end()), lane.max.y),
                    )
                    .intersect(tl);
                    let original = waveform_geometry(&h, clip_rect);
                    let neighbour_wave = waveform_geometry(&h, neighbour_rect);
                    assert_eq!(original.len(), channels);
                    let side = if fade_in { "in" } else { "out" };
                    let operation = if bend { "bend" } else { "length" };
                    let screenshot = channels == 1 && mode == "peak";
                    if screenshot {
                        fade_shot(&mut h, &format!("waveform-{side}-{operation}-before"));
                    }
                    let (press, to) = if bend {
                        let c = before.find_clip(clip.id).unwrap().1;
                        let midpoint = fade_midpoint(&before, tl, lane, c, fade_in).unwrap();
                        (midpoint, midpoint - vec2(0.0, 16.0))
                    } else {
                        (
                            pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0),
                            pos2(xs + (xe - xs) * if fade_in { 0.7 } else { 0.3 }, lane.min.y + 4.0),
                        )
                    };
                    h.hover_at(press);
                    h.drag_at(press);
                    h.hover_at(to);
                    h.step();
                    h.step();
                    assert!(if bend {
                        matches!(h.state().gesture, Some(Gesture::FadeCurve { .. }))
                    } else {
                        matches!(h.state().gesture, Some(Gesture::Fade { .. }))
                    });
                    let preview = waveform_geometry(&h, clip_rect);
                    assert_eq!(preview.len(), channels);
                    for channel in 0..channels {
                        assert_ne!(
                            preview[channel], original[channel],
                            "the waveform must respond while the button is held: {side}, {operation}, {mode}, channel={channel}"
                        );
                    }
                    assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
                    assert_eq!(h.state().engine.session(), &before);
                    assert_eq!(h.state().engine.undo_history().len(), undo_len);
                    assert!(
                        std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio),
                        "preview must retain cached media and peaks"
                    );
                    if screenshot {
                        fade_shot(&mut h, &format!("waveform-{side}-{operation}-preview"));
                    }
                    h.drop_at(to);
                    h.step();
                    assert!(h.state().gesture.is_none());
                    assert_eq!(waveform_geometry(&h, clip_rect), preview, "release must not cause a waveform jump");
                    assert_eq!(waveform_geometry(&h, neighbour_rect), neighbour_wave);
                    assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                    assert!(std::sync::Arc::ptr_eq(h.state().engine.session().pool.get(source).unwrap(), &audio));
                    if screenshot {
                        fade_shot(&mut h, &format!("waveform-{side}-{operation}-released"));
                    }
                    assert!(h.state_mut().engine.undo());
                    assert_eq!(h.state().engine.session(), &before);
                    h.step();
                    assert_eq!(waveform_geometry(&h, clip_rect), original);
                }
            }
        }
    }
}

#[test]
fn fade_drag_previews_and_commits_stay_inside_the_clip() {
    for fade_in in [true, false] {
        for release_outside in [false, true] {
            let mut h = smart_demo();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
            let s = h.state().engine.session();
            let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
            let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
            let before = s.clone();
            let undo_len = h.state().engine.undo_history().len();
            start_drag(&mut h, corner);
            assert!(matches!(h.state().gesture, Some(Gesture::Fade { fade_in: side, .. }) if side == fade_in));
            let name = if fade_in { "fade-in" } else { "fade-out" };
            for (label, x) in [("left", xs - 60.0), ("right", xe + 60.0), ("inside", (xs + xe) / 2.0)] {
                h.hover_at(pos2(x, corner.y));
                h.step();
                let s = h.state().engine.session();
                let range = fade_drag_range(&clip, fade_in, sample_at(s, tl, x)).unwrap();
                let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                assert_eq!(curves.len(), 1, "only one fade curve should be painted");
                let (curve, clip_rect) = &curves[0];
                let points = &curve.points;
                assert_eq!(points.first().unwrap().x, x_of(s, tl, range.start));
                assert_eq!(points.last().unwrap().x, x_of(s, tl, range.end));
                assert!(points.iter().all(|p| p.x >= xs && p.x <= xe && lane.contains(*p)));
                assert!(clip_rect.min.x >= xs && clip_rect.max.x <= xe);
                assert_eq!(h.state().engine.session(), &before, "drag must only preview");
                if !release_outside {
                    fade_shot(&mut h, &format!("{name}-{label}"));
                }
            }
            // Releasing at a new point without an intervening move must use that point.
            let release = pos2(if release_outside { if fade_in { xe + 60.0 } else { xs - 60.0 } } else { xs + (xe - xs) * 0.3 }, corner.y);
            h.drop_at(release);
            h.step();
            assert!(h.state().gesture.is_none());
            let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, release.x)).unwrap();
            let mut expected = before.clone();
            let c = expected.find_clip_mut(clip.id).unwrap();
            let fade = transit_model::Fade { len: range.end - range.start, shape: transit_model::FadeShape::SCurve, curvature: 0.0 };
            if fade_in {
                c.fade_in = fade;
            } else {
                c.fade_out = fade;
            }
            assert_eq!(h.state().engine.session(), &expected);
            assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
            if release_outside {
                assert_eq!(fade.len, clip.length);
                fade_shot(&mut h, &format!("{name}-committed"));
            }
            assert!(h.state_mut().engine.undo());
            assert_eq!(h.state().engine.session(), &before);
            assert!(h.state_mut().engine.redo());
            assert_eq!(h.state().engine.session(), &expected);
        }
    }
}

#[test]
fn a_fade_drag_replaces_the_old_curve_and_matches_the_released_curve() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            let mut h = smart_demo();
            h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
            h.step();
            let tl = timeline(&h);
            let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
            let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
            c.fade_in = transit_model::Fade { len: c.length / 8, shape: transit_model::FadeShape::Linear, curvature: 0.0 };
            c.fade_out = transit_model::Fade { len: c.length / 8, shape: transit_model::FadeShape::EqualPower, curvature: 0.0 };
            h.step();
            let s = h.state().engine.session();
            let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
            let clip_rect = Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y));
            let original = fade_curves(&h, clip_rect);
            assert_eq!(original.len(), 2);
            let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
            let before = s.clone();
            let corner = pos2(if fade_in { xs + 6.0 } else { xe - 6.0 }, lane.min.y + 4.0);
            start_drag(&mut h, corner);
            let to = pos2(if fade_in { xs + (xe - xs) * 0.65 } else { xs + (xe - xs) * 0.35 }, corner.y);
            h.hover_at(to);
            h.step();
            let preview = fade_curves(&h, clip_rect);
            assert_eq!(preview.len(), 2, "one curve per side, without the old curve underneath");
            assert_ne!(preview[edited], original[edited]);
            assert_eq!(preview[opposite], original[opposite]);
            assert_eq!(h.state().engine.session(), &before);
            assert!(!h.output().shapes.iter().any(|shape| {
                matches!(&shape.shape, Shape::LineSegment { points, stroke } if stroke.width == 2.0 && stroke.color == Color32::WHITE && points.iter().all(|p| clip_rect.contains(*p)))
            }), "the straight preview must be gone");
            let side = if fade_in { "in" } else { "out" };
            if show_name {
                fade_shot(&mut h, &format!("fade-{side}-curve-dragging"));
            }
            h.drop_at(to);
            h.step();
            assert_eq!(fade_curves(&h, clip_rect), preview, "curve geometry, thickness and colour must be identical after release");
            if show_name {
                fade_shot(&mut h, &format!("fade-{side}-curve-released"));
            }
        }
    }
}

fn painted_fade_control(h: &Harness<SoundApp>, clip_rect: Rect) -> Pos2 {
    let controls: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Circle(circle) if circle.radius == 3.5 && clip_rect.contains(circle.center) => {
                let radius = circle.radius + circle.stroke.width * 0.5;
                for offset in [vec2(radius, 0.0), vec2(-radius, 0.0), vec2(0.0, radius), vec2(0.0, -radius)] {
                    assert!(shape.clip_rect.contains(circle.center + offset), "the complete control must remain visible");
                }
                Some(circle.center)
            }
            _ => None,
        })
        .collect();
    assert_eq!(controls.len(), 1, "the midpoint must stay visible without hover or a held mouse button");
    controls[0]
}

#[test]
fn fade_midpoint_handles_remain_visible_and_can_be_grabbed_again() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for shape in [transit_model::FadeShape::SCurve, transit_model::FadeShape::Exponential] {
                for height in ["micro", "jumbo"] {
                    let mut h = smart_demo();
                    let tl = timeline(&h);
                    let (clip, _) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
                    let spp = h.state().engine.session().edit.zoom.samples_per_px * 0.5;
                    h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                    h.state_mut().engine.execute("track.height", &json!({"tracks": [tid.0], "height": height})).unwrap();
                    h.state_mut().engine.execute("view.zoom_set", &json!({"samples_per_px": spp})).unwrap();
                    h.state_mut().engine.execute("view.scroll", &json!({"to": clip.start - clip.length / 5})).unwrap();
                    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                    let fade = transit_model::Fade { len: c.length * 2 / 3, shape, curvature: 0.0 };
                    if fade_in {
                        c.fade_in = fade;
                    } else {
                        c.fade_out = fade;
                    }
                    h.step();
                    let row = h.state().edit_layout.rows.iter().find(|(id, _)| *id == tid.0).unwrap().1;
                    let lane = lane_rect(h.state().engine.session().track(tid).unwrap(), rect(row), tl);
                    let s = h.state().engine.session();
                    let clip_rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
                    let control = painted_fade_control(&h, clip_rect);
                    let undo_len = h.state().engine.undo_history().len();
                    start_drag(&mut h, control);
                    let outside = control + vec2(0.0, 500.0);
                    h.hover_at(outside);
                    h.step();
                    h.drop_at(outside);
                    h.step();
                    let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                    assert_eq!(if fade_in { c.fade_in.curvature } else { c.fade_out.curvature }, -1.0);
                    assert!(h.state().gesture.is_none());
                    painted_fade_control(&h, clip_rect);
                    let side = if fade_in { "in" } else { "out" };
                    let screenshot = show_name && height == "jumbo" && shape == transit_model::FadeShape::SCurve;
                    if screenshot {
                        fade_shot(&mut h, &format!("fade-{side}-smooth-released"));
                    }
                    for iteration in 0..2 {
                        let before = h.state().engine.session().clone();
                        let control = painted_fade_control(&h, clip_rect);
                        h.hover_at(control);
                        h.step();
                        assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                        start_drag(&mut h, control);
                        let to = control - vec2(0.0, (lane.height() - 2.0 - clip_name_height(&before, lane)) * 0.1);
                        h.hover_at(to);
                        h.step();
                        assert!(
                            matches!(h.state().gesture, Some(Gesture::FadeCurve { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in)
                        );
                        h.drop_at(to);
                        h.step();
                        let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
                        let current = if fade_in { c.fade_in } else { c.fade_out };
                        let previous = before.find_clip(clip.id).unwrap().1;
                        let previous = if fade_in { previous.fade_in } else { previous.fade_out };
                        assert!(current.curvature > previous.curvature);
                        assert_eq!((current.len, current.shape), (previous.len, previous.shape));
                        painted_fade_control(&h, clip_rect);
                        assert_eq!(h.state().engine.undo_history().len(), undo_len + iteration + 2);
                        if screenshot && iteration == 1 {
                            fade_shot(&mut h, &format!("fade-{side}-smooth-regrabbed"));
                        }
                        let after = h.state().engine.session().clone();
                        assert!(h.state_mut().engine.undo());
                        assert_eq!(h.state().engine.session(), &before);
                        assert!(h.state_mut().engine.redo());
                        assert_eq!(h.state().engine.session(), &after);
                    }
                }
            }
        }
    }
}

#[test]
fn fade_midpoints_drag_vertically_without_moving_the_endpoints() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for dy in [-16.0, 16.0] {
                let mut h = smart_demo();
                h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                h.step();
                let tl = timeline(&h);
                let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                let target = transit_model::Fade { len: c.length * 2 / 3, shape: transit_model::FadeShape::SCurve, curvature: 0.0 };
                let other = transit_model::Fade { len: c.length / 8, shape: transit_model::FadeShape::Linear, curvature: -0.2 };
                (c.fade_in, c.fade_out) = if fade_in { (target, other) } else { (other, target) };
                h.step();
                let before = h.state().engine.session().clone();
                let clip_rect = Rect::from_min_max(pos2(x_of(&before, tl, clip.start), lane.min.y), pos2(x_of(&before, tl, clip.end()), lane.max.y));
                let original = fade_curves(&h, clip_rect);
                let (edited, opposite) = if fade_in { (0, 1) } else { (1, 0) };
                let middle = original[edited].0.points.len() / 2;
                let midpoint = original[edited].0.points[middle];
                let undo_len = h.state().engine.undo_history().len();
                h.hover_at(midpoint);
                h.step();
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                click(&mut h, midpoint);
                assert_eq!(h.state().engine.session(), &before, "clicking the curve control should not change the selection");
                h.drag_at(midpoint);
                let to = midpoint + vec2(0.0, dy);
                h.hover_at(to);
                h.step();
                h.step(); // Paint the frame requested when the drag gesture starts.
                let curvature = match h.state().gesture {
                    Some(Gesture::FadeCurve { clip: id, fade_in: side, curvature, .. }) if id == clip.id && side == fade_in => curvature,
                    _ => panic!("the midpoint must start a curve gesture"),
                };
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeVertical);
                assert!(if dy < 0.0 { curvature > 0.0 } else { curvature < 0.0 });
                let preview = fade_curves(&h, clip_rect);
                assert_eq!(preview.len(), 2);
                assert_eq!(preview[opposite], original[opposite]);
                assert_eq!(preview[edited].0.points.first(), original[edited].0.points.first());
                assert_eq!(preview[edited].0.points.last(), original[edited].0.points.last());
                assert_eq!(preview[edited].0.points[middle].x, midpoint.x);
                assert!(
                    (preview[edited].0.points[middle].y - (midpoint.y + dy)).abs() < 0.001,
                    "side={fade_in}, names={show_name}, dy={dy}, midpoint={midpoint:?}, drawn={:?}, gesture={:?}",
                    preview[edited].0.points[middle],
                    h.state().gesture
                );
                assert_eq!(h.state().engine.session(), &before);
                let side = if fade_in { "in" } else { "out" };
                let direction = if dy < 0.0 { "up" } else { "down" };
                if show_name {
                    fade_shot(&mut h, &format!("fade-{side}-bend-{direction}-preview"));
                }
                h.drop_at(to);
                h.step();
                let mut expected = before.clone();
                let c = expected.find_clip_mut(clip.id).unwrap();
                if fade_in {
                    c.fade_in.curvature = curvature;
                } else {
                    c.fade_out.curvature = curvature;
                }
                assert_eq!(h.state().engine.session(), &expected);
                assert_eq!(fade_curves(&h, clip_rect), preview, "the released envelope must match the preview");
                let rel = if fade_in { target.len / 2 } else { clip.length - target.len / 2 };
                let previous_gain = before.find_clip(clip.id).unwrap().1.gain_at(rel);
                let new_gain = expected.find_clip(clip.id).unwrap().1.gain_at(rel);
                assert!(if dy < 0.0 { new_gain > previous_gain } else { new_gain < previous_gain });
                assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                if show_name {
                    fade_shot(&mut h, &format!("fade-{side}-bend-{direction}-released"));
                }
                assert!(h.state_mut().engine.undo());
                assert_eq!(h.state().engine.session(), &before);
                assert!(h.state_mut().engine.redo());
                assert_eq!(h.state().engine.session(), &expected);
            }
        }
    }
}

#[test]
fn fade_curve_drags_cancel_safely_or_return_to_the_original_curve() {
    for invalidate in [0, 1, 2] {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
        c.fade_out = transit_model::Fade { len: c.length * 2 / 3, shape: transit_model::FadeShape::SCurve, curvature: 0.0 };
        h.step();
        let midpoint = fade_midpoint(h.state().engine.session(), tl, lane, h.state().engine.session().find_clip(clip.id).unwrap().1, false).unwrap();
        let before = h.state().engine.session().clone();
        let undo_len = h.state().engine.undo_history().len();
        start_drag(&mut h, midpoint);
        h.hover_at(midpoint - vec2(0.0, 16.0));
        h.step();
        assert!(matches!(h.state().gesture, Some(Gesture::FadeCurve { .. })));
        if invalidate == 1 {
            h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap().length = 0;
        } else if invalidate == 2 {
            let (tid, _) = before.find_clip(clip.id).unwrap();
            h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
        }
        h.hover_at(midpoint);
        h.step();
        h.drop_at(midpoint);
        h.step();
        assert!(h.state().gesture.is_none());
        assert_eq!(h.state().engine.undo_history().len(), undo_len);
        if invalidate == 0 {
            assert_eq!(h.state().engine.session(), &before);
        }
    }
}

#[test]
fn existing_fades_drag_from_their_upper_curve_endpoints() {
    for fade_in in [true, false] {
        for show_name in [true, false] {
            for full_length in [false, true] {
                let mut h = smart_demo();
                h.state_mut().engine.execute("view.clip_name", &json!({"value": show_name})).unwrap();
                h.step();
                let tl = timeline(&h);
                let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
                let len = if full_length { clip.length } else { clip.length * 2 / 3 };
                let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
                let fade = transit_model::Fade { len, shape: transit_model::FadeShape::SCurve, curvature: 0.35 };
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                h.step();
                let s = h.state().engine.session();
                let (xs, xe) = (x_of(s, tl, clip.start), x_of(s, tl, clip.end()));
                let curves = fade_curves(&h, Rect::from_min_max(pos2(xs, lane.min.y), pos2(xe, lane.max.y)));
                assert_eq!(curves.len(), 1);
                let curve = &curves[0].0;
                // Take the handle from the painted curve, rather than duplicating hit geometry.
                let head = *if fade_in { curve.points.last() } else { curve.points.first() }.unwrap();
                let press = head + vec2(3.0, 2.0);
                if fade_in && full_length {
                    let (tid, _) = s.find_clip(clip.id).unwrap();
                    assert_ne!(
                        clip_at(s.track(tid).unwrap(), sample_at(s, tl, press.x)).unwrap().id,
                        clip.id,
                        "the pointer should be over the adjacent clip"
                    );
                }
                let before = s.clone();
                let undo_len = h.state().engine.undo_history().len();
                h.hover_at(press);
                h.step();
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                let side = if fade_in { "in" } else { "out" };
                if show_name && !full_length {
                    fade_shot(&mut h, &format!("fade-{side}-endpoint-hover"));
                }
                h.drag_at(press);
                let to = head + vec2(if fade_in { -35.0 } else { 35.0 }, 0.0);
                h.hover_at(to);
                h.step();
                assert!(matches!(h.state().gesture, Some(Gesture::Fade { clip: id, fade_in: side, .. }) if id == clip.id && side == fade_in));
                assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::ResizeNwSe);
                assert_eq!(h.state().engine.session(), &before, "pressing the handle must not move the edit selection");
                if show_name && !full_length {
                    fade_shot(&mut h, &format!("fade-{side}-endpoint-dragging"));
                }
                h.drop_at(to);
                h.step();
                let range = fade_drag_range(&clip, fade_in, sample_at(&before, tl, to.x)).unwrap();
                let mut expected = before.clone();
                let c = expected.find_clip_mut(clip.id).unwrap();
                let fade = transit_model::Fade { len: range.end - range.start, shape: transit_model::FadeShape::SCurve, curvature: 0.35 };
                assert!(fade.len < len);
                if fade_in {
                    c.fade_in = fade;
                } else {
                    c.fade_out = fade;
                }
                assert_eq!(h.state().engine.session(), &expected);
                assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
                assert!(h.state_mut().engine.undo());
                assert_eq!(h.state().engine.session(), &before);
            }
        }
    }
}

#[test]
fn existing_fade_handles_preserve_selector_and_other_smart_tool_zones() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 30.0 && xe < tl.max.x - 100.0 && xe - xs > 100.0);
    let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
    c.fade_out = transit_model::Fade { len: c.length / 2, shape: transit_model::FadeShape::SCurve, curvature: 0.0 };
    h.step();
    let s = h.state().engine.session();
    let rect = Rect::from_min_max(pos2(x_of(s, tl, clip.start), lane.min.y), pos2(x_of(s, tl, clip.end()), lane.max.y));
    let curves = fade_curves(&h, rect);
    let head = *curves[0].0.points.first().unwrap();
    h.hover_at(head + vec2(0.0, 16.0));
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
    h.state_mut().engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
    h.step();
    h.hover_at(head);
    h.step();
    assert_eq!(h.output().platform_output.cursor_icon, egui::CursorIcon::Text);
    start_drag(&mut h, head);
    assert!(matches!(h.state().gesture, Some(Gesture::Select { .. })));
    h.drop_at(head + vec2(20.0, 0.0));
    h.step();
}

#[test]
fn a_fade_drag_handles_its_clip_shrinking_to_one_sample() {
    for fade_in in [true, false] {
        let mut h = smart_demo();
        let tl = timeline(&h);
        let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
        let s = h.state().engine.session();
        let corner = pos2(x_of(s, tl, if fade_in { clip.start } else { clip.end() }) + if fade_in { 6.0 } else { -6.0 }, lane.min.y + 4.0);
        start_drag(&mut h, corner);
        assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
        let c = h.state_mut().engine.session_mut().find_clip_mut(clip.id).unwrap();
        c.length = 1;
        c.clamp_fades();
        let undo_len = h.state().engine.undo_history().len();
        h.drop_at(corner + vec2(40.0, 0.0));
        h.step();
        let c = h.state().engine.session().find_clip(clip.id).unwrap().1;
        assert_eq!(if fade_in { c.fade_in.len } else { c.fade_out.len }, 1);
        assert_eq!(h.state().engine.undo_history().len(), undo_len + 1);
        assert!(h.state().gesture.is_none());
    }
}

#[test]
fn a_fade_drag_cancels_if_its_clip_disappears() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
    let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
    start_drag(&mut h, corner);
    assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
    let (tid, _) = h.state().engine.session().find_clip(clip.id).unwrap();
    h.state_mut().engine.session_mut().track_mut(tid).unwrap().playlist_mut().unwrap().clips.retain(|c| c.id != clip.id);
    let undo_len = h.state().engine.undo_history().len();
    h.hover_at(corner + vec2(40.0, 0.0));
    h.step();
    h.drop_at(corner + vec2(40.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    assert_eq!(h.state().engine.undo_history().len(), undo_len);
}

#[test]
fn a_fade_drag_survives_its_clip_shrinking_to_nothing() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xs > tl.min.x + 20.0 && xe - xs > 100.0);
    let corner = pos2(x_of(h.state().engine.session(), tl, clip.start) + 6.0, lane.min.y + 4.0);
    start_drag(&mut h, corner);
    assert!(matches!(h.state().gesture, Some(Gesture::Fade { .. })));
    if let Some(c) = h.state_mut().engine.session_mut().find_clip_mut(clip.id) {
        c.length = 0;
    }
    let undo_len = h.state().engine.undo_history().len();
    h.hover_at(corner + vec2(20.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    h.drop_at(corner + vec2(20.0, 0.0));
    h.step();
    assert!(h.state().gesture.is_none());
    assert_eq!(h.state().engine.undo_history().len(), undo_len);
}
