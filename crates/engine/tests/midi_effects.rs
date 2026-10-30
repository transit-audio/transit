//! The MIDI effect slot (#201, ADR 0036) through the engine's commands: add, remove, move, bypass
//! and parameter edits are one undo step each, the chain saves and loads, its automation lanes
//! follow their effect, and it is part of what a freeze renders. Also writes the corpus fixture.

use serde_json::{Value, json};
use transit_engine::{Engine, EngineError};
use transit_model::{AutoParam, ChannelFormat, Clip, Insert, MidiEffectSlot, Session, TrackKind};
use transit_time::{SampleRate, TICKS_PER_QUARTER};

fn engine() -> Engine {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    e.execute("track.new", &json!({"kind": "audio", "name": "Gtr"})).unwrap();
    e
}

fn run(e: &mut Engine, cmd: &str, p: Value) -> Value {
    e.execute(cmd, &p).unwrap_or_else(|err| panic!("{cmd} {p}: {err:?}"))
}

fn chain(e: &Engine) -> Vec<(String, bool, Vec<(String, f32)>)> {
    let t = e.session().tracks.iter().find(|t| t.name == "Keys").unwrap();
    t.midi_effects.iter().map(|f| (f.plugin.clone(), f.bypass, f.params.iter().map(|(k, v)| (k.clone(), *v)).collect())).collect()
}

#[track_caller]
fn refused(r: Result<Value, EngineError>, what: &str) {
    let Err(EngineError::BadParams(_, msg)) = r else { panic!("expected a refusal, got {r:?}") };
    assert!(msg.contains(what), "`{msg}` should mention `{what}`");
}

#[test]
fn every_edit_is_one_undo_step() {
    let mut e = engine();
    let edits = [
        ("mix.midi_effect", json!({"track": "Keys", "plugin": "midi_transpose", "params": {"semitones": 7}})),
        ("mix.midi_effect", json!({"track": "Keys", "plugin": "midi_velocity", "at": 0})),
        ("mix.midi_effect_move", json!({"track": "Keys", "from": 0, "to": 1})),
        ("mix.midi_effect_bypass", json!({"track": "Keys", "index": 0})),
        ("mix.midi_effect_param", json!({"track": "Keys", "index": 1, "param": "scale", "value": 50})),
        ("mix.midi_effect_remove", json!({"track": "Keys", "index": 0})),
    ];
    let mut states = vec![chain(&e)];
    for (cmd, p) in &edits {
        let before = e.undo_pushes();
        run(&mut e, cmd, p.clone());
        assert_eq!(e.undo_pushes(), before + 1, "{cmd}: one undo step");
        states.push(chain(&e));
    }
    assert_eq!(chain(&e), vec![("midi_velocity".to_string(), false, vec![("max".into(), 127.0), ("min".into(), 1.0), ("offset".into(), 0.0), ("scale".into(), 50.0)])]);
    for want in states.iter().rev().skip(1) {
        assert!(e.undo());
        assert_eq!(&chain(&e), want);
    }
    assert!(chain(&e).is_empty());
}

#[test]
fn edits_are_checked() {
    let mut e = engine();
    refused(e.execute("mix.midi_effect", &json!({"track": "Gtr", "plugin": "midi_transpose"})), "MIDI or instrument track");
    refused(e.execute("mix.midi_effect", &json!({"track": "Keys", "plugin": "eq7"})), "not a MIDI effect");
    refused(e.execute("mix.midi_effect", &json!({"track": "Keys", "plugin": "midi_transpose", "params": {"cents": 1}})), "no parameter");
    for _ in 0..transit_model::MAX_MIDI_EFFECTS {
        run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "midi_velocity"}));
    }
    refused(e.execute("mix.midi_effect", &json!({"track": "Keys", "plugin": "midi_velocity"})), "the most it holds");
    refused(e.execute("mix.midi_effect_remove", &json!({"track": "Keys", "index": 8})), "not a MIDI effect of the track");
    refused(e.execute("mix.midi_effect_move", &json!({"track": "Keys", "from": 2, "to": 2})), "same place");
    refused(e.execute("mix.midi_effect_param", &json!({"track": "Keys", "index": 0, "param": "scale", "value": "x"})), "number");
    let v = run(&mut e, "mix.midi_effect_param", json!({"track": "Keys", "index": 0, "param": "scale", "value": 1e9}));
    assert_eq!(v["value"], json!(400.0), "clamped into range");
    let list = run(&mut e, "engine.midi_effects", json!({}));
    let ids: Vec<&str> = list.as_array().unwrap().iter().filter_map(|i| i["id"].as_str()).collect();
    assert_eq!(ids, ["midi_transpose", "midi_velocity"]);
}

