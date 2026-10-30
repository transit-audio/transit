//! The Files tab's preview (#211), driven as a user drives it: clicks, keys and control-channel
//! commands, one frame at a time. Assertions are on what the app shows and on the document,
//! never on timing.

use super::*;

/// The document's state the preview must never change: undo steps, sources, tracks, dirty.
fn document(t: &T) -> (Vec<String>, usize, usize, bool) {
    let s = t.app.engine.session();
    (t.app.engine.undo_history(), s.sources.len(), s.tracks.len(), t.app.engine.is_dirty())
}

fn exec(t: &mut T, command: &str, params: Value) -> Value {
    let (req, _rx) = crate::ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let ctx = egui::Context::default();
    let v = crate::control::handle_for_test(&mut t.app, &ctx, &req).unwrap();
    t.frames(2);
    v
}

fn previewing(t: &T) -> Option<String> {
    t.app.browser.preview.path.as_ref().map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
}

/// The sample folder added and expanded, with Kicks/ open, and no audio device.
fn kicks_without_a_device() -> T {
    let mut t = T::new();
    t.add_and_expand();
    t.click(mid(t.row(&t.path("Kicks")).rect.unwrap()));
    t.settle();
    t
}

/// [`kicks_without_a_device`] with a device-free Player: its blocks run only in [`step`], which
/// also runs the preview's feeder, so nothing waits on a thread.
fn kicks() -> T {
    let mut t = kicks_without_a_device();
    let player = transit_playback::Player::prepare_device_free(t.app.engine.session_arc()).unwrap();
    t.app.player = Some(player);
    t.frames(2);
    t
}

/// `n` device blocks of 512 frames, then a UI frame.
fn step(t: &mut T, n: usize) {
    for _ in 0..n {
        t.app.player.as_ref().unwrap().step_device_free();
    }
    t.frames(1);
}

#[test]
fn the_headphone_toggle_previews_each_selected_file_and_escape_stops_it() {
    let mut t = kicks();
    let before = document(&t);
    assert!(!t.app.ui.browser.auto_preview, "off until the user turns it on");
    t.click(mid(t.app.browser.layout.preview.auto));
    assert!(t.app.ui.browser.auto_preview && t.app.browser.layout.preview.auto_on);
    t.click(mid(t.row_named("Kick 01.wav").rect.unwrap()));
    assert_eq!(previewing(&t).as_deref(), Some("Kick 01.wav"));
    assert_eq!(t.app.browser.preview.state, "loading");
    t.click(mid(t.row_named("Kick 02.wav").rect.unwrap()));
    assert_eq!(previewing(&t).as_deref(), Some("Kick 02.wav"), "another file selected: the old preview stops, the new one starts");
    t.key("escape", json!({}));
    assert_eq!(previewing(&t), None, "Escape stops it");
    assert_eq!(
        t.app.browser.layout.preview.file.as_ref().map(PathBuf::from),
        Some(PathBuf::from(t.path("Kicks/Kick 02.wav"))),
        "the footer keeps the selection"
    );
    t.click(mid(t.app.browser.layout.preview.auto));
    assert!(!t.app.ui.browser.auto_preview);
    t.click(mid(t.row_named("Kick 01.wav").rect.unwrap()));
    assert_eq!(previewing(&t), None, "with the toggle off a selection does not preview");
    assert_eq!(document(&t), before, "no undo step, no source, no track, not dirty");
}

#[test]
fn the_selected_rows_play_control_previews_and_another_selection_stops_it() {
    let mut t = kicks();
    t.click(mid(t.row_named("Kick 01.wav").rect.unwrap()));
    assert!(t.row_named("Kick 02.wav").play.is_none(), "only the selected file has the control");
    let play = t.row_named("Kick 01.wav").play.expect("the selected file's play control");
    assert_eq!(previewing(&t), None);
    t.click(mid(play));
    assert_eq!(previewing(&t).as_deref(), Some("Kick 01.wav"));
    assert_eq!(t.app.browser.selected, vec![PathBuf::from(t.path("Kicks/Kick 01.wav"))], "the control does not change the selection");
    t.click(mid(t.row_named("Kick 02.wav").rect.unwrap()));
    assert_eq!(previewing(&t), None, "another file selected stops it");
    // The footer's control previews the selected file too, and stops it.
    t.click(mid(t.app.browser.layout.preview.play));
    assert_eq!(previewing(&t).as_deref(), Some("Kick 02.wav"));
    t.click(mid(t.app.browser.layout.preview.play));
    assert_eq!(previewing(&t), None);
}

#[test]
fn starting_the_transport_stops_the_preview() {
    let mut t = kicks();
    let kick = t.path("Kicks/Kick 01.wav");
    let v = exec(&mut t, "preview.play", json!({"path": kick}));
    assert_eq!(v["result"]["path"], json!(kick), "{v}");
    assert_eq!(previewing(&t).as_deref(), Some("Kick 01.wav"));
    exec(&mut t, "transport.play", json!({}));
    step(&mut t, 2);
    assert!(t.app.is_playing());
    assert_eq!(previewing(&t), None);
    // A preview started while the transport runs plays: the transport did not start again.
    exec(&mut t, "preview.play", json!({"path": kick}));
    assert_eq!(previewing(&t).as_deref(), Some("Kick 01.wav"));
}

