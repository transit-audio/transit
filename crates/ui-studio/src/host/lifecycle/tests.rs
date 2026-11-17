//! ADR 0062 slice S1: the derived transport phase. The derivation precedence on plain
//! observations, the state texts, and agreement with today's status text and flags through the
//! device-free Player (each step renders one block, then runs the host loop's transport part).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{Services, StudioApp};
use serde_json::json;
use transit_engine::Engine;
use transit_model::TrackId;
use transit_playback::CountIn;
use transit_playback::record::start::{Point, Ticket};
use transit_playback::timeline_map::Position;

const PLAN: CountIn = CountIn { frames: 96_000, beat: 24_000, per_bar: 4 };

fn ticket() -> Ticket {
    let (mut control, _publisher) = transit_playback::record::start::pair(transit_mix::EngineEpoch::FIRST);
    control.prepare().unwrap().ticket()
}

fn point() -> Point {
    Point { host_ns: 1, output_frame: 2, position: Position { sample: 3, fraction: 0, denominator: 1 } }
}

fn pass(latch: Option<Point>, admission_pending: bool, count_in: Option<CountIn>) -> PassObservation {
    PassObservation { ticket: ticket(), from: Some(0), count_in, latch, admission_pending }
}

fn derive(o: Observation) -> TransportPhase {
    TransportPhase::derive(&o)
}

#[test]
fn derived_phase_puts_an_active_render_first() {
    let everything = Observation {
        render_active: true,
        stop_tracked: true,
        stop_reason: StopReason::Reset,
        resume_at: Some(5),
        pass: Some(pass(Some(point()), true, Some(PLAN))),
        record_off_tracked: true,
        count_in_observed: true,
        play_start_tracked: true,
        playing: true,
    };
    assert_eq!(derive(everything), TransportPhase::Rendering { job: RenderJob::Live });
    let o = Observation { render_active: false, ..everything };
    assert_eq!(derive(o), TransportPhase::StopPending { then: AfterStop::Reset }, "a pending Stop comes next");
}

#[test]
fn derived_phase_never_reads_a_pending_reset_as_park() {
    let stop = |reason, resume_at| derive(Observation { stop_tracked: true, stop_reason: reason, resume_at, playing: true, ..Default::default() });
    assert_eq!(stop(StopReason::Reset, None), TransportPhase::StopPending { then: AfterStop::Reset });
    assert_eq!(stop(StopReason::Reset, Some(7)), TransportPhase::StopPending { then: AfterStop::Reset }, "Reset supersedes Resume");
    assert_eq!(stop(StopReason::Locate, Some(7)), TransportPhase::StopPending { then: AfterStop::Resume(7) });
    assert_eq!(stop(StopReason::User, None), TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } });
    // A take-safe locate whose resume Stop was dropped (Stop or Record cleared it) parks.
    assert_eq!(stop(StopReason::Locate, None), TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } });
    // The resume is still owed after its Stop retired, until the pass restarts.
    let owed = derive(Observation { stop_reason: StopReason::Locate, resume_at: Some(7), ..Default::default() });
    assert_eq!(owed, TransportPhase::StopPending { then: AfterStop::Resume(7) });
    // The reason is kept: a Stop or locate required while a Reset's Stop is tracked stays Reset.
    let mut meta = RequestMeta::default();
    meta.stop_required(StopReason::Reset, false);
    meta.stop_required(StopReason::User, true);
    meta.stop_required(StopReason::Locate, true);
    assert_eq!(meta.stop_reason, StopReason::Reset);
    meta.stop_required(StopReason::User, false);
    assert_eq!(meta.stop_reason, StopReason::User, "once that Stop retired, a new Stop is the user's");
}

