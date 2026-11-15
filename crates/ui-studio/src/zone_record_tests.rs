//! Record from and into the pre-count zone (ADR 0052 decisions 7 and 8, slice 7): a cursor in the
//! zone, the pre-roll and the cycle range start no earlier than the session start, and the beat
//! pulse follows the transport in the zone. No audio device: the plan is read before any
//! capture, and the count-in is the silent frame-clock one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::{Services, StudioApp};
use serde_json::json;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, Session, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz (the default session).
const BAR: Samples = 96_000;
const BEAT: Samples = BAR / 4;

/// A studio with an armed MIDI track and, when `zone` is set, a clip at `zone` (before bar 1).
fn app(zone: Option<Samples>) -> StudioApp {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    if let Some(start) = zone {
        let id = s.new_clip_id();
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: Q });
        transit_engine::edit::place_clip(&mut s, t, Clip::midi(id, "zone", start, BAR, seq));
    }
    let mut app = StudioApp::new(transit_engine::Engine::new(s), None, Services::default());
    app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
    app.ui.count_in = true;
    app.ui.count_in_bars = 1;
    app
}

fn planned(app: &StudioApp) -> (Samples, Option<Samples>, Option<Samples>) {
    let plan = app.take_plan.as_ref().expect("a pass is planned");
    (plan.start, plan.pass, plan.count_in)
}

/// A cursor in the zone records from there, with the whole count-in before it (it used to start
/// at bar 1).
#[test]
fn record_from_a_cursor_in_the_zone_starts_there_with_its_count_in() {
    let mut app = app(Some(-BAR));
    app.set_cursor(-2 * BEAT, None);
    app.begin_pass(Vec::new());
    assert_eq!(planned(&app), (-2 * BEAT, None, Some(BAR)), "the record start and the count-in before it");
    assert_eq!(app.recording_pass_inspect()["capture_start"], -2 * BEAT - BAR);
}

/// Pre-roll from bar 1 reaches into the zone when there is one, and stops at bar 1 without one.
#[test]
fn the_pre_roll_from_bar_1_starts_at_the_session_start() {
    for (zone, from) in [(Some(-BAR), -BEAT), (None, 0)] {
        let mut app = app(zone);
        app.ui.count_in = false;
        let e = &mut app.engine.session_mut().edit;
        e.pre_post_roll = true;
        e.pre_roll = BEAT;
        app.set_cursor(0, None);
        app.begin_pass(Vec::new());
        assert_eq!(planned(&app), (from, None, Some(0)), "zone {zone:?}");
    }
}

/// A stored cycle range that starts before the session start (its zone has gone) records from
/// the session start in laps of what is left, as Play loops it; one that ends there is no cycle.
/// With a zone, a cycle range in it records from its start.
#[test]
fn cycle_recording_reads_the_cycle_range_from_the_session_start() {
    let mut app = app(None);
    let e = &mut app.engine.session_mut().edit;
    e.loop_playback = true;
    e.loop_range = Range::new(-BAR, BAR);
    app.begin_pass(Vec::new());
    assert_eq!(planned(&app), (0, Some(BAR), Some(BAR)));
    assert_eq!(app.take_cycle, Some(Range::new(0, BAR)), "the laps are cut by the loop Play loops");

    let mut app = self::app(None);
    let e = &mut app.engine.session_mut().edit;
    e.loop_playback = true;
    e.loop_range = Range::new(-BAR, -BEAT);
    app.set_cursor(BAR, None);
    app.begin_pass(Vec::new());
    assert_eq!(planned(&app), (BAR, None, Some(BAR)), "nothing left of the cycle: from the cursor");

    let mut app = self::app(Some(-BAR));
    let e = &mut app.engine.session_mut().edit;
    e.loop_playback = true;
    e.loop_range = Range::new(-BAR, BAR);
    app.begin_pass(Vec::new());
    assert_eq!(planned(&app), (-BAR, Some(2 * BAR), Some(BAR)), "a cycle in the zone");
}

/// Record pressed while cycling at the floored loop start loop-records in whole laps.
#[test]
fn record_while_cycling_at_the_floored_loop_start_records_in_passes() {
    let mut app = app(None);
    let e = &mut app.engine.session_mut().edit;
    e.loop_playback = true;
    e.loop_range = Range::new(-BAR, BAR);
    app.engine.transport.playing = true;
    app.engine.transport.position = 0;
    app.begin_pass(Vec::new());
    assert_eq!(planned(&app), (0, Some(BAR), Some(0)));
}

/// While playing in the zone the beat pulse follows the transport (it sat on bar 1's downbeat).
#[test]
fn the_beat_pulse_follows_the_transport_in_the_zone() {
    let mut app = app(Some(-BAR));
    app.engine.transport.playing = true;
    app.engine.transport.position = -BEAT / 2;
    assert_eq!(app.beat_phase(), Some((0.5, false)), "half way through the zone's last beat");
}

/// Audio recorded before bar 1 that is not in its clip ends the status line, as the lap notes do
/// (Stop and crash recovery both report through it).
#[test]
fn audio_left_before_bar_1_is_said_on_the_status_line() {
    let mut app = app(None);
    app.status = "Recorded 1 clip (2.0 s) on Audio 1".into();
    let mark = app.lap_mark();
    app.engine.warn(transit_engine::io::before_bar_one_note(0.5, "Vox_01.wav"));
    app.report_lap_notes(mark);
    assert_eq!(
        app.status,
        "Recorded 1 clip (2.0 s) on Audio 1. Audio recorded before bar 1 is not in the clip (audio clips start at bar 1): its first 0.50 s stay in Vox_01.wav."
    );
}
