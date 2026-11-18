//! A built-in instrument added with `mix.instrument` stores every parameter at its default in the
//! session (#753): a later change to a default never moves a saved session, because the saved
//! value, not the build's default, is what loads.

use crate::Engine;
use serde_json::json;
use transit_model::TrackId;

const BUILT_INS: [&str; 5] = ["macro_oscillator", "modal_voice", "resonator", "texture_synth", "sampler"];

fn instrument_track(e: &mut Engine, plugin: &str) -> TrackId {
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.instrument", &json!({"track": t.0, "plugin": plugin})).unwrap();
    t
}

#[test]
fn an_added_instrument_saves_every_default_and_loads_the_saved_values() {
    let mut e = Engine::default();
    let tracks: Vec<(TrackId, &str)> = BUILT_INS.iter().map(|p| (instrument_track(&mut e, p), *p)).collect();
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
    for (t, plugin) in &tracks {
        let ins = r.session().track(*t).and_then(|x| x.instrument.clone()).unwrap();
        let info = transit_dsp::plugin_info(plugin).unwrap();
        assert_eq!(ins.params.len(), info.params.len(), "{plugin}: every parameter loads");
        for p in info.params {
            assert_eq!(ins.params.get(p.id), Some(&p.default), "{plugin}.{}", p.id);
        }
    }
}
