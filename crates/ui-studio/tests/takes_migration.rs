//! The load-time take migration reaches the status line through the session-open action.

use serde_json::json;
use transit_ui_studio::{Services, StudioApp};

#[test]
fn opening_legacy_takes_reports_the_migration_in_the_status_line() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus/v0.5.1-pre-take-lanes.transit");
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    app.run("session.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(app.status, "2 track(s) recorded with an earlier build now show their laps as takes");
    assert!(app.engine.is_dirty());
    assert_eq!(app.engine.undo_label(), Some("Migrate takes"));
}
