//! #385: Save As, Open and Bounce start in the session's folder, else the remembered session
//! folder, else documents (the temporary folder under test), never wherever the last dialog was.

use crate::StudioApp;
use crate::session_folder::documents_dir;
use serde_json::json;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new(tag: &str) -> Dir {
        let dir = transit_test_support::TestDir::new(&format!("385-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

/// An app whose Save As answers `answer` and records the default it was offered, and whose Open
/// records the folder it was asked to start in and picks nothing.
fn app(answer: Rc<RefCell<Option<String>>>) -> (StudioApp, Rc<RefCell<String>>, Rc<RefCell<Option<PathBuf>>>) {
    let mut app = crate::tests::app();
    let (offered, started) = (Rc::new(RefCell::new(String::new())), Rc::new(RefCell::new(None)));
    let (o, s) = (offered.clone(), started.clone());
    app.services.pick_save = Some(Box::new(move |default: &str| {
        *o.borrow_mut() = default.to_string();
        answer.borrow().clone()
    }));
    app.services.pick_open = Some(Box::new(move |purpose: &str, start: Option<&Path>| {
        assert_eq!(purpose, "session.open");
        *s.borrow_mut() = start.map(Path::to_path_buf);
        None
    }));
    (app, offered, started)
}

fn folder(offered: &str) -> PathBuf {
    Path::new(offered).parent().unwrap().to_path_buf()
}

#[test]
fn save_as_starts_in_the_session_folder_then_the_remembered_one() {
    let songs = Dir::new("songs");
    let answer = Rc::new(RefCell::new(None));
    let (mut app, offered, started) = app(answer.clone());
    // A new session on a machine with nothing remembered: documents.
    assert!(!app.save_as(), "cancelled");
    assert_eq!(folder(&offered.borrow()), documents_dir());
    assert_eq!(app.ui.last_session_dir, None, "a cancelled dialog remembers nothing");
    // Saved into the songs folder: it is remembered, and the next Save As starts there.
    *answer.borrow_mut() = Some(songs.0.join("tune.transit").to_string_lossy().into_owned());
    assert!(app.save_as());
    assert_eq!(app.ui.last_session_dir.as_deref(), Some(songs.0.path()));
    *answer.borrow_mut() = None;
    assert!(!app.save_as());
    assert_eq!(folder(&offered.borrow()), songs.0.path(), "a saved session starts in its own folder");
    // A new, never-saved session starts in the remembered folder, and so does Open.
    app.run("session.new", json!({})).unwrap();
    assert!(app.engine.path.is_none());
    assert!(!app.save_as());
    assert_eq!(folder(&offered.borrow()), songs.0.path(), "a new session starts in the remembered folder");
    app.open_dialog();
    assert_eq!(started.borrow().as_deref(), Some(songs.0.path()), "Open starts in the remembered folder");
}

#[test]
fn opening_a_session_remembers_its_folder_and_a_deleted_one_falls_through_to_documents() {
    let elsewhere = Dir::new("elsewhere");
    let file = elsewhere.0.join("old.transit");
    {
        let mut writer = crate::tests::app();
        writer.run("session.save_as", json!({"path": file.to_string_lossy(), "replace": true})).unwrap();
    }
    let answer = Rc::new(RefCell::new(None));
    let (mut app, offered, started) = app(answer);
    app.run("session.open", json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!(app.ui.last_session_dir.as_deref(), Some(elsewhere.0.path()), "an open remembers the folder");
    app.run("session.new", json!({})).unwrap();
    drop(elsewhere);
    assert!(!app.save_as());
    assert_eq!(folder(&offered.borrow()), documents_dir(), "a remembered folder that is gone falls through");
    app.open_dialog();
    assert_eq!(started.borrow().as_deref(), Some(documents_dir().as_path()));
}

#[test]
fn bounce_follows_the_session_rule() {
    let songs = Dir::new("bounce");
    let answer = Rc::new(RefCell::new(Some(songs.0.join("tune.transit").to_string_lossy().into_owned())));
    let (mut app, _, _) = app(answer);
    assert!(app.save_as());
    app.run("session.new", json!({})).unwrap();
    app.open_bounce();
    let form = app.finish.bounce_form.clone().expect("the form opens");
    assert_eq!(folder(&form.path), songs.0.path(), "a new session bounces into the remembered session folder");
    assert_eq!(Path::new(&form.dir), songs.0.join("Stems"));
}
