//! ADR 0051 decision 4: parameter labels baked into DSP. The Resonator's Tonic and the Random
//! Sequencer's Octave keep their stored index and their fixed C3 = 60 `choices`; the UI names them
//! through one helper, `middle_c::param_text`, in the Middle C setting, at every site that shows a
//! parameter's value text. The control channel's listing does not follow the setting.

use crate::middle_c::{choice_texts, param_text, parse_note_entry};
use crate::middle_c_tests::{Rig, minus};
use serde_json::{Value, json};
use transit_dsp::ParamInfo;
use transit_midi::MiddleC::{C3, C4};
use transit_model::TrackId;

fn tonic() -> &'static ParamInfo {
    transit_dsp::plugin_info("resonator").unwrap().param("tonic").unwrap()
}

fn octave() -> &'static ParamInfo {
    transit_dsp::midi_fx::info("random_sequencer").unwrap().param("octave").unwrap()
}

#[test]
fn param_text_names_each_entry_in_the_setting_and_c3_matches_the_baked_names() {
    let (t, o) = (tonic(), octave());
    for (i, c3, c4) in [(0.0, "C-1", "C0"), (24.0, "C1", "C2"), (48.0, "C3", "C4"), (60.0, "C4", "C5"), (1.0, "C#-1", "C#0")] {
        assert_eq!((param_text(t, i, C3), param_text(t, i, C4)), (c3.to_string(), c4.to_string()), "tonic {i}");
    }
    for (i, c3, c4) in [(0.0, "-2", "-1"), (4.0, "2", "3"), (8.0, "6", "7")] {
        assert_eq!((param_text(o, i, C3), param_text(o, i, C4)), (c3.to_string(), c4.to_string()), "octave {i}");
    }
    // Under C3 the helper reads exactly the fixed strings the listing keeps.
    for p in [t, o] {
        let fixed: Vec<String> = p.choices.iter().map(|c| (*c).to_string()).collect();
        assert_eq!(choice_texts(p, C3), fixed, "{}", p.id);
        assert_ne!(choice_texts(p, C4), fixed, "{} follows the setting", p.id);
        assert_eq!((0..p.choices.len()).map(|i| p.format(i as f32)).collect::<Vec<_>>(), fixed, "format keeps the fixed names");
    }
    // Control: a parameter without the marker reads its own format in either setting.
    let model = transit_dsp::plugin_info("resonator").unwrap().param("model").unwrap();
    assert_eq!((param_text(model, 1.0, C3), param_text(model, 1.0, C4)), (model.format(1.0), model.format(1.0)));
    let cutoff = transit_dsp::plugin_info("eq_7band").unwrap().param("hpf_freq").unwrap();
    assert_eq!(param_text(cutoff, 1234.0, C4), cutoff.format(1234.0));
}

#[test]
fn a_typed_note_reads_through_the_marker_before_the_fixed_names() {
    let t = tonic();
    assert_eq!(parse_note_entry(t, "C1", C3), Some(24.0));
    assert_eq!(parse_note_entry(t, "C1", C4), Some(12.0), "C1 under C4 is MIDI 24, entry 12");
    assert_eq!(parse_note_entry(t, "db1", C3), Some(25.0));
    assert_eq!(parse_note_entry(t, "C9", C3), None, "past the list");
    assert_eq!(parse_note_entry(t, "60", C3), None, "a number is not a note here");
    let o = octave();
    assert_eq!(parse_note_entry(o, "3", C4), Some(4.0));
    assert_eq!(parse_note_entry(o, "3", C3), Some(5.0));
    assert_eq!(parse_note_entry(o, "7", C3), None);
    // `device_editor::parse` checks the marker first: under C4 a typed C1 is never the C3 string.
    assert_eq!(crate::device_editor::parse(t, "C1", C4), Some(12.0));
    assert_eq!(crate::device_editor::parse(t, "C1", C3), Some(24.0));
    // Negative control: a parameter without the marker still matches its choices by name.
    let model = transit_dsp::plugin_info("resonator").unwrap().param("model").unwrap();
    assert_eq!(crate::device_editor::parse(model, model.choices[2], C4), Some(2.0));
}

