//! The legacy one-sample path stays reachable in the new window: the sample line, Load Sample,
//! Clear and the parameter rows, a dropped file loading as the sample, and New Kit switching the
//! window to the pads in one undo step.

use super::test_rig::{T, mid};
use serde_json::json;

#[test]
fn a_one_sample_sampler_shows_its_legacy_controls_and_new_kit() {
    let mut t = T::new();
    let tr = t.sampler_track();
    t.open_card(0);
    let w = t.window(tr);
    let p = t.panel(tr);
    let legacy = p.legacy.clone().expect("the legacy body");
    assert!(p.pads.is_empty(), "no pads before a kit");
    assert_eq!(w.sample, "No sample");
    assert!(w.picker != [0.0; 4]);
    // Every legacy parameter has its row, inside the card.
    let ids: Vec<&str> = super::test_rig::info().params.iter().map(|p| p.id).collect();
    for id in &ids {
        let r = t.row(tr, id);
        assert!(r.control[1] >= legacy.panel[1], "{id} in the card");
    }
    assert!(legacy.new_kit[1] > w.picker[1] && legacy.new_kit[3] <= legacy.panel[3]);
    // The window holds the card: content-sized.
    assert!(p.footer[1] >= legacy.panel[3]);
    // A file dropped on the window loads as the one sample.
    let hit = t.sample("Hit.wav", 2400);
    t.drop_file(&hit, mid(legacy.panel));
    assert!(t.window(tr).sample.starts_with("Hit"), "{}", t.window(tr).sample);
    // New Kit: one step, the window shows the pads, the one sample is left unset.
    let before = t.undo_len();
    t.click(mid(t.panel(tr).legacy.unwrap().new_kit), 1);
    assert_eq!(t.undo_len(), before + 1);
    let p = t.panel(tr);
    assert!(p.legacy.is_none() && p.pads.len() == 16);
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert!(ins.sample.is_none() && ins.sampler_patch.is_some());
    // Undo goes back to the one sample and its controls.
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert!(t.panel(tr).legacy.is_some());
    assert!(t.window(tr).sample.starts_with("Hit"));
}

#[test]
fn a_kits_window_takes_no_dropped_file_as_the_one_sample() {
    // Negative control for the legacy drop: on the pads, a file dropped off the grid is not loaded.
    let (mut t, tr) = T::open();
    let hit = t.sample("Hit.wav", 2400);
    let footer = t.panel(tr).footer;
    t.drop_file(&hit, mid(footer));
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert!(ins.sample.is_none());
    assert!(t.patch(tr).zones.is_empty());
}
