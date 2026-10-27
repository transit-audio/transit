use super::*;

#[test]
fn new_session_stops_playback_and_the_old_clock() {
    for (command, params) in [
        ("session.new", json!({"name": "New project", "sample_rate": 96_000})),
        ("session.new", json!({"template": "demo"})),
        ("session.close", json!({})),
    ] {
        let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({"from": 48_000})).unwrap();
        app.handle_transport(0.25);
        assert!(app.is_playing());
        assert!(app.position() > 48_000);
        app.run(command, params).unwrap();
        let insertion = app.engine.session().edit.selection.start;
        app.handle_transport(0.25);
        assert!(!app.is_playing(), "{command}");
        assert_eq!(app.position(), insertion, "{command}");
        assert_eq!(app.engine.transport.position, insertion, "{command}");
        app.handle_transport(0.25);
        assert_eq!(app.engine.transport.position, insertion, "the old clock must stay stopped");
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(app.is_playing());
        assert_eq!(app.position(), insertion + i64::from(app.engine.session().sample_rate.hz()) / 4);
    }
}

#[test]
fn new_session_cancels_play_queued_before_the_next_frame() {
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({})).unwrap();
    app.run("session.new", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    assert_eq!(app.engine.transport.position, 0);
}

#[test]
fn session_reset_does_not_apply_old_stop_edits_to_the_new_session() {
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({"from": 48_000})).unwrap();
    app.handle_transport(0.25);
    let mut session = transit_model::Session::default();
    session.edit.selection = transit_time::Range::new(12_000, 24_000);
    session.edit.insertion_follows_playback = true;
    session.edit.scrolling = "after_playback".into();
    let selection = session.edit.selection;
    app.engine.replace_session(session);
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    assert_eq!(app.engine.session().edit.selection, selection);
    assert_eq!(app.position(), selection.start);
    assert_eq!(app.engine.session().edit.zoom.scroll, 0);
    assert!(!app.engine.is_dirty());
}

#[test]
#[ignore = "requires a real default audio device; opt in only on a test audio machine"]
fn ordinary_play_after_stop_is_full_speed() {
    use serde_json::json;
    let engine = transit_engine::Engine::default();
    let player = transit_playback::Player::new(engine.session_arc());
    let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
    app.engine.execute("edit.select", &json!({"start": 0, "end": 48000})).unwrap();
    app.engine.execute("options.loop_playback", &json!({"value": true})).unwrap();
    app.engine.execute("options.pre_post_roll", &json!({"value": false})).unwrap();
    let speed = |app: &super::SoundApp| app.player.as_ref().map(|p| p.speed()).unwrap_or(0.0);
    let settle = |app: &mut super::SoundApp, playing: bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            app.apply_editor_edits();
            app.poll_player();
            app.handle_transport(1.0 / 60.0);
            if !app.player_requests.pending() && app.is_playing() == playing {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{}", app.ui.status);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    assert!((speed(&app) - 1.0).abs() < 1e-3);

    app.engine.execute("transport.half_speed", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    settle(&mut app, true);
    assert!(app.is_playing());
    assert!((speed(&app) - 0.5).abs() < 1e-3, "half-speed playback");

    app.engine.execute("transport.stop", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    settle(&mut app, false);
    assert!(!app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "stop restores full speed");

    app.engine.execute("transport.play", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    settle(&mut app, true);
    assert!(app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "ordinary play stays at full speed");

    app.engine.execute("transport.stop", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    app.engine.execute("options.loop_playback", &json!({"value": false})).unwrap();
    app.engine.execute("transport.half_speed", &json!({})).unwrap();
    app.handle_transport(1.0 / 60.0);
    settle(&mut app, true);
    assert!((speed(&app) - 0.5).abs() < 1e-3);
    app.player.as_ref().unwrap().stop().unwrap();
    app.handle_transport(1.0 / 60.0);
    settle(&mut app, false);
    assert!(!app.is_playing());
    assert!((speed(&app) - 1.0).abs() < 1e-3, "a half-speed pass that ends on its own restores full speed");
}