#[test]
fn derived_phase_orders_the_pass_by_latch_count_in_admission_then_clock_wait() {
    let o =
        |pass, count_in_observed| Observation { pass: Some(pass), count_in_observed, play_start_tracked: true, playing: true, ..Default::default() };
    let p = pass(Some(point()), true, Some(PLAN));
    assert_eq!(derive(o(p, true)), TransportPhase::Recording { latch: point(), count_in: Some(PLAN) }, "the Started latch first");
    let p = pass(None, true, Some(PLAN));
    assert_eq!(derive(o(p, true)), TransportPhase::CountIn { ticket: p.ticket, from: 0, plan: PLAN }, "then an observed count-in");
    assert_eq!(derive(o(p, false)), TransportPhase::RecordPending { ticket: p.ticket, from: Some(0), count_in: Some(PLAN) });
    let p = pass(None, false, None);
    assert_eq!(derive(o(p, false)), TransportPhase::WaitingForClock { ticket: p.ticket, from: Some(0), count_in: None });
    // An observed count-in the pass did not send (no plan) is no count-in phase.
    assert_eq!(derive(o(p, true)), TransportPhase::WaitingForClock { ticket: p.ticket, from: Some(0), count_in: None });
}

#[test]
fn derived_phase_record_off_alone_is_not_a_pass() {
    let off = Observation { record_off_tracked: true, ..Default::default() };
    assert_eq!(derive(off), TransportPhase::Stopped);
    assert_eq!(derive(Observation { playing: true, ..off }), TransportPhase::Playing);
}

#[test]
fn derived_phase_takes_a_play_group_then_playback_then_stopped() {
    let o = Observation { play_start_tracked: true, playing: true, ..Default::default() };
    assert_eq!(derive(o), TransportPhase::StartPending);
    assert_eq!(derive(Observation { play_start_tracked: false, ..o }), TransportPhase::Playing);
    assert_eq!(derive(Observation::default()), TransportPhase::Stopped);
}

#[test]
fn state_text_names_the_waits() {
    let t = ticket();
    let text = |phase| state_text(&phase, false);
    assert_eq!(text(TransportPhase::StartPending), Some("Play pending"));
    assert_eq!(text(TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: Some(PLAN) }), Some("Count-in"));
    assert_eq!(text(TransportPhase::RecordPending { ticket: t, from: None, count_in: None }), Some("Record pending"));
    assert_eq!(text(TransportPhase::CountIn { ticket: t, from: 0, plan: PLAN }), Some("Count-in"));
    assert_eq!(text(TransportPhase::WaitingForClock { ticket: t, from: None, count_in: None }), Some("Record pending"));
    assert_eq!(text(TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } }), Some("Stop pending"));
    assert_eq!(text(TransportPhase::StopPending { then: AfterStop::Resume(3) }), Some("Stop pending"));
    for quiet in [
        TransportPhase::Stopped,
        TransportPhase::Playing,
        TransportPhase::Recording { latch: point(), count_in: None },
        TransportPhase::StopPending { then: AfterStop::Reset },
        TransportPhase::Rendering { job: RenderJob::Live },
    ] {
        assert_eq!(text(quiet), None, "{quiet:?}");
        assert_eq!(state_text(&quiet, true), Some("All MIDI notes off pending"), "{quiet:?}");
    }
    assert_eq!(state_text(&TransportPhase::StartPending, true), Some("Play pending"), "the phase's text outranks the release");
}

