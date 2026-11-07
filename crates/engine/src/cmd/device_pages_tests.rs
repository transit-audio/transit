//! `mix.device_pages` and `mix.device_pages_list` (ADR 0047 slice 1): one undo step per edit, the
//! read rules on what a command sends, the addressing, move and copy carrying pages, and the
//! device keeping its identity.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use serde_json::{Value, json};

fn engine() -> (Engine, u64) {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = r["tracks"][0].as_u64().unwrap();
    e.execute("mix.instrument", &json!({"track": t, "plugin": "macro_oscillator"})).unwrap();
    e.execute("mix.insert", &json!({"track": t, "slot": "b", "plugin": "eq_7band"})).unwrap();
    e.execute("mix.midi_effect", &json!({"track": t, "plugin": "midi_transpose"})).unwrap();
    (e, t)
}

fn list(e: &mut Engine, t: u64, device: &str) -> Value {
    e.execute("mix.device_pages_list", &json!({"track": t, "device": device})).unwrap()["pages"].clone()
}

fn one_page(ids: &[&str]) -> Value {
    json!([{ "name": "User 1", "slots": ids }])
}

#[test]
fn each_edit_is_one_undo_step_and_undo_restores_the_pages() {
    let (mut e, t) = engine();
    let pushes = e.undo_pushes();
    let v = e.execute("mix.device_pages", &json!({"track": t, "device": "instrument", "pages": one_page(&["level", "timbre"])})).unwrap();
    assert_eq!(v["changed"], json!(true));
    assert_eq!(e.undo_pushes(), pushes + 1, "one step");
    assert_eq!(e.undo_label(), Some("Set Device Pages"));
    let pages = &e.session().tracks[0].instrument.as_ref().unwrap().pages;
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].slots[1].as_deref(), Some("timbre"));
    // Two pages at once are still one step; the same pages again are none.
    let two = json!([{ "name": "A", "slots": ["level"] }, { "name": "B", "slots": ["morph"] }]);
    e.execute("mix.device_pages", &json!({"track": t, "device": "instrument", "pages": two})).unwrap();
    assert_eq!(e.undo_pushes(), pushes + 2);
    let again = e.execute("mix.device_pages", &json!({"track": t, "device": "instrument", "pages": two})).unwrap();
    assert_eq!(again["changed"], json!(false));
    assert_eq!(e.undo_pushes(), pushes + 2, "no change, no step");
    assert!(e.undo());
    assert_eq!(e.session().tracks[0].instrument.as_ref().unwrap().pages.len(), 1, "undo brings back the one page");
    assert!(e.undo());
    assert!(e.session().tracks[0].instrument.as_ref().unwrap().pages.is_empty());
    assert!(e.redo());
    assert_eq!(e.session().tracks[0].instrument.as_ref().unwrap().pages.len(), 1);
}

#[test]
fn a_command_is_read_by_the_files_rules_and_refuses_what_has_no_pages() {
    let (mut e, t) = engine();
    let messy = json!([{ "name": "   A very long name for a page, longer than allowed  ", "slots": ["level", "level", "", 3, "x".repeat(65), "morph", "a", "b", "c"] }]);
    e.execute("mix.device_pages", &json!({"track": t, "device": "b", "pages": messy})).unwrap();
    let page = &e.session().tracks[0].mixer.inserts[1].as_ref().unwrap().pages[0];
    assert_eq!(page.name, "A very long name for a page, lon");
    let slots: Vec<Option<&str>> = page.slots.iter().map(Option::as_deref).collect();
    assert_eq!(slots, [Some("level"), None, None, None, None, Some("morph"), Some("a"), Some("b")]);
    let bad = |e: &mut Engine, p: Value| e.execute("mix.device_pages", &p).unwrap_err().to_string();
    let seventeen: Vec<Value> = (0..17).map(|_| json!({ "slots": [] })).collect();
    assert!(bad(&mut e, json!({"track": t, "device": "b", "pages": seventeen})).contains("at most 16"));
    assert!(bad(&mut e, json!({"track": t, "device": "b", "pages": "x"})).contains("must be an array"));
    assert!(bad(&mut e, json!({"track": t, "device": "c", "pages": []})).contains("no device at `c`"));
    assert!(bad(&mut e, json!({"track": t, "device": "midi2", "pages": []})).contains("no device at `midi2`"));
    assert!(bad(&mut e, json!({"track": t, "device": "k", "pages": []})).contains("is not a..j"));
    assert!(bad(&mut e, json!({"track": t, "pages": []})).contains("`device` required"));
    assert!(bad(&mut e, json!({"track": t, "device": "rack", "pages": []})).contains("not a rack"));
    e.execute("mix.insert", &json!({"track": t, "slot": "c", "plugin": "invert"})).unwrap();
    assert!(bad(&mut e, json!({"track": t, "device": "c", "pages": one_page(&["x"])})).contains("no parameters"));
    e.execute("mix.insert", &json!({"track": t, "slot": "d", "hardware": {"send": "Out 3-4", "return": "In 3-4"}})).unwrap();
    assert!(bad(&mut e, json!({"track": t, "device": "d", "pages": one_page(&["x"])})).contains("no parameters"));
    let pushes = e.undo_pushes();
    let _ = bad(&mut e, json!({"track": t, "device": "b", "pages": seventeen}));
    assert_eq!(e.undo_pushes(), pushes, "a refusal is no step");
    assert_eq!(e.session().tracks[0].mixer.inserts[1].as_ref().unwrap().pages.len(), 1, "and changes nothing");
}

