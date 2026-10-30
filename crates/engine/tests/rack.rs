//! Rack commands (ADR 0034, #197): a rack made from instrument tracks, its zones, members and
//! macros; one undo step per edit; a save and load round trip; macro moves write the mapped
//! parameters through the range and the curve; a macro's MIDI Learn source lives on the rack.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]

use serde_json::{Value, json};
use transit_engine::Engine;
use transit_model::{Control, Route, Session, TrackId, TrackKind, Zone};

/// An engine with two instrument tracks grouped into a rack, and an audio track outside it.
fn racked() -> (Engine, TrackId, [TrackId; 2]) {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "count": 2})).unwrap();
    let ids: Vec<u64> = r["tracks"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    e.execute("track.new", &json!({"kind": "audio"})).unwrap();
    let r = e.execute("rack.new", &json!({"tracks": ids})).unwrap();
    let rack = TrackId(r["rack"].as_u64().unwrap());
    (e, rack, [TrackId(ids[0]), TrackId(ids[1])])
}

fn rack_of(e: &Engine, rack: TrackId) -> transit_model::Rack {
    e.session().track(rack).unwrap().rack.clone().unwrap()
}

#[test]
fn group_into_rack_makes_a_summing_folder_whose_instruments_are_members() {
    let (e, rack, [a, b]) = racked();
    let s = e.session();
    let f = s.track(rack).unwrap();
    assert!(f.is_rack() && f.kind == TrackKind::Folder);
    assert_eq!(f.name, "Rack 1");
    let Route::Bus(bus) = f.mixer.input else { panic!("the rack reads its bus") };
    for m in [a, b] {
        let t = s.track(m).unwrap();
        assert_eq!((t.folder, &t.mixer.output), (Some(rack), &Route::Bus(bus)), "members sum through the rack's bus");
    }
    let r = rack_of(&e, rack);
    assert_eq!(
        r.members.iter().map(|m| (m.track, m.key_zone, m.vel_zone)).collect::<Vec<_>>(),
        [(a, Zone::FULL, Zone::FULL), (b, Zone::FULL, Zone::FULL)]
    );
    assert_eq!(s.rack_members(f).count(), 2);
    assert_eq!(e.undo_history().len(), 3, "the rack is one undo step");
}

#[test]
fn zone_macro_and_member_edits_are_one_undo_step_each() {
    let (mut e, rack, [a, b]) = racked();
    let steps = e.undo_history().len();
    e.execute("rack.zone", &json!({"track": rack.0, "member": a.0, "key": {"low": 36, "high": 60, "fade_high": 55}, "velocity": {"low": 20}}))
        .unwrap();
    assert_eq!(e.undo_history().len(), steps + 1);
    let m = rack_of(&e, rack).members[0].clone();
    assert_eq!(m.key_zone, Zone { low: 36, fade_low: 36, fade_high: 55, high: 60 });
    assert_eq!(m.vel_zone, Zone { low: 20, fade_low: 20, fade_high: 127, high: 127 });
    assert!(e.undo());
    assert_eq!(rack_of(&e, rack).members[0].key_zone, Zone::FULL, "undo restores the zone");
    assert!(e.redo());

    e.execute("mix.insert", &json!({"track": b.0, "slot": 0, "plugin": "gain"})).unwrap();
    let steps = e.undo_history().len();
    let info = transit_dsp::plugin_info("gain").unwrap();
    let p = info.params.first().unwrap().id;
    e.execute(
        "rack.macro_map",
        &json!({"track": rack.0, "macro": 2, "member": b.0, "slot": "a", "param": p, "min": 0.25, "max": 0.75, "curve": 0.5}),
    )
    .unwrap();
    e.execute("rack.macro_name", &json!({"track": rack.0, "macro": 2, "name": "Drive"})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 2, "a mapping and a rename: one step each");
    assert_eq!(rack_of(&e, rack).macro_name(1), "Drive");
    assert!(e.undo() && e.undo());
    assert!(rack_of(&e, rack).macros[1].mappings.is_empty(), "undo removes the mapping");
    assert!(e.redo() && e.redo());

    let steps = e.undo_history().len();
    e.execute("rack.member_remove", &json!({"track": rack.0, "member": b.0})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1);
    let s = e.session();
    assert_eq!(s.track(b).unwrap().folder, None, "the member leaves the folder");
    assert_eq!(s.track(b).unwrap().mixer.output, Route::Main);
    assert!(rack_of(&e, rack).member(b).is_none() && rack_of(&e, rack).macros[1].mappings.is_empty());
    assert!(e.undo());
    assert!(rack_of(&e, rack).member(b).is_some() && rack_of(&e, rack).macros[1].mappings.len() == 1, "undo brings member and mapping back");
    e.execute("rack.member_remove", &json!({"track": rack.0, "member": b.0})).unwrap();
    e.execute("rack.member_add", &json!({"track": rack.0, "member": b.0})).unwrap();
    assert_eq!(e.session().track(b).unwrap().folder, Some(rack));
    assert!(rack_of(&e, rack).member(b).is_some());
}

