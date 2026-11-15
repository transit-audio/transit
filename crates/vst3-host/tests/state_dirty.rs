//! Issue #574, VST3 side: `IComponentHandler2::setDirty(true)` (the plug-in's own state changed,
//! such as a preset picked in its editor) reaches the app through the editor handle's
//! `take_state_changed`, once; a change with no `setDirty` reports nothing there.

#![cfg(not(target_os = "macos"))] // Editor handles drive windows on the process main thread on macOS.

mod common;

/// A Gain state that arms fixture code `code` (see `tests/fixture`, "call log and presets"): the
/// pick runs at the controller's next `getState`; 64 makes it use the extended handlers, ending
/// with `setDirty(true)`.
fn arming(code: u32) -> Vec<u8> {
    let comp = 0.5f64.to_le_bytes();
    let ctrl: Vec<u8> = [0.5f64, -1.0, -1.0, f64::from(code)].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut out = b"SCV3".to_vec();
    out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    out.extend_from_slice(&comp);
    out.extend_from_slice(&(ctrl.len() as u32).to_le_bytes());
    out.extend_from_slice(&ctrl);
    out
}

#[test]
fn set_dirty_is_reported_once_through_the_editor_handle() {
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    // A pick announced by edits only: no setDirty.
    assert!(p.load_state(&arming(1)));
    assert!(reader.save_state().is_some());
    let _ = ed.idle();
    assert!(!ed.take_state_changed(), "no setDirty, nothing reported");
    // A pick through the extended handlers: setDirty(true).
    assert!(p.load_state(&arming(1 + 2 + 64)));
    assert!(reader.save_state().is_some());
    assert!(ed.take_state_changed(), "setDirty reached the app");
    assert!(!ed.take_state_changed(), "once");
    drop(reader);
    drop(ed);
    drop(p);
}
