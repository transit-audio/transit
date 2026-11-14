//! The pre-count zone's UI floors (ADR 0052 slice 3): the cursor, the ruler and timeline clicks,
//! the snap helpers, a clip drop and an Alt-drag copy, nudge, Play's pre-roll and Cycle, the
//! counter, the grid, the ruler's bar numbers, the scroll and the zone's darker lanes reach back
//! to the session start, and stop at bar 1 without a zone. No command makes content before bar 1
//! yet, so each session is built with a clip before bar 1 directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::tests::{drag, frame_shapes, frames, hold_drag};
use crate::theme::TOKENS;
use crate::{Services, StudioApp};
use egui::Shape;
use egui::epaint::ClippedShape;
use serde_json::json;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (the default session).
const BAR: Samples = 96_000;
const BEAT: Samples = BAR / 4;

/// A studio over a session with one MIDI track holding a one-note MIDI clip per `(start, length)`.
fn zone_app(clips: &[(Samples, Samples)]) -> (StudioApp, TrackId, Vec<ClipId>) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let mut ids = Vec::new();
    for &(start, length) in clips {
        let id = s.new_clip_id();
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: Q });
        transit_engine::edit::place_clip(&mut s, t, Clip::midi(id, "m", start, length, seq));
        ids.push(id);
    }
    (StudioApp::new(transit_engine::Engine::new(s), None, Services::default()), t, ids)
}

fn start_of(app: &StudioApp, id: ClipId) -> Samples {
    app.engine.session().find_clip(id).unwrap().1.start
}

fn flat(shapes: &[ClippedShape]) -> Vec<Shape> {
    fn walk(s: &Shape, out: &mut Vec<Shape>) {
        if let Shape::Vec(v) = s {
            v.iter().for_each(|s| walk(s, out));
        } else {
            out.push(s.clone());
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|s| walk(&s.shape, &mut out));
    out
}

/// The texts painted inside `r` (`[x0, y0, x1, y1]`).
fn texts_in(shapes: &[ClippedShape], r: [f32; 4]) -> Vec<String> {
    flat(shapes)
        .iter()
        .filter_map(|s| match s {
            Shape::Text(t) if t.pos.x >= r[0] && t.pos.x <= r[2] && t.pos.y >= r[1] && t.pos.y <= r[3] => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

/// The x of timeline position `at` in the last frame's layout.
fn x_of(app: &StudioApp, at: Samples) -> f32 {
    let l = &app.arrange.layout;
    l.timeline[0] + ((at - l.scroll) as f64 / l.samples_per_px) as f32
}

/// Frames of the real UI at a zoom where a bar is 240 points, scrolled to `to`.
fn framed(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, spp: f64, to: Samples) -> Vec<ClippedShape> {
    app.run("view.zoom_set", json!({"samples_per_px": spp})).unwrap();
    app.run("view.scroll", json!({"to": to})).unwrap();
    frames(app, ctx, 3, time);
    frame_shapes(app, ctx, time)
}

#[test]
fn the_ruler_numbers_bars_0_and_minus_1_in_the_zone_and_starts_at_bar_1_without_one() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, ..) = zone_app(&[(-2 * BAR, BAR)]);
    let shapes = framed(&mut app, &ctx, &mut time, 400.0, -2 * BAR);
    assert_eq!(app.arrange.layout.scroll, -2 * BAR, "the view scrolls to the session start");
    let ruler = app.arrange.layout.ruler;
    let labels = texts_in(&shapes, ruler);
    for bar in ["-1", "0", "1", "2"] {
        assert!(labels.iter().any(|l| l == bar), "bar {bar} is numbered in the ruler: {labels:?}");
    }
    let x = |at| x_of(&app, at);
    let at_label = |text: &str| {
        flat(&shapes).iter().find_map(|s| match s {
            Shape::Text(t) if t.galley.job.text == text && t.pos.y <= ruler[3] => Some(t.pos.x),
            _ => None,
        })
    };
    // A label sits 4 points right of its bar line.
    assert!((at_label("-1").unwrap() - (x(-2 * BAR) + 4.0)).abs() < 1.0, "bar -1 starts at the session start");
    assert!((at_label("0").unwrap() - (x(-BAR) + 4.0)).abs() < 1.0, "bar 0 is the bar before bar 1");
    assert!((at_label("1").unwrap() - (x(0) + 4.0)).abs() < 1.0, "bar 1 stays at sample 0");

    // Without a zone the ruler starts at bar 1, as before.
    let (mut plain, ..) = zone_app(&[(BAR, BAR)]);
    let shapes = framed(&mut plain, &ctx, &mut time, 400.0, -2 * BAR);
    assert_eq!(plain.arrange.layout.scroll, 0, "no further than bar 1");
    let labels = texts_in(&shapes, plain.arrange.layout.ruler);
    assert!(labels.iter().any(|l| l == "1") && !labels.iter().any(|l| l == "0" || l == "-1"), "{labels:?}");
}

#[test]
fn the_zone_lanes_are_darker_from_the_session_start_to_bar_1_and_absent_without_a_zone() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, t, _) = zone_app(&[(-2 * BAR, BAR)]);
    let shapes = framed(&mut app, &ctx, &mut time, 400.0, -3 * BAR);
    let lane = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().lane;
    let zone: Vec<egui::Rect> = flat(&shapes)
        .iter()
        .filter_map(|s| match s {
            Shape::Rect(r) if r.fill == TOKENS.lane_pre_count => Some(r.rect),
            _ => None,
        })
        .collect();
    assert_eq!(zone.len(), 1, "one tinted span per lane: {zone:?}");
    let z = zone[0];
    assert!((z.min.x - x_of(&app, -2 * BAR)).abs() < 0.5 && (z.max.x - x_of(&app, 0)).abs() < 0.5, "from the session start to bar 1: {z:?}");
    assert!((z.min.y - lane[1]).abs() < 0.5, "over the track's lane");

    let (mut plain, ..) = zone_app(&[(0, BAR)]);
    let shapes = framed(&mut plain, &ctx, &mut time, 400.0, 0);
    assert!(!flat(&shapes).iter().any(|s| matches!(s, Shape::Rect(r) if r.fill == TOKENS.lane_pre_count)), "no zone, no tint");
}

