//! The Texture Synth in a session (#241 slice 3): chosen as a track's instrument it stores every
//! parameter at its default and plays the track's notes; its parameters are set by command, saved
//! and loaded with the session (the render after a JSON round trip is the same bits); a session
//! that stored only some of them loads the rest at their defaults; automation lanes address them
//! by id; the render is the same at 512- and 64-frame blocks; a frozen cloud stored on the slot
//! (`mix.insert_state`) saves and loads with the session and sounds from the render's first block.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use transit_engine::Engine;
use transit_model::{AutoParam, Session, TrackId};
use transit_time::Range;

const ID: &str = "texture_synth";

fn engine() -> (Engine, TrackId) {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Texture Synth"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    e.execute("mix.instrument", &json!({"track": track, "plugin": ID})).unwrap();
    let r = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 48_000})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    for (pitch, start) in [(57, 0), (64, 480), (60, 1440), (67, 1440)] {
        e.execute("event.midi_note", &json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 600, "velocity": 100})).unwrap();
    }
    (e, TrackId(track))
}

fn render(s: &Session, block: usize) -> Vec<Vec<f32>> {
    transit_mix::render_range(s, Range::new(0, 72_000), block)
}

fn bits(a: &[Vec<f32>]) -> Vec<u32> {
    a.iter().flatten().map(|v| v.to_bits()).collect()
}

fn params(e: &Engine, t: TrackId) -> std::collections::BTreeMap<String, f32> {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().params.clone()
}

#[test]
fn choosing_the_instrument_stores_every_parameter_at_its_default_and_plays_the_notes() {
    let (e, t) = engine();
    let info = transit_dsp::plugin_info(ID).unwrap();
    assert_eq!(info.name, "Texture Synth");
    assert!(info.is_instrument);
    let p = params(&e, t);
    assert_eq!(p.len(), info.params.len());
    for param in info.params {
        assert_eq!(p.get(param.id), Some(&param.default), "{}", param.id);
    }
    let out = render(e.session(), 512);
    assert!(out[0].iter().any(|v| v.abs() > 0.01) && out[1].iter().any(|v| v.abs() > 0.01), "the clip plays the instrument");
    assert!(bits(&render(e.session(), 64)) == bits(&out), "64- and 512-frame blocks render the same bits");
}

#[test]
fn parameters_save_and_load_with_the_session() {
    let (mut e, t) = engine();
    let set = [
        ("quality", 1.0),
        ("density", 60.0),
        ("size", 40.0),
        ("feedback", 80.0),
        ("pitch", -5.0),
        ("dry_wet", 60.0),
        ("shaper", 1.0),
        ("mod1_source", 5.0),
        ("mod1_dest", 16.0),
        ("mod1_amount", 40.0),
    ];
    for (param, value) in set {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value})).unwrap();
    }
    let before = render(e.session(), 512);
    let loaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    let ins = loaded.track(t).unwrap().instrument.as_ref().unwrap();
    assert_eq!(ins.plugin, ID);
    assert_eq!(ins.params, params(&e, t));
    for (param, value) in set {
        assert_eq!(ins.params.get(param), Some(&value), "{param}");
    }
    assert!(bits(&render(&loaded, 512)) == bits(&before), "the loaded session renders the same bits");
    let (plain, _) = engine();
    assert!(bits(&render(plain.session(), 512)) != bits(&before), "the parameters matter to the render");
}

/// A session that stored only the module's controls (as a hand-made or older file might) loads the
/// instrument's other parameters at their defaults: it renders what a fresh instrument renders.
#[test]
fn missing_parameters_load_at_their_defaults() {
    let (plain, t) = engine();
    let mut old = plain.session().clone();
    let ins = old.track_mut(t).unwrap().instrument.as_mut().unwrap();
    let base = ["time", "size", "shape", "pitch", "density", "feedback", "dry_wet", "reverb"];
    ins.params.retain(|k, _| base.contains(&k.as_str()));
    let text = old.to_json().unwrap();
    let loaded = Session::from_json(&text).unwrap();
    assert_eq!(loaded.track(t).unwrap().instrument.as_ref().unwrap().params.len(), base.len());
    assert!(bits(&render(&loaded, 512)) == bits(&render(plain.session(), 512)), "the missing parameters take their defaults");
}

#[test]
fn automation_lanes_address_the_parameters_by_id() {
    let info = transit_dsp::plugin_info(ID).unwrap();
    for param in info.params {
        let lane = AutoParam::parse(&format!("plugin:instrument:{}", param.id)).unwrap();
        assert_eq!(lane, AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: param.id.to_string() });
    }
}

/// A frozen cloud captured from a live instance and stored on the instrument slot saves and loads
/// with the session; the loaded session's render starts from it (the frozen grains sound from the
/// first block, with no note), where a session without it is silent; storing an empty state (Freeze
/// off at the save) clears it.
#[test]
fn a_frozen_cloud_saves_and_loads_with_the_session() {
    // A session without clips: the cloud is the only sound.
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Cloud"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    e.execute("mix.instrument", &json!({"track": t.0, "plugin": ID})).unwrap();
    for (param, value) in [("size", 100.0), ("density", 15.0), ("freeze", 1.0)] {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value})).unwrap();
    }
    let mut live = transit_dsp::create(ID).unwrap();
    live.prepare(48_000.0, 512, 2);
    for (param, value) in [("size", 100.0), ("density", 15.0)] {
        assert!(live.set_param(param, value));
    }
    let mut io = vec![vec![0.0f32; 512]; 2];
    live.note_on(0, 60, 110);
    for b in 0..60 {
        if b == 40 {
            assert!(live.set_param("freeze", 1.0));
        }
        live.process(&mut io, 512);
    }
    live.note_off(0, 60);
    live.process(&mut io, 512);
    let state = live.save_state().expect("a frozen cloud");
    let silent = render(e.session(), 512);
    assert!(silent.iter().flatten().all(|v| *v == 0.0), "no clip, no state: silence");
    e.execute("mix.insert_state", &json!({"track": t.0, "slot": "instrument", "state": transit_model::b64::encode(&state)})).unwrap();
    let loaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(loaded.track(t).unwrap().instrument.as_ref().unwrap().state_bytes().as_deref(), Some(&state[..]));
    let out = render(&loaded, 512);
    assert!(out[0][..512].iter().any(|v| v.abs() > 0.001), "the frozen cloud sounds from the first block");
    assert!(bits(&render(&loaded, 64)) == bits(&out), "64- and 512-frame blocks render the same bits");
    e.execute("mix.insert_state", &json!({"track": t.0, "slot": "instrument", "state": ""})).unwrap();
    assert!(e.session().track(t).unwrap().instrument.as_ref().unwrap().state.is_none(), "an empty state clears it");
}