#[test]
fn the_control_channel_drives_the_preview_and_never_touches_the_document() {
    let mut t = kicks();
    let before = document(&t);
    let v = exec(&mut t, "preview.volume", json!({"db": -12.5}));
    assert_eq!(v["result"]["db"], json!(-12.5), "{v}");
    assert_eq!(t.app.ui.browser.preview_db, -12.5);
    assert_eq!(exec(&mut t, "preview.volume", json!({"db": 40.0}))["result"]["db"], json!(6.0), "clamped to +6 dB");
    assert_eq!(exec(&mut t, "preview.auto", json!({}))["result"]["auto"], json!(true), "toggles");
    assert_eq!(exec(&mut t, "preview.auto", json!({"on": false}))["result"]["auto"], json!(false));
    let nki = t.path("Kit.nki");
    let refused = exec(&mut t, "preview.play", json!({"path": nki}));
    assert_eq!(refused["ok"], json!(false), "a Kontakt file is not previewed: {refused}");
    assert_eq!(exec(&mut t, "preview.play", json!({}))["ok"], json!(false), "no path and no selected file");
    t.click(mid(t.row_named("Kick 02.wav").rect.unwrap()));
    let v = exec(&mut t, "preview.play", json!({}));
    assert_eq!(v["result"]["path"].as_str().map(PathBuf::from), Some(PathBuf::from(t.path("Kicks/Kick 02.wav"))), "the selected file: {v}");
    assert_eq!(exec(&mut t, "preview.status", json!({}))["result"]["state"], json!("loading"));
    assert_eq!(exec(&mut t, "preview.stop", json!({}))["result"]["path"], Value::Null);
    assert_eq!(document(&t), before);
    // The toggle and the volume are view preferences: kept in ui.json, an old one loads with defaults.
    let saved = serde_json::to_value(&t.app.ui.browser).unwrap();
    assert_eq!((saved["auto_preview"].clone(), saved["preview_db"].clone()), (json!(false), json!(6.0)));
    let old: crate::browser::BrowserPrefs = serde_json::from_value(json!({"show": true, "width": 300.0})).unwrap();
    assert_eq!((old.auto_preview, old.preview_db), (false, crate::browser::preview::DEFAULT_DB));
}

#[test]
fn a_device_free_player_plays_the_preview_to_its_end_and_the_report_says_so() {
    let mut t = kicks();
    let before = document(&t);
    // Kick 02 is a quarter second: 12,000 frames at 48 kHz, 24 device blocks of 512.
    let kick = t.path("Kicks/Kick 02.wav");
    exec(&mut t, "preview.play", json!({"path": kick}));
    assert_eq!(t.app.browser.preview.state, "loading");
    step(&mut t, 4);
    assert_eq!(t.app.browser.preview.state, "playing");
    assert!(t.app.preview_line().starts_with("Preview playing: Kick 02.wav"), "{}", t.app.preview_line());
    assert!(t.app.browser.preview.progress > 0.0 && t.app.browser.preview.progress < 1.0);
    step(&mut t, 30);
    assert_eq!(previewing(&t), None, "it ended");
    assert_eq!(t.app.preview_line(), "Preview playing: no");
    assert_eq!(document(&t), before);
}

#[test]
fn without_an_audio_device_a_preview_is_refused_and_nothing_shows_stop() {
    let mut t = kicks_without_a_device();
    t.click(mid(t.app.browser.layout.preview.auto));
    t.click(mid(t.row_named("Kick 01.wav").rect.unwrap()));
    assert_eq!(previewing(&t), None, "auto-preview starts nothing it cannot play");
    assert_eq!(t.app.status, crate::browser::preview::NO_DEVICE);
    let kick = t.path("Kicks/Kick 01.wav");
    assert_eq!(exec(&mut t, "preview.play", json!({"path": kick}))["ok"], json!(false));
    t.click(mid(t.row_named("Kick 01.wav").play.unwrap()));
    assert_eq!(previewing(&t), None);
    assert_eq!(t.app.preview_line(), "Preview playing: no");
}

#[test]
fn the_footer_shows_the_selected_files_facts_and_reads_its_waveform_in_the_background() {
    let mut t = kicks();
    t.click(mid(t.row_named("Kick 01.wav").rect.unwrap()));
    assert_eq!(t.app.browser.layout.preview.facts, "0.5 s · 48 kHz · mono");
    let t0 = std::time::Instant::now();
    // timing: stall-bound waits for the background overview reader, a ceiling far above jitter
    while !t.app.browser.layout.preview.overview && t0.elapsed().as_secs() < 10 {
        t.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(t.app.browser.layout.preview.overview, "the waveform was read");
    let o = t.app.browser.preview.overview(&PathBuf::from(t.path("Kicks/Kick 01.wav"))).unwrap().unwrap();
    assert!(o.bins.iter().any(|(lo, hi)| *lo < 0.0 && *hi > 0.0), "it holds the file's swing");
}