#[test]
fn a_ruler_click_in_the_zone_puts_the_cursor_there_and_stops_at_the_session_start() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, ..) = zone_app(&[(-2 * BAR, BAR)]);
    framed(&mut app, &ctx, &mut time, 400.0, -2 * BAR);
    let r = app.arrange.layout.ruler;
    let x = x_of(&app, -BAR - BEAT);
    crate::tests::click(&mut app, (x, r[3] - 4.0));
    frames(&mut app, &ctx, 4, &mut time);
    let at = app.cursor();
    assert!((at - (-BAR - BEAT)).abs() <= 400, "the cursor lands in the zone where clicked: {at}");
    assert_eq!(app.engine.transport.position, app.cursor(), "the stopped playhead follows");
    // The calls themselves floor at the session start, and at bar 1 without a zone.
    app.click_ruler(-5 * BAR);
    assert_eq!(app.cursor(), -2 * BAR);
    app.click_empty(None, -BAR);
    assert_eq!(app.cursor(), -BAR, "a click on the empty timeline in the zone");
    let (mut plain, ..) = zone_app(&[(BAR, BAR)]);
    plain.click_ruler(-BAR);
    assert_eq!(plain.cursor(), 0);
}

#[test]
fn a_cursor_left_in_a_zone_that_has_gone_reads_bar_1() {
    let (mut app, t, ids) = zone_app(&[(-2 * BAR, BAR), (BAR, BAR)]);
    app.set_cursor(-BAR, None);
    assert_eq!(app.cursor(), -BAR);
    app.run("edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": -BAR, "end": -BAR})).unwrap();
    assert_eq!(app.engine.session().start(), 0);
    assert_eq!(app.cursor(), 0, "floored when read, at the new session start");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.cursor(), -BAR, "the stored cursor was not rewritten");
}

