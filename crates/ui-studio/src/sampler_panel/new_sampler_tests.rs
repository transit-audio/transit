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
    // The empty kit `mix.sampler_new_patch` makes (its default seed, 1).
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

#[test]
fn the_sampler_with_no_kit_files_saves_nothing_and_never_writes_a_parameter_preset() {
    // The Sampler's presets are kit files (#467): with none listed, previous and next do nothing,
    // and Save opens the list for Save As; the parameter preset path writes nothing for it.
    let dir = std::env::temp_dir().join(format!("transit-sampler-presets-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (mut t, tr) = T::kit();
    t.app.services.config_dir = Some(dir.clone());
    t.frames(2);
    let title = t.panel(tr).title;
    assert_eq!((title.preset_group.as_str(), title.preset_name.as_str()), ("", "<factory default>"));
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    for r in [title.prev, title.next, title.save] {
        t.click(super::test_rig::mid(r), 1);
    }
    t.key("escape");
    t.frames(2);
    assert!(!dir.join("presets").exists(), "no preset file is written");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before, "nothing changes");
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.builtin_editor.windows.iter().any(|w| w.key.0 == tr), "the window stays open");
    // Save As itself (what the list's Save As sends): nothing for the Sampler, a file for another
    // built-in instrument.
    let synth = t.app.run("track.new", json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let save_as = |t: &mut T, track: u64, name: &str| {
        let key = (transit_model::TrackId(track), transit_mix::INSTRUMENT_SLOT);
        let ins = t.app.builtin_at(key).unwrap();
        let info = transit_dsp::plugin_info(&ins.plugin).unwrap();
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let picked = crate::presets::Picked { save: Some(name.to_string()), ..Default::default() };
            crate::presets::act(&mut t.app, ui, key, &ins, info, picked, egui::Id::new("t"));
        });
        out.textures_delta.clear();
    };
    save_as(&mut t, tr.0, "Kit");
    assert!(crate::presets::user_names(&dir, "sampler").is_empty() && !dir.join("presets").join("sampler").exists(), "no Sampler preset written");
    save_as(&mut t, synth, "Mine");
    assert_eq!(crate::presets::user_names(&dir, "macro_oscillator"), ["Mine"], "the Macro Oscillator's Save As still writes");
    let _ = std::fs::remove_dir_all(dir);
}
