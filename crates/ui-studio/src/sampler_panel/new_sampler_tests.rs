//! A Sampler created now starts as an empty kit (#454), in the step that creates it; recalling the
//! factory default on a kit leaves the kit.

use super::test_rig::T;
use serde_json::json;

#[test]
fn a_new_instrument_track_with_a_sampler_opens_on_an_empty_kit_in_one_step() {
    let mut t = T::new();
    let (undo, tracks) = (t.undo_len(), t.app.engine.session().tracks.len());
    // The browser's drop below the tracks: `track.new` and `mix.instrument` as one step.
    t.app.drop_plugin("sampler", "Sampler", true, crate::browser::Target::Below { at: None }).unwrap();
    t.frames(3);
    assert_eq!(t.undo_len(), undo + 1, "one step");
    let tr = t.app.engine.session().tracks.last().unwrap().id;
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert!(ins.sample.is_none(), "no one-sample field");
    // The empty kit New Kit makes (its default seed, 1).
    assert_eq!(t.patch(tr), transit_model::sampler_patch::PatchV1 { seed: 1, ..Default::default() });
    t.app.focus(tr);
    t.frames(4);
    t.open_card(0);
    let p = t.panel(tr);
    assert_eq!(p.pads.len(), 16, "the Pads view");
    assert!(p.bar.load_kit.is_some() && p.bar.load_enabled, "Load kit available");
    // One undo removes the track and its Sampler.
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.app.engine.session().tracks.len(), tracks);
}

#[test]
fn the_factory_default_on_a_kit_leaves_the_kit() {
    let (mut t, tr) = T::kit();
    let before = t.patch(tr);
    let info = transit_dsp::plugin_info("sampler").unwrap();
    let preset = crate::presets::Preset { plugin: "sampler".into(), version: 1, params: Default::default() };
    t.app.apply_preset((tr, transit_mix::INSTRUMENT_SLOT), info, "<factory default>", &preset);
    t.frames(3);
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert!(ins.sample.is_none() && ins.sampler_patch.is_some(), "still a kit");
    assert_eq!(t.patch(tr), before, "its zones and pads kept");
    let p = t.panel(tr);
    assert_eq!(p.pads.len(), 16);
}
