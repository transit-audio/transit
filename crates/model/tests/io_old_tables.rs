//! ADR 0015, amendment for #431, rule 11 ("Tables saved by older builds") over `io_maps` entries in
//! the serialized form older builds wrote: v0.7.0 to v0.7.3 (`IoMap` with `IoPath.live`) and
//! v0.6.x (without `live`). The fixtures follow those versions' structs field for field (`git show
//! v0.7.2:crates/model/src/io_map.rs`, `v0.6.2` for the form without `live`), pretty-printed as
//! `ui.json` is. Each is loaded, used as rule 11 says, saved as a new build saves it and reloaded,
//! and its input half compared with what was read.
#![allow(clippy::unwrap_used)]

use serde_json::Value;
use transit_model::{IoDirection, IoMap, adopt_input};

fn fixture(name: &str) -> (String, IoMap) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/io_tables").join(name);
    let text = std::fs::read_to_string(path).unwrap();
    let map = serde_json::from_str(&text).unwrap();
    (text, map)
}

/// The input half as a new build serializes it: the count and the input paths in order.
fn half(map: &IoMap) -> String {
    let paths: Vec<_> = map.paths_of(IoDirection::Input).collect();
    serde_json::to_string(&(map.inputs, paths)).unwrap()
}

/// The input half as the file holds it, with a missing `live` read as its default.
fn raw_half(text: &str) -> Value {
    let v: Value = serde_json::from_str(text).unwrap();
    let mut paths: Vec<Value> = v["paths"].as_array().unwrap().iter().filter(|p| p["direction"] == "Input").cloned().collect();
    for p in &mut paths {
        p.as_object_mut().unwrap().entry("live").or_insert(Value::Bool(false));
    }
    serde_json::json!([v["inputs"], paths])
}

/// Saved as a new build saves it (the output record keeps the old input half as read), written,
/// reloaded: the input half is byte for byte what was read, and matches the file's.
fn saved_and_reloaded(text: &str, old: &IoMap, composed: &IoMap) -> IoMap {
    let record = composed.output_record(Some(old));
    let written = serde_json::to_vec_pretty(&record).unwrap();
    let back: IoMap = serde_json::from_slice(&written).unwrap();
    assert_eq!(half(&back), half(old), "the input half round-trips byte for byte");
    assert_eq!(serde_json::from_str::<Value>(&half(&back)).unwrap(), raw_half(text), "and is the file's");
    assert!(!String::from_utf8(written).unwrap().contains("input_device"), "no new field in an older build's record");
    back
}

fn names(map: &IoMap) -> Vec<&str> {
    map.paths_of(IoDirection::Input).map(|p| p.name.as_str()).collect()
}

/// #759 case 1 (v0.7.2): a JACK group's table that says 2 inputs and holds no input paths. Its key
/// is the input group's identity, so its input half is the starting record and the defaults are
/// made once (no `defaults_for`).
#[test]
fn a_v072_table_with_inputs_and_no_input_paths_gets_the_defaults_once() {
    let (text, old) = fixture("v072_jack_inputs_no_paths.json");
    let (map, record) = adopt_input(&old, &[], std::slice::from_ref(&old), "Headset", 2);
    assert_eq!(names(&map), ["In 1", "In 2", "In 1-2"]);
    assert_eq!((record.defaults_for, record.inputs), (2, 2));
    let back = saved_and_reloaded(&text, &old, &map);
    assert_eq!((back.inputs, names(&back).len()), (2, 0), "the old record still says what v0.7.2 wrote");
}

/// ASIO (v0.7.3): driver, output and input are one, so the table loads with the same paths and
/// Main, and its round trip (stored before inputs were recorded) still applies.
#[test]
fn a_v073_asio_table_loads_with_the_same_paths_main_and_measurement() {
    let (text, old) = fixture("v073_asio_defaults_and_user_path.json");
    let (map, _) = adopt_input(&old, &[], std::slice::from_ref(&old), "Scarlett 2i2 USB", 2);
    assert_eq!(names(&map), ["In 1", "In 2", "In 1-2", "Guitar"]);
    assert_eq!(map.paths, old.paths, "the same paths in the same order");
    assert_eq!(map.main, old.main);
    assert_eq!(map.hardware_latency("Out 1-2", "Guitar", 48_000), Some(612), "the return's identity is established");
    saved_and_reloaded(&text, &old, &map);
}

