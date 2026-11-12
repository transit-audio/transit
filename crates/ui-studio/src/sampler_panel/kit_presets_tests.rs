//! The Sampler's kit presets in the title bar (ADR 0041, amendment 2026-11-11, #467), driven the
//! way a user drives them: generated audio, a scratch config folder (never the real one), one frame
//! at a time with no wall clock.

use super::kit_presets::{self, QUESTION, REPLACE};
use super::test_rig::{T, mid};
use serde_json::{Value, json};
use std::path::PathBuf;
use transit_engine::io::kit_presets as files;
use transit_model::TrackId;
use transit_model::sampler_patch::PatchV1;

/// Every painted text that reads exactly `want`, by its center.
pub(super) fn painted(t: &T, want: &str) -> Vec<(f32, f32)> {
    fn walk(shape: &egui::Shape, want: &str, out: &mut Vec<(f32, f32)>) {
        match shape {
            egui::Shape::Text(s) if s.galley.text() == want => out.push((s.pos.x + s.galley.size().x / 2.0, s.pos.y + s.galley.size().y / 2.0)),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    t.shapes.iter().for_each(|s| walk(&s.shape, want, &mut found));
    found
}

/// A Sampler kit (pads A1 to A3) with its editor open and a scratch config folder.
pub(super) fn rig() -> (T, TrackId, PathBuf) {
    let (mut t, tr) = T::kit();
    let root = t.dir.join("config");
    std::fs::create_dir_all(&root).unwrap();
    t.app.services.config_dir = Some(root.clone());
    t.frames(2);
    (t, tr, root)
}

pub(super) fn uid(t: &T, tr: TrackId) -> u64 {
    t.app.engine.session().track(tr).unwrap().instrument.as_ref().unwrap().uid.get()
}

pub(super) fn pending(t: &T, tr: TrackId) -> Option<kit_presets::Pending> {
    t.app.builtin_editor.devices.sampler.views.get(&uid(t, tr)).and_then(|v| v.kit.pending.clone())
}

pub(super) fn kit_file(root: &std::path::Path, name: &str) -> PathBuf {
    files::folder(root).join(format!("{name}.tkit"))
}

/// Saves the track's kit as `name` through the engine, as the list's Save As sends it.
pub(super) fn save_kit(t: &mut T, tr: TrackId, root: &std::path::Path, name: &str) {
    t.app.run("mix.sampler_kit_save", json!({"track": tr.0, "path": kit_file(root, name)})).unwrap();
}

/// Loads `name` (a generated sample) onto zone and pad `n`.
pub(super) fn load_pad(t: &mut T, tr: TrackId, n: u64, name: &str) {
    let p = t.sample(name, 2400);
    t.app.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": n, "pad": n, "path": p.to_string_lossy()})).unwrap();
    t.frames(2);
}

pub(super) fn open_list(t: &mut T, tr: TrackId) {
    let field = t.panel(tr).title.field;
    t.click(mid(field), 1);
}

/// Opens the kit list and clicks the entry `name` (below the field).
pub(super) fn pick(t: &mut T, tr: TrackId, name: &str) {
    open_list(t, tr);
    // In the list's popup: under the field and across its width (never a bank button or a pad).
    let field = t.panel(tr).title.field;
    let in_list = |p: &(f32, f32)| p.1 > field[3] && p.0 > field[0] && p.0 < field[0] + crate::device_editor::title_bar::FIELD_W;
    let at = painted(t, name).into_iter().find(in_list).unwrap_or_else(|| panic!("{name} is listed"));
    t.click(at, 1);
}

/// Types `name` into the list's Save As field and presses Enter.
pub(super) fn save_as(t: &mut T, tr: TrackId, name: &str) {
    open_list(t, tr);
    let hint = painted(t, "Save As\u{2026}");
    t.click(*hint.first().expect("the Save As field is in the list"), 1);
    t.send("ui.text", json!({"text": name}));
    t.key("enter");
}

/// The patch with every source id cleared: what two recalls of one kit share.
fn normalized(p: &PatchV1) -> PatchV1 {
    let mut p = p.clone();
    for r in &mut p.sources {
        r.source = transit_model::SourceId(0);
    }
    p
}

pub(super) fn doc(t: &T) -> Value {
    serde_json::to_value(t.app.engine.session()).unwrap()
}

#[test]
fn the_list_shows_the_kit_files_and_no_parameter_presets_and_never_not_available() {
    let (mut t, tr, root) = rig();
    // The retired one-sample presets and a kit side by side in the kit folder.
    let dir = files::folder(&root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Plucky.json"), r#"{"plugin": "sampler", "version": 1, "params": {}}"#).unwrap();
    save_kit(&mut t, tr, &root, "Boom Kit");
    t.frames(2);
    let title = t.panel(tr).title;
    assert_eq!((title.preset_group.as_str(), title.preset_name.as_str()), ("", "<factory default>"), "no kit recalled or saved yet");
    open_list(&mut t, tr);
    let below = t.panel(tr).title.field[3];
    assert!(painted(&t, "Boom Kit").iter().any(|p| p.1 > below), "the kit file is listed");
    assert!(painted(&t, "Plucky").is_empty(), "never a parameter preset");
    assert!(painted(&t, "Factory").is_empty(), "no factory section");
    assert!(painted(&t, "Save As\u{2026}").len() == 1, "Save As");
    t.key("escape");
    // The browser takes its clicks (next asks about this loaded kit) and its hover never reads
    // "Not available yet".
    let next = t.panel(tr).title.next;
    t.point(mid(next));
    assert!(painted(&t, crate::device_editor::title_bar::UNAVAILABLE).is_empty());
    t.click(mid(next), 1);
    assert_eq!(pending(&t, tr).expect("next takes its click").name, "Boom Kit");
    t.key("escape");
}

#[test]
fn save_as_then_recall_into_another_sampler_restores_the_kit() {
    let (mut t, tr, root) = rig();
    save_as(&mut t, tr, "808");
    assert!(kit_file(&root, "808").is_file(), "Save As writes the kit file");
    assert!(!files::folder(&root).join("808.json").exists(), "and no parameter preset");
    let title = t.panel(tr).title;
    assert_eq!((title.preset_group.as_str(), title.preset_name.as_str()), ("User", "808"), "the field shows the saved kit");
    let original = t.patch(tr);
    // Another Sampler, its own window.
    let close = t.panel(tr).title.close;
    t.click(mid(close), 1);
    let other = t.sampler_track();
    t.open_card(0);
    let undo = t.undo_len();
    pick(&mut t, other, "808");
    assert!(pending(&t, other).is_none(), "an empty kit is never asked about");
    assert_eq!(normalized(&t.patch(other)), normalized(&original), "the whole kit restored");
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
    assert_eq!(t.panel(other).title.preset_name, "808");
    assert_eq!(t.app.status, "Kit \"808\" loaded: 3 pads, 3 zones.");
}

#[test]
fn save_writes_only_over_the_kit_last_recalled_or_saved() {
    let (mut t, tr, root) = rig();
    let save = t.panel(tr).title.save;
    // No kit recalled or saved here: Save opens the list and writes nothing.
    t.click(mid(save), 1);
    assert_eq!(painted(&t, "Save As\u{2026}").len(), 1, "the list is open for Save As");
    t.key("escape");
    assert!(files::list(&root).is_empty(), "nothing written");
    save_as(&mut t, tr, "Kit A");
    load_pad(&mut t, tr, 4, "Rim.wav");
    t.click(mid(save), 1);
    let a: Value = serde_json::from_slice(&std::fs::read(kit_file(&root, "Kit A")).unwrap()).unwrap();
    assert_eq!(a["kit"]["sources"].as_array().unwrap().len(), 4, "Save wrote over A, the kit last saved");
    // B, saved elsewhere, then recalled here: Save now writes B and leaves A alone.
    save_kit(&mut t, tr, &root, "Kit B");
    pick(&mut t, tr, "Kit B");
    assert!(pending(&t, tr).is_none(), "unchanged since its last save: no question");
    let a_bytes = std::fs::read(kit_file(&root, "Kit A")).unwrap();
    load_pad(&mut t, tr, 5, "Clap.wav");
    t.click(mid(save), 1);
    let b: Value = serde_json::from_slice(&std::fs::read(kit_file(&root, "Kit B")).unwrap()).unwrap();
    assert_eq!(b["kit"]["sources"].as_array().unwrap().len(), 5, "Save wrote over B");
    assert_eq!(std::fs::read(kit_file(&root, "Kit A")).unwrap(), a_bytes, "A untouched");
    // Undo past the recall: the field no longer shows B, so Save writes nothing and opens the list.
    t.app.run("edit.undo", json!({})).unwrap();
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_ne!(t.panel(tr).title.preset_name, "Kit B");
    let b_bytes = std::fs::read(kit_file(&root, "Kit B")).unwrap();
    t.click(mid(save), 1);
    assert_eq!(painted(&t, "Save As\u{2026}").len(), 1, "the list opens for Save As");
    assert_eq!(std::fs::read(kit_file(&root, "Kit B")).unwrap(), b_bytes, "B untouched");
    assert_eq!(std::fs::read(kit_file(&root, "Kit A")).unwrap(), a_bytes, "A untouched");
}

#[test]
fn the_question_comes_only_for_a_loaded_kit_changed_since_its_last_recall_or_save() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "Kit A");
    load_pad(&mut t, tr, 4, "Rim.wav");
    save_kit(&mut t, tr, &root, "Kit B");
    // A kit with zones and no record of a recall or save here: asked.
    pick(&mut t, tr, "Kit A");
    let asked = pending(&t, tr).expect("asked: no record counts as changed");
    assert_eq!(asked.detail, "4 loaded pads will be replaced by \"Kit A\".");
    let m = crate::chrome::modal_layout(&t.ctx, kit_presets::modal_id(uid(&t, tr))).expect("the question is drawn");
    assert_eq!(painted(&t, QUESTION).len(), 1);
    assert!(m.buttons.iter().any(|b| b.default && b.text == REPLACE));
    t.key("enter");
    assert!(pending(&t, tr).is_none());
    assert_eq!(t.patch(tr).zones.len(), 3, "Enter replaces");
    // Unchanged since that recall: next replaces without asking.
    let next = t.panel(tr).title.next;
    t.click(mid(next), 1);
    assert!(pending(&t, tr).is_none(), "an unchanged kit is never asked about");
    assert_eq!(t.patch(tr).zones.len(), 4, "B recalled");
    // Changed: asked again.
    load_pad(&mut t, tr, 5, "Clap.wav");
    t.click(mid(next), 1);
    assert_eq!(pending(&t, tr).expect("changed since the recall").detail, "5 loaded pads will be replaced by \"Kit A\".");
    t.key("escape");
    // An empty kit is never asked about.
    let other = {
        let close = t.panel(tr).title.close;
        t.click(mid(close), 1);
        let o = t.sampler_track();
        t.open_card(0);
        o
    };
    pick(&mut t, other, "Kit B");
    assert!(pending(&t, other).is_none(), "nothing loaded: no question");
    assert_eq!(t.patch(other).zones.len(), 4);
}

#[test]
fn cancel_changes_nothing() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "Kit A");
    load_pad(&mut t, tr, 4, "Rim.wav");
    let before = doc(&t);
    let undo = t.undo_len();
    pick(&mut t, tr, "Kit A");
    let m = crate::chrome::modal_layout(&t.ctx, kit_presets::modal_id(uid(&t, tr))).expect("asked");
    t.click(mid(m.buttons.iter().find(|b| b.text == "Cancel").unwrap().rect), 1);
    assert!(pending(&t, tr).is_none(), "the question is gone");
    assert_eq!(doc(&t), before, "nothing changed");
    assert_eq!(t.undo_len(), undo);
    // Escape cancels the same way, and the window stays.
    let prev = t.panel(tr).title.prev;
    t.click(mid(prev), 1);
    assert!(pending(&t, tr).is_some());
    t.key("escape");
    assert!(pending(&t, tr).is_none());
    assert_eq!(doc(&t), before);
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.builtin_editor.windows.iter().any(|w| w.key.0 == tr), "the window stays open");
}

