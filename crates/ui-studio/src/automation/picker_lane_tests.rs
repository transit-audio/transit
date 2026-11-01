//! Issue #239: picker entries reach the actual popup, lane header and undoable commands.

use super::{T, mid};
use crate::automation::{Lane, choices};
use serde_json::json;
use transit_model::{AutoParam, TrackId};

fn entries(t: &mut T, track: TrackId) -> Vec<(String, String, String)> {
    let s = t.app.engine.session_arc();
    choices(&mut t.app, &s, s.track(track).unwrap())
}

fn pick_and_undo(t: &mut T, track: TrackId, label: &str, id: &str, value: f32) {
    t.app.toggle_automation(track);
    t.frames(3);
    t.click(mid(t.lane(track, "volume").picker));
    let item = t.app.auto.layout.menu.iter().find(|m| m.value == id).expect("entry is drawn in the popup");
    assert_eq!(item.label, label);
    let before = t.undo_len();
    t.menu(label);
    let row = t.lane(track, id);
    assert_eq!(row.label, label, "the header and picker share the label");
    assert_eq!(row.value, value);
    let param = AutoParam::parse(id).unwrap();
    // Picking only opens the lane view: no model lane, no breakpoint, no undo entry.
    assert!(t.points(track, &param).is_empty());
    assert!(t.app.engine.session().track(track).unwrap().lane(&param).is_none());
    assert_eq!(t.undo_len(), before);
    // Drawing a point afterwards goes through the command path: one undo step.
    t.app.add_point(track, id, 480, value).unwrap();
    assert_eq!(t.points(track, &param), [(480, value)]);
    assert_eq!(t.undo_len(), before + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.app.engine.session().track(track).unwrap().lane(&param).is_none());
}

#[test]
fn midi_effect_picker_lists_parameters_and_picks_the_same_lane() {
    let mut t = T::new();
    let track = t.track("instrument", "Keys");
    for plugin in ["midi_transpose", "midi_velocity"] {
        t.app.run("mix.midi_effect", json!({"track": track.0, "plugin": plugin})).unwrap();
    }
    t.app.run("mix.midi_effect_param", json!({"track": track.0, "index": 0, "param": "semitones", "value": -12.0})).unwrap();
    let listed = entries(&mut t, track);
    let effects: Vec<_> = listed.iter().filter(|e| e.0 == "MIDI effects").map(|e| (e.1.as_str(), e.2.as_str())).collect();
    assert_eq!(
        effects,
        [
            ("Transpose · Semitones", "plugin:midi1:semitones"),
            ("Velocity · Scale", "plugin:midi2:scale"),
            ("Velocity · Offset", "plugin:midi2:offset"),
            ("Velocity · Minimum", "plugin:midi2:min"),
            ("Velocity · Maximum", "plugin:midi2:max"),
        ]
    );
    assert!(!listed.iter().any(|e| e.0 == "Macros"));
    for (_, label, id) in listed.iter().filter(|e| e.0 == "MIDI effects") {
        let s = t.app.engine.session_arc();
        let lane = Lane::parse(id).unwrap();
        assert_eq!(lane.id(), *id);
        assert!(lane.valid(s.track(track).unwrap()));
        assert_eq!(lane.label(&mut t.app, &s, s.track(track).unwrap()), *label);
    }
    pick_and_undo(&mut t, track, "Transpose · Semitones", "plugin:midi1:semitones", -12.0);
}