/// v0.6.x wrote no `live`: the table loads, its defaults are made once beside the user's path
/// (none duplicates a channel set the table already reads), and a narrower USB mode keeps the
/// definitions unavailable.
#[test]
fn a_v062_table_without_live_loads_and_keeps_its_input_half() {
    let (text, old) = fixture("v062_no_live.json");
    assert!(old.paths.iter().all(|p| !p.live));
    let (map, _) = adopt_input(&old, &[], std::slice::from_ref(&old), "Model Mixer ASIO Driver", 12);
    assert!(map.available_input("Vocal").is_some_and(|p| p.channels == [11]));
    assert!(map.input_path("In 2").is_some() && map.input_path("In 11-12").is_some());
    assert!(map.input_path("In 12").is_none(), "Vocal already reads channel 12");
    let (two, _) = adopt_input(&old, &[], std::slice::from_ref(&old), "Model Mixer ASIO Driver", 2);
    assert!(two.input_path("Vocal").is_some() && two.available_input("Vocal").is_none(), "unavailable, not deleted");
    saved_and_reloaded(&text, &old, &map);
}

/// A JACK group that was the output (v0.7.3) and is now only the input: its input half is the
/// input's starting record, under another output; its External Instrument figure stays in its own
/// record and applies only where that record's key is the open input.
#[test]
fn a_v073_table_of_a_group_that_is_now_only_the_input() {
    let (text, old) = fixture("v073_jack_group_now_input.json");
    let hdmi = IoMap::for_device("JACK", "HDMI", 0, 2);
    let (map, _) = adopt_input(&hdmi, &[], std::slice::from_ref(&old), "USB Audio", 2);
    assert_eq!(map.key(), ("JACK", "HDMI"));
    assert_eq!(names(&map), ["In 1", "Synth", "In 2"], "In 1-2 would read Synth's channels");
    assert!(map.external_latency.is_empty(), "the figure stays with its own record");
    let port = transit_model::MidiOutPort { backend: "alsa".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() };
    let (same, _) = adopt_input(&old, &[], std::slice::from_ref(&old), "USB Audio", 2);
    assert_eq!(same.external_measurement(&port, "Synth", 48_000, Some(1024)).map(|l| l.frames), Some(1061));
    let (other, _) = adopt_input(&old, &[], std::slice::from_ref(&old), "Mic", 1);
    assert!(other.external_measurement(&port, "Synth", 48_000, Some(1024)).is_none() && other.external_latency_elsewhere(&port, "Synth"));
    saved_and_reloaded(&text, &old, &same);
}

/// Native WASAPI (v0.7.1): the record is keyed by the render endpoint's name, which does not say
/// which capture endpoint its input paths belonged to: the input starts from the defaults, the old
/// half stays on disk unused, and the round trip through it says "measure again".
#[test]
fn a_v071_wasapi_record_is_not_used_for_the_paired_capture_endpoint() {
    let (text, old) = fixture("v071_wasapi_render_keyed.json");
    let capture = "{0.0.1.00000000}.{6a2b0c3e-0000-0000-0000-000000000000}";
    let (map, _) = adopt_input(&old, &[], std::slice::from_ref(&old), capture, 2);
    assert_eq!(names(&map), ["In 1", "In 2", "In 1-2"]);
    assert!(map.input_path("Mic").is_none());
    assert_eq!(map.hardware_latency("Out 1-2", "Mic", 48_000), None);
    assert!(map.hardware_latency_elsewhere("Out 1-2", "Mic", 48_000));
    saved_and_reloaded(&text, &old, &map);
}
