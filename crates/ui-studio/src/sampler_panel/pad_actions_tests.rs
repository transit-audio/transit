//! User gestures through the panel, with generated audio and no player.

use super::test_rig::{T, mid};
use serde_json::json;
use transit_model::TrackId;

fn menu(t: &mut T, track: TrackId, pad: u8) {
    let (x, y) = t.pad_at(track, pad);
    t.send("ui.click", json!({"x": x, "y": y, "button": "right"}));
}

fn choose(t: &mut T, track: TrackId, text: &str) {
    let row = t.panel(track).pad_menu.into_iter().find(|row| row.0 == text).expect("menu row").1;
    t.click(mid(row), 1);
}

#[test]
fn clear_pad_removes_only_its_zone_and_resets_metadata_in_one_undo_step() {
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 2, "name": "Closed", "note": 12, "choke": 3, "mute": true, "solo": true}));
    let before = serde_json::to_value(t.patch(tr)).unwrap();
    let undo = t.undo_len();
    menu(&mut t, tr, 1);
    choose(&mut t, tr, "Clear pad");
    let patch = t.patch(tr);
    let pad = patch.pad(1);
    assert!(pad.zone.is_none() && pad.name.is_none() && !pad.mute && !pad.solo && pad.choke == 0, "{pad:?}");
    assert!(patch.zone(1).is_none(), "an otherwise unused zone is removed");
    assert!(patch.zone(0).is_some() && patch.zone(2).is_some(), "other pads keep their zones");
    assert_eq!(patch.pad_note(1), 12, "Clear pad keeps the trigger note; Reset note is separate");
    assert_eq!(t.undo_len(), undo + 1);
    t.run("edit.undo", json!({}));
    assert_eq!(serde_json::to_value(t.patch(tr)).unwrap(), before);
}

#[test]
fn delete_and_backspace_clear_the_selected_pad_without_deleting_the_track() {
    let (mut t, tr) = T::kit();
    for key in ["Delete", "Backspace"] {
        t.click(t.pad_at(tr, 1), 1);
        let undo = t.undo_len();
        t.key(key);
        assert!(t.app.engine.session().track(tr).is_some());
        assert!(t.patch(tr).pad(1).zone.is_none(), "{key}");
        assert_eq!(t.undo_len(), undo + 1, "{key}");
        t.run("edit.undo", json!({}));
        assert!(t.patch(tr).pad(1).zone.is_some());
    }
    let rows = crate::shortcuts::filtered("Clear pad");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, crate::shortcuts::Scope::Sampler);
    assert_eq!(rows[0].1.len(), 2);
}

#[test]
fn reset_note_is_only_shown_for_stored_notes_and_is_one_undo_step() {
    let (mut t, tr) = T::kit();
    menu(&mut t, tr, 0);
    assert!(t.panel(tr).pad_menu.iter().all(|row| row.0 != "Reset note"));
    t.key("Escape");
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 1, "note": 12}));
    let before = serde_json::to_value(t.patch(tr)).unwrap();
    let undo = t.undo_len();
    menu(&mut t, tr, 0);
    choose(&mut t, tr, "Reset note");
    assert!(!t.patch(tr).pad_note_stored(0));
    assert_eq!(t.patch(tr).pad_note(0), 36);
    assert_eq!(t.undo_len(), undo + 1);
    t.run("edit.undo", json!({}));
    assert_eq!(serde_json::to_value(t.patch(tr)).unwrap(), before);
    // The default note is now owned by another pad. The engine's refusal is kept verbatim.
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 17, "note": 36}));
    let undo = t.undo_len();
    let error = t.app.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 1, "note": null})).unwrap_err();
    menu(&mut t, tr, 0);
    choose(&mut t, tr, "Reset note");
    assert_eq!(t.panel(tr).note.unwrap().0, error);
    assert_eq!(t.undo_len(), undo);
    assert_eq!(t.patch(tr).pad_note(0), 12);
}

/// A selected arrangement clip on the Sampler's track (the target of the arrangement's Delete).
fn selected_clip(t: &mut T, track: TrackId) -> u64 {
    let r = t.app.run("midi.new_clip", json!({"tracks": [track.0], "start": 0, "end": 96_000})).unwrap();
    let clip = r["clip"].as_u64().expect("a clip id");
    t.run("edit.select", json!({"clips": [clip]}));
    clip
}