#[test]
fn the_snap_reaches_zone_grid_lines_and_floors_at_the_session_start() {
    let (app, ..) = zone_app(&[(-2 * BAR, BAR)]);
    let s = app.engine.session();
    let g = crate::grid::GridView::default();
    let spp = 1024.0;
    assert_eq!(crate::automation::grid_ticks(s, spp, &g), Q, "the grid is a beat");
    assert_eq!(crate::automation::snap(s, spp, &g, -BAR / 2 - 1000), -BAR / 2, "a zone grid line");
    assert_eq!(crate::automation::snap(s, spp, &g, -5 * BAR), -2 * BAR, "never before the session start");
    assert_eq!(crate::automation::snap_delta(s, spp, &g, BAR, -BAR - BAR / 2 - 1000), -BAR - BAR / 2, "a drag into the zone");
    assert_eq!(crate::automation::snap_delta(s, spp, &g, BAR, -9 * BAR), -3 * BAR, "stops at the session start");
    let (plain, ..) = zone_app(&[(BAR, BAR)]);
    let s = plain.engine.session();
    assert_eq!(crate::automation::snap(s, spp, &g, -400), 0, "bar 1 without a zone");
    assert_eq!(crate::automation::snap_delta(s, spp, &g, BAR, -2 * BAR), -BAR);
}

#[test]
fn with_snap_on_a_clip_dropped_in_the_zone_lands_on_a_zone_grid_line() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, _, ids) = zone_app(&[(-2 * BAR, BAR / 4), (BAR, BAR)]);
    framed(&mut app, &ctx, &mut time, 1024.0, -2 * BAR);
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == ids[1].0).unwrap().rect;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    let px_per_beat = (BEAT as f64 / app.arrange.layout.samples_per_px) as f32;
    // 6.4 beats back from bar 2: 2.4 beats before bar 1, which snaps to 2 beats before it.
    drag(&mut app, grab, (grab.0 - 6.4 * px_per_beat, grab.1), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(start_of(&app, ids[1]), -2 * BEAT, "on the zone's beat line");
}

#[test]
fn an_alt_drag_copy_of_a_midi_clip_lands_in_the_zone() {
    let (mut app, t, ids) = zone_app(&[(-2 * BAR, BAR / 4), (BAR, BAR / 4)]);
    app.copy_clip_to(ids[1], -BAR, t).unwrap();
    let starts: Vec<Samples> = app.engine.session().track(t).unwrap().clips().iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![-2 * BAR, -BAR, BAR], "the copy lands where it was dropped, in the zone");
    app.copy_clip_to(ids[1], -9 * BAR, t).unwrap();
    assert!(app.engine.session().track(t).unwrap().clips().iter().all(|c| c.start >= -2 * BAR), "never before the session start");
}

#[test]
fn nudge_left_of_a_zone_clip_with_room_before_it_moves_it_earlier() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    // The first clip sets the session start; the second sits in the zone with room before it.
    let (mut app, _, ids) = zone_app(&[(-2 * BAR, BAR / 4), (-BAR, BAR / 4)]);
    frames(&mut app, &ctx, 3, &mut time);
    let spp = app.arrange.layout.samples_per_px;
    assert_eq!(crate::automation::grid_ticks(app.engine.session(), spp, &app.arrange.grid), Q, "a beat step");
    app.run("edit.select", json!({"clips": [ids[1].0]})).unwrap();
    app.nudge_selected(-1, false).unwrap();
    assert_eq!(start_of(&app, ids[1]), -BAR - BEAT, "one beat earlier, still in the zone");
    // At the session start there is nowhere left to go.
    app.run("edit.select", json!({"clips": [ids[0].0]})).unwrap();
    assert!(app.nudge_selected(-1, false).is_err());
    assert_eq!(start_of(&app, ids[0]), -2 * BAR);
}

#[test]
fn play_with_pre_roll_from_bar_1_starts_in_the_zone_and_at_bar_1_without_one() {
    for (clips, want) in [(vec![(-BAR, BAR)], -BAR / 2), (vec![(BAR, BAR)], 0)] {
        let (mut app, ..) = zone_app(&clips);
        let e = &mut app.engine.session_mut().edit;
        e.pre_post_roll = true;
        e.pre_roll = BAR / 2;
        app.set_cursor(0, None);
        app.play_from_cursor();
        assert_eq!(app.sim.as_ref().map(|s| s.0), Some(want), "zone clips {clips:?}");
    }
}