#[test]
fn rack_macro_picker_lists_parameters_and_picks_the_same_lane() {
    let mut t = T::new();
    let member = t.track("instrument", "Keys");
    let rack = TrackId(t.app.run("rack.new", json!({"tracks": [member.0]})).unwrap()["rack"].as_u64().unwrap());
    t.app.run("rack.macro_name", json!({"track": rack.0, "macro": 1, "name": "Brightness"})).unwrap();
    t.app.run("rack.macro", json!({"track": rack.0, "macro": 1, "value": 0.75})).unwrap();
    let listed = entries(&mut t, rack);
    let macros: Vec<_> = listed.iter().filter(|e| e.0 == "Macros").cloned().collect();
    assert_eq!(macros.len(), 8);
    assert!(!listed.iter().any(|e| e.0 == "MIDI effects"));
    assert!(!entries(&mut t, member).iter().any(|e| e.0 == "Macros"), "macro automation belongs to the rack");
    for (i, (_, label, id)) in macros.iter().enumerate() {
        assert_eq!(*id, format!("plugin:253:macro{}", i + 1));
        assert_eq!(*label, if i == 0 { "Brightness".into() } else { format!("Macro {}", i + 1) });
        let s = t.app.engine.session_arc();
        let lane = Lane::parse(id).unwrap();
        assert_eq!(lane.id(), *id);
        assert!(lane.valid(s.track(rack).unwrap()));
        assert_eq!(lane.label(&mut t.app, &s, s.track(rack).unwrap()), *label);
    }
    pick_and_undo(&mut t, rack, "Brightness", "plugin:253:macro1", 0.75);
}

#[test]
fn every_registered_effect_and_all_eight_slots_use_the_effect_parameter_model() {
    let mut t = T::new();
    let track = t.track("instrument", "Keys");
    let registry = transit_dsp::midi_fx::REGISTRY;
    for index in 0..transit_model::MAX_MIDI_EFFECTS {
        let info = registry[index % registry.len()];
        t.app.run("mix.midi_effect", json!({"track": track.0, "plugin": info.id})).unwrap();
    }
    let listed = entries(&mut t, track);
    let s = t.app.engine.session_arc();
    let tr = s.track(track).unwrap();
    for (index, effect) in tr.midi_effects.iter().enumerate() {
        let info = transit_dsp::midi_fx::info(&effect.plugin).unwrap();
        for param in info.params {
            let id = format!("plugin:midi{}:{}", index + 1, param.id);
            let lane = Lane::parse(&id).unwrap();
            let expected = AutoParam::Plugin { slot: 240 + index as u8, param: param.id.into() };
            assert_eq!(AutoParam::parse(&lane.id()), Some(expected));
            assert!(listed.iter().any(|e| e.0 == "MIDI effects" && e.2 == id), "{id}");
            assert_eq!(lane.info(tr), Some(param));
            assert_eq!(lane.static_value(tr), param.default);
            assert_eq!(lane.denorm(tr, 0.0), param.from_normalized(0.0));
            assert_eq!(lane.denorm(tr, 1.0), param.from_normalized(1.0));
            assert_eq!(lane.norm(tr, param.default), param.to_normalized(param.default));
            assert_eq!(lane.text(tr, param.default), param.format(param.default));
        }
    }
    // Distinct slots never collapse to the generic insert-letter fallback.
    let ids: std::collections::HashSet<_> = listed.iter().map(|e| &e.2).collect();
    assert_eq!(ids.len(), listed.len());
}

#[test]
fn picking_an_existing_effect_curve_preserves_it_and_its_undo_history() {
    let mut t = T::new();
    let track = t.track("instrument", "Keys");
    t.app.run("mix.midi_effect", json!({"track": track.0, "plugin": "midi_velocity"})).unwrap();
    let id = "plugin:midi1:scale";
    t.app.add_point(track, id, 123, 250.0).unwrap();
    t.app.toggle_automation(track);
    t.frames(3);
    t.click(mid(t.lane(track, "volume").picker));
    let before = t.undo_len();
    t.menu("Velocity · Scale");
    assert_eq!(t.lane(track, id).label, "Velocity · Scale");
    assert_eq!(t.points(track, &AutoParam::parse(id).unwrap()), [(123, 250.0)]);
    assert_eq!(t.undo_len(), before);
}

