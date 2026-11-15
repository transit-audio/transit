//! The hidden-content trim floor in the arranger (ADR 0052 decision 7, slice 6b): the start-trim
//! drag, its preview and `edit.trim_clip` stop at the same floor (one engine function), with Snap
//! on and off; the end-trim drag and the loop handle of a clip ending before bar 1 stop at the
//! session start instead of lengthening the clip to bar 1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::tests::{drag, frames, hold_drag, release_at};
use crate::{Services, StudioApp};
use egui::Modifiers;
use serde_json::json;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, ClipLoop, Session, TrackKind};
use transit_time::{Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (the default session).
const BAR: Samples = 96_000;
const BEAT: Samples = BAR / 4;
/// Far enough left that only the floor stops a trim back.
const FAR: Samples = -10 * BAR;

fn note(pitch: u8, start: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length: Q }
}

/// A count-in take: a pedal press at its start (tick 0), then notes before and after bar 1.
fn take() -> Sequence {
    Sequence {
        notes: vec![note(48, Q / 4), note(52, 2 * Q), note(60, 4 * Q)],
        ctrls: vec![CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(64), value: 127 }],
    }
}

/// A studio over `s`, where each `(start, length, sequence, looping)` is a MIDI clip on its own MIDI
/// track; the first is the one the tests trim.
fn studio(mut s: Session, clips: Vec<(Samples, Samples, Sequence, Option<ClipLoop>)>) -> (StudioApp, ClipId) {
    let mut ids = Vec::new();
    for (start, length, seq, looping) in clips {
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        let mut c = Clip::midi(id, "m", start, length, seq);
        c.looping = looping;
        transit_engine::edit::place_clip(&mut s, t, c);
        ids.push(id);
    }
    (StudioApp::new(transit_engine::Engine::new(s), None, Services::default()), ids[0])
}

fn the_clip(app: &StudioApp, id: ClipId) -> Clip {
    app.engine.session().find_clip(id).unwrap().1.clone()
}

/// One case of the matrix: a fresh studio, the clip to trim, where its start may reach, and
/// whether reaching it gives back the clip as it was before its first trim.
struct Case {
    name: &'static str,
    build: fn() -> (StudioApp, ClipId, Option<Clip>),
    floor: Samples,
}

/// `id` trimmed to `at` by the command, after keeping a copy of it as it was.
fn trimmed(mut app: StudioApp, id: ClipId, at: Samples) -> (StudioApp, ClipId, Option<Clip>) {
    let original = the_clip(&app, id);
    app.run("edit.trim_clip", json!({"clip": id.0, "start": at})).unwrap();
    (app, id, Some(original))
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no zone, nothing hidden: bar 1",
            build: || {
                let (app, id) = studio(Session::default(), vec![(2 * BAR, BAR, take(), None)]);
                (app, id, None)
            },
            floor: 0,
        },
        Case {
            name: "a count-in take trimmed to bar 1: its pedal press",
            build: || {
                let (app, id) = studio(Session::default(), vec![(-BAR, 3 * BAR, take(), None)]);
                trimmed(app, id, 0)
            },
            floor: -BAR,
        },
        Case {
            name: "the take off the grid: exactly its pedal press, Snap or not",
            build: || {
                let (app, id) = studio(Session::default(), vec![(-BAR - 7_000, 3 * BAR, take(), None)]);
                trimmed(app, id, 0)
            },
            floor: -BAR - 7_000,
        },
        Case {
            name: "a longer zone from another track: the session start",
            build: || {
                let (app, id) = studio(Session::default(), vec![(-BAR, 3 * BAR, take(), None), (-3 * BAR, BAR, Sequence::default(), None)]);
                let (app, id, _) = trimmed(app, id, 0);
                (app, id, None)
            },
            floor: -3 * BAR,
        },
        Case {
            name: "a looped clip: its negative ticks are loop material, the session start",
            build: || {
                let seq = Sequence { notes: vec![note(55, -Q), note(60, 0)], ctrls: Vec::new() };
                let (app, id) = studio(Session::default(), vec![(BAR, 2 * BAR, seq, Some(ClipLoop { start: 0, end: BAR }))]);
                (app, id, None)
            },
            floor: 0,
        },
        Case {
            name: "across a tempo change: found in ticks",
            build: || {
                let mut s = Session::default();
                s.tempo.set_tempo(4 * Q, 60.0).unwrap();
                let seq = Sequence { notes: vec![note(60, 0), note(64, 9_600)], ctrls: Vec::new() };
                let (app, id) = studio(s, vec![(-BEAT, 360_000, seq, None)]);
                trimmed(app, id, 288_000)
            },
            floor: -24_000,
        },
        Case {
            name: "a neighbor on the track ends before the hidden content: the neighbor's end",
            build: || {
                let (app, id) = studio(Session::default(), vec![(-BAR, 3 * BAR, take(), None)]);
                let (mut app, id, _) = trimmed(app, id, 0);
                neighbor(&mut app, id, -3 * BAR, 2 * BAR + BAR / 2);
                (app, id, None)
            },
            floor: -BAR / 2,
        },
    ]
}