#[test]
fn cycle_plays_from_bar_1_after_the_zones_clip_is_deleted() {
    let (mut app, t, ids) = zone_app(&[(-BAR, BAR / 4), (BAR, BAR)]);
    app.run("transport.loop", json!({"start": -BAR, "end": BAR, "enabled": true})).unwrap();
    app.set_cursor(0, None);
    app.play_from_cursor();
    assert_eq!(app.sim.as_ref().map(|s| (s.0, s.2)), Some((-BAR, Some(Range::new(-BAR, BAR)))), "with the zone the loop starts in it");
    app.stop_play();
    app.run("edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": -BAR, "end": -BAR})).unwrap();
    assert_eq!(app.engine.session().start(), 0);
    assert_eq!(app.engine.session().edit.loop_range, Range::new(-BAR, BAR), "the stored loop is unchanged");
    app.play_from_cursor();
    assert_eq!(app.sim.as_ref().map(|s| (s.0, s.2)), Some((0, Some(Range::new(0, BAR)))), "Cycle plays from bar 1");
    app.stop_play();
    // A stored loop wholly before the session start loops nothing; Play starts at the cursor.
    app.run("edit.undo", json!({})).unwrap();
    app.run("transport.loop", json!({"start": -BAR, "end": -BAR / 2, "enabled": true})).unwrap();
    app.run("edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": -BAR, "end": -BAR})).unwrap();
    assert_eq!(app.wanted_loop(), None);
}

#[test]
fn the_counter_counts_bars_down_before_bar_1_and_signs_min_secs() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, ..) = zone_app(&[(-2 * BAR, BAR)]);
    assert_eq!(crate::transport::bars_beats(&app, -BAR), "  0. 1. 1. 000");
    assert_eq!(crate::transport::bars_beats(&app, -2 * BAR + BEAT), " -1. 2. 1. 000");
    assert_eq!(crate::transport::min_sec(&app, -BAR), "-0:02.000");
    assert_eq!(crate::transport::bars_beats(&app, 0), "  1. 1. 1. 000", "bar 1 unchanged");
    assert_eq!(crate::transport::min_sec(&app, 72_000), "0:01.500", "positive readings unchanged");
    // The transport bar shows the cursor in the zone.
    app.set_cursor(-BAR, None);
    frames(&mut app, &ctx, 2, &mut time);
    let shapes = frame_shapes(&mut app, &ctx, &mut time);
    let all: Vec<String> = flat(&shapes).iter().filter_map(|s| if let Shape::Text(t) = s { Some(t.galley.job.text.clone()) } else { None }).collect();
    assert!(all.iter().any(|t| t == "-0:02.000"), "the time readout");
    assert!(all.iter().any(|t| t == "  0. 1. 1. 000"), "the bars readout");
}

#[test]
fn the_view_follows_the_playhead_back_into_the_zone_and_fit_shows_it() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, ..) = zone_app(&[(-2 * BAR, BAR), (BAR, BAR)]);
    framed(&mut app, &ctx, &mut time, 1024.0, 20 * BAR);
    app.run("transport.play", json!({"from": -2 * BAR})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.is_playing());
    assert_eq!(app.arrange.layout.scroll, -2 * BAR, "paged back to the session start, not to bar 1");
    app.stop_play();
    app.run("view.zoom_fit", json!({"width_px": 1000})).unwrap();
    let z = &app.engine.session().edit.zoom;
    assert_eq!(z.scroll, -2 * BAR, "Fill Window With Session starts at the session start");
    let (mut plain, ..) = zone_app(&[(BAR, BAR)]);
    plain.run("view.zoom_fit", json!({"width_px": 1000})).unwrap();
    assert_eq!(plain.engine.session().edit.zoom.scroll, 0);
}

#[test]
fn a_loop_strip_drag_reaches_the_zone_and_stops_at_its_start() {
    use crate::loopbar::{Grab, drag_range};
    let snap = |at: Samples| (at as f64 / 100.0).round() as Samples * 100;
    assert_eq!(drag_range(Grab::Create { anchor: -1_030 }, 480, -5_000, snap), Range::new(-1_000, 500));
    assert_eq!(drag_range(Grab::Left { end: 800 }, -9_000, -5_000, snap), Range::new(-5_000, 800));
    let body = Grab::Body { start: 300, end: 800, press: 500 };
    assert_eq!(drag_range(body, -1_000, -5_000, snap), Range::new(-1_200, -700), "a body moves into the zone");
    assert_eq!(drag_range(body, -9_000, -5_000, snap), Range::new(-5_000, -4_500), "and stops at its start, length kept");
}