/// An instrument track with a Resonator, the devices pane shown.
fn resonator(r: &mut Rig) -> TrackId {
    r.app.ui.show_devices = true;
    let t = crate::tests::new_track(&mut r.app, json!({"kind": "instrument"}));
    r.app.run("mix.instrument", json!({"track": t.0, "plugin": "resonator"})).unwrap();
    r.app.focus(t);
    r.frames(4);
    t
}

fn editor_row(r: &Rig, t: TrackId, id: &str) -> String {
    let w = r.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).expect("the editor window");
    w.rows.iter().find(|row| row.param == id).map(|row| row.value.clone()).unwrap_or_else(|| panic!("a {id} row"))
}

#[test]
fn the_resonator_tonic_in_its_editor_follows_the_setting() {
    let mut r = Rig::new();
    let t = resonator(&mut r);
    let button = r.app.mixer.devices.cards[0].editor_button;
    r.click(button);
    assert_eq!(editor_row(&r, t, "tonic"), "C1");
    let (gone, new) = r.flip(C4);
    assert_eq!(editor_row(&r, t, "tonic"), "C2", "the next frame");
    assert_eq!((gone, new), (vec!["C1".to_string()], vec!["C2".to_string()]), "nothing else in the frame changed");
    // The stored value is the index, untouched by the flip.
    let stored = r.app.engine.session().track(t).unwrap().instrument.as_ref().unwrap().params.get("tonic").copied();
    assert!(stored.is_none() || stored == Some(24.0), "{stored:?}");
}

#[test]
fn the_automation_lane_value_text_for_the_tonic_follows_the_setting() {
    let mut r = Rig::new();
    let t = resonator(&mut r);
    r.app.toggle_automation(t);
    r.frames(3);
    r.app.pick_lane(t, "volume", "plugin:instrument:tonic");
    r.frames(3);
    let readout = |r: &Rig| r.app.auto.layout.lanes.iter().find(|l| l.lane == "plugin:instrument:tonic").map(|l| l.readout.clone()).unwrap();
    assert_eq!(readout(&r), "C1");
    let (gone, new) = r.flip(C4);
    assert_eq!(readout(&r), "C2", "the next frame");
    assert_eq!((gone, new), (vec!["C1".to_string()], vec!["C2".to_string()]));
}

#[test]
fn the_param_knob_value_text_names_the_tonic_in_the_setting() {
    // The macro strip and device page knob (`widgets::param_knob`): its label slot shows the value
    // while the pointer is over the dial.
    let shown = |mc| {
        let ctx = egui::Context::default();
        let mut text = String::new();
        for time in [0.0, 0.1] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                time: Some(time),
                events: vec![egui::Event::PointerMoved(egui::pos2(31.0, 28.0))],
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                let child = egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)));
                ui.scope_builder(child, |ui| {
                    text = crate::widgets::param_knob::show(ui, egui::Id::new("tonic_knob"), "Tonic", tonic(), 24.0, mc).text;
                });
            });
            out.textures_delta.clear();
        }
        text
    };
    assert_eq!(shown(C3), "C1");
    assert_eq!(shown(C4), "C2");
}

#[test]
fn the_random_sequencer_octave_button_follows_the_setting() {
    let mut r = Rig::new();
    r.app.ui.show_devices = true;
    let t = crate::tests::new_track(&mut r.app, json!({"kind": "instrument"}));
    r.app.run("mix.midi_effect", json!({"track": t.0, "plugin": "random_sequencer"})).unwrap();
    r.app.focus(t);
    r.frames(4);
    let row = r.app.mixer.devices.midi_fx.clone().expect("the MIDI effects group").rows[0].clone();
    let pitch = row.sections.iter().find(|s| s.0 == "Pitch").cloned().expect("the Pitch section");
    if !pitch.1 {
        r.click(pitch.2);
    }
    let row = r.app.mixer.devices.midi_fx.clone().unwrap().rows[0].clone();
    assert!(row.params.iter().any(|p| p.0 == "octave"), "{:?}", row.params);
    let (before, after) = r.flip_frames(C4);
    let (gone, new) = (minus(&before, &after), minus(&after, &before));
    assert_eq!((gone, new), (vec!["2".to_string()], vec!["3".to_string()]), "only the octave's text changed");
}