/// Put an empty MIDI clip from `start` for `length` on `of`'s track.
fn neighbor(app: &mut StudioApp, of: ClipId, start: Samples, length: Samples) {
    let s = app.engine.session_mut();
    let t = s.find_clip(of).unwrap().0;
    let n = s.new_clip_id();
    transit_engine::edit::place_clip(s, t, Clip::midi(n, "n", start, length, Sequence::default()));
}

/// Drag `id`'s start edge to the window's left edge: (the preview's start while held, the start
/// after the release).
fn drag_start_far_left(app: &mut StudioApp, id: ClipId, m: Modifiers) -> (Samples, Samples) {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    app.run("view.zoom_set", json!({"samples_per_px": 4096.0})).unwrap();
    app.run("view.scroll", json!({"to": FAR})).unwrap();
    frames(app, &ctx, 3, &mut time);
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == id.0).unwrap().rect;
    let edge = (rect[0] + 1.0, (rect[1] + rect[3]) / 2.0 + 6.0);
    let to = (2.0, edge.1);
    // The pointer travels further than the floor is from the edge.
    let reach = app.arrange.layout.samples_per_px * f64::from(edge.0 - to.0);
    assert!(reach > (the_clip(app, id).start + 3 * BAR) as f64, "the drag reaches past every floor in the matrix: {reach}");
    hold_drag(app, &ctx, &mut time, edge, to, m);
    assert!(
        matches!(app.arrange.gesture, Some(crate::arrange::Gesture::Clip { kind: crate::arrange::DragKind::TrimStart, .. })),
        "{:?}",
        app.arrange.gesture
    );
    let ghost = app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).map(|g| g.start).expect("a trim preview");
    release_at(app, &ctx, &mut time, to, m);
    (ghost, the_clip(app, id).start)
}

#[test]
fn the_start_trim_drag_its_preview_and_the_command_stop_at_the_same_floor() {
    for case in cases() {
        // The command alone.
        let (mut app, id, _) = (case.build)();
        app.run("edit.trim_clip", json!({"clip": id.0, "start": FAR})).unwrap();
        assert_eq!(the_clip(&app, id).start, case.floor, "{}: edit.trim_clip", case.name);
        // The drag, with Snap on and with Shift (no snap).
        for m in [Modifiers::NONE, Modifiers::SHIFT] {
            let (mut app, id, original) = (case.build)();
            let floor = crate::arrange::trim_floor(app.engine.session(), id, true);
            assert_eq!(floor, transit_engine::edit::start_trim_floor(app.engine.session(), &the_clip(&app, id)), "{}", case.name);
            let (preview, committed) = drag_start_far_left(&mut app, id, m);
            assert_eq!((preview, committed), (case.floor, case.floor), "{} with {m:?}: preview and drop", case.name);
            if let Some(original) = original {
                assert_eq!(the_clip(&app, id), original, "{} with {m:?}: the clip as it was before its first trim", case.name);
                assert_eq!(app.engine.session().start(), case.floor.min(0), "{}: the session start is where the clip began", case.name);
            }
        }
    }
}

#[test]
fn a_count_in_take_trimmed_to_bar_1_and_dragged_back_brings_the_zone_back() {
    let (mut app, id, original) = (cases()[1].build)();
    assert_eq!(app.engine.session().start(), 0, "trimmed to bar 1, no zone");
    drag_start_far_left(&mut app, id, Modifiers::NONE);
    assert_eq!(app.engine.session().start(), -BAR);
    assert_eq!(Some(the_clip(&app, id)), original);
    // One undo step: back to the trimmed take.
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().start(), 0);
}

