//! Follow-ups from the kit presets review (#467): a refused step never holds another gesture's merge
//! open (F1), deleting a kit asks first (P1), Save As over an existing kit asks first (P2), and
//! Replace chosen on a question raised by a previous or next step. Driven the way a user drives
//! them, with a scratch config folder and no wall clock.

use super::kit_presets::{self, DELETE_KIT, REPLACE};
use super::kit_presets_tests::{doc, kit_file, load_pad, open_list, painted, pending, pick, rig, save_as, save_kit, uid};
use super::test_rig::{T, mid};
use serde_json::json;
use std::path::Path;
use transit_engine::io::kit_presets as files;
use transit_model::TrackId;

fn confirm(t: &T, tr: TrackId) -> Option<kit_presets::Confirm> {
    t.app.builtin_editor.devices.sampler.views.get(&uid(t, tr)).and_then(|v| v.kit.confirm.clone()).map(|(c, _)| c)
}

fn confirm_layout(t: &T, tr: TrackId) -> crate::chrome::ModalLayout {
    crate::chrome::modal_layout(&t.ctx, kit_presets::confirm_id(uid(t, tr))).expect("the file question is drawn")
}

fn button(m: &crate::chrome::ModalLayout, text: &str) -> [f32; 4] {
    m.buttons.iter().find(|b| b.text == text).unwrap_or_else(|| panic!("{text}: {:?}", m.buttons)).rect
}

/// Clicks the list's `Delete "name"` entry.
fn click_delete(t: &mut T, tr: TrackId, name: &str) {
    open_list(t, tr);
    let below = t.panel(tr).title.field[3];
    let at = painted(t, &format!("Delete \"{name}\"")).into_iter().find(|p| p.1 > below).expect("Delete is listed");
    t.click(at, 1);
}

/// A recall step through the panel's own code, in this frame (no idle frame between it and what
/// came before).
fn step_now(t: &mut T, tr: TrackId, file: &Path) {
    let key = (tr, transit_mix::INSTRUMENT_SLOT);
    let ins = t.app.builtin_at(key).unwrap();
    let info = transit_dsp::plugin_info(&ins.plugin).unwrap();
    kit_presets::recall(&mut t.app, crate::device_editor::Dev { key, ins: &ins, info }, file, true);
}

/// A level gesture on zone 1, merged under `key` as a knob drag or a wheel is.
fn gesture(t: &mut T, tr: TrackId, value: f32, key: &str) {
    t.app.execute_ui_merged("mix.sampler_control", &json!({"track": tr.0, "id": "zone001_level", "value": value}), key).unwrap();
}

#[test]
fn deleting_a_kit_asks_first_with_no_default_and_escape_cancels() {
    let (mut t, tr, root) = rig();
    save_as(&mut t, tr, "808");
    let file = kit_file(&root, "808");
    assert!(file.is_file());
    let before = doc(&t);
    click_delete(&mut t, tr, "808");
    assert!(matches!(confirm(&t, tr), Some(kit_presets::Confirm::Delete { .. })), "asked");
    assert!(file.is_file(), "nothing deleted before the answer");
    let m = confirm_layout(&t, tr);
    assert_eq!(painted(&t, "Delete the kit \"808\"?").len(), 1);
    assert_eq!(m.detail_text.as_deref(), Some("This cannot be undone."));
    assert_eq!(m.buttons.iter().map(|b| (b.text.as_str(), b.default)).collect::<Vec<_>>(), [("Cancel", false), (DELETE_KIT, false)], "no default");
    t.key("Enter");
    assert!(confirm(&t, tr).is_some() && file.is_file(), "Enter does nothing");
    t.key("Escape");
    assert!(confirm(&t, tr).is_none() && file.is_file(), "Escape cancels");
    click_delete(&mut t, tr, "808");
    let m = confirm_layout(&t, tr);
    t.click(mid(button(&m, DELETE_KIT)), 1);
    assert!(confirm(&t, tr).is_none());
    assert!(!file.exists(), "Delete Kit deletes the file");
    assert_eq!(doc(&t), before, "the kit on the Sampler stays");
}

#[test]
fn save_as_over_an_existing_kit_asks_first_and_save_over_the_windows_own_kit_does_not() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "Kit A");
    let file = kit_file(&root, "Kit A");
    let bytes = std::fs::read(&file).unwrap();
    load_pad(&mut t, tr, 4, "Rim.wav");
    // Save As onto the existing name: asked, and Enter in the field does not answer it.
    save_as(&mut t, tr, "Kit A");
    assert!(matches!(confirm(&t, tr), Some(kit_presets::Confirm::Overwrite { .. })), "asked");
    assert_eq!(painted(&t, "Replace the kit file \"Kit A\"?").len(), 1);
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "nothing written before the answer");
    let m = confirm_layout(&t, tr);
    t.click(mid(button(&m, "Cancel")), 1);
    assert!(confirm(&t, tr).is_none());
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "Cancel writes nothing");
    save_as(&mut t, tr, "Kit A");
    assert!(confirm_layout(&t, tr).buttons.iter().any(|b| b.default && b.text == REPLACE));
    t.key("Enter");
    assert!(confirm(&t, tr).is_none());
    let a: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(a["kit"]["sources"].as_array().unwrap().len(), 4, "Replace wrote the kit");
    assert_eq!(t.panel(tr).title.preset_name, "Kit A");
    // Save over the window's own kit: no question.
    load_pad(&mut t, tr, 5, "Clap.wav");
    let save = t.panel(tr).title.save;
    t.click(mid(save), 1);
    assert!(confirm(&t, tr).is_none(), "Save over this window's kit never asks");
    let a: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(a["kit"]["sources"].as_array().unwrap().len(), 5);
    // Save As a new name: no question.
    save_as(&mut t, tr, "Kit C");
    assert!(confirm(&t, tr).is_none() && kit_file(&root, "Kit C").is_file());
}