fn clip_alive(t: &T, clip: u64) -> bool {
    t.app.engine.session().tracks.iter().any(|tr| tr.clips().iter().any(|c| c.id.0 == clip))
}

#[test]
fn delete_with_the_pad_menu_open_reaches_neither_the_pad_nor_the_arrangement() {
    let (mut t, tr) = T::kit();
    let clip = selected_clip(&mut t, tr);
    t.click(t.pad_at(tr, 1), 1);
    menu(&mut t, tr, 1);
    assert!(!t.panel(tr).pad_menu.is_empty(), "the menu is open");
    let undo = t.undo_len();
    for key in ["Delete", "Backspace"] {
        t.key(key);
        assert!(clip_alive(&t, clip), "{key} with the menu open keeps the arrangement's selected clip");
        assert!(t.patch(tr).pad(1).zone.is_some(), "{key} with the menu open leaves the pad as it is");
        assert_eq!(t.undo_len(), undo, "{key}");
    }
    // The menu closed, the key clears the pad and still not the clip.
    t.key("Escape");
    t.click(t.pad_at(tr, 1), 1);
    t.key("Delete");
    assert!(t.patch(tr).pad(1).zone.is_none());
    assert!(clip_alive(&t, clip));
}

#[test]
fn clearing_a_pad_with_nothing_to_reset_adds_no_undo_step() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 5), 1);
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    t.key("Delete");
    menu(&mut t, tr, 5);
    choose(&mut t, tr, "Clear pad");
    assert_eq!(t.undo_len(), undo, "an empty pad with nothing set: no step");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before);
}

#[test]
fn clear_on_an_empty_named_pad_resets_it_in_one_step_and_keeps_its_note() {
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 6, "name": "Spare", "note": 110, "choke": 2, "mute": true}));
    let before = serde_json::to_value(t.patch(tr)).unwrap();
    let undo = t.undo_len();
    t.click(t.pad_at(tr, 5), 1);
    t.key("Delete");
    let pad = t.patch(tr).pad(5);
    assert!(pad.zone.is_none() && pad.name.is_none() && pad.choke == 0 && !pad.mute && !pad.solo, "{pad:?}");
    assert_eq!(t.patch(tr).pad_note(5), 110, "the stored note stays");
    assert_eq!(t.undo_len(), undo + 1);
    t.run("edit.undo", json!({}));
    assert_eq!(serde_json::to_value(t.patch(tr)).unwrap(), before);
}

#[test]
fn clear_on_a_slice_bound_pad_frees_the_slice_and_keeps_the_markers_and_loop() {
    let (mut t, tr) = T::open();
    let path = t.sample("Loop.wav", 48_000);
    t.run("mix.sampler_slice_loop", json!({"track": tr.0, "path": path.to_string_lossy()}));
    t.run("mix.sampler_slice_marker", json!({"track": tr.0, "op": "add", "frame": 24_000, "snap": false}));
    t.run("mix.sampler_slices_to_pads", json!({"track": tr.0}));
    let slices = |t: &mut T| t.app.engine.execute("mix.sampler_slice_state", &json!({"track": tr.0})).unwrap()["loop"].clone();
    let before = slices(&mut t);
    assert_eq!(before["manual"][1]["pad"], 2, "slice 2 plays on pad A2");
    let undo = t.undo_len();
    t.click(t.pad_at(tr, 1), 1);
    t.key("Delete");
    let after = slices(&mut t);
    assert!(t.patch(tr).pad(1).zone.is_none());
    assert!(after["manual"][1]["pad"].is_null(), "the slice is unbound");
    let frames = |l: &serde_json::Value| l["manual"].as_array().unwrap().iter().map(|s| s["frame"].clone()).collect::<Vec<_>>();
    assert_eq!(frames(&after), frames(&before), "the markers are untouched");
    assert_eq!(after["source"], before["source"], "the loop keeps its source");
    assert_eq!(after["manual"][0]["pad"], 1, "the other slice keeps its pad");
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn backspace_while_typing_the_start_note_keeps_the_pad() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 1), 1);
    let field = t.panel(tr).bar.start.field;
    t.click(mid(field), 2);
    assert!(t.panel(tr).bar.start.editing, "the field is typing");
    for key in ["Backspace", "Delete"] {
        t.key(key);
        assert!(t.patch(tr).pad(1).zone.is_some(), "{key} goes to the typing");
    }
    assert!(t.panel(tr).bar.start.editing);
}