#[test]
fn a_macro_move_writes_the_mapped_parameter_through_its_range_and_curve() {
    let (mut e, rack, [a, _]) = racked();
    e.execute("mix.insert", &json!({"track": a.0, "slot": 0, "plugin": "gain"})).unwrap();
    let info = transit_dsp::plugin_info("gain").unwrap();
    let p = info.params.first().unwrap();
    e.execute(
        "rack.macro_map",
        &json!({"track": rack.0, "macro": 1, "member": a.0, "slot": "a", "param": p.id, "min": 0.25, "max": 0.75, "curve": 0.5}),
    )
    .unwrap();
    let stored = |e: &Engine| e.session().track(a).unwrap().mixer.inserts[0].as_ref().unwrap().params[p.id];
    // Mapping moves the parameter to the macro (at 0): the bottom of the range.
    assert_eq!(stored(&e), p.clamp(p.from_normalized(0.25)));
    let steps = e.undo_history().len();
    e.execute("rack.macro", &json!({"track": rack.0, "macro": 1, "value": 0.5})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1, "a macro move is one step");
    // 0.5 -> curve 0.5 -> 0.25 -> 0.25 + 0.5 x 0.25 = 0.375 of the parameter's travel.
    assert_eq!(stored(&e), p.from_normalized(0.375));
    assert_eq!(rack_of(&e, rack).macros[0].value, 0.5);
    // A gesture merges like a fader drag.
    e.execute_merged("rack.macro", &json!({"track": rack.0, "macro": 1, "value": 0.6}), "gesture").unwrap();
    e.execute_merged("rack.macro", &json!({"track": rack.0, "macro": 1, "value": 1.0}), "gesture").unwrap();
    assert_eq!(e.undo_history().len(), steps + 2);
    assert_eq!(stored(&e), p.from_normalized(0.75));
    assert!(e.undo());
    assert_eq!(stored(&e), p.from_normalized(0.375));
    // Mapping the parameter to another macro moves it there (one macro per parameter).
    e.execute("rack.macro_map", &json!({"track": rack.0, "macro": 3, "member": a.0, "slot": "a", "param": p.id})).unwrap();
    let r = rack_of(&e, rack);
    assert!(r.macros[0].mappings.is_empty() && r.macros[2].mappings.len() == 1);
    assert!(e.execute("rack.macro_map", &json!({"track": rack.0, "macro": 9, "member": a.0, "slot": "a", "param": p.id})).is_err());
    assert!(e.execute("rack.macro_map", &json!({"track": rack.0, "macro": 1, "member": a.0, "slot": "b", "param": p.id})).is_err(), "an empty slot");
}