#[test]
fn the_chain_saves_loads_and_its_lanes_follow_each_effect() {
    let mut e = engine();
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "midi_transpose", "params": {"semitones": -5}}));
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "midi_velocity"}));
    run(&mut e, "mix.midi_effect_bypass", json!({"track": "Keys", "index": 1, "value": true}));
    let lane = |i: usize| AutoParam::Plugin { slot: AutoParam::midi_effect_slot(i).unwrap(), param: "semitones".into() };
    e.session_mut().tracks[0].lane_mut(&lane(0)).set_point(0, 3.0);
    let json = e.session().to_json().unwrap();
    let loaded = Session::from_json(&json).unwrap();
    assert_eq!(loaded.tracks[0].midi_effects, e.session().tracks[0].midi_effects);
    assert!(loaded.tracks[0].lane(&lane(0)).is_some());
    run(&mut e, "mix.midi_effect_move", json!({"track": "Keys", "from": 0, "to": 1}));
    let t = &e.session().tracks[0];
    assert!(t.lane(&lane(0)).is_none() && t.lane(&lane(1)).is_some(), "the lane moved with Transpose");
    run(&mut e, "mix.midi_effect_remove", json!({"track": "Keys", "index": 1}));
    assert!(e.session().tracks[0].automation.iter().all(|l| l.param != lane(1)), "and went with it");
}

#[test]
fn a_midi_effect_is_part_of_what_a_freeze_renders() {
    let mut e = engine();
    let before = e.session().tracks[0].freeze_hash();
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "midi_transpose"}));
    let with = e.session().tracks[0].clone();
    assert_ne!(with.freeze_hash(), before);
    run(&mut e, "mix.midi_effect_param", json!({"track": "Keys", "index": 0, "param": "semitones", "value": 2}));
    assert!(!e.session().tracks[0].same_freeze_inputs(&with), "a parameter is material too");
    run(&mut e, "mix.midi_effect_remove", json!({"track": "Keys", "index": 0}));
    assert_eq!(e.session().tracks[0].freeze_hash(), before, "an empty chain hashes as before the slot existed");
}

/// The corpus fixture (#201): an instrument track playing a MIDI clip through Transpose (with an
/// automated parameter) and a bypassed Velocity.
fn midi_effect_session() -> Session {
    let sr = SampleRate::HZ_48000;
    let mut s = Session::new("MIDI Effects", sr);
    let _ = s.tempo.set_tempo(0, 120.0);
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let mut seq = transit_midi::Sequence::default();
    for (i, pitch) in [60u8, 64, 67].into_iter().enumerate() {
        seq.notes.push(transit_midi::Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * TICKS_PER_QUARTER, length: TICKS_PER_QUARTER });
    }
    let cid = s.new_clip_id();
    let clip = Clip::midi(cid, "Line", 0, sr.samples(2.0), seq);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("subtractive_synth"));
    let mut transpose = MidiEffectSlot::new("midi_transpose");
    transpose.params.insert("semitones".into(), 7.0);
    tr.add_midi_effect(transpose, None);
    let mut velocity = MidiEffectSlot::new("midi_velocity");
    for (k, v) in [("scale", 80.0), ("offset", 10.0), ("min", 20.0), ("max", 120.0)] {
        velocity.params.insert(k.into(), v);
    }
    velocity.bypass = true;
    tr.add_midi_effect(velocity, None);
    let lane = tr.lane_mut(&AutoParam::Plugin { slot: AutoParam::midi_effect_slot(0).unwrap(), param: "semitones".into() });
    lane.set_point(0, 7.0);
    lane.set_point(sr.samples(1.0), -5.0);
    tr.playlist_mut().unwrap().clips.push(clip);
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    s
}

#[test]
#[ignore = "writes the MIDI effect fixture (#201) into crates/model/tests/corpus; run it by hand"]
fn write_midi_effect_fixture() {
    let file = "v0.6.0-midi-effects.transit";
    let scratch = std::env::temp_dir().join(format!("transit-corpus-{}-midi-effects", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut engine = Engine::new(midi_effect_session());
    transit_engine::io::save_session(&mut engine, scratch.join(file).to_str().unwrap()).unwrap();
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus");
    std::fs::copy(scratch.join(file), out.join(file)).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
    println!("wrote {}", out.join(file).display());
}

#[test]
fn the_fixture_session_builds_its_chain() {
    let s = midi_effect_session();
    let t = &s.tracks[0];
    assert_eq!(t.midi_effects.iter().map(|f| f.plugin.as_str()).collect::<Vec<_>>(), ["midi_transpose", "midi_velocity"]);
    assert!(t.midi_effects[1].bypass);
}
