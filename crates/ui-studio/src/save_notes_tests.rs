//! What a save posts reaches the status line (the long take that still plays from its old file,
//! the frozen recording's report): the one warning, or the first and a count; said once, by the
//! save that posted it.

use serde_json::json;

fn save_as(app: &mut crate::StudioApp, dir: &transit_test_support::TestDir, name: &str) {
    app.run("session.save_as", json!({"path": dir.join(name), "replace": true})).unwrap();
}

#[test]
fn one_warning_from_a_save_is_the_status_line() {
    let mut app = crate::tests::app();
    let mark = app.engine.message_pushes();
    app.engine.warn("The long take \"Guitar\" still plays from A.wav: its copy could not be opened. The next save tries again.");
    app.say_save_notes(mark);
    assert_eq!(app.status, "The long take \"Guitar\" still plays from A.wav: its copy could not be opened. The next save tries again.");
}

#[test]
fn several_warnings_show_the_first_and_a_count_and_all_stay_in_the_engine_list() {
    let mut app = crate::tests::app();
    let mark = app.engine.message_pushes();
    for n in 1..=3 {
        app.engine.warn(format!("Warning {n}."));
    }
    app.say_save_notes(mark);
    assert_eq!(app.status, "Warning 1. 2 more warnings in the log");
    let held: Vec<_> = app.engine.messages_since(mark).to_vec();
    assert_eq!(held, ["Warning 1.", "Warning 2.", "Warning 3."]);
}

#[test]
fn a_clean_save_says_nothing_and_an_older_message_is_not_said_again() {
    let dir = transit_test_support::TestDir::new("save-notes");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = crate::tests::app();
    // Posted before the save (an earlier action's): the save is not the one that posted it.
    app.engine.warn("An older warning.");
    app.status = "Earlier line".into();
    save_as(&mut app, &dir, "one.transit");
    assert_eq!(app.status, "Earlier line", "nothing was posted by the save");
    // A later save and a later unrelated action do not bring it back either.
    app.run("session.save", json!({})).unwrap();
    app.status.clear();
    app.run("transport.stop", json!({})).unwrap();
    app.run("session.save", json!({})).unwrap();
    assert_eq!(app.status, "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A frozen recording that carries a media report: the save posts it (the engine's own channel),
/// and the app's Save and Save As say it in the status line, once.
#[test]
fn a_save_that_posts_a_frozen_recording_report_says_it_in_the_status_line_once() {
    let dir = transit_test_support::TestDir::new("save-notes-frozen");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = crate::tests::app();
    let t = app.run("track.new", json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
    app.run("mix.insert", json!({"track": t, "slot": 0, "plugin": "gain"})).unwrap();
    let id = transit_model::TrackId(t);
    let report = "The frozen recording of Gain could not be read: a media problem.";
    let ins = app.engine.session_mut().track_mut(id).unwrap().mixer.inserts[0].as_mut().unwrap();
    ins.frozen_recording = Some(transit_model::FrozenRecording { report: report.into(), ..Default::default() });
    app.status.clear();
    save_as(&mut app, &dir, "frozen.transit");
    assert_eq!(app.status, report);
    // An unrelated action later does not bring it back: only the save that posted it says it.
    app.status.clear();
    app.run("track.rename", json!({"track": t, "name": "Tape"})).ok();
    assert_ne!(app.status, report, "an unrelated action does not say it");
    let _ = std::fs::remove_dir_all(&dir);
}