#[test]
fn a_start_trim_drag_on_a_clip_wholly_before_bar_1_does_not_panic() {
    // Start trims keep their bar-1 floor until the hidden-content floor (ADR 0052, Slices 6); the
    // preview must not panic on a clip that ends before bar 1 meanwhile.
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, _, ids) = zone_app(&[(-2 * BAR, BAR)]);
    framed(&mut app, &ctx, &mut time, 1024.0, -2 * BAR);
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == ids[0].0).unwrap().rect;
    let edge = (rect[0] + 1.0, (rect[1] + rect[3]) / 2.0 + 6.0);
    hold_drag(&mut app, &ctx, &mut time, edge, (edge.0 + 10.0, edge.1), egui::Modifiers::NONE);
    assert!(
        matches!(app.arrange.gesture, Some(crate::arrange::Gesture::Clip { kind: crate::arrange::DragKind::TrimStart, .. })),
        "{:?}",
        app.arrange.gesture
    );
}

#[test]
fn a_view_left_before_a_zone_that_has_gone_shows_bar_1() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, t, ids) = zone_app(&[(-2 * BAR, BAR), (BAR, BAR)]);
    framed(&mut app, &ctx, &mut time, 400.0, -2 * BAR);
    assert_eq!(app.arrange.layout.scroll, -2 * BAR);
    app.run("edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": -BAR, "end": -BAR})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert_eq!(app.engine.session().edit.zoom.scroll, -2 * BAR, "the stored scroll is not rewritten");
    assert_eq!(app.arrange.layout.scroll, 0, "floored when read: the view starts at bar 1");
    app.run("edit.undo", json!({})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert_eq!(app.arrange.layout.scroll, -2 * BAR, "undo brings the zone and the view back");
}

#[test]
fn the_bounce_dialogs_whole_song_starts_at_the_session_start() {
    let dir = transit_test_support::TestDir::new("pre-count-bounce");
    std::fs::create_dir_all(&dir).unwrap();
    for (clips, want) in [(vec![(-BAR, BAR), (BAR, BAR)], -BAR), (vec![(BAR, BAR)], 0)] {
        let (mut app, ..) = zone_app(&clips);
        app.open_bounce();
        let f = app.finish.bounce_form.as_mut().unwrap();
        f.range = crate::finish::BounceRange::Whole;
        f.path = dir.join(format!("mix{want}.wav")).to_string_lossy().into_owned();
        app.start_bounce().unwrap();
        let r = app.finish.job.as_ref().expect("the bounce runs").range();
        assert_eq!((r.start, r.end), (want, 2 * BAR), "zone clips {clips:?}");
        app.finish.job = None;
    }
}

#[test]
fn a_group_drag_previews_where_the_group_stops() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    // The first clip sets the session start at -BAR; the second and third are dragged together.
    let (mut app, _, ids) = zone_app(&[(-BAR, BEAT), (0, BEAT), (2 * BAR, BEAT)]);
    framed(&mut app, &ctx, &mut time, 1024.0, -2 * BAR);
    app.run("edit.select", json!({"clips": [ids[1].0, ids[2].0]})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == ids[2].0).unwrap().rect;
    let grab = (rect[0] + 12.0, (rect[1] + rect[3]) / 2.0 + 6.0);
    let px_per_beat = (BEAT as f64 / app.arrange.layout.samples_per_px) as f32;
    // Ten beats back: the clip at bar 1 reaches the session start after four.
    hold_drag(&mut app, &ctx, &mut time, grab, (grab.0 - 10.0 * px_per_beat, grab.1), egui::Modifiers::NONE);
    let ghost = |id: ClipId| app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).map(|g| g.start);
    assert_eq!((ghost(ids[1]), ghost(ids[2])), (Some(-BAR), Some(2 * BAR - BAR)), "the group stops together at the session start");
}
