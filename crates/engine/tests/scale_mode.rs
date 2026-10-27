use serde_json::json;
use transit_engine::Engine;
use transit_midi::scale::{KeyScale, Scale, ScaleMode};
use transit_model::{ClipId, Session};

#[test]
fn scale_mode_is_per_clip_undoable_persistent_and_never_rewrites_notes() {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 61, "start_ticks": 0})).unwrap();
    let original = e.session().find_clip(ClipId(clip)).unwrap().1.clone();
    let mode = ScaleMode { enabled: true, fold: true, key: KeyScale { root: 0, scale: Scale::Minor } };
    e.execute("midi.scale_mode", &json!({"clip": clip, "enabled": true, "fold": true, "scale": "minor"})).unwrap();
    assert_eq!(e.session().find_clip(ClipId(clip)).unwrap().1.scale_mode, mode);
    assert_eq!(e.session().find_clip(ClipId(clip)).unwrap().1.content, original.content);
    let saved = e.session().to_json().unwrap();
    assert!(saved.contains("clip_scale_mode"));
    let loaded = Session::from_json(&saved).unwrap();
    assert_eq!(loaded.find_clip(ClipId(clip)).unwrap().1.scale_mode, mode);
    assert!(e.undo());
    assert_eq!(e.session().find_clip(ClipId(clip)).unwrap().1, &original);
    assert!(e.redo());
    assert_eq!(e.session().find_clip(ClipId(clip)).unwrap().1.scale_mode, mode);
    let other = e.execute("midi.new_clip", &json!({"track": track, "start": 96000, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    assert_eq!(e.session().find_clip(ClipId(other)).unwrap().1.scale_mode, ScaleMode::default());
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 64, "start_ticks": 480})).unwrap();
    let notes = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][1]["pitch"], 64, "record/control entry is never scale filtered");

    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("scale_mode");
    let restored: transit_model::Clip = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.scale_mode, ScaleMode::default());
}

#[test]
fn scale_settings_reject_invalid_input_atomically_and_noop_preserves_document() {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    e.execute("midi.scale_mode", &json!({"clip": clip, "enabled": true})).unwrap();
    let before = e.session_arc();
    for invalid in [json!({"root": 12}), json!({"root": -1}), json!({"scale": "dorien"}), json!({"fold": 1}), json!({"enabled": null})] {
        let mut p = invalid;
        p["clip"] = json!(clip);
        if p.get("fold").is_none() {
            p["fold"] = json!(true);
        }
        if p.get("enabled").is_none() {
            p["enabled"] = json!(false);
        }
        assert!(e.execute("midi.scale_mode", &p).is_err());
        assert_eq!(e.session().to_json().unwrap(), before.to_json().unwrap());
    }
    e.execute("midi.scale_mode", &json!({"clip": clip, "enabled": true})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&e.session_arc(), &before));
}

#[test]
fn scale_move_batch_validates_before_editing_and_preserves_chord_identity() {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    for pitch in [60, 62] {
        e.execute("midi.note_add", &json!({"clip":clip,"pitch":pitch,"start_ticks":0})).unwrap();
    }
    let original = e.session().to_json().unwrap();
    for bad in [
        json!({"indices":[0,1],"pitches":[62,128]}),
        json!({"indices":[0,2],"pitches":[62,63]}),
        json!({"indices":[0,0],"pitches":[62,63]}),
        json!({"indices":[0,1],"pitches":[62]}),
        json!({"indices":[0,1],"pitches":[62,63],"ticks":i64::MAX}),
    ] {
        let mut p = bad;
        p["clip"] = json!(clip);
        assert!(e.execute("midi.notes_move", &p).is_err());
        assert_eq!(e.session().to_json().unwrap(), original);
    }
    let result = e.execute("midi.notes_move", &json!({"clip":clip,"indices":[0,1],"pitches":[62,63]})).unwrap();
    assert_eq!(result["notes"][0]["pitch"], 62);
    assert_eq!(result["notes"][1]["pitch"], 63);
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), original);
}

#[test]
fn scale_group_move_applies_distinct_pitches_as_one_undo_step() {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    let key = KeyScale { root: 0, scale: Scale::Minor };
    let chord = [60u8, 61];
    for pitch in chord {
        e.execute("midi.note_add", &json!({"clip":clip,"pitch":pitch,"start_ticks":0})).unwrap();
    }
    let original = e.session().to_json().unwrap();
    let moved = key.transpose_group(&chord, chord[0], 1);
    assert_eq!(moved, [62, 63]);
    e.execute("midi.notes_move", &json!({"clip":clip,"indices":[0,1],"pitches":moved})).unwrap();
    let notes = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
    assert_eq!((notes["notes"][0]["pitch"].as_u64(), notes["notes"][1]["pitch"].as_u64()), (Some(62), Some(63)));
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), original);
    assert!(!e.undo() || e.session().to_json().unwrap() != original, "the move was a single step");
}

#[test]
fn named_scales_set_through_the_command_and_old_session_names_still_load() {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 96000})).unwrap()["clip"].as_u64().unwrap();
    e.execute("midi.scale_mode", &json!({"clip": clip, "root": 9, "scale": "minor-pentatonic"})).unwrap();
    let key = e.session().find_clip(ClipId(clip)).unwrap().1.scale_mode.key;
    assert_eq!(key, KeyScale { root: 9, scale: Scale::MinorPentatonic });
    let saved = e.session().to_json().unwrap();
    assert!(saved.contains("minor-pentatonic"));
    assert_eq!(Session::from_json(&saved).unwrap().find_clip(ClipId(clip)).unwrap().1.scale_mode.key, key);
    // A session written before the scale list existed stores plain `major` / `minor`.
    for (old, scale) in [("major", Scale::Major), ("minor", Scale::Minor)] {
        let legacy = saved.replace("minor-pentatonic", old);
        let loaded = Session::from_json(&legacy).unwrap();
        assert_eq!(loaded.find_clip(ClipId(clip)).unwrap().1.scale_mode.key.scale, scale);
    }
}
