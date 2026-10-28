//! Unsaved work outlives a crash or an exit without Quit and is offered at the next launch.
//! Ported from the retired legacy UI's `recovery` tests (#177): the engine and autosave behavior
//! is shared, so the headless parts run on `StudioApp`.

use crate::{Services, StudioApp};
use serde_json::json;
use std::path::{Path, PathBuf};
use transit_engine::autosave::leftovers;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Autosaves are written in the background.
fn eventually(done: impl Fn() -> bool) -> bool {
    for _ in 0..300 {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    done()
}

fn autosaved(root: &Path) -> String {
    let folders = std::fs::read_dir(root).into_iter().flatten().flatten();
    folders.map(|d| std::fs::read_to_string(d.path().join("session.transit")).unwrap_or_default()).collect()
}

fn launch(root: &Path) -> StudioApp {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    app.autosave_dir = Some(root.to_path_buf());
    app.leftovers = leftovers(root);
    app
}

#[test]
fn an_exit_without_quit_offers_unsaved_changes_at_the_next_launch() {
    let dir = folder("recovery-quit");
    let root = dir.join("Autosave");
    let path = dir.join("Song.transit");
    let mut first = launch(&root);
    first.run("session.save_as", json!({"path": path})).unwrap();
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    drop(first);

    let mut next = launch(&root);
    assert_eq!(next.leftovers.len(), 1, "the next launch is offered the unsaved work");
    let left = next.leftovers[0].clone();
    next.recover(&left);
    let e = &next.engine;
    assert!(e.session().track_by_name("Vocal").is_some() && e.is_dirty());
    assert_eq!(e.path, Some(path.to_string_lossy().into_owned()));
    assert!(leftovers(&root).is_empty());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    next.run("session.save", json!({})).unwrap();
    assert!(eventually(|| std::fs::read_dir(&root).unwrap().count() == 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_change_is_autosaved_within_a_second() {
    let dir = folder("recovery-changes");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    for name in ["Vocal", "Bass"] {
        app.run("track.new", json!({"name": name})).unwrap();
        app.autosave_changes();
        assert!(eventually(|| autosaved(&root).contains(name)), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovering_keeps_the_unsaved_work_it_replaces() {
    let dir = folder("recovery-replace");
    let root = dir.join("Autosave");
    let mut first = launch(&root);
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    let left = leftovers(&root);
    assert_eq!((left.len(), left[0].name.as_str(), left[0].origin.as_deref()), (1, "Untitled", None));
    let mut next = launch(&root);
    next.run("track.new", json!({"name": "Drums"})).unwrap();
    next.recover(&left[0]);
    assert!(next.engine.session().track_by_name("Vocal").is_some() && next.engine.path.is_none());
    let kept = leftovers(&root);
    assert_eq!(kept.len(), 1);
    let mut third = launch(&root);
    third.recover(&kept[0]);
    assert!(third.engine.session().track_by_name("Drums").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_session_or_quitting_drops_its_autosave() {
    let dir = folder("recovery-close");
    let root = dir.join("Autosave");
    for end in ["session.close", "app.quit"] {
        let mut first = launch(&root);
        first.run("track.new", json!({"name": "Vocal"})).unwrap();
        first.close_autosave();
        let mut next = launch(&root);
        let left = next.leftovers[0].clone();
        next.recover(&left);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "{end}");
        next.run(end, json!({})).unwrap();
        next.close_autosave();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "{end}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quitting_while_recording_keeps_the_take_for_the_next_launch() {
    let dir = folder("recovery-recording");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
    app.engine.transport.recording = true;
    app.quit_requested = true;
    app.close_autosave();
    assert!(!app.engine.transport.recording);
    assert_eq!(leftovers(&root).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ported from the legacy `bounce` tests (#177): while a bounce holds its own copy of the session,
/// a session-replacing command is refused, and Quit cancels the bounce.
#[test]
fn the_session_cannot_be_replaced_while_it_bounces() {
    let dir = folder("bounce-refuses");
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.open_bounce();
    let form = app.finish.bounce_form.as_mut().expect("the bounce form opens");
    form.range = crate::finish::BounceRange::Whole;
    form.path = dir.join("Mix.wav").to_string_lossy().into_owned();
    app.start_bounce().unwrap();
    assert!(app.finish.job.is_some(), "{}", app.status);
    let before = app.engine.session().name.clone();
    for id in ["session.new", "session.close"] {
        assert!(app.run(id, json!({})).is_err(), "{id}");
        assert_eq!(app.status, "Finish or cancel the bounce first");
        assert_eq!(app.engine.session().name, before, "{id}");
    }
    app.run("app.quit", json!({})).unwrap();
    assert!(app.finish.job.is_none(), "Quit cancels the bounce");
    let _ = std::fs::remove_dir_all(&dir);
}
