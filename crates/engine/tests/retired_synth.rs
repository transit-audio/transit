//! A session saved by a build that had the Subtractive Synth (retired in v0.6.0, #234) opens with a
//! Macro Oscillator in its place, plays, reports it and reads as modified. ADR 0033 has the rule.

use serde_json::json;
use std::path::PathBuf;
use transit_engine::Engine;
use transit_time::Range;

const STATUS: &str = "used the retired Subtractive Synth and now carry the Macro Oscillator";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus").join(name)).unwrap()
}

fn instruments(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().filter_map(|t| t.instrument.as_ref().map(|i| i.plugin.clone())).collect()
}

#[test]
fn a_new_instrument_track_carries_the_macro_oscillator_with_defaults() {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = transit_model::TrackId(r["tracks"][0].as_u64().unwrap());
    let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.plugin, "macro_oscillator");
    assert_eq!(transit_dsp::plugin_info(&ins.plugin).unwrap().param("voices").unwrap().default, 7.0, "8 voices");
    assert!(transit_dsp::plugin_info("subtractive_synth").is_none(), "the old id is not registered");
}

#[test]
fn a_session_with_the_old_synth_opens_with_the_substitution_plays_and_reports_it() {
    let text = fixture("v0.4.1-demo.transit");
    let old = text.matches("\"plugin\": \"subtractive_synth\"").count();
    assert!(old > 0, "the fixture names the old synth in an instrument slot");
    let dir = std::env::temp_dir().join(format!("transit-retired-synth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Demo.transit");
    std::fs::write(&path, &text).unwrap();

    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let ids = instruments(&e);
    assert!(!ids.is_empty() && ids.iter().all(|i| i != "subtractive_synth"), "{ids:?}");
    assert_eq!(ids.iter().filter(|i| *i == "macro_oscillator").count(), old);
    let status = format!("{old} track(s) {STATUS}");
    assert!(e.messages.contains(&status), "{:?}", e.messages);
    assert!(e.is_dirty(), "the substitution shows as a modified session");

    // It plays: the demo's keys and lead make sound through the new instrument.
    let end = e.session().content_end().clamp(1, 48_000 * 20);
    let out = transit_mix::render_range(e.session(), Range::new(0, end), 512);
    assert!(out.iter().flatten().any(|x| x.abs() > 1e-4), "the substituted instrument is silent");
    assert!(out.iter().flatten().all(|x| x.is_finite()));

    // Saved, the session no longer names the old id and opens clean and unmodified.
    e.execute("session.save", &json!({})).unwrap();
    assert!(!e.is_dirty());
    assert!(!std::fs::read_to_string(&path).unwrap().contains("subtractive_synth"));
    let mut again = Engine::default();
    again.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(!again.messages.iter().any(|m| m.contains(STATUS)), "{:?}", again.messages);
    assert!(!again.is_dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_from_bytes_substitutes_and_reads_as_modified_too() {
    let text = fixture("v0.4.1-features.transit");
    let mut e = Engine::default();
    transit_engine::io::open_session_bytes(&mut e, "Features.transit", text.as_bytes()).unwrap();
    assert!(!instruments(&e).iter().any(|i| i == "subtractive_synth"));
    assert!(e.messages.iter().any(|m| m.ends_with(STATUS)), "{:?}", e.messages);
    assert!(e.is_dirty());
}
