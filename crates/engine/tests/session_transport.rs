use serde_json::json;
use transit_engine::{Engine, TransportRequest, TransportStatus};
use transit_model::Session;
use transit_time::Range;

#[test]
fn replacing_session_resets_status_and_discards_old_transport_requests() {
    let mut engine = Engine::default();
    engine.transport = TransportStatus { playing: true, recording: true, position: 48_000 };
    engine.execute("transport.play", &json!({"from": 96_000})).unwrap();
    engine.execute("transport.record", &json!({})).unwrap();
    let mut session = Session::default();
    session.edit.selection = Range::point(12_000);
    engine.replace_session(session);
    assert_eq!(engine.transport, TransportStatus { position: 12_000, ..TransportStatus::default() });
    assert_eq!(engine.transport_requests, vec![TransportRequest::ResetSession]);
    assert!(!engine.is_dirty());
}

#[test]
fn failed_new_session_keeps_the_current_transport() {
    let mut engine = Engine::default();
    engine.transport = TransportStatus { playing: true, position: 48_000, ..TransportStatus::default() };
    engine.execute("transport.play", &json!({})).unwrap();
    let status = engine.transport;
    let requests = engine.transport_requests.clone();
    assert!(engine.execute("session.new", &json!({"sample_rate": 0})).is_err());
    assert_eq!(engine.transport, status);
    assert_eq!(engine.transport_requests, requests);
}

#[test]
fn undo_and_redo_never_change_record_arm_or_monitor() {
    let mut e = Engine::default();
    let id = e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
    e.execute("mix.input_monitor", &json!({"tracks": [id], "value": true})).unwrap();
    e.execute("mix.volume", &json!({"tracks": [id], "db": -3.0})).unwrap();
    let armed = |e: &Engine| e.session().tracks[0].mixer.record_arm && e.session().tracks[0].mixer.input_monitor;
    assert!(e.undo() && armed(&e));
    assert!(e.redo() && armed(&e));
    e.execute("mix.record_arm", &json!({"tracks": [id], "value": false})).unwrap();
    assert!(e.undo());
    assert!(!e.session().tracks[0].mixer.record_arm, "undo does not re-arm");
}

#[test]
fn arming_does_not_make_the_document_dirty() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap();
    e.mark_clean();
    e.execute("mix.record_arm", &json!({"tracks": [e.session().tracks[0].id.0], "value": true})).unwrap();
    assert!(!e.is_dirty());
}
