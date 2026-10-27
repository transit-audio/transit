use super::*;

#[test]
fn record_with_nothing_armed_says_what_to_do() {
    let mut app = SoundApp::new(transit_engine::Engine::default(), None, Services::default());
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing() && !app.engine.transport.recording);
    assert!(app.record_hint().is_some_and(|why| why.contains("Track › New")), "{:?}", app.record_hint());

    app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(app.record_hint().is_some_and(|why| why.contains("R button")), "{:?}", app.record_hint());
    assert_eq!(Some(app.ui.status.as_str()), app.record_hint());
    app.last_frame = Some(5.0);
    assert_eq!(app.record_hint(), None, "the hint shows for a few seconds");

    app.last_frame = None;
    app.run("transport.record", json!({})).unwrap();
    app.handle_transport(0.25);
    app.run("mix.record_arm", json!({})).unwrap();
    assert_eq!(app.record_hint(), None, "arming a track answers the hint");
}