#[test]
fn moving_or_removing_effects_never_redirects_a_selected_point_to_another_effect() {
    for remove in [false, true] {
        let mut t = T::new();
        let track = t.track("instrument", "Keys");
        for _ in 0..2 {
            t.app.run("mix.midi_effect", json!({"track": track.0, "plugin": "midi_transpose"})).unwrap();
        }
        let first = "plugin:midi1:semitones";
        let second = "plugin:midi2:semitones";
        t.app.add_point(track, second, 0, 24.0).unwrap();
        t.app.add_point(track, first, 0, 12.0).unwrap();
        t.app.toggle_automation(track);
        t.app.pick_lane(track, "volume", first);
        t.frames(3);
        assert_eq!(t.app.auto.selection.len(), 1);
        if remove {
            t.app.run("mix.midi_effect_remove", json!({"track": track.0, "index": 0})).unwrap();
        } else {
            t.app.run("mix.midi_effect_move", json!({"track": track.0, "from": 0, "to": 1})).unwrap();
        }
        t.frames(3);
        if remove {
            assert!(t.app.auto.selection.is_empty(), "the removed effect's selection must disappear");
        } else {
            assert_eq!(t.app.auto.selection[0].lane, second, "selection follows the effect identity");
            assert_eq!(t.lane(track, second).value, 12.0);
            t.app.run("edit.undo", json!({})).unwrap();
            t.frames(3);
            assert_eq!(t.app.auto.selection[0].lane, first);
            t.app.run("edit.redo", json!({})).unwrap();
            t.frames(3);
            assert_eq!(t.app.auto.selection[0].lane, second);
        }
        t.app.delete_points().unwrap();
        assert_eq!(t.points(track, &AutoParam::parse(first).unwrap()), [(0, 24.0)], "the other effect's curve survives");
    }
}

#[test]
fn search_enter_picks_midi_effect_and_macro_entries() {
    for macro_lane in [false, true] {
        let mut t = T::new();
        let member = t.track("instrument", "Keys");
        let (track, query, id, label) = if macro_lane {
            let rack = TrackId(t.app.run("rack.new", json!({"tracks": [member.0]})).unwrap()["rack"].as_u64().unwrap());
            t.app.run("rack.macro_name", json!({"track": rack.0, "macro": 1, "name": "Brightness"})).unwrap();
            (rack, "brightness", "plugin:253:macro1", "Brightness")
        } else {
            t.app.run("mix.midi_effect", json!({"track": member.0, "plugin": "midi_transpose"})).unwrap();
            (member, "semitones", "plugin:midi1:semitones", "Transpose · Semitones")
        };
        t.app.toggle_automation(track);
        t.frames(3);
        t.click(mid(t.lane(track, "volume").picker));
        t.app.auto.picker.query = query.into();
        t.frames(3);
        assert_eq!(t.app.auto.layout.menu.iter().map(|item| item.value.as_str()).collect::<Vec<_>>(), [id]);
        let before = t.undo_len();
        t.key("Enter");
        assert_eq!(t.lane(track, id).label, label);
        assert_eq!(t.undo_len(), before);
        assert!(t.points(track, &AutoParam::parse(id).unwrap()).is_empty());
    }
}

#[test]
fn the_plus_button_opens_a_new_effect_lane_view_like_the_picker() {
    let mut t = T::new();
    let track = t.track("instrument", "Keys");
    t.app.run("mix.midi_effect", json!({"track": track.0, "plugin": "midi_transpose"})).unwrap();
    t.app.toggle_automation(track);
    t.frames(3);
    t.click(mid(t.lane(track, "volume").add));
    t.click(mid(t.lane(track, "pan").add));
    let before = t.undo_len();
    t.click(mid(t.lane(track, "mute").add));
    let id = "plugin:midi1:semitones";
    assert_eq!(t.lane(track, id).label, "Transpose · Semitones");
    assert!(t.points(track, &AutoParam::parse(id).unwrap()).is_empty());
    assert!(t.app.engine.session().track(track).unwrap().lane(&AutoParam::parse(id).unwrap()).is_none());
    assert_eq!(t.undo_len(), before);
}
