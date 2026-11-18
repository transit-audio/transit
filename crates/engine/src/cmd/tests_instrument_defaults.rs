//! Every way a built-in instrument enters a session stores its whole parameter table at the
//! build's defaults (#753): a later change to a default never moves a session saved after it,
//! because the saved value, not the build's default, is what loads. Loading never fills anything
//! in: a map stored empty (a session from before this) stays empty, so a default change reaches
//! such a session as one describable event.

use crate::Engine;
use serde_json::json;
use transit_model::{Insert, TrackId};

const BUILT_INS: [&str; 5] = ["macro_oscillator", "modal_voice", "resonator", "texture_synth", "sampler"];

fn last_track(e: &Engine) -> TrackId {
    e.session().tracks.last().unwrap().id
}

fn instrument_of(e: &Engine, t: TrackId) -> Insert {
    e.session().track(t).and_then(|x| x.instrument.clone()).unwrap()
}

/// Every parameter of `ins`'s table is stored, at its default (and nothing else is stored).
fn assert_every_default(ins: &Insert, path: &str) {
    let info = transit_dsp::plugin_info(&ins.plugin).unwrap_or_else(|| panic!("{path}: {} is a built-in", ins.plugin));
    assert_eq!(ins.params.len(), info.params.len(), "{path}: {} stores its whole table", ins.plugin);
    for p in info.params {
        assert_eq!(ins.params.get(p.id), Some(&p.default), "{path}: {}.{} at its default", ins.plugin, p.id);
    }
}

#[test]
fn every_creation_path_stores_the_whole_table_at_its_defaults() {
    let mut e = Engine::default();
    // track.new: the default instrument (Add Track > Instrument), and each built-in named.
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    assert_every_default(&instrument_of(&e, last_track(&e)), "track.new");
    for plugin in BUILT_INS {
        e.execute("track.new", &json!({"kind": "instrument", "instrument": plugin})).unwrap();
        let ins = instrument_of(&e, last_track(&e));
        assert_eq!(ins.plugin, plugin);
        assert_every_default(&ins, "track.new with an instrument");
    }
    // mix.instrument: replacing the instrument.
    for plugin in BUILT_INS {
        e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
        let t = last_track(&e);
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": plugin})).unwrap();
        assert_every_default(&instrument_of(&e, t), "mix.instrument");
    }
    // mix.sampler_new_patch on another instrument's track makes a Sampler.
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = last_track(&e);
    e.execute("mix.sampler_new_patch", &json!({"track": t.0})).unwrap();
    let ins = instrument_of(&e, t);
    assert_eq!(ins.plugin, "sampler");
    assert_every_default(&ins, "mix.sampler_new_patch");
    // A hosted instrument named to track.new keeps its empty map (its own state rules).
    e.execute("track.new", &json!({"kind": "instrument", "instrument": "clap:test.hosted"})).unwrap();
    assert!(instrument_of(&e, last_track(&e)).params.is_empty(), "a hosted plug-in's map stays empty");
}

#[test]
fn a_midi_file_import_stores_the_whole_table() {
    let mut seq = transit_midi::Sequence::default();
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
    let smf = transit_midi::Smf { format: 1, tracks: vec![transit_midi::SmfTrack { name: "Keys".into(), sequence: seq }], ..Default::default() };
    let mut e = Engine::default();
    let ids = crate::io::import_midi_bytes(&mut e, &transit_midi::write_smf(&smf).unwrap(), 0, false).unwrap();
    assert_eq!(ids.len(), 1);
    assert_every_default(&instrument_of(&e, ids[0]), "MIDI file import");
}

#[test]
fn a_dawproject_import_stores_the_whole_table() {
    let dir = transit_test_support::TestDir::new("instrument-defaults-dawproject");
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = Engine::default();
    a.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let path = dir.join("song.dawproject");
    crate::io::export_dawproject(&a, &path.to_string_lossy()).unwrap();
    let mut b = Engine::default();
    crate::io::open_dawproject(&mut b, &path.to_string_lossy()).unwrap();
    let inst: Vec<Insert> = b.session().tracks.iter().filter_map(|t| t.instrument.clone()).collect();
    assert_eq!(inst.len(), 1, "the instrument track comes in");
    assert_every_default(&inst[0], "DAWproject import");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_added_instrument_saves_every_default_and_loads_the_saved_values() {
    let mut e = Engine::default();
    let mut tracks = Vec::new();
    for plugin in BUILT_INS {
        e.execute("track.new", &json!({"kind": "instrument", "instrument": plugin})).unwrap();
        tracks.push((last_track(&e), plugin));
    }
    let text = e.session().to_json().unwrap();
    // The saved document names every parameter with the value it had: the Level a synth was
    // added at is in the file, and so is the Sampler's Output.
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    for (t, plugin) in &tracks {
        let info = transit_dsp::plugin_info(plugin).unwrap();
        let stored = doc["tracks"].as_array().unwrap().iter().find(|x| x["id"] == json!(t.0)).unwrap()["instrument"]["params"].clone();
        for p in info.params {
            assert_eq!(stored[p.id].as_f64(), Some(f64::from(p.default)), "{plugin}.{} saved at its default", p.id);
        }
    }
    // Loaded, the session holds what was saved, whatever the build's defaults are.
    let mut r = Engine::default();
    crate::io::open_session_bytes(&mut r, "saved.transit", text.as_bytes()).unwrap();
    for (t, _) in &tracks {
        assert_every_default(&instrument_of(&r, *t), "saved and loaded");
    }
}

#[test]
fn loading_a_session_never_fills_an_empty_map() {
    // A session saved before every path stored the defaults: its instruments and an insert hold
    // empty maps. Loading and saving again keeps them empty; nothing is filled in on the way.
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "instrument": "macro_oscillator"})).unwrap();
    let synth = last_track(&e);
    e.execute("track.new", &json!({"kind": "instrument", "instrument": "sampler"})).unwrap();
    let sampler = last_track(&e);
    e.execute("mix.insert", &json!({"track": synth.0, "plugin": "compressor"})).unwrap();
    for t in [synth, sampler] {
        let tr = e.session_mut().track_mut(t).unwrap();
        tr.instrument.as_mut().unwrap().params.clear();
        if let Some(ins) = tr.mixer.inserts[0].as_mut() {
            ins.params.clear();
        }
    }
    let text = e.session().to_json().unwrap();
    let empty = |e: &Engine| {
        let s = e.session();
        let i = |t: TrackId| s.track(t).unwrap().instrument.as_ref().unwrap().params.is_empty();
        let insert = s.track(synth).unwrap().mixer.inserts[0].as_ref().unwrap().params.is_empty();
        (i(synth), i(sampler), insert)
    };
    let mut r = Engine::default();
    crate::io::open_session_bytes(&mut r, "old.transit", text.as_bytes()).unwrap();
    assert_eq!(empty(&r), (true, true, true), "loading fills nothing in");
    let again = r.session().to_json().unwrap();
    let mut r2 = Engine::default();
    crate::io::open_session_bytes(&mut r2, "again.transit", again.as_bytes()).unwrap();
    assert_eq!(empty(&r2), (true, true, true), "nor does saving it again");
}
