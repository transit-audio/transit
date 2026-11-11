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
