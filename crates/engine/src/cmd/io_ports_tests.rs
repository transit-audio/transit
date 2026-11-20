//! ADR 0015, amendment for #431, rule 5 (slice C) in the engine: a path over a JACK virtual
//! input's changed ports refuses to record with the path named, `setup.io_map use_current_ports`
//! makes the ports open now the baseline only while the transport is stopped and only when they
//! differ, and `set` cannot move the guard.
#![allow(clippy::unwrap_used)]

use crate::Engine;
use serde_json::json;
use transit_model::{InputMap, IoMap};

/// The engine's table for the virtual input "Firefox" whose ports came back in another order.
fn reordered(e: &mut Engine) {
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let baseline = vec!["output_FL".to_string(), "output_FR".to_string()];
    let paths = transit_model::default_input_paths(2);
    let saved = InputMap { system: "JACK".into(), device: "Firefox".into(), inputs: 2, defaults_for: 2, paths, ports: baseline };
    let (map, _) = transit_model::adopt_input_ports(&out, &[saved], &[], "Firefox", 2, &["output_FR".into(), "output_FL".into()]);
    e.set_io_map(map);
}

#[test]
fn a_path_over_changed_ports_refuses_to_record_with_the_path_named() {
    let mut e = Engine::default();
    reordered(&mut e);
    let id = e.execute("track.new", &json!({"name": "Stream", "format": "stereo"})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("track.input", &json!({"tracks": [id], "input": "In 1-2"})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
    let why = crate::io::input_refusal(&e, 2, None).expect("refused");
    assert_eq!(
        why,
        "track `Stream` cannot record: the input `In 1-2` is unavailable (ports changed: use the current ports, or wait for the original ones)"
    );
    let state = e.execute("setup.io_map", &json!({})).unwrap();
    assert_eq!(state["input_ports"]["changed"], true);
    assert_eq!(state["unavailable_inputs"][0]["why"], "ports changed: use the current ports, or wait for the original ones");
}

#[test]
fn use_the_current_ports_works_only_while_stopped_and_only_on_a_mismatch() {
    let mut e = Engine::default();
    reordered(&mut e);
    e.transport.playing = true;
    let refused = e.execute("setup.io_map", &json!({"action": "use_current_ports"})).unwrap_err();
    assert!(refused.to_string().contains(crate::USE_PORTS_STOPPED), "{refused}");
    assert!(e.io_map().ports_mismatch(), "nothing changed");
    e.transport.playing = false;
    e.transport.recording = true;
    assert!(e.execute("setup.io_map", &json!({"action": "use_current_ports"})).is_err());
    e.transport.recording = false;
    let state = e.execute("setup.io_map", &json!({"action": "use_current_ports"})).unwrap();
    assert_eq!(state["input_ports"]["changed"], false);
    assert_eq!(e.io_map().input_baseline, ["output_FR", "output_FL"]);
    assert!(e.io_map().available_input("In 1-2").is_some());
    assert!(e.execute("setup.io_map", &json!({"action": "use_current_ports"})).is_err(), "no mismatch: nothing to use");
}

#[test]
fn set_keeps_the_guard_of_the_open_input() {
    let mut e = Engine::default();
    reordered(&mut e);
    let mut map = e.io_map().clone();
    map.input_baseline = map.input_ports.clone();
    e.execute("setup.io_map", &json!({"action": "set", "map": map})).unwrap();
    assert!(e.io_map().ports_mismatch(), "a script cannot confirm ports: the guard is the device's");
    assert_eq!(e.io_map().input_baseline, ["output_FL", "output_FR"]);
}
