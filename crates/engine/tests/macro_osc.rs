//! The Macro Oscillator in a session (#195 slice 3): chosen as a track's instrument it stores every
//! parameter at its default, its parameters are set by command, saved and loaded with the session
//! (the render after a JSON round trip is the same bits), and automation lanes address them by id;
//! with #196 the voice count and the note-to-macro mapping too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use transit_engine::Engine;
use transit_model::{AutoParam, AutomationLane, Session, TrackId};
use transit_time::Range;

const ID: &str = "macro_oscillator";

fn engine() -> (Engine, TrackId) {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument", "name": "Macro"})).unwrap();
    let track = r["tracks"][0].as_u64().unwrap();
    e.execute("mix.instrument", &json!({"track": track, "plugin": ID})).unwrap();
    let r = e.execute("midi.new_clip", &json!({"track": track, "start": 0, "length": 48_000})).unwrap();
    let clip = r["clip"].as_u64().unwrap();
    for (pitch, start) in [(57, 0), (64, 480), (60, 1440)] {
        e.execute("event.midi_note", &json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 600, "velocity": 100})).unwrap();
    }
    (e, TrackId(track))
}

fn render(s: &Session) -> Vec<Vec<f32>> {
    transit_mix::render_range(s, Range::new(0, 72_000), 512)
}

fn bits(a: &[Vec<f32>]) -> Vec<u32> {
    a.iter().flatten().map(|v| v.to_bits()).collect()
}

fn params(e: &Engine, t: TrackId) -> std::collections::BTreeMap<String, f32> {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().params.clone()
}

#[test]
fn choosing_the_instrument_stores_every_parameter_at_its_default() {
    let (e, t) = engine();
    let info = transit_dsp::plugin_info(ID).unwrap();
    assert_eq!(info.name, "Macro Oscillator");
    let p = params(&e, t);
    assert_eq!(p.len(), info.params.len());
    for param in info.params {
        assert_eq!(p.get(param.id), Some(&param.default), "{}", param.id);
    }
    let out = render(e.session());
    assert!(out[0].iter().any(|v| v.abs() > 0.01), "the clip plays the instrument");
}

#[test]
fn parameters_save_and_load_with_the_session() {
    let (mut e, t) = engine();
    for (param, value) in [("model", 13.0), ("harmonics", 30.0), ("timbre", 80.0), ("decay", 70.0), ("tune", -5.0), ("aux_mix", 40.0)] {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value})).unwrap();
    }
    let before = render(e.session());
    let text = e.session().to_json().unwrap();
    let loaded = Session::from_json(&text).unwrap();
    let ins = loaded.track(t).unwrap().instrument.as_ref().unwrap();
    assert_eq!(ins.plugin, ID);
    assert_eq!(ins.params, params(&e, t));
    assert_eq!(ins.params.get("model"), Some(&13.0));
    assert_eq!(ins.params.get("tune"), Some(&-5.0));
    assert!(bits(&render(&loaded)) == bits(&before), "the loaded session renders the same bits");
    let mut fresh = engine().0;
    fresh.execute("mix.instrument_param", &json!({"track": t.0, "param": "model", "value": 13.0})).unwrap();
    assert!(bits(&render(fresh.session())) != bits(&before), "the parameters matter to the render");
}

/// The voice count and the note-to-macro mapping (#196) are parameters like the others: saved,
/// loaded, and the loaded session plays the same bits; a session saved before #196 (without them)
/// opens with eight voices and no mapping.
#[test]
fn voices_and_the_mapping_save_and_load_with_the_session() {
    let (mut e, t) = engine();
    let set = [("voices", 2.0), ("velocity_curve", 1.0), ("velocity_timbre", 70.0), ("key_curve", 2.0), ("key_morph", -35.0), ("key_decay", 20.0)];
    for (param, value) in set {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value})).unwrap();
    }
    let before = render(e.session());
    let loaded = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    let ins = loaded.track(t).unwrap().instrument.as_ref().unwrap();
    for (param, value) in set {
        assert_eq!(ins.params.get(param), Some(&value), "{param}");
    }
    assert!(bits(&render(&loaded)) == bits(&before), "the loaded session renders the same bits");
    let (plain, _) = engine();
    assert!(bits(&render(plain.session())) != bits(&before), "voices and mapping change the render");
    // An older session: only the eleven parameters of #195.
    let mut old = plain.session().clone();
    let ins = old.track_mut(t).unwrap().instrument.as_mut().unwrap();
    ins.params.retain(|k, _| !k.starts_with("velocity_") && !k.starts_with("key_") && k != "voices");
    assert_eq!(ins.params.len(), 11);
    assert!(bits(&render(&old)) == bits(&render(plain.session())), "the new parameters take their defaults");
}

#[test]
fn automation_lanes_address_the_parameters_by_id() {
    let info = transit_dsp::plugin_info(ID).unwrap();
    for param in info.params {
        let lane = AutoParam::parse(&format!("plugin:instrument:{}", param.id)).unwrap();
        assert_eq!(lane, AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: param.id.to_string() });
        assert_eq!(lane.label(), format!("instrument {}", param.id));
    }
    let (mut e, t) = engine();
    let plain = render(e.session());
    let mut lane = AutomationLane::new(AutoParam::parse("plugin:instrument:timbre").unwrap());
    lane.set_point(0, 0.0);
    lane.set_point(72_000, 100.0);
    let track = e.session_mut().track_mut(t).unwrap();
    track.mixer.automation_mode = transit_model::AutomationMode::Read;
    track.automation.push(lane);
    let automated = render(e.session());
    assert!(bits(&automated) != bits(&plain), "a timbre lane changes the render");
    let text = e.session().to_json().unwrap();
    let loaded = Session::from_json(&text).unwrap();
    assert!(bits(&render(&loaded)) == bits(&automated), "the lane saves and loads with the session");
}

/// A session saved while the branch had a `hardware_tuning` parameter (removed: the port always
/// plays concert pitch) opens without an error; the open logs the ignored id and the render is the
/// same as without it.
#[test]
fn a_session_with_the_removed_tuning_id_opens_and_logs_it() {
    let (mut e, t) = engine();
    let clean = render(e.session());
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    ins.params.insert("hardware_tuning".into(), 1.0);
    let dir = std::env::temp_dir().join(format!("transit-macro-osc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.transit");
    std::fs::write(&path, e.session().to_json().unwrap()).unwrap();
    let mut opened = Engine::default();
    let r = opened.execute("session.open", &json!({"path": path.to_string_lossy()}));
    let _ = std::fs::remove_dir_all(&dir);
    r.expect("the session opens");
    assert_eq!(params(&opened, t).get("hardware_tuning"), Some(&1.0), "the value stays in the document");
    let lines = transit_engine::io::unknown_param_lines(opened.session());
    assert_eq!(lines, ["session: track \"Macro\": Macro Oscillator ignores the unknown parameter `hardware_tuning`"]);
    assert!(bits(&render(opened.session())) == bits(&clean), "the device ignores it");
    assert!(transit_engine::io::unknown_param_lines(e.session()).len() == 1);
    let (fresh, _) = engine();
    assert!(transit_engine::io::unknown_param_lines(fresh.session()).is_empty());
}
