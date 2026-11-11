//! Generated audio and an injected folder picker drive the real panel, without native dialogs.

use super::test_rig::{T, mid};
use serde_json::json;
use std::{cell::Cell, rc::Rc};
use transit_model::TrackId;

fn picker(t: &mut T, folder: Option<String>) -> Rc<Cell<usize>> {
    let calls = Rc::new(Cell::new(0));
    let seen = calls.clone();
    t.app.services.pick_open = Some(Box::new(move |purpose, start| {
        assert_eq!(purpose, "folder");
        assert!(start.is_none());
        seen.set(seen.get() + 1);
        folder.clone()
    }));
    calls
}

fn load(t: &mut T, tr: TrackId) {
    t.click(mid(t.panel(tr).bar.load_kit.unwrap()), 1);
}

fn modal(t: &T, tr: TrackId) -> crate::chrome::ModalLayout {
    let uid = t.app.engine.session().track(tr).unwrap().instrument.as_ref().unwrap().uid.get();
    crate::chrome::modal_layout(&t.ctx, super::kit_load::modal_id(uid)).expect("the preview count is visible")
}

#[test]
fn load_kit_previews_then_loads_the_panels_track_in_one_undo_step() {
    let (mut t, tr) = T::kit();
    let folder = t.dir.join("kit");
    std::fs::create_dir(&folder).unwrap();
    super::test_rig::wav(&folder.join("Z.wav"), 64);
    super::test_rig::wav(&folder.join("a.wav"), 32);
    let calls = picker(&mut t, Some(folder.to_string_lossy().into_owned()));
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    // A different selection must not redirect this panel's command.
    t.app.ui.editor_follow.clear();
    let other = t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    t.app.focus(TrackId(other));
    t.frames(3);
    let before_preview = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = undo + 1;
    load(&mut t, tr);
    assert_eq!(calls.get(), 1);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before_preview, "preview edits nothing");
    assert_eq!(t.undo_len(), undo);
    let preview = modal(&t, tr);
    assert_eq!(preview.detail_text.as_deref(), Some("2 files · pads A4–A5"));
    t.key("Enter");
    assert_eq!(t.undo_len(), undo + 1);
    let patch = t.patch(tr);
    assert_eq!(patch.zones.len(), 5);
    assert_eq!(patch.zone(patch.pad(3).zone.unwrap()).unwrap().label.as_deref(), Some("a"));
    assert_eq!(patch.zone(patch.pad(4).zone.unwrap()).unwrap().label.as_deref(), Some("Z"));
    assert_eq!(calls.get(), 1, "idle frames do not reopen the picker");
    assert!(t.panel(tr).note.is_none());
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before_preview, "one undo restores the full document");
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
}

#[test]
fn load_kit_reports_preview_and_full_load_refusals_verbatim_without_an_undo_step() {
    let (mut t, tr) = T::open();
    let folder = t.dir.join("kit");
    std::fs::create_dir(&folder).unwrap();
    let calls = picker(&mut t, Some(folder.to_string_lossy().into_owned()));
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    let expected = t.app.run("mix.sampler_load_kit", json!({"track": tr.0, "folder": folder, "preview": true})).unwrap();
    load(&mut t, tr);
    assert_eq!(t.panel(tr).note.as_ref().unwrap().0, expected["notice"].as_str().unwrap());
    assert_eq!(t.undo_len(), undo);
    let audio = folder.join("Hit.wav");
    super::test_rig::wav(&audio, 64);
    load(&mut t, tr);
    let preview = modal(&t, tr);
    assert!(t.panel(tr).note.is_none(), "an admitted preview clears the previous refusal");
    // The file changes after preview; the real command must check it again.
    std::fs::write(&audio, b"not audio").unwrap();
    let expected = t.app.run("mix.sampler_load_kit", json!({"track": tr.0, "folder": folder})).unwrap();
    t.click(mid(preview.buttons.iter().find(|b| b.default).unwrap().rect), 1);
    let p = t.panel(tr);
    let (note, rect) = p.note.as_ref().unwrap();
    assert_eq!(note, expected["notice"].as_str().unwrap());
    assert!(rect[1] >= p.panel("pads").unwrap()[3]);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    assert_eq!(t.undo_len(), undo);
    assert_eq!(calls.get(), 2);
    // An ordinary command error takes the same note row as a data-shaped admission refusal.
    picker(&mut t, Some("relative-kit-folder".into()));
    let expected = t.app.run("mix.sampler_load_kit", json!({"track": tr.0, "folder": "relative-kit-folder", "preview": true})).unwrap_err();
    load(&mut t, tr);
    assert_eq!(t.panel(tr).note.unwrap().0, expected);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn canceling_the_folder_or_the_preview_changes_nothing() {
    let (mut t, tr) = T::open();
    t.sample("Hit.wav", 64);
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    let calls = picker(&mut t, None);
    load(&mut t, tr);
    assert_eq!(calls.get(), 1);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    let folder = t.dir.to_string_lossy().into_owned();
    picker(&mut t, Some(folder));
    load(&mut t, tr);
    let preview = modal(&t, tr);
    t.key("Delete");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    t.click(mid(preview.buttons.iter().find(|b| b.text == "Cancel").unwrap().rect), 1);
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    assert_eq!(t.undo_len(), undo);
    assert!(t.panel(tr).note.is_none());
    load(&mut t, tr);
    t.key("Escape");
    assert_eq!(t.app.builtin_editor.windows.len(), 1, "Escape cancels the preview, keeping its editor open");
    let uid = t.app.engine.session().track(tr).unwrap().instrument.as_ref().unwrap().uid.get();
    assert!(super::view(&mut t.app, uid).kit_preview.is_none());
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
    assert_eq!(t.undo_len(), undo);
}
