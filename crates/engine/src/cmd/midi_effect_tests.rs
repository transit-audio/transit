//! The add command's preview gate for hosted MIDI effects (ADR 0064 HM2, decisions 2.6, 10.2 and
//! 12): a hosted id is accepted through the command (the entry point of the control channel and
//! headless scripts) only when the scan's probe found a readable note output, never on a rack, and
//! no listing offers it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::qualifies;
use crate::Engine;
use serde_json::json;
use std::cell::RefCell;
use transit_clap_host::{ClapDescriptor, ProbeOutcome};
use transit_vst3_host::Vst3Descriptor;

thread_local! {
    /// The hosted ids the test's scan says qualify (a unit test reads no plug-in cache).
    pub(super) static QUALIFIED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

const NOTES: &str = "vst3:0123456789ABCDEF0123456789ABCDEF";
const SILENT: &str = "clap:test.notes.silent";

fn engine() -> (Engine, u64) {
    QUALIFIED.with(|q| *q.borrow_mut() = vec![NOTES.to_string()]);
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"kind": "instrument"})).unwrap();
    let t = r["tracks"][0].as_u64().unwrap();
    e.execute("mix.instrument", &json!({"track": t, "plugin": "macro_oscillator"})).unwrap();
    (e, t)
}

#[test]
fn a_qualifying_hosted_id_is_accepted_through_the_command_on_an_instrument_track() {
    let (mut e, t) = engine();
    let r = e.execute("mix.midi_effect", &json!({"track": t, "plugin": NOTES})).unwrap();
    assert_eq!((r["index"].as_u64(), r["plugin"].as_str()), (Some(0), Some(NOTES)));
    let tr = e.session().tracks.iter().find(|x| x.id.0 == t).unwrap();
    assert_eq!(tr.midi_effects.iter().map(|f| f.plugin.as_str()).collect::<Vec<_>>(), [NOTES]);
    assert!(tr.midi_effects[0].params.is_empty() && tr.midi_effects[0].state.is_none());
    // No listing offers it: the list of MIDI effects is the built-in registry's.
    let listed = e.execute("engine.midi_effects", &json!({})).unwrap();
    let ids: Vec<&str> = listed.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(ids, transit_dsp::midi_fx::REGISTRY.iter().map(|i| i.id).collect::<Vec<_>>());
    assert!(!ids.contains(&NOTES));
}

#[test]
fn a_hosted_id_without_a_readable_note_output_is_refused() {
    let (mut e, t) = engine();
    let err = e.execute("mix.midi_effect", &json!({"track": t, "plugin": SILENT})).unwrap_err().to_string();
    assert!(err.contains("no note output transit reads"), "{err}");
    assert!(e.session().tracks.iter().all(|x| x.midi_effects.is_empty()));
}

#[test]
fn a_hosted_id_is_refused_on_a_rack() {
    let (mut e, t) = engine();
    let rack = e.execute("rack.new", &json!({"tracks": [t]})).unwrap()["rack"].as_u64().unwrap();
    let err = e.execute("mix.midi_effect", &json!({"track": rack, "plugin": NOTES})).unwrap_err().to_string();
    assert!(err.contains("Hosted MIDI effects go on an instrument track until racks run one chain for all members"), "{err}");
    assert!(e.session().tracks.iter().all(|x| x.midi_effects.is_empty()));
    // A built-in still goes on the rack (the refusal is the hosted id's).
    e.execute("mix.midi_effect", &json!({"track": rack, "plugin": "midi_transpose"})).unwrap();
}

#[test]
fn a_hosted_id_with_params_is_refused() {
    let (mut e, t) = engine();
    let err = e.execute("mix.midi_effect", &json!({"track": t, "plugin": NOTES, "params": {"gain": 0.5}})).unwrap_err().to_string();
    assert!(err.contains("a hosted MIDI effect takes no `params` here"), "{err}");
    assert!(e.session().tracks.iter().all(|x| x.midi_effects.is_empty()));
}

fn vst3(id: &str, probe: ProbeOutcome) -> Vst3Descriptor {
    Vst3Descriptor {
        id: id.into(),
        class_id: String::new(),
        name: "N".into(),
        vendor: "V".into(),
        version: String::new(),
        sdk_version: String::new(),
        sub_categories: Vec::new(),
        category: transit_dsp::Category::Instrument,
        is_instrument: true,
        path: Default::default(),
        probe,
    }
}

fn clap(id: &str, probe: ProbeOutcome) -> ClapDescriptor {
    ClapDescriptor {
        id: id.into(),
        plugin_id: String::new(),
        name: "N".into(),
        vendor: "V".into(),
        version: String::new(),
        description: String::new(),
        features: Vec::new(),
        category: transit_dsp::Category::Other,
        is_instrument: false,
        path: Default::default(),
        probe,
    }
}

#[test]
fn only_a_probe_that_found_a_note_output_qualifies() {
    let yes = ProbeOutcome { note_output: Some(true), ..ProbeOutcome::default() };
    let no = ProbeOutcome { note_output: Some(false), ..ProbeOutcome::default() };
    let failed = ProbeOutcome { probe_error: Some("crashed".into()), ..ProbeOutcome::default() };
    let v = [vst3("vst3:A", yes.clone()), vst3("vst3:B", no.clone()), vst3("vst3:C", failed.clone())];
    let c = [clap("clap:a", yes), clap("clap:b", no), clap("clap:c", failed)];
    let q = |id: &str| qualifies(id, Some(&v), Some(&c));
    assert_eq!([q("vst3:A"), q("vst3:B"), q("vst3:C"), q("vst3:D")], [true, false, false, false]);
    assert_eq!([q("clap:a"), q("clap:b"), q("clap:c"), q("clap:d")], [true, false, false, false]);
    assert!(!qualifies("vst3:A", None, None), "nothing scanned: nothing qualifies");
}
