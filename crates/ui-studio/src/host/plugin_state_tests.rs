use super::*;
use transit_playback::PlayerError;

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
