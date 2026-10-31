use super::*;
use transit_playback::PlayerError;

#[test]
fn no_player_capture_keeps_texture_media_and_reports_the_skip() {
    let mut s = transit_model::Session::default();
    let t = s.add_track(transit_model::TrackKind::Audio, transit_model::ChannelFormat::Stereo, None);
    let mut ins = transit_model::Insert::new("texture");
    ins.frozen_recording = Some(transit_model::FrozenRecording { path: "Audio Files/Pad.wav".into(), ..Default::default() });
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    let mut engine = transit_engine::Engine::default();
    engine.replace_session(s);
    let mut app = StudioApp::new(engine, None, crate::Services::default());
    let before = serde_json::to_value(app.engine.session()).unwrap();
    assert!(app.capture_plugin_states().is_ok());
    assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
    assert!(app.with_state_notice("Saved".into()).contains("no audio callback"));
}

#[test]
fn failed_state_capture_aborts_save_and_preserves_document() {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
    app.run("track.new", serde_json::json!({"name": "Unsaved", "count": 1})).unwrap();
    let before = serde_json::to_value(app.engine.session()).unwrap();
    let revision = app.engine.revision;
    let mut saved = false;
    let result = app.store_plugin_states(Err(PlayerError::SessionPending)).map(|()| {
        saved = true;
    });
    assert!(result.is_err());
    assert!(!saved);
    assert_eq!(app.status, PlayerError::SessionPending.to_string());
    assert_eq!(app.engine.revision, revision);
    assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
    assert!(app.engine.is_dirty());
}