#[test]
fn a_rack_survives_save_and_load_and_an_old_file_loads_without_one() {
    let (mut e, rack, [a, b]) = racked();
    e.execute("rack.zone", &json!({"track": rack.0, "member": b.0, "key": {"low": 60, "fade_low": 64}})).unwrap();
    e.execute("mix.insert", &json!({"track": a.0, "slot": 0, "plugin": "gain"})).unwrap();
    let p = transit_dsp::plugin_info("gain").unwrap().params.first().unwrap().id;
    e.execute("rack.macro_map", &json!({"track": rack.0, "macro": 1, "member": a.0, "slot": "a", "param": p, "curve": -0.5})).unwrap();
    e.execute("rack.macro", &json!({"track": rack.0, "macro": 1, "value": 0.3})).unwrap();
    let source = json!({"port": "KeyStep", "channel": 1, "kind": "cc", "number": 74});
    e.execute("midi.map", &json!({"source": source, "target": format!("track:{}:macro:1", rack.0)})).unwrap();
    let text = e.session().to_json().unwrap();
    let back = Session::from_json(&text).unwrap();
    assert_eq!(back.track(rack).unwrap().rack, e.session().track(rack).unwrap().rack, "lossless");
    assert_eq!(back.to_json().unwrap(), text);
    assert!(back.midi_mappings.is_empty(), "a macro's MIDI source is stored on the rack, not in the session's mappings");
    // A file from before racks: no `rack` key anywhere loads every folder as a plain group.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    for t in v["tracks"].as_array_mut().unwrap() {
        t.as_object_mut().unwrap().remove("rack");
    }
    let old = Session::from_json(&v.to_string()).unwrap();
    assert!(old.tracks.iter().all(|t| t.rack.is_none()));
    assert!(old.track(rack).unwrap().is_folder());
    // A plain track writes no `rack` key, so files without racks are unchanged.
    assert!(!serde_json::to_string(old.track(a).unwrap()).unwrap().contains("\"rack\""));
}

#[test]
fn a_macro_learns_a_controller_stored_on_the_rack() {
    let (mut e, rack, [a, _]) = racked();
    let target = format!("track:{}:macro:4", rack.0);
    let source = json!({"port": "KeyStep", "channel": 1, "kind": "cc", "number": 20});
    let steps = e.undo_history().len();
    e.execute("midi.map", &json!({"source": source, "target": target})).unwrap();
    assert_eq!(e.undo_history().len(), steps + 1, "mapping a macro is one undo step");
    let r = rack_of(&e, rack);
    assert_eq!(r.macros[3].midi.as_ref().map(|m| m.number), Some(20));
    let in_force = e.session().midi_mappings.with_racks(e.session());
    assert_eq!(in_force.iter().map(|m| m.target.control.clone()).collect::<Vec<_>>(), [Control::Macro(3)]);
    let listed = e.execute("midi.mappings", &json!({})).unwrap();
    assert_eq!(listed["session"][0]["target"], json!(target));
    assert!(e.execute("midi.map", &json!({"source": source, "target": target, "scope": "profile"})).is_err(), "session only");
    assert!(e.execute("midi.map", &json!({"source": source, "target": format!("track:{}:macro:1", a.0)})).is_err(), "not a rack");
    e.execute("midi.unmap", &json!({"target": target})).unwrap();
    assert!(rack_of(&e, rack).macros[3].midi.is_none());
}

#[test]
fn a_track_dragged_into_a_rack_becomes_a_member_and_one_dragged_out_leaves() {
    let (mut e, rack, [_, b]) = racked();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let c = TrackId(r["tracks"][0].as_u64().unwrap());
    e.execute("track.move_to_folder", &json!({"tracks": [c.0], "folder": rack.0})).unwrap();
    assert!(rack_of(&e, rack).member(c).is_some(), "joins with full zones");
    e.execute("track.remove_from_folder", &json!({"tracks": [b.0]})).unwrap();
    assert!(rack_of(&e, rack).member(b).is_none());
    // An audio track inside the rack sums but plays no notes.
    let audio = e.session().tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id;
    e.execute("track.move_to_folder", &json!({"tracks": [audio.0], "folder": rack.0})).unwrap();
    assert!(rack_of(&e, rack).member(audio).is_none());
    assert_eq!(e.session().track(audio).unwrap().folder, Some(rack));
}