#[test]
fn the_list_has_both_tiers_with_slot_states() {
    let (mut e, t) = engine();
    let pages = list(&mut e, t, "instrument");
    assert_eq!(pages.as_array().unwrap().len(), 1);
    assert_eq!(pages[0]["tier"], "factory");
    assert_eq!(pages[0]["name"], "Main");
    assert_eq!(pages[0]["slots"][5], json!({"id": "cutoff", "name": "Cutoff", "state": "ok", "enable": "shaper"}));
    e.execute("mix.device_pages", &json!({"track": t, "device": "instrument", "pages": one_page(&["level", "not_a_param", "timbre"])})).unwrap();
    let pages = list(&mut e, t, "instrument");
    let user = &pages[1];
    assert_eq!((user["tier"].as_str(), user["name"].as_str()), (Some("user"), Some("User 1")));
    let states: Vec<&str> = user["slots"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["ok", "missing", "ok", "empty", "empty", "empty", "empty", "empty"]);
    assert_eq!(user["slots"][1]["id"], "not_a_param");
    assert_eq!(user["slots"][0], json!({"id": "level", "name": "Level", "state": "ok"}));
    // A MIDI effect by position, an insert by letter.
    assert_eq!(list(&mut e, t, "midi1")[0]["slots"][0]["state"], "ok");
    assert_eq!(list(&mut e, t, "B")[0]["name"], "Main");
    assert!(e.execute("mix.device_pages_list", &json!({"track": t, "device": "a"})).is_err());
}

#[test]
fn an_uninstalled_plug_ins_user_slots_read_missing() {
    let (mut e, t) = engine();
    // Tests never reach the hosts: a hosted id has no table and no read in flight, as for a
    // plug-in that is not installed (the session came from a machine that had it).
    e.session_mut().tracks[0].instrument = Some(transit_model::Insert::new("clap:test.not.installed"));
    e.execute("mix.device_pages", &json!({"track": t, "device": "instrument", "pages": one_page(&["1032", "1033"])})).unwrap();
    let pages = list(&mut e, t, "instrument");
    assert_eq!(pages.as_array().unwrap().len(), 1, "no factory tier without a table");
    assert_eq!(pages[0]["slots"][0], json!({"id": "1032", "name": null, "state": "missing"}));
    assert_eq!(pages[0]["slots"][1]["state"], "missing");
    assert_eq!(pages[0]["slots"][2]["state"], "empty");
}

