//! Issue #574, CLAP side: `clap_host_state.mark_dirty` (the plug-in's own state changed with no
//! parameter event, such as a patch loaded in its browser) reaches the app through the editor
//! handle's `take_state_changed`, once, whether or not the editor is open.

#![cfg(not(target_os = "macos"))] // Editor handles run on the process main thread on macOS.

mod common;

#[test]
fn mark_dirty_is_reported_once_through_the_editor_handle() {
    common::fixture_dir();
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("the gain fixture has an editor handle");
    assert!(p.load_state(b"CMD:log-start"));
    assert!(!ed.take_state_changed(), "a host load is not a change the plug-in reports");
    assert!(p.load_state(b"CMD:mark-dirty"));
    assert!(ed.take_state_changed(), "mark_dirty reached the app");
    assert!(!ed.take_state_changed(), "once");
    // An idle in between keeps it for the app (it used to be discarded there).
    assert!(p.load_state(b"CMD:mark-dirty"));
    let _ = ed.idle();
    assert!(ed.take_state_changed(), "not lost to the editor's idle");
}

/// A plug-in without the gui extension has no editor handle; its mark_dirty reaches the app
/// through its state handle.
#[test]
fn mark_dirty_of_a_plugin_without_a_gui_reaches_the_app_through_its_state_handle() {
    common::fixture_dir();
    // The fixture's `no_gui` command configures the next instance made on this thread.
    let mut first = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    assert!(first.load_state(b"CMD:no_gui"));
    drop(first);
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    p.prepare(48_000.0, 64, 2);
    assert!(p.editor().is_none(), "no editor handle without a GUI");
    let mut reader = p.state_reader().expect("a state handle");
    assert!(!reader.take_state_changed(), "nothing reported yet");
    assert!(p.load_state(b"CMD:mark-dirty"));
    assert!(reader.take_state_changed(), "mark_dirty reached the app without a GUI");
    assert!(!reader.take_state_changed(), "once");
}
