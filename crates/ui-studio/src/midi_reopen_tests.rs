#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A reopen of the audio device keeps the open MIDI inputs open (#760): the real app frame loop
//! over real Players on the fake MIDI backend, through the one reopen seam
//! (`audio_connection::replace`) that Apply, both Reconnects and the automatic reopen use.
use crate::{Services, StudioApp};
use serde_json::json;
use std::time::{Duration, Instant};
use transit_midi_io::fake::Fake;
use transit_model::{Mapping, MidiSource, SourceKind, TrackId, TransportControl, TransportPrefs};

/// A device-free Player whose MIDI side runs on `fake`.
fn player(fake: &Fake, session: std::sync::Arc<transit_model::Session>) -> transit_playback::Player {
    let mut player = transit_playback::Player::prepare_device_free(session).unwrap();
    player.enable_midi_for_test(fake.io());
    player
}

/// One frame of the app as the host runs it, a little apart in real time so the ingress worker
/// and the MIDI device thread can run.
fn frame(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64) {
    *time += 1.0 / 60.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
        time: Some(*time),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    // A block of the device-free Player: its plan (with the instrument a route needs) is adopted
    // on the audio side.
    if let Some(player) = &app.player {
        player.step_device_free();
    }
    std::thread::sleep(Duration::from_millis(2));
}

#[test]
fn reconnecting_the_audio_device_keeps_the_open_midi_input_open() {
    let fake = Fake::default();
    let key = fake.add_input("KeyStep Pro MIDI IN");
    let engine = transit_engine::Engine::default();
    let first = player(&fake, engine.session_arc());
    let mut app = StudioApp::new(engine, Some(first), Services::default());
    let (ctx, mut time) = (egui::Context::default(), 0.0);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let track = TrackId(r["tracks"][0].as_u64().unwrap());
    app.run("mix.record_arm", json!({"tracks": [track.0], "value": true})).unwrap();
    let routed = |app: &StudioApp| app.player.as_ref().and_then(|p| p.midi_status().ok()).is_some_and(|s| s.open_inputs == 1 && s.routes == 1);
    // timing: stall-bound ten seconds catches a stuck worker or port; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while !routed(&app) {
        assert!(Instant::now() < deadline, "the armed track's input did not open (status: {})", app.status);
        frame(&mut app, &ctx, &mut time);
    }
    assert_eq!(fake.input_opens(&key), 1);
    let again = fake.clone();
    app.reopen_with = Some(Box::new(move |session, _cfg| player(&again, session)));
    app.reconnect_with_player(false).unwrap();
    // timing: stall-bound ten seconds catches a stuck worker or port; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while !routed(&app) {
        assert!(Instant::now() < deadline, "the input is not routed on the reopened device (status: {})", app.status);
        frame(&mut app, &ctx, &mut time);
    }
    assert_eq!(fake.input_opens(&key), 1, "the reopen closed and reopened the MIDI input");
}

/// A learned transport button held down before the reopen and let go while no device was open: its
/// release is not acted on, and the first press after the reopen acts (the held latch is cleared at
/// the handover, #760 review round 2).
#[test]
fn a_learned_button_released_during_the_reopen_acts_on_its_next_press() {
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let port = fake.input_port(&key).unwrap();
    let engine = transit_engine::Engine::default();
    let first = player(&fake, engine.session_arc());
    let mut app = StudioApp::new(engine, Some(first), Services::default());
    let (ctx, mut time) = (egui::Context::default(), 0.0);
    // The controller's profile (machine preferences): CC 20 on channel 1 toggles Cycle.
    let source = MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 20 };
    let prefs = TransportPrefs { mappings: vec![Mapping::new(source, TransportControl::Cycle.target())], ..Default::default() };
    app.ui.midi_transport.insert("Controller".into(), prefs);
    let open = |app: &StudioApp| app.player.as_ref().and_then(|p| p.midi_status().ok()).is_some_and(|s| s.open_inputs == 1);
    // timing: stall-bound ten seconds catches a stuck worker or port; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while !open(&app) {
        assert!(Instant::now() < deadline, "the controller's port did not open (status: {})", app.status);
        frame(&mut app, &ctx, &mut time);
    }
    // timing: stall-bound ten seconds catches a stuck worker or port; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.engine.session().edit.loop_playback {
        assert!(Instant::now() < deadline, "the press before the reopen did not act (status: {})", app.status);
        assert!(port.deliver(0, &[0xB0, 20, 127]));
        frame(&mut app, &ctx, &mut time);
    }
    let again = fake.clone();
    let release = port.clone();
    app.reopen_with = Some(Box::new(move |session, _cfg| {
        assert!(release.deliver(0, &[0xB0, 20, 0]), "the release, while no device is open");
        player(&again, session)
    }));
    app.reconnect_with_player(false).unwrap();
    assert!(app.engine.session().edit.loop_playback);
    // timing: stall-bound ten seconds catches a stuck worker or port; success always requires the named state
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.engine.session().edit.loop_playback {
        assert!(Instant::now() < deadline, "the first press after the reopen was swallowed (status: {})", app.status);
        assert!(port.deliver(0, &[0xB0, 20, 127]));
        frame(&mut app, &ctx, &mut time);
    }
    assert_eq!(fake.input_opens(&key), 1);
}