#[test]
fn move_and_copy_carry_the_pages_and_the_device_keeps_its_uid() {
    let (mut e, t) = engine();
    let uid = e.session().tracks[0].mixer.inserts[1].as_ref().unwrap().uid.get();
    e.execute("mix.device_pages", &json!({"track": t, "device": "b", "pages": one_page(&["x"])})).unwrap();
    let ins = e.session().tracks[0].mixer.inserts[1].as_ref().unwrap();
    assert_eq!(ins.uid.get(), uid, "a pages edit keeps the device's identity");
    e.execute("mix.insert_move", &json!({"track": t, "from": "b", "to": "e"})).unwrap();
    let moved = e.session().tracks[0].mixer.inserts[4].as_ref().unwrap();
    assert_eq!((moved.uid.get(), moved.pages.len()), (uid, 1), "the move carries the pages");
    e.execute("mix.insert_copy", &json!({"track": t, "from": "e", "to": "a"})).unwrap();
    let copy = e.session().tracks[0].mixer.inserts[0].as_ref().unwrap();
    assert_eq!(copy.pages, e.session().tracks[0].mixer.inserts[4].as_ref().unwrap().pages, "the copy carries them");
    assert_ne!(copy.uid.get(), uid);
    // MIDI effects: the pages follow the effect when the chain is reordered.
    e.execute("mix.midi_effect", &json!({"track": t, "plugin": "midi_velocity"})).unwrap();
    e.execute("mix.device_pages", &json!({"track": t, "device": "midi1", "pages": one_page(&["semitones"])})).unwrap();
    e.execute("mix.midi_effect_move", &json!({"track": t, "from": 0, "to": 1})).unwrap();
    let fx = e.session().tracks[0].midi_effects.clone();
    assert_eq!((fx[1].plugin.as_str(), fx[1].pages.len()), ("midi_transpose", 1));
    assert!(fx[0].pages.is_empty());
    // `midi<n>` resolves when the command runs: midi2 is now the transpose.
    assert_eq!(list(&mut e, t, "midi2")[1]["slots"][0]["state"], "ok");
    // The whole slot record travels in a session file too.
    let back = transit_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(back.tracks[0].midi_effects[1].pages, fx[1].pages);
}

#[test]
fn a_rack_lists_its_macros_and_takes_no_user_pages() {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let a = r["tracks"][0].as_u64().unwrap();
    let id = e.execute("rack.new", &json!({"tracks": [a]})).unwrap()["rack"].as_u64().unwrap();
    let pages = list(&mut e, id, "rack");
    assert_eq!(pages[0]["name"], "Macros");
    assert_eq!(pages[0]["slots"][0]["id"], "macro1");
    let err = e.execute("mix.device_pages", &json!({"track": id, "device": "rack", "pages": []})).unwrap_err().to_string();
    assert!(err.contains("takes no user pages"), "{err}");
}

/// Pages are not freeze material (ADR 0047 decision 4): a pages edit on a frozen track is
/// allowed and keeps its freeze and hash. Control: a parameter edit of the same device is still
/// refused, so the frozen check is live.
#[test]
fn a_frozen_track_accepts_a_pages_edit_and_keeps_its_freeze() {
    let (mut e, t) = engine();
    let tr = &mut e.session_mut().tracks[0];
    let frozen = transit_model::Frozen { source: transit_model::SourceId(1), start: 0, length: 10, hash: tr.freeze_hash(), revision: 1, latency: 0 };
    tr.freeze = Some(frozen.clone());
    let hash = e.session().tracks[0].freeze_hash();
    for device in ["instrument", "b", "midi1"] {
        e.execute("mix.device_pages", &json!({"track": t, "device": device, "pages": one_page(&["level"])})).unwrap();
    }
    let tr = &e.session().tracks[0];
    assert_eq!(tr.instrument.as_ref().unwrap().pages.len(), 1, "the edit landed");
    assert_eq!(tr.midi_effects[0].pages.len(), 1);
    assert_eq!(tr.freeze_hash(), hash, "the freeze hash is unchanged");
    assert_eq!(tr.freeze, Some(frozen), "the track stays frozen");
    // Control: a real parameter edit on the frozen track is still refused.
    let err = e.execute("mix.instrument_param", &json!({"track": t, "param": "level", "value": -12.0})).unwrap_err();
    assert!(matches!(err, crate::EngineError::TrackFrozen), "{err}");
    let err = e.execute("mix.insert_param", &json!({"track": t, "slot": "b", "param": e.session().tracks[0].mixer.inserts[1].as_ref().unwrap().params.keys().next().unwrap().clone(), "value": 0.5})).unwrap_err();
    assert!(matches!(err, crate::EngineError::TrackFrozen), "{err}");
}