#[test]
fn a_run_of_three_steps_is_one_undo_step_back_to_the_kit_before_it() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "K1");
    for (n, (zone, file)) in [(4, "Rim.wav"), (5, "Clap.wav"), (6, "Tom.wav")].into_iter().enumerate() {
        load_pad(&mut t, tr, zone, file);
        save_kit(&mut t, tr, &root, &format!("K{}", n + 2));
    }
    pick(&mut t, tr, "K1");
    t.key("enter");
    let first = t.patch(tr);
    assert_eq!(first.zones.len(), 3);
    let undo = t.undo_len();
    let next = t.panel(tr).title.next;
    for zones in [4, 5, 6] {
        t.click(mid(next), 1);
        assert!(pending(&t, tr).is_none(), "the run asks at most at its first step");
        assert_eq!(t.patch(tr).zones.len(), zones);
    }
    assert_eq!(t.panel(tr).title.preset_name, "K4");
    assert_eq!(t.undo_len(), undo + 1, "three steps, one undo step");
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(t.patch(tr), first, "undo returns to the kit before the run");
    assert_eq!(t.panel(tr).title.preset_name, "K1");
    // Previous wraps from the first to the last; any edit ends the run.
    t.app.run("edit.redo", json!({})).unwrap();
    load_pad(&mut t, tr, 7, "Shaker.wav");
    let undo = t.undo_len();
    let prev = t.panel(tr).title.prev;
    t.click(mid(prev), 1);
    t.key("enter");
    assert_eq!(t.panel(tr).title.preset_name, "K3");
    assert_eq!(t.undo_len(), undo + 1, "a new run after the edit: its own undo step");
}