/// Review finding 1: Group into Rack on tracks that sit in a group (or another rack) takes them
/// out of it first, so their audio sums on the rack's bus, through the rack's fader, and the old
/// rack no longer lists them.
#[test]
fn a_rack_made_from_grouped_tracks_sums_them_on_its_own_bus() {
    let (mut e, old, [a, b]) = racked();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let c = TrackId(r["tracks"][0].as_u64().unwrap());
    // A and B sit in rack `old`; C sits in a plain group.
    let g = e.execute("track.move_to_new_folder", &json!({"tracks": [c.0]})).unwrap();
    let group = TrackId(g["folder"].as_u64().unwrap());
    let r = e.execute("rack.new", &json!({"tracks": [a.0, c.0]})).unwrap();
    let rack = TrackId(r["rack"].as_u64().unwrap());
    let s = e.session();
    let Route::Bus(bus) = s.track(rack).unwrap().mixer.input else { panic!("the rack reads its bus") };
    for m in [a, c] {
        let t = s.track(m).unwrap();
        assert_eq!((t.folder, &t.mixer.output), (Some(rack), &Route::Bus(bus)), "{} sums on the rack's bus", t.name);
    }
    assert_eq!(rack_of(&e, rack).members.iter().map(|m| m.track).collect::<Vec<_>>(), [a, c]);
    assert_eq!(rack_of(&e, old).members.iter().map(|m| m.track).collect::<Vec<_>>(), [b], "the old rack drops A");
    assert_eq!(s.tracks.iter().filter(|t| t.folder == Some(group)).count(), 0, "the old group is empty");
    assert_eq!(e.undo_history().iter().filter(|l| l.contains("Rack")).count(), 2, "one step per rack");

    // The summed output: notes on the new rack sound, and they all pass its fader. Muting the
    // old group or the old rack changes nothing; muting the new rack silences them.
    let mut s = e.session().clone();
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start: 0, length: 960 });
    let cid = s.new_clip_id();
    s.track_mut(rack).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "n", 0, 48_000, seq));
    let range = transit_time::Range::new(0, 24_000);
    let loud = |s: &Session| transit_mix::render_range(s, range, 256).iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(loud(&s) > 1e-3, "the rack plays");
    for t in [old, group] {
        s.track_mut(t).unwrap().mixer.mute = true;
    }
    assert!(loud(&s) > 1e-3, "nothing of the rack goes through the old group or rack");
    s.track_mut(rack).unwrap().mixer.mute = true;
    assert_eq!(loud(&s), 0.0, "everything the rack plays passes its own fader");
}

/// Review finding 7: every way out of a rack's folder drops the member and its macro mappings.
#[test]
fn every_move_out_of_a_rack_drops_the_member() {
    let (mut e, rack, [a, b]) = racked();
    e.execute("mix.insert", &json!({"track": a.0, "slot": 0, "plugin": "gain"})).unwrap();
    let p = transit_dsp::plugin_info("gain").unwrap().params.first().unwrap().id;
    e.execute("rack.macro_map", &json!({"track": rack.0, "macro": 1, "member": a.0, "slot": "a", "param": p})).unwrap();
    // Into a new folder.
    e.execute("track.move_to_new_folder", &json!({"tracks": [a.0]})).unwrap();
    let r = rack_of(&e, rack);
    assert!(r.member(a).is_none() && r.macros[0].mappings.is_empty());
    // Into an existing folder.
    let g = e.execute("track.new", &json!({"kind": "folder"})).unwrap();
    let g = g["tracks"][0].as_u64().unwrap();
    e.execute("track.move_to_folder", &json!({"tracks": [b.0], "folder": g})).unwrap();
    assert!(rack_of(&e, rack).members.is_empty());
    // And back in: members again, with full zones.
    e.execute("track.move_to_folder", &json!({"tracks": [a.0, b.0], "folder": rack.0})).unwrap();
    assert_eq!(rack_of(&e, rack).members.len(), 2);
}
