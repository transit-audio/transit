//! The Random Sequencer MIDI effect (#202) through the engine's commands: it is listed, added with
//! its defaults, a preset recall (every parameter in one command) is one undo step, its settings
//! save and load, and a session that names only some of its parameters loads with the rest at
//! their defaults.

use serde_json::{Value, json};
use transit_engine::{Engine, EngineError};
use transit_model::Session;

fn engine() -> Engine {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    e
}

fn run(e: &mut Engine, cmd: &str, p: Value) -> Value {
    e.execute(cmd, &p).unwrap_or_else(|err| panic!("{cmd} {p}: {err:?}"))
}

#[test]
fn it_is_listed_and_added_with_its_defaults() {
    let mut e = engine();
    let list = run(&mut e, "engine.midi_effects", json!({}));
    let entry = list.as_array().unwrap().iter().find(|i| i["id"] == "random_sequencer").cloned().expect("listed");
    assert_eq!(entry["name"], "Random Sequencer");
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "random_sequencer"}));
    let fx = &e.session().tracks[0].midi_effects[0];
    let info = transit_dsp::midi_fx::info("random_sequencer").unwrap();
    assert_eq!(fx.params.len(), info.params.len());
    for p in info.params {
        assert_eq!(fx.params.get(p.id).copied(), Some(p.default), "{}", p.id);
    }
    assert_eq!(fx.params["rate"], 16.0, "1/16");
    assert_eq!(fx.params["seed_latch"], 1.0, "the seed latched");
}

#[test]
fn setting_every_parameter_at_once_is_one_undo_step_and_checked() {
    let mut e = engine();
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "random_sequencer"}));
    let before = e.undo_pushes();
    let v = run(&mut e, "mix.midi_effect_params", json!({"track": "Keys", "index": 0, "params": {"rate": 13, "freeze": 1, "seed": 1e9}}));
    assert_eq!(v["set"], 3);
    assert_eq!(e.undo_pushes(), before + 1, "one undo step");
    let fx = &e.session().tracks[0].midi_effects[0];
    assert_eq!((fx.params["rate"], fx.params["freeze"], fx.params["seed"]), (13.0, 1.0, 9999.0), "clamped into range");
    let r = e.execute("mix.midi_effect_params", &json!({"track": "Keys", "index": 0, "params": {"rate": 2, "tempo": 1}}));
    let Err(EngineError::BadParams(_, msg)) = r else { panic!("expected a refusal, got {r:?}") };
    assert!(msg.contains("no parameter"), "{msg}");
    assert_eq!(e.session().tracks[0].midi_effects[0].params["rate"], 13.0, "a refused recall changes nothing");
    assert!(e.undo());
    assert_eq!(e.session().tracks[0].midi_effects[0].params["rate"], 16.0);
}

#[test]
fn its_settings_save_and_load_and_missing_ones_take_their_defaults() {
    let mut e = engine();
    run(&mut e, "mix.midi_effect", json!({"track": "Keys", "plugin": "random_sequencer", "params": {"scale": 2, "deja_vu": 50, "seed": 42}}));
    let json = e.session().to_json().unwrap();
    let loaded = Session::from_json(&json).unwrap();
    assert_eq!(loaded.tracks[0].midi_effects, e.session().tracks[0].midi_effects);
    // A hand-edited file that names two parameters: the effect reads the others at their defaults.
    let mut v: Value = serde_json::from_str(&json).unwrap();
    v["tracks"][0]["midi_effects"][0]["params"] = json!({"play": 3.0, "seed": 7.0});
    let sparse = Session::from_json(&v.to_string()).unwrap();
    let fx = &sparse.tracks[0].midi_effects[0];
    assert_eq!(fx.params.len(), 2);
    let mut effect = transit_dsp::midi_fx::Builtin::new(&fx.plugin).unwrap();
    for p in transit_dsp::midi_fx::info(&fx.plugin).unwrap().params {
        if let Some(v) = fx.params.get(p.id) {
            assert!(effect.effect().set_param(p.id, *v));
        }
        let want = fx.params.get(p.id).copied().unwrap_or(p.default);
        assert_eq!(effect.effect_ref().param(p.id), Some(want), "{}", p.id);
    }
}