#[test]
fn a_missing_sample_shows_the_hatch_and_hover_on_the_pads_view() {
    let (mut t, tr, root) = rig();
    save_as(&mut t, tr, "Kit");
    std::fs::remove_file(t.dir.join("CH Hat.wav")).unwrap();
    pick(&mut t, tr, "Kit");
    assert!(pending(&t, tr).is_none(), "unchanged since its save");
    let p = t.panel(tr);
    let missing: Vec<_> = p.pads.iter().filter_map(|r| r.missing.as_ref().map(|(text, h)| (r.index, text.clone(), h.rect, h.lines.len()))).collect();
    assert_eq!(missing.len(), 1, "{missing:?}");
    let (index, text, rect, lines) = &missing[0];
    assert_eq!((*index, text.as_str()), (1, "CH Hat.wav is missing"));
    assert_eq!(*rect, p.pad(1).unwrap().pad.rect, "the hatch covers the pad's cell");
    assert!(*lines > 0);
    let (note, _) = p.note.as_ref().expect("the load note names the missing file");
    assert!(note.contains("1 sample was not found and plays silence: CH Hat.wav ("), "{note}");
    assert!(p.pad(0).unwrap().missing.is_none(), "the found samples draw as before");
    let _ = root;
}

#[test]
fn a_refusal_shows_the_engines_words_and_leaves_the_kit() {
    let (mut t, tr, root) = rig();
    let dir = files::folder(&root);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Newer.tkit"), r#"{"format": "transit-sampler-kit", "version": 2, "kit": {}}"#).unwrap();
    t.frames(2);
    let refusal = t.app.run("mix.sampler_kit_load", json!({"track": tr.0, "path": dir.join("Newer.tkit"), "replace": true})).unwrap();
    let words = refusal["notice"].as_str().unwrap().to_string();
    let before = doc(&t);
    let undo = t.undo_len();
    pick(&mut t, tr, "Newer");
    t.key("enter");
    let p = t.panel(tr);
    assert_eq!(p.note.expect("the refusal's note").0, words, "the engine's words");
    assert_eq!(t.app.status, words);
    assert_eq!(doc(&t), before, "the kit stays");
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn other_devices_keep_their_preset_browsers() {
    let (mut t, _tr, root) = rig();
    // A Macro Oscillator beside a kit folder that holds a kit.
    std::fs::create_dir_all(files::folder(&root)).unwrap();
    save_kit(&mut t, _tr, &root, "Kit");
    let synth = TrackId(t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.app.focus(synth);
    t.frames(4);
    t.open_card(0);
    let title = |t: &T| t.window(synth).macro_osc.expect("the Macro Oscillator's panel").title.clone();
    let factory = crate::presets::factory_for("macro_oscillator");
    let next = title(&t).next;
    t.click(mid(next), 1);
    assert_eq!((title(&t).preset_group.as_str(), title(&t).preset_name.as_str()), ("Factory", factory[0].0), "its factory presets, as before");
    let field = title(&t).field;
    t.click(mid(field), 1);
    assert!(!painted(&t, "Factory").is_empty(), "its list opens with the Factory heading");
    assert!(painted(&t, "Kit").is_empty(), "no kit in another device's list");
}

#[test]
fn after_a_recall_the_view_bank_and_pad_stay_and_a_zone_the_kit_lacks_is_cleared() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "Kit A");
    load_pad(&mut t, tr, 4, "Rim.wav");
    let u = uid(&t, tr);
    let set = |t: &mut T, zone: u16| {
        let v = t.app.builtin_editor.devices.sampler.views.get_mut(&u).unwrap();
        (v.selected, v.bank, v.instrument.selected, v.slices.selected) = (7, 1, Some(zone), 2);
    };
    let read = |t: &T| {
        let v = &t.app.builtin_editor.devices.sampler.views[&u];
        (v.view, v.selected, v.bank, v.instrument.selected, v.slices.selected)
    };
    // Zone 4 (id 3) is not in the kit recalled: cleared.
    set(&mut t, 3);
    pick(&mut t, tr, "Kit A");
    t.key("enter");
    assert_eq!(t.patch(tr).zones.len(), 3);
    assert_eq!(read(&t), (super::View::Pads, 7, 1, None, 0));
    // A zone the new kit has stays selected.
    load_pad(&mut t, tr, 4, "Rim.wav");
    set(&mut t, 1);
    pick(&mut t, tr, "Kit A");
    t.key("enter");
    assert_eq!(read(&t), (super::View::Pads, 7, 1, Some(1), 0));
}
