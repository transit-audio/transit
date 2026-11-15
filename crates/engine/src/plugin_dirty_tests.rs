//! Issue #574: a hosted plug-in's own state change (CLAP `mark_dirty`, VST3 `setDirty`) marks the
//! session modified until it is saved or replaced, and Undo does not clear it.

use super::*;

#[test]
fn a_plugin_state_change_marks_the_session_modified_until_it_is_saved() {
    let mut e = crate::demo::demo_engine();
    e.mark_clean();
    let (rev, undo) = (e.revision, e.undo_history().len());
    assert!(!e.is_dirty());
    e.mark_plugin_state_changed();
    assert!(e.is_dirty(), "the plug-in's change is unsaved work");
    assert_eq!((e.revision, e.undo_history().len()), (rev, undo), "not a document change");
    // An edit and its undo back to the saved document leave the plug-in's change unsaved.
    e.execute("mix.mute", &json!({"track": "Kick"})).unwrap();
    e.undo();
    assert!(e.is_dirty(), "undo cannot take back a change in the plug-in");
    e.mark_clean();
    assert!(!e.is_dirty(), "a save stores it");
    e.mark_plugin_state_changed();
    e.replace_session(transit_model::Session::default());
    assert!(!e.is_dirty(), "a new session starts clean");
}

#[test]
fn undo_doc_and_redo_doc_are_what_undo_and_redo_bring_back() {
    let mut e = crate::demo::demo_engine();
    assert!(e.undo_doc().is_none() && e.redo_doc().is_none());
    let before = e.session_arc();
    e.execute("mix.mute", &json!({"track": "Kick"})).unwrap();
    let after = e.session_arc();
    assert!(Arc::ptr_eq(e.undo_doc().unwrap(), &before));
    e.undo();
    assert!(Arc::ptr_eq(e.redo_doc().unwrap(), &after));
    assert!(e.undo_doc().is_none());
}

#[test]
fn undo_doc_back_walks_the_history_from_the_next_undo() {
    let mut e = crate::demo::demo_engine();
    let first = e.session_arc();
    e.execute("mix.mute", &json!({"track": "Kick"})).unwrap();
    let second = e.session_arc();
    e.execute("mix.solo", &json!({"track": "Kick"})).unwrap();
    assert!(Arc::ptr_eq(e.undo_doc_back(0).unwrap(), &second));
    assert!(Arc::ptr_eq(e.undo_doc_back(1).unwrap(), &first));
    assert!(e.undo_doc_back(2).is_none());
}