/// A studio holding one MIDI clip wholly before bar 1, from two bars before it to one bar before,
/// shown at 1024 samples a point from the session start.
fn zone_clip() -> (StudioApp, ClipId, egui::Context, f64) {
    let (mut app, id) = studio(Session::default(), vec![(-2 * BAR, BAR, take(), None)]);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    app.run("view.zoom_set", json!({"samples_per_px": 1024.0})).unwrap();
    app.run("view.scroll", json!({"to": -2 * BAR})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    (app, id, ctx, time)
}

#[test]
fn an_end_trim_of_a_clip_ending_before_bar_1_stays_before_bar_1() {
    let (mut app, id, ctx, mut time) = zone_clip();
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == id.0).unwrap().rect;
    let px_beat = (BEAT as f64 / app.arrange.layout.samples_per_px) as f32;
    let edge = (rect[2] - 1.0, (rect[1] + rect[3]) / 2.0 + 6.0);
    let to = (edge.0 - px_beat, edge.1);
    hold_drag(&mut app, &ctx, &mut time, edge, to, Modifiers::NONE);
    let ghost = app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).map(|g| (g.start, g.end));
    assert_eq!(ghost, Some((-2 * BAR, -BAR - BEAT)), "the preview shortens the clip by a beat");
    release_at(&mut app, &ctx, &mut time, to, Modifiers::NONE);
    let c = the_clip(&app, id);
    assert_eq!((c.start, c.end()), (-2 * BAR, -BAR - BEAT), "not lengthened to bar 1");
    // Lengthening by a beat from there stays before bar 1 too.
    app.trim_clip(id, false, -BAR).unwrap();
    assert_eq!(the_clip(&app, id).end(), -BAR);
}

#[test]
fn the_loop_handle_of_a_clip_ending_before_bar_1_loops_to_the_drop_point_not_to_bar_1() {
    let (mut app, id, ctx, mut time) = zone_clip();
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == id.0).unwrap().rect;
    let px_beat = (BEAT as f64 / app.arrange.layout.samples_per_px) as f32;
    let handle = (rect[2] - 12.0, rect[1] + 5.0);
    drag(&mut app, handle, (handle.0 + 2.0 * px_beat, handle.1), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    let c = the_clip(&app, id);
    assert_eq!((c.start, c.end()), (-2 * BAR, -BAR + 2 * BEAT), "two beats more, on the grid");
    assert_eq!(c.looping, Some(ClipLoop { start: 0, end: BAR }), "looped over its bar");
}

#[test]
fn an_end_trim_drag_previews_the_neighbor_it_stops_at() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, id) = studio(Session::default(), vec![(0, BAR, take(), None)]);
    neighbor(&mut app, id, 2 * BAR, BAR);
    app.run("view.zoom_set", json!({"samples_per_px": 4096.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == id.0).unwrap().rect;
    let edge = (rect[2] - 1.0, (rect[1] + rect[3]) / 2.0 + 6.0);
    // Far right, past the neighbor at bar 3.
    let to = (1390.0, edge.1);
    assert!(app.arrange.layout.samples_per_px * f64::from(to.0 - edge.0) > (2 * BAR) as f64);
    hold_drag(&mut app, &ctx, &mut time, edge, to, Modifiers::NONE);
    let ghost = app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).map(|g| (g.start, g.end));
    assert_eq!(ghost, Some((0, 2 * BAR)), "the preview stops at the neighbor");
    release_at(&mut app, &ctx, &mut time, to, Modifiers::NONE);
    let c = the_clip(&app, id);
    assert_eq!((c.start, c.end()), (0, 2 * BAR), "where the drop lands");
}

#[test]
fn a_drop_that_brings_the_zone_back_scrolls_the_view_to_the_new_session_start() {
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let (mut app, id, _) = (cases()[1].build)();
    let (_, committed) = drag_start_far_left(&mut app, id, Modifiers::NONE);
    assert_eq!(committed, -BAR);
    frames(&mut app, &ctx, 2, &mut time);
    assert_eq!(app.arrange.layout.scroll, -BAR, "the pickups are in sight, not left of bar 1");
    // A trim that does not move the session start leaves the view where it is.
    let (mut app, id) = studio(Session::default(), vec![(4 * BAR, 2 * BAR, take(), None)]);
    app.run("view.scroll", json!({"to": 2 * BAR})).unwrap();
    app.trim_clip(id, true, 3 * BAR).unwrap();
    assert_eq!(app.engine.session().edit.zoom.scroll, 2 * BAR);
}