/// The device-free host, with one armed MIDI track; its scratch folder stays alive with it.
fn app() -> (StudioApp, transit_test_support::TestDir) {
    let dir = transit_test_support::TestDir::new("transport-phase");
    let engine = Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    app.autosave_dir = Some(dir.join("Autosave"));
    let t = TrackId(app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
    for _ in 0..3 {
        step(&mut app);
    }
    (app, dir)
}

/// One audio block, then the host loop's transport part in `logic`'s order.
fn step(app: &mut StudioApp) {
    app.player.as_ref().unwrap().step_device_free();
    app.apply_editor_edits();
    app.poll_player();
    app.handle_transport(0.0);
    if app.player_requests.play.is_none() && app.player_requests.locate.is_none() && app.player_requests.recording.is_none() {
        app.sync_player_session();
    }
}

/// The derived phase, checked against today's flags and status text.
fn agreed(app: &StudioApp) -> TransportPhase {
    let phase = app.transport_phase();
    let (status, why) = (app.status.as_str(), format!("{phase:?} with {:?}", app.status));
    let stopping = app.midi_stop_pending() || app.midi_transport.take_locate.is_some();
    match phase {
        TransportPhase::Stopped => {
            assert!(!app.is_playing() && !app.play_pending() && !stopping && app.take_plan.is_none(), "{why}");
        }
        TransportPhase::StartPending => {
            assert!(app.play_pending() && !app.is_playing() && app.take_plan.is_none(), "{why}");
            assert_eq!(status, "Play pending", "{why}");
        }
        TransportPhase::Playing => {
            assert!(app.is_playing() && !app.play_pending() && !stopping && app.take_plan.is_none(), "{why}");
        }
        TransportPhase::RecordPending { count_in, .. } => {
            assert!(app.recording_active() && app.player_requests.recording.is_some(), "{why}");
            assert_eq!(status, if count_in.is_some() { "Count-in" } else { "Record pending" }, "{why}");
        }
        TransportPhase::CountIn { .. } => {
            assert!(app.recording_active() && app.count_in_state().is_some(), "{why}");
            assert_eq!(status, "Count-in", "{why}");
        }
        TransportPhase::WaitingForClock { .. } => {
            // Today the clock wait already reads as recording (ADR 0062 B1 changes that in S2d).
            assert!(app.recording_active() && app.engine.transport.recording, "{why}");
            assert_eq!(status, "Recording", "{why}");
        }
        TransportPhase::Recording { .. } => {
            assert!(app.recording_active() && app.engine.transport.recording && app.is_playing(), "{why}");
            assert_eq!(status, "Recording", "{why}");
        }
        // The text while a Stop is pending is whatever was written last (a placed take's report
        // follows "Stop pending"); the sequences check it where it is known.
        TransportPhase::StopPending { .. } => assert!(stopping, "{why}"),
        TransportPhase::Rendering { .. } => assert!(app.finish.realtime.running.is_some(), "{why}"),
    }
    phase
}

/// The phase's name without its data.
fn name(phase: TransportPhase) -> &'static str {
    match phase {
        TransportPhase::Stopped => "Stopped",
        TransportPhase::StartPending => "StartPending",
        TransportPhase::Playing => "Playing",
        TransportPhase::RecordPending { .. } => "RecordPending",
        TransportPhase::CountIn { .. } => "CountIn",
        TransportPhase::WaitingForClock { .. } => "WaitingForClock",
        TransportPhase::Recording { .. } => "Recording",
        TransportPhase::StopPending { then: AfterStop::Park { .. } } => "StopPending Park",
        TransportPhase::StopPending { then: AfterStop::Resume(_) } => "StopPending Resume",
        TransportPhase::StopPending { then: AfterStop::Reset } => "StopPending Reset",
        TransportPhase::Rendering { .. } => "Rendering",
    }
}

/// Step until `done` (at most `limit` blocks), checking agreement after the command and after
/// every step; the phases seen, each run of one phase once.
fn run_until(app: &mut StudioApp, seen: &mut Vec<&'static str>, limit: usize, done: impl Fn(&StudioApp) -> bool) {
    let mut note = |phase| {
        let n = name(phase);
        if seen.last() != Some(&n) {
            seen.push(n);
        }
    };
    note(agreed(app));
    for _ in 0..limit {
        if done(app) {
            return;
        }
        step(app);
        note(agreed(app));
    }
    assert!(done(app), "not reached in {limit} blocks: {seen:?}, {:?}", app.status);
}

fn is(want: &'static str) -> impl Fn(&StudioApp) -> bool {
    move |app| name(app.transport_phase()) == want
}