#[test]
fn the_plugin_listing_keeps_the_fixed_names_and_the_marker_whatever_the_setting() {
    let mut app = crate::tests::app();
    let listing = |app: &mut crate::StudioApp| app.run("engine.plugins", json!({})).unwrap();
    let under_c3 = listing(&mut app);
    app.run("options.middle_c", json!({"value": "C4"})).unwrap();
    let under_c4 = listing(&mut app);
    assert_eq!(under_c3, under_c4, "the listing does not follow the setting");
    let find = |v: &Value, plugin: &str, param: &str| -> Value {
        let list = v.as_array().or_else(|| v["plugins"].as_array()).expect("a list of plugins");
        let p = list.iter().find(|p| p["id"] == plugin).unwrap_or_else(|| panic!("{plugin} listed"));
        p["params"].as_array().unwrap().iter().find(|x| x["id"] == param).cloned().unwrap()
    };
    let tonic = find(&under_c4, "resonator", "tonic");
    assert_eq!(tonic["choices"][24], "C1", "the fixed C3 name under C4");
    assert_eq!(tonic["note_labels"], json!({"first_note": 12, "step": 1, "octave_only": false}));
    let model = find(&under_c4, "resonator", "model");
    assert_eq!(model["note_labels"], Value::Null);
}

#[test]
fn presets_and_automation_written_before_read_the_same_values_after() {
    // The factory presets hold the Octave index (3, 4 and 5); they load unchanged and only the
    // label differs with the setting.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets/random_sequencer");
    let mut octaves = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let raw: Value = serde_json::from_str(&text).unwrap();
        let preset = crate::presets::parse(&text).unwrap();
        let v = preset.params["octave"];
        assert_eq!(f64::from(v), raw["params"]["octave"].as_f64().unwrap(), "the stored index reads as written");
        octaves.push(v);
    }
    octaves.sort_by(f32::total_cmp);
    assert_eq!(octaves, [3.0, 4.0, 5.0]);
    assert_eq!(octaves.iter().map(|v| param_text(octave(), *v, C3)).collect::<Vec<_>>(), ["1", "2", "3"]);
    // A session with a Tonic lane, written and read back under each setting, keeps its numbers.
    let mut app = crate::tests::app();
    let t = crate::tests::new_track(&mut app, json!({"kind": "instrument"}));
    app.run("mix.instrument", json!({"track": t.0, "plugin": "resonator"})).unwrap();
    app.run("mix.instrument_param", json!({"track": t.0, "param": "tonic", "value": 30.0})).unwrap();
    app.add_point(t, "plugin:instrument:tonic", 0, 24.0).unwrap();
    app.add_point(t, "plugin:instrument:tonic", 48_000, 36.0).unwrap();
    let written = serde_json::to_string(&*app.engine.session()).unwrap();
    for mc in ["C3", "C4"] {
        app.run("options.middle_c", json!({"value": mc})).unwrap();
        let read: transit_model::Session = serde_json::from_str(&written).unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), written, "under {mc}");
        let track = read.track(t).unwrap();
        assert_eq!(track.instrument.as_ref().unwrap().params.get("tonic"), Some(&30.0));
        let lane = track.lane(&transit_model::AutoParam::parse("plugin:instrument:tonic").unwrap()).unwrap();
        assert_eq!(lane.points.iter().map(|p| p.value).collect::<Vec<_>>(), [24.0, 36.0]);
    }
    assert_eq!(serde_json::to_string(&*app.engine.session()).unwrap(), written, "the flips wrote nothing into the session");
}
