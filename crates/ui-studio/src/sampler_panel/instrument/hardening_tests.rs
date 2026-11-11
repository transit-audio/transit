//! Pointer edge cases of the key map and the waveform handles through the panel's rig: hit areas
//! that never overlap, drags released outside the window, and a drag whose widget went away.

use super::super::test_rig::{T, mid};
use super::rig::*;
use serde_json::json;

/// A zone over a one-second 48 kHz WAV with its region set to `start..end`, the Instrument view
/// showing.
fn region(start: u64, end: u64) -> (T, transit_model::TrackId) {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "One.wav", 48_000, json!({"keys": [48, 52]}));
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"start": start, "end": end}}));
    show_instrument(&mut t, tr);
    (t, tr)
}

#[test]
fn close_handles_each_take_their_own_press() {
    let (mut t, tr) = region(1000, 1500);
    let w = layout(&t, tr).zone.unwrap().wave;
    let (s_flag, e_flag, e_line) = (w.start_flag, w.end_flag, w.end_line);
    let edge = super::key_map::EDGE;
    // A point on the S flag's visible part that E's line band also reaches.
    let x = e_flag[2] + 2.0;
    assert!(x > s_flag[0] && x < s_flag[2] && x > e_flag[2] && x < e_line[2] + edge, "x {x} s {s_flag:?} e {e_flag:?} line {e_line:?}");
    let y = mid(s_flag).1;
    let log = view(&t).sent.len();
    drag(&mut t, (x, y), (x + 40.0, y));
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("\"start\"") && !s[0].contains("\"end\""), "the S flag's press moves S: {s:?}");
    // And E's flag, where it shows, takes E's press.
    let w = layout(&t, tr).zone.unwrap().wave;
    let (ex, ey) = (w.end_flag[0] + 1.0, mid(w.end_flag).1);
    assert!(ex < w.start_flag[0], "on E's flag, left of S's: {ex} {:?}", w.start_flag);
    let log = view(&t).sent.len();
    drag(&mut t, (ex, ey), (ex + 200.0, ey));
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].contains("\"end\""), "{s:?}");
}

#[test]
fn a_handle_drag_released_outside_the_window_sends_one_edit_and_clears() {
    let (mut t, tr) = region(1000, 20_000);
    let w = layout(&t, tr).zone.unwrap().wave;
    let from = mid(w.end_flag);
    let out = (t.screen.x + 300.0, t.screen.y + 300.0);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 + 30.0, from.1)), moved(out), press(out, false)]);
    assert_eq!(sent(&t, log), vec!["mix.sampler_zone_edit {\"region\":{\"end\":48000},\"zone\":1}".to_string()]);
    assert_eq!(t.undo_len(), undo + 1);
    assert!(view(&t).wave_drag.is_none(), "nothing stays held");
}

#[test]
fn a_key_map_drag_released_outside_the_window_sends_one_edit_and_clears() {
    let (mut t, tr) = layered();
    let r = layout(&t, tr).key_map.zone(1).unwrap().rect;
    let from = (r[2] - 2.0, (r[1] + r[3]) / 2.0);
    let out = (t.screen.x + 300.0, -200.0);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 + 30.0, from.1)), moved(out), press(out, false)]);
    assert_eq!(sent(&t, log).len(), 1);
    assert_eq!(stored(&t, tr, 1).0, [48, layout(&t, tr).key_map.hi], "clamped to the last key shown");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(view(&t).drag.is_none());
}

#[test]
fn a_drag_whose_widget_went_away_is_dropped_and_escape_closes_the_window_again() {
    let (mut t, tr) = region(1000, 20_000);
    let w = layout(&t, tr).zone.unwrap().wave;
    let from = mid(w.end_flag);
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 + 30.0, from.1))]);
    assert!(view(&t).wave_drag.is_some());
    // The zone goes while the button is held: the Zone panel and its handles are not drawn.
    t.run("mix.sampler_zone_clear", json!({"track": tr.0, "zone": 1}));
    let log = view(&t).sent.len();
    steps(&mut t, vec![press((from.0 + 30.0, from.1), false)]);
    assert!(view(&t).wave_drag.is_none(), "the drag ends with the button");
    assert_eq!(sent(&t, log).len(), 0);
    t.key("Escape");
    assert!(t.app.builtin_editor.windows.is_empty(), "Escape is the window's again");
}