#[test]
fn other_devices_delete_their_presets_without_asking() {
    let (mut t, _tr, root) = rig();
    let synth = TrackId(t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.app.focus(synth);
    t.frames(4);
    t.open_card(0);
    let info = transit_dsp::plugin_info("macro_oscillator").unwrap();
    crate::presets::save_user(&root, info, "Mine", &Default::default()).unwrap();
    let title = |t: &T| t.window(synth).macro_osc.expect("the Macro Oscillator's panel").title.clone();
    let field = title(&t).field;
    t.click(mid(field), 1);
    let at = painted(&t, "Mine").into_iter().find(|p| p.1 > field[3]).expect("the user preset is listed");
    t.click(at, 1);
    assert_eq!(title(&t).preset_name, "Mine");
    t.click(mid(field), 1);
    let at = painted(&t, "Delete \"Mine\"").into_iter().find(|p| p.1 > field[3]).expect("Delete is listed");
    t.click(at, 1);
    assert!(crate::presets::user_names(&root, "macro_oscillator").is_empty(), "deleted at once, as before");
    assert!(painted(&t, "Delete the kit \"Mine\"?").is_empty() && painted(&t, DELETE_KIT).is_empty(), "no question");
}

#[test]
fn replace_on_a_question_raised_by_a_step_starts_the_run_and_the_run_is_one_undo_step() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "K1");
    load_pad(&mut t, tr, 4, "Rim.wav");
    save_kit(&mut t, tr, &root, "K2");
    load_pad(&mut t, tr, 5, "Clap.wav");
    let changed = t.patch(tr);
    let undo = t.undo_len();
    let next = t.panel(tr).title.next;
    t.click(mid(next), 1);
    let asked = pending(&t, tr).expect("a changed kit: the step asks");
    assert!(asked.step);
    t.key("Enter");
    assert_eq!(t.patch(tr).zones.len(), 3, "Replace recalled K1");
    assert_eq!(t.undo_len(), undo + 1);
    t.click(mid(next), 1);
    assert!(pending(&t, tr).is_none());
    assert_eq!(t.patch(tr).zones.len(), 4, "K2");
    assert_eq!(t.undo_len(), undo + 1, "the step after the answer joins the same undo step");
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(t.patch(tr), changed, "one undo returns to the kit before the run");
}

#[test]
fn a_refused_step_never_holds_another_gestures_merge_open() {
    let (mut t, tr, root) = rig();
    save_kit(&mut t, tr, &root, "K1");
    load_pad(&mut t, tr, 4, "Rim.wav");
    save_kit(&mut t, tr, &root, "K2");
    let newer = files::folder(&root).join("Newer.tkit");
    std::fs::write(&newer, r#"{"format": "transit-sampler-kit", "version": 2, "kit": {}}"#).unwrap();
    t.frames(2);
    // A run: K1 (asked, Replace), K2, then the refusing kit: the kit of the last step stays.
    pick(&mut t, tr, "K1");
    t.key("Enter");
    let undo = t.undo_len();
    step_now(&mut t, tr, &kit_file(&root, "K2"));
    t.frames(2);
    let k2 = doc(&t);
    step_now(&mut t, tr, &newer);
    t.frames(2);
    assert_eq!(doc(&t), k2, "the refusal leaves the kit of the last successful step");
    assert_eq!(t.undo_len(), undo + 1);
    // A later gesture on another control is its own undo step.
    gesture(&mut t, tr, -3.0, "knob");
    t.frames(2);
    assert_eq!(t.undo_len(), undo + 2, "a gesture after the run: its own undo step");
    // A gesture still merging when a step fails with an error in its frame (bad parameters here):
    // the error does not start a run, so the idle
    // frames end the gesture and the next one is its own undo step. (A refusal answered as data
    // ends the other merge in the engine already.)
    let undo = t.undo_len();
    gesture(&mut t, tr, -6.0, "wheel");
    step_now(&mut t, tr, Path::new(""));
    assert!(!kit_presets::stepping(&t.app), "a refused step is no run");
    t.frames(2);
    gesture(&mut t, tr, -9.0, "wheel");
    t.frames(2);
    assert_eq!(t.undo_len(), undo + 2, "two gestures, two undo steps");
}
