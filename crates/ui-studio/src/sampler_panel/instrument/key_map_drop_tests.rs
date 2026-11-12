//! Another sample onto free keys while zones exist (#492, part B): a file dropped on free keys
//! makes a zone over the whole free run rooted under the pointer in one step, a drop on covered
//! keys is refused and changes nothing, Load… with zones lands on the lowest free run, and with no
//! free key it is unavailable.

use super::super::test_rig::{T, mid};
use super::key_map_drop;
use super::rig::*;
use serde_json::json;
use transit_model::TrackId;

/// Zones over keys 0 to 4, 40 to 47 and 60 to 72, and a sample on pad A1 (zone 4, its note 36):
/// free runs 5..35, 37..39, 48..59 and 73..127. The Instrument view showing.
fn gaps() -> (T, TrackId) {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "Low.wav", 4_800, json!({"keys": [0, 4], "root": 2}));
    zone(&mut t, tr, 2, "Mid.wav", 4_800, json!({"keys": [40, 47], "root": 43}));
    zone(&mut t, tr, 3, "High.wav", 4_800, json!({"keys": [60, 72], "root": 66}));
    let path = t.sample("Kick.wav", 4_800);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": 4, "pad": 1, "path": path.to_string_lossy()}));
    show_instrument(&mut t, tr);
    (t, tr)
}

/// A point over key `note` in the well, halfway up.
fn over(t: &T, tr: TrackId, note: u8) -> (f32, f32) {
    let w = layout(t, tr).key_map.well;
    (key_x(t, tr, note), (w[1] + w[3]) / 2.0)
}

fn zone_of(t: &T, tr: TrackId, n: u16) -> Option<([u8; 2], [u8; 2], u8)> {
    t.patch(tr).zone(n - 1).map(|z| (z.keys, z.velocities, z.root))
}

#[test]
fn a_drop_on_free_keys_between_two_zones_makes_a_zone_over_the_free_run_in_one_step() {
    let (mut t, tr) = gaps();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let file = t.sample("Between.wav", 4_800);
    let at = over(&t, tr, 50);
    t.drop_file(&file, at);
    assert_eq!(zone_of(&t, tr, 5), Some(([48, 59], [1, 127], 50)), "the free run around key 50, rooted under the pointer: {:?}", layout(&t, tr).note);
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].starts_with("mix.sampler_zone_source ") && s[0].contains("\"zone\":5"), "one command: {s:?}");
    assert_eq!(view(&t).selected, Some(5), "the new zone is selected");
    for (n, keys) in [(1, [0, 4]), (2, [40, 47]), (3, [60, 72])] {
        assert_eq!(zone_of(&t, tr, n).unwrap().0, keys, "zone {n} unchanged");
    }
    // A pad's zone covers only its pad's note: the keys either side of it are free.
    let file = t.sample("BesidePad.wav", 4_800);
    let at = over(&t, tr, 38);
    t.drop_file(&file, at);
    assert_eq!(zone_of(&t, tr, 6), Some(([37, 39], [1, 127], 38)));
}

#[test]
fn a_drop_on_covered_keys_is_refused_and_changes_nothing() {
    let (mut t, tr) = gaps();
    let before = t.patch(tr);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let file = t.sample("Over.wav", 4_800);
    let at = over(&t, tr, 44);
    t.drop_file(&file, at);
    assert_eq!(t.patch(tr), before, "nothing changes");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "no command");
    let note = layout(&t, tr).note.expect("the reason under the map").0;
    let name = transit_midi::note_name(44, t.app.ui.middle_c);
    assert_eq!(note, format!("{name} already plays zone 2: drop on a free key (layers on the key map come in a later release)."));
    // The pad's note is covered too.
    let at = over(&t, tr, 36);
    t.drop_file(&file, at);
    assert_eq!(t.patch(tr), before);
    assert!(layout(&t, tr).note.unwrap().0.contains("zone 4"), "the pad's zone plays its note");
}

#[test]
fn load_with_zones_lands_on_the_lowest_free_run_rooted_at_its_first_c() {
    let (mut t, tr) = gaps();
    let path = t.sample("Loaded.wav", 4_800);
    let p = path.to_string_lossy().to_string();
    t.app.services.pick_open = Some(Box::new(move |purpose, _| {
        assert_eq!(purpose, "mix.sampler_zone_source");
        Some(p.clone())
    }));
    let k = layout(&t, tr).key_map;
    let (load, why) = k.load.clone().expect("Load… in the label row with zones");
    assert_eq!(why, None, "available while a key is free");
    assert!(load[0] > k.hint[2] && load[2] <= k.well[2] + 0.01, "right of the hint, inside the panel: {load:?} {:?}", k.hint);
    let undo = t.undo_len();
    t.click(mid(load), 1);
    assert_eq!(zone_of(&t, tr, 5), Some(([5, 35], [1, 127], 12)), "keys 5 to 35, rooted at C (12): {:?}", layout(&t, tr).note);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(view(&t).selected, Some(5));
    // At the window's minimum width Load… still clears the hint.
    t.screen = egui::vec2(super::super::MIN_WIDTH + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(4);
    let k = layout(&t, tr).key_map;
    let (load, _) = k.load.clone().unwrap();
    assert!(load[0] > k.hint[2] && load[2] <= k.well[2] + 0.01, "at 856: {load:?} {:?}", k.hint);
    // Run without a C: its lowest key.
    let state = view(&t).snapshot.data.unwrap();
    assert_eq!(key_map_drop::lowest_free(&state), Some(([37, 39], 37)));
}

#[test]
fn with_no_free_key_load_is_unavailable_with_the_reason() {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "All.wav", 4_800, json!({"keys": [0, 127]}));
    show_instrument(&mut t, tr);
    t.app.services.pick_open = Some(Box::new(|_, _| panic!("no picker for an unavailable Load…")));
    let (load, why) = layout(&t, tr).key_map.load.expect("drawn, unavailable");
    assert_eq!(why.as_deref(), Some(key_map_drop::FULL));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(load), 1);
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
}