#[test]
fn derived_phase_agrees_with_todays_text_and_flags_for_play_and_stop() {
    let (mut app, _dir) = app();
    let mut seen = Vec::new();
    app.run("transport.play", json!({})).unwrap();
    run_until(&mut app, &mut seen, 20, is("Playing"));
    app.run("transport.stop", json!({})).unwrap();
    step(&mut app);
    assert_eq!(name(agreed(&app)), "StopPending Park");
    assert_eq!(app.status, "Stop pending");
    run_until(&mut app, &mut seen, 20, is("Stopped"));
    assert_eq!(seen, ["Stopped", "StartPending", "Playing", "StopPending Park", "Stopped"]);
    assert_eq!(app.status, "Stopped");
}

#[test]
fn derived_phase_agrees_with_todays_text_and_flags_through_a_count_in() {
    let (mut app, _dir) = app();
    app.ui.count_in = true;
    app.ui.count_in_bars = 1;
    let mut seen = Vec::new();
    app.run("transport.record", json!({})).unwrap();
    run_until(&mut app, &mut seen, 400, is("Recording"));
    let phase = app.transport_phase();
    assert!(matches!(phase, TransportPhase::Recording { count_in: Some(c), .. } if c.frames > 0), "{phase:?}");
    app.run("transport.stop", json!({})).unwrap();
    run_until(&mut app, &mut seen, 20, |a| is("Stopped")(a) && a.player_requests.recording.is_none());
    assert_eq!(seen, ["Stopped", "RecordPending", "CountIn", "Recording", "StopPending Park", "Stopped"]);
}

#[test]
fn derived_phase_agrees_with_todays_text_and_flags_without_a_count_in() {
    let (mut app, _dir) = app();
    let mut seen = Vec::new();
    app.run("transport.record", json!({})).unwrap();
    run_until(&mut app, &mut seen, 400, is("Recording"));
    app.run("transport.stop", json!({})).unwrap();
    // A record-off still tracked after the Stop completed is no pass: the phase is Stopped.
    run_until(&mut app, &mut seen, 20, is("Stopped"));
    assert_eq!(seen.first(), Some(&"Stopped"));
    assert_eq!(seen[1], "RecordPending");
    assert_eq!(&seen[seen.len() - 3..], ["Recording", "StopPending Park", "Stopped"]);
    assert!(seen.iter().all(|n| *n != "CountIn"), "{seen:?}");
}

#[test]
fn derived_phase_agrees_with_todays_text_and_flags_for_a_take_safe_locate() {
    let (mut app, _dir) = app();
    let mut seen = Vec::new();
    app.run("transport.record", json!({})).unwrap();
    run_until(&mut app, &mut seen, 400, is("Recording"));
    app.run("transport.locate", json!({"at": 96_000})).unwrap();
    run_until(&mut app, &mut seen, 20, is("StopPending Resume"));
    run_until(&mut app, &mut seen, 400, is("Recording"));
    let resumed = seen.iter().position(|n| *n == "StopPending Resume").expect("the locate finalizes, then resumes");
    assert_eq!(seen.last(), Some(&"Recording"), "{seen:?}");
    assert!(seen[resumed..].iter().all(|n| *n != "StopPending Park" && *n != "Stopped"), "never read as a park: {seen:?}");
}

#[test]
fn derived_phase_agrees_with_todays_text_and_flags_when_the_session_is_replaced() {
    let (mut app, _dir) = app();
    let mut seen = Vec::new();
    app.run("transport.play", json!({})).unwrap();
    run_until(&mut app, &mut seen, 20, is("Playing"));
    app.engine.execute("session.new", &json!({})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(name(agreed(&app)), "StopPending Reset");
    // A Stop pressed while the replaced session's Stop is pending keeps it a Reset.
    app.run("transport.stop", json!({})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(name(agreed(&app)), "StopPending Reset");
    run_until(&mut app, &mut seen, 20, is("Stopped"));
    assert_eq!(seen, ["Stopped", "StartPending", "Playing", "StopPending Reset", "Stopped"]);
}
