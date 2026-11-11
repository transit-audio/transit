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
