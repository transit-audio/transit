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

fn armed(e: &mut Engine, name: &str, format: &str, input: &str) {
    let id = e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap();
    e.execute("track.input", &json!({"tracks": [id], "input": input})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
}

const PORTS_CHANGED: &str = "ports changed: use the current ports, or wait for the original ones";

/// Review of slice C, must-fix 1, end to end: an older both-halves table `Interface` with `Mic` on
/// channel 0, no input record; the hardware ports are gone, the monitor ports remain, so the saved
/// choice (the preference off) resolves to the virtual device. Recording on `Mic` is refused.
#[test]
fn imported_hardware_paths_refuse_to_record_over_a_virtual_device_of_the_same_name() {
    let mut e = Engine::default();
    let mut old = IoMap::for_device("JACK", "Interface", 2, 2);
    let mic = transit_model::IoPath {
        live: false,
        name: "Mic".into(),
        direction: transit_model::IoDirection::Input,
        channels: vec![0],
        format: transit_model::ChannelFormat::Mono,
    };
    old.add_path(mic).unwrap();
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let (map, _) = transit_model::adopt_input_ports(&out, &[], &[old], "Interface", 2, &["monitor_FL".into(), "monitor_FR".into()]);
    e.set_io_map(map);
    armed(&mut e, "Vox", "mono", "Mic");
    let why = crate::io::input_refusal(&e, 2, None).expect("refused");
    assert_eq!(why, format!("track `Vox` cannot record: the input `Mic` is unavailable ({PORTS_CHANGED})"));
}

/// Must-fix 2, end to end: the virtual open sets the baseline, a hardware open clears it, `Mic` is
/// made and saved, the hardware ports go away again: recording on `Mic` is refused.
#[test]
fn a_path_made_on_hardware_refuses_to_record_over_the_virtual_device_later() {
    let mut e = Engine::default();
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let monitor: Vec<String> = vec!["monitor_FL".into(), "monitor_FR".into()];
    let (_, first) = transit_model::adopt_input_ports(&out, &[], &[], "Interface", 2, &monitor);
    let (hardware, _) = transit_model::adopt_input(&out, &[first], &[], "Interface", 2);
    e.set_io_map(hardware);
    e.execute("setup.io_map", &json!({"action": "add_path", "direction": "input", "name": "Mic", "channels": [0]})).unwrap();
    let saved = e.io_map().input_record(2).unwrap();
    assert!(saved.ports.is_empty());
    let (map, _) = transit_model::adopt_input_ports(&out, &[saved], &[], "Interface", 2, &monitor);
    e.set_io_map(map);
    armed(&mut e, "Vox", "mono", "Mic");
    let why = crate::io::input_refusal(&e, 2, None).expect("refused");
    assert_eq!(why, format!("track `Vox` cannot record: the input `Mic` is unavailable ({PORTS_CHANGED})"));
}

/// Must-fix 3, end to end: six ports, the third changed, a 5.1 track on `In 1-2` (the path, then
/// the label): recording is refused and the take plan reads silence on every channel.
#[test]
fn a_wide_track_over_a_changed_port_refuses_to_record_and_plans_silence() {
    let six: Vec<String> = ["A", "B", "C", "D", "E", "F"].iter().map(|p| (*p).to_string()).collect();
    let reopened: Vec<String> = ["A", "B", "X", "D", "E", "F"].iter().map(|p| (*p).to_string()).collect();
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let (_, record) = transit_model::adopt_input_ports(&out, &[], &[], "Firefox", 6, &six);
    let (map, _) = transit_model::adopt_input_ports(&out, &[record], &[], "Firefox", 6, &reopened);
    for label in [false, true] {
        let mut e = Engine::default();
        let mut map = map.clone();
        if label {
            map.remove_path_in("In 1-2", Some(transit_model::IoDirection::Input)).unwrap();
        }
        e.set_io_map(map);
        armed(&mut e, "Surround", "5.1", "In 1-2");
        let why = crate::io::input_refusal(&e, 6, None).expect("refused");
        assert_eq!(why, format!("track `Surround` cannot record: the input `In 1-2` is unavailable ({PORTS_CHANGED})"), "label {label}");
        let dir = transit_test_support::TestDir::new("takes-ports");
        let plan = crate::io::plan_takes(&e, &dir, 6);
        assert_eq!(plan[0].1, vec![usize::MAX; 6], "silence, never port X (label {label})");
    }
}
