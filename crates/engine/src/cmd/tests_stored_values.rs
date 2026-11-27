//! The last two places that stored only what was set now store every value (#753): the Macro
//! Oscillator loading puts in place of the retired Subtractive Synth, and a clip's effects. A later
//! change of a default then moves neither once the session is saved again.

use crate::Engine;
use serde_json::json;
use transit_model::legacy_synth::{DEFAULT_INSTRUMENT, RETIRED_SYNTH_ID, SUBSTITUTE_LEVEL_DB};
use transit_model::{ChannelFormat, Insert, Session, TrackId, TrackKind};

/// A session text with one instrument track whose slot names the retired synth.
fn old_session() -> (String, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    let mut old = Insert::new(RETIRED_SYNTH_ID);
    old.params.insert("cutoff".into(), 300.0);
    s.track_mut(t).unwrap().instrument = Some(old);
    (s.to_json().unwrap(), t)
}

/// `ins` stores the whole Macro Oscillator table at the build's defaults, except the substitute's level.
fn assert_substitute_table(ins: &Insert, path: &str) {
    let info = transit_dsp::plugin_info(DEFAULT_INSTRUMENT).unwrap();
    assert_eq!(ins.plugin, DEFAULT_INSTRUMENT, "{path}");
    assert_eq!(ins.params.len(), info.params.len(), "{path}: the whole table, nothing else");
    for p in info.params {
        let want = if p.id == "level" { SUBSTITUTE_LEVEL_DB } else { p.default };
        assert_eq!(ins.params.get(p.id), Some(&want), "{path}: {} stored", p.id);
    }
}

#[test]
fn a_substituted_macro_oscillator_stores_its_whole_table_and_keeps_its_level() {
    let (text, t) = old_session();
    let mut e = Engine::default();
    crate::io::open_session_bytes(&mut e, "old.transit", text.as_bytes()).unwrap();
    assert_substitute_table(e.session().track(t).unwrap().instrument.as_ref().unwrap(), "opened");
    // Saved, every id is in the file, so a later change of a default cannot reach it.
    let saved = e.session().to_json().unwrap();
    let doc: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let params = &doc["tracks"][0]["instrument"]["params"];
    for p in transit_dsp::plugin_info(DEFAULT_INSTRUMENT).unwrap().params {
        assert!(params.get(p.id).is_some(), "{} saved", p.id);
    }
    let mut r = Engine::default();
    crate::io::open_session_bytes(&mut r, "saved.transit", saved.as_bytes()).unwrap();
    assert_substitute_table(r.session().track(t).unwrap().instrument.as_ref().unwrap(), "saved and loaded");
    assert!(!r.is_dirty(), "a saved substitution loads clean");
}

#[test]
fn a_session_opened_from_a_file_and_a_donor_import_substitute_with_the_whole_table_too() {
    let (text, _) = old_session();
    let dir = transit_test_support::TestDir::new("b753p-substitute");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.transit");
    std::fs::write(&path, &text).unwrap();
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert_substitute_table(e.session().tracks.iter().find_map(|t| t.instrument.as_ref()).unwrap(), "session.open");
    let mut host = Engine::default();
    host.execute("file.import_session_data", &json!({"path": path.to_string_lossy(), "tracks": ["Lead"]})).unwrap();
    assert_substitute_table(host.session().track_by_name("Lead").unwrap().instrument.as_ref().unwrap(), "file.import_session_data");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_session_without_the_retired_synth_gets_nothing_filled_in_on_load() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().params.clear();
    let text = e.session().to_json().unwrap();
    let mut r = Engine::default();
    crate::io::open_session_bytes(&mut r, "empty.transit", text.as_bytes()).unwrap();
    assert!(r.session().track(t).unwrap().instrument.as_ref().unwrap().params.is_empty(), "a stored empty map stays empty");
}

fn clip_values(e: &Engine, clip: u64) -> std::collections::BTreeMap<String, f64> {
    let pre = format!("clip_fx.{clip}.");
    e.session().edit.values.iter().filter_map(|(k, v)| k.strip_prefix(&pre).map(|n| (n.to_string(), *v))).collect()
}

fn engine_with_clip() -> (Engine, u64) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let buf = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] };
    let src = crate::io::add_source(&mut s, "tone", buf, None, transit_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, transit_model::Clip::audio(id, "tone", src, 0, 0, 4_800));
    (Engine::new(s), id.0)
}

#[test]
fn setting_one_clip_effect_value_stores_its_modules_whole_table() {
    let (mut e, clip) = engine_with_clip();
    e.execute("clip.effects_set", &json!({"clip": clip, "params": {"eq.output_gain": 3.0, "gain": -2.0}})).unwrap();
    let v = clip_values(&e, clip);
    let eq = transit_dsp::plugins::eq::EQ7_INFO.params;
    assert_eq!(v.get("eq.output_gain"), Some(&3.0), "what was set stays");
    for p in eq.iter().filter(|p| p.id != "output_gain") {
        assert_eq!(v.get(&format!("eq.{}", p.id)), Some(&f64::from(p.default)), "eq.{} at its default", p.id);
    }
    assert!(v.keys().all(|k| k == "gain" || k.starts_with("eq.")), "the compressor stays off: {:?}", v.keys().collect::<Vec<_>>());
    assert_eq!(v.get("gain"), Some(&-2.0), "the gain is one value, stored as set");
    assert_eq!(v.len(), eq.len() + 1);
    // The compressor is brought by its own first value, the equalizer's values stay.
    e.execute("clip.effects_set", &json!({"clip": clip, "params": {"comp.threshold": -20.0}})).unwrap();
    let v = clip_values(&e, clip);
    let comp = transit_dsp::plugins::dynamics::COMP_INFO.params;
    assert_eq!(v.get("comp.threshold"), Some(&-20.0));
    assert_eq!(v.len(), eq.len() + comp.len() + 1);
    assert_eq!(v.get("eq.output_gain"), Some(&3.0));
    // A value set again moves only itself.
    e.execute("clip.effects_set", &json!({"clip": clip, "params": {"eq.output_gain": 5.0}})).unwrap();
    assert_eq!(clip_values(&e, clip).len(), eq.len() + comp.len() + 1);
    assert_eq!(clip_values(&e, clip).get("eq.output_gain"), Some(&5.0));
}

#[test]
fn a_gain_alone_or_a_bypass_brings_no_module() {
    let (mut e, clip) = engine_with_clip();
    e.execute("clip.effects_set", &json!({"clip": clip, "params": {"gain": -3.0}})).unwrap();
    assert_eq!(clip_values(&e, clip).len(), 1);
    e.execute("clip.effects_bypass", &json!({"clip": clip})).unwrap();
    assert_eq!(clip_values(&e, clip).len(), 1);
}

#[test]
fn pasting_clip_effects_stores_every_value_of_the_modules_it_brings() {
    let (mut e, clip) = engine_with_clip();
    // A clipboard from a session that stored only what was set.
    e.clipboard.clip_effects = vec![("comp.ratio".to_string(), 8.0)];
    e.execute("edit.paste_clip_effects", &json!({"clip": clip})).unwrap();
    let v = clip_values(&e, clip);
    let comp = transit_dsp::plugins::dynamics::COMP_INFO.params;
    assert_eq!(v.len(), comp.len());
    assert_eq!(v.get("comp.ratio"), Some(&8.0));
    assert_eq!(v.get("comp.threshold"), Some(&f64::from(comp.iter().find(|p| p.id == "threshold").unwrap().default)));
}
