//! Typing in the Instrument view through the panel's rig: the crossfade field's Escape, its
//! refusals, its error state, and typed values that never land on another zone.

use super::super::test_rig::{T, mid};
use super::rig::*;
use super::zone_panel;
use serde_json::json;

/// Zones 1 (keys 48 to 52) and 2 (keys 55 to 59) over one-second 48 kHz WAVs, both looping
/// forward over the whole file, the Instrument view showing.
fn two_loops() -> (T, transit_model::TrackId) {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "A.wav", 48_000, json!({"keys": [48, 52]}));
    zone(&mut t, tr, 2, "B.wav", 48_000, json!({"keys": [55, 59]}));
    for z in [1, 2] {
        t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": z, "region": {"loop_mode": "forward"}}));
    }
    show_instrument(&mut t, tr);
    (t, tr)
}

/// Clicks the crossfade field and types `text` over what it holds (no Enter).
fn type_crossfade(t: &mut T, tr: transit_model::TrackId, text: &str) {
    let field = layout(t, tr).zone.unwrap().crossfade;
    t.click(mid(field), 1);
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": text}));
}

#[test]
fn escape_in_the_crossfade_field_gives_the_typing_up_and_keeps_the_window() {
    let (mut t, tr) = two_loops();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_crossfade(&mut t, tr, "20");
    assert!(layout(&t, tr).zone.unwrap().typing);
    t.key("Escape");
    assert!(!t.app.builtin_editor.windows.is_empty(), "Escape gave the typing up; the window stays");
    let z = layout(&t, tr).zone.unwrap();
    assert_eq!((z.crossfade_text.as_str(), z.typing), ("0.0 ms", false), "the field reads the stored value again");
    assert_eq!((t.patch(tr).zone(0).unwrap().crossfade, t.undo_len(), sent(&t, log).len()), (0, undo, 0));
    t.key("Escape");
    assert!(t.app.builtin_editor.windows.is_empty(), "with nothing typed, Escape closes the window");
}

#[test]
fn an_over_limit_crossfade_shows_the_engines_refusal() {
    let (mut t, tr) = two_loops();
    let undo = t.undo_len();
    // The loop is 48,000 frames: at most 24,000 frames (500.0 ms) of crossfade.
    type_crossfade(&mut t, tr, "900");
    t.key("Enter");
    assert_eq!(t.patch(tr).zone(0).unwrap().crossfade, 0);
    assert_eq!(t.undo_len(), undo);
    let note = layout(&t, tr).note.expect("the refusal in the note row").0;
    assert!(note.starts_with("Crossfade is at most 500.0 ms (24000 frames)"), "{note}");
    assert_eq!(layout(&t, tr).zone.unwrap().crossfade_text, "0.0 ms", "the field keeps the engine's value");
}

#[test]
fn text_that_is_not_a_crossfade_keeps_the_error_ring_and_sends_nothing() {
    let (mut t, tr) = two_loops();
    let log = view(&t).sent.len();
    type_crossfade(&mut t, tr, "twelve");
    t.key("Enter");
    let z = layout(&t, tr).zone.unwrap();
    assert!(z.crossfade_error, "the error ring: {z:?} note {:?} sent {:?}", layout(&t, tr).note, sent(&t, log));
    assert_eq!(z.crossfade_text, "twelve", "the typing stays to be corrected");
    assert_eq!(layout(&t, tr).note.unwrap().0, zone_panel::invalid("twelve"));
    assert_eq!(sent(&t, log).len(), 0);
    // Correcting it commits and clears the ring.
    type_crossfade(&mut t, tr, "10");
    t.key("Enter");
    assert_eq!(t.patch(tr).zone(0).unwrap().crossfade, 480);
    assert!(!layout(&t, tr).zone.unwrap().crossfade_error);
}

#[test]
fn a_crossfade_typed_for_one_zone_commits_to_it_when_another_zone_is_selected() {
    let (mut t, tr) = two_loops();
    assert_eq!(view(&t).selected, Some(1));
    let undo = t.undo_len();
    type_crossfade(&mut t, tr, "10");
    let z2 = layout(&t, tr).key_map.zone(2).unwrap().rect;
    t.click(mid(z2), 1);
    assert_eq!(view(&t).selected, Some(2));
    assert_eq!(t.patch(tr).zone(0).unwrap().crossfade, 480, "zone 1 takes what was typed for it");
    assert_eq!(t.patch(tr).zone(1).unwrap().crossfade, 0, "zone 2 is untouched");
    assert_eq!(t.undo_len(), undo + 1);
    let z = layout(&t, tr).zone.unwrap();
    assert_eq!((z.crossfade_text.as_str(), z.typing), ("0.0 ms", false), "zone 2's own value shows");
}

#[test]
fn a_knob_value_typed_for_one_zone_never_lands_on_another() {
    let (mut t, tr) = layered();
    let knob = |t: &T| t.panel(tr).knobs.into_iter().find(|k| k.field == "transpose").expect("the Tune knob");
    t.click(mid(knob(&t).knob.value), 1);
    assert!(knob(&t).knob.editing);
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": "5"}));
    let z2 = layout(&t, tr).key_map.zone(2).unwrap().rect;
    t.click(mid(z2), 1);
    assert_eq!(view(&t).selected, Some(2));
    t.key("Enter");
    let p = t.patch(tr);
    assert_eq!((p.zone(0).unwrap().transpose, p.zone(1).unwrap().transpose), (0.0, 0.0), "the typing was given up, not moved to zone 2");
    assert!(!knob(&t).knob.editing);
}
