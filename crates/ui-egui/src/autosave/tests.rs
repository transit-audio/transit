use super::*;

#[test]
fn a_hidden_window_still_autosaves() {
    let dir = std::env::temp_dir().join(format!("soundcraft-hidden-autosave-{}", std::process::id()));
    let mut app = SoundApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.autosave_dir = Some(dir.clone());
    app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
    app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
    let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
    let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
    let _ = egui::Context::default().run_logic(&raw, |ctx| app.logic(ctx));
    assert!(app.latest_autosave().is_some());
    let _ = std::fs::remove_dir_all(dir);
}
