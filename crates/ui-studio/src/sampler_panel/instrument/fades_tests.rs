//! Edge fades through the panel's rig: the wedge the key map draws from the widths playback uses,
//! `fades none`, and the Zone panel's four fields as one edit with the engine's refusals.

use super::super::test_rig::{T, mid};
use super::key_map::Keys;
use super::rig::*;
use crate::tokens::sampler as tk;
use serde_json::json;

fn q(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

#[test]
fn fade_wedge_drawn_and_fades_none_when_zero() {
    let (mut t, tr) = layered();
    let k = layout(&t, tr).key_map;
    assert!(k.zones.iter().all(|z| z.fade.is_none()), "no widths, no wedge");
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades none");
    // Zone 1 (keys 48 to 52, velocities 1 to 63): two keys up from the low edge, ten velocities
    // down from the top.
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"fades": {"key_lo": 2, "key_hi": 0, "vel_lo": 0, "vel_hi": 10}}}));
    let k = layout(&t, tr).key_map;
    let z1 = k.zone(1).unwrap();
    let (inner, floor) = z1.fade.expect("the wedge");
    assert_eq!(floor, tk::FADE_FLOOR_ALPHA);
    let keys = Keys::new(k.lo, k.hi, k.frame.0, k.frame.1);
    let (top, bottom) = (k.well[1] + 1.0, k.well[3] - 1.0);
    let y = |v: f32| bottom - v / 127.0 * (bottom - top);
    // Full from key 50 (48 + 2) and up to velocity 53 (63 - 10); the other two edges unfaded.
    assert_eq!(inner, [q(keys.span(50).0), q(y(53.0)), z1.rect[2], z1.rect[3]]);
    assert!(k.zone(2).unwrap().fade.is_none() && k.zone(3).unwrap().fade.is_none());
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades 2 · 0 · 0 · 10");
}

/// Opens the fades fields (a click on `none`) and types `text` into field `i`, then Enter.
fn type_fade(t: &mut T, tr: transit_model::TrackId, i: usize, text: &str) {
    let z = layout(t, tr).zone.unwrap();
    if z.fade_fields.is_empty() {
        let r = z.fields[3].1;
        t.click((r[2] - 4.0, (r[1] + r[3]) / 2.0), 1);
    }
    let f = layout(t, tr).zone.unwrap().fade_fields;
    assert_eq!(f.len(), 4, "the four fields");
    t.click(mid(f[i]), 1);
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": text}));
    t.key("Enter");
}

#[test]
fn the_fade_fields_send_the_four_widths_as_one_edit() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    type_fade(&mut t, tr, 0, "3");
    assert_eq!(
        sent(&t, log),
        vec!["mix.sampler_zone_edit {\"playback\":{\"fades\":{\"key_hi\":0,\"key_lo\":3,\"vel_hi\":0,\"vel_lo\":0}},\"zone\":1}".to_string()]
    );
    assert_eq!(t.patch(tr).zone(0).unwrap().fades.widths(), [3, 0, 0, 0]);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades 3 · 0 · 0 · 0");
    assert!(layout(&t, tr).key_map.zone(1).unwrap().fade.is_some(), "the wedge follows");
}

#[test]
fn fade_refusals_are_the_engines_text() {
    let (mut t, tr) = layered();
    let undo = t.undo_len();
    // Zone 1 spans keys 48 to 52: at most 4 keys of fade together.
    type_fade(&mut t, tr, 1, "9");
    assert_eq!(layout(&t, tr).note.expect("the note").0, "Key fades are at most 4 keys together for this zone.");
    assert_eq!(t.patch(tr).zone(0).unwrap().fades.widths(), [0; 4]);
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades none", "the field reads the stored widths again");
    // A pad's zone has none.
    let z3 = layout(&t, tr).key_map.zone(3).unwrap().rect;
    t.click(mid(z3), 1);
    type_fade(&mut t, tr, 0, "1");
    assert_eq!(layout(&t, tr).note.unwrap().0, "A pad's zone has no fades: it plays one key and one velocity layer.");
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn escape_in_a_fade_field_gives_the_typing_up_and_keeps_the_window() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let r = layout(&t, tr).zone.unwrap().fields[3].1;
    t.click((r[2] - 4.0, (r[1] + r[3]) / 2.0), 1);
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": "2"}));
    t.key("Escape");
    assert!(!t.app.builtin_editor.windows.is_empty(), "the window stays");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades none");
}
