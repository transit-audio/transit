//! ADR 0015, amendment for #431, rule 5 and rule 13 (slice C): the port-mapping guard of a JACK
//! virtual input. The ordered port names are stored with its input record; the first creation sets
//! the baseline, a reopen never replaces it, and a path (or label) over a port that changed or went
//! away is unavailable as a whole until the original ports return or the user uses the current
//! ones. Measurements through a virtual input apply only over the ports they were measured through.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::{HardwareLatency, InputUnavailable, resolve_input};

fn ports(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn available(m: &IoMap) -> Vec<&str> {
    m.paths_of(IoDirection::Input).filter(|p| m.input_unavailable(p).is_none()).map(|p| p.name.as_str()).collect()
}

fn why(m: &IoMap, name: &str) -> Option<InputUnavailable> {
    m.input_state(name).and_then(Result::err)
}

/// Opens the virtual device "Firefox" with `open` ports over the saved records and keeps the
/// record, as the app does at every open.
fn open(saved: &mut Vec<InputMap>, open: &[&str]) -> IoMap {
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let width = u16::try_from(open.len()).unwrap();
    let (map, record) = adopt_input_ports(&out, saved, &[], "Firefox", width, &ports(open));
    saved.retain(|r| r.key() != record.key());
    saved.push(record);
    map
}

#[test]
fn the_first_creation_of_a_virtual_input_table_sets_the_baseline() {
    let mut saved = Vec::new();
    let map = open(&mut saved, &["output_FL", "output_FR"]);
    assert_eq!(saved[0].ports, ["output_FL", "output_FR"], "stored with the input record");
    assert_eq!(available(&map), ["In 1", "In 2", "In 1-2"]);
    assert!(!map.ports_mismatch(), "nothing to offer");
    let json = serde_json::to_value(&saved[0]).unwrap();
    assert_eq!(json["ports"], serde_json::json!(["output_FL", "output_FR"]));
    // A hardware group's record carries none, and its table no guard.
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let (hw, record) = adopt_input(&out, &[], &[], "USB Mic", 1);
    assert!(record.ports.is_empty() && hw.input_ports.is_empty());
    assert!(serde_json::to_value(&record).unwrap().get("ports").is_none(), "not written for a hardware group");
}

/// The slice C gate: the same width, another order. Every path over a moved index is unavailable
/// as a whole (recording refuses with the path named, monitoring is silent), the definitions and
/// the baseline are kept, and a reopen alone never replaces the baseline. Recovery both ways.
#[test]
fn a_same_width_reorder_makes_the_paths_over_moved_ports_unavailable_until_recovered() {
    let mut saved = Vec::new();
    open(&mut saved, &["output_FL", "output_FR"]);
    let mut map = open(&mut saved, &["output_FR", "output_FL"]);
    assert!(available(&map).is_empty(), "{:?}", available(&map));
    assert_eq!(why(&map, "In 1-2"), Some(InputUnavailable::PortsChanged));
    assert_eq!(resolve_input(&map, "In 1", 1), Err(InputUnavailable::PortsChanged), "monitoring and recording read nothing");
    assert_eq!(InputUnavailable::PortsChanged.to_string(), "ports changed: use the current ports, or wait for the original ones");
    assert_eq!(saved[0].ports, ["output_FL", "output_FR"], "the reopen kept the baseline");
    assert_eq!(saved[0].paths.len(), 3, "every definition kept");
    assert!(map.ports_mismatch());
    // The original order returns: every path is available again.
    let back = open(&mut saved, &["output_FL", "output_FR"]);
    assert_eq!(available(&back), ["In 1", "In 2", "In 1-2"]);
    assert!(!back.ports_mismatch());
    // Or the user takes the current order (the I/O paths page's Use the current ports).
    map = open(&mut saved, &["output_FR", "output_FL"]);
    map.use_current_ports();
    assert_eq!(available(&map), ["In 1", "In 2", "In 1-2"]);
    let record = map.input_record(2).unwrap();
    assert_eq!(record.ports, ["output_FR", "output_FL"], "the new baseline is saved with the record");
}

/// A changed port name (a stream back with another serial) holds back only the paths over it; a
/// legacy label over it is unavailable too, never another stream.
#[test]
fn a_changed_port_holds_back_only_the_paths_and_labels_over_it() {
    let mut saved = Vec::new();
    let mut first = open(&mut saved, &["output_FL", "output_FR"]);
    first.remove_path_in("In 2", Some(IoDirection::Input)).unwrap();
    let record = first.input_record(2).unwrap();
    saved = vec![record];
    let map = open(&mut saved, &["output_FL", "output_FR-57"]);
    assert_eq!(available(&map), ["In 1"]);
    assert_eq!(why(&map, "In 1-2"), Some(InputUnavailable::PortsChanged));
    assert_eq!(resolve_input(&map, "In 1", 2), Ok(vec![Some(0), Some(0)]), "the port that did not change reads as before");
    assert_eq!(resolve_input(&map, "In 2", 1), Err(InputUnavailable::PortsChanged), "the label \"In 2\" (no path of that name now)");
}

/// Slice A's width rules still hold for a virtual device: a narrower open makes paths unavailable
/// and keeps them, widening re-enables them. A port past the baseline was never confirmed.
#[test]
fn a_width_change_keeps_slice_as_rules_and_a_new_port_is_not_confirmed() {
    let mut saved = Vec::new();
    open(&mut saved, &["output_FL", "output_FR"]);
    let narrow = open(&mut saved, &["output_FL"]);
    assert_eq!(available(&narrow), ["In 1"]);
    assert_eq!(why(&narrow, "In 2"), Some(InputUnavailable::OutOfRange { opened: 1 }));
    let wide = open(&mut saved, &["output_FL", "output_FR"]);
    assert_eq!(available(&wide), ["In 1", "In 2", "In 1-2"], "widening re-enables them");
    let wider = open(&mut saved, &["output_FL", "output_FR", "output_FL-61"]);
    assert_eq!(why(&wider, "In 3"), Some(InputUnavailable::PortsChanged), "a third port the baseline does not have");
    assert!(wider.ports_mismatch());
    assert_eq!(available(&wider), ["In 1", "In 2", "In 1-2"]);
}

/// A record made while the group was hardware has no baseline: opened as a virtual device later
/// (the same name, now non-physical ports), nothing of it is read until the user confirms the ports.
#[test]
fn a_record_without_a_baseline_opened_as_a_virtual_device_reads_nothing_unconfirmed() {
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let (_, hardware) = adopt_input(&out, &[], &[], "Firefox", 2);
    let (map, record) = adopt_input_ports(&out, &[hardware], &[], "Firefox", 2, &ports(&["output_FL", "output_FR"]));
    assert!(record.ports.is_empty(), "a reopen never sets the baseline");
    assert!(available(&map).is_empty());
    assert!(map.ports_mismatch());
}

fn hw(input_ports: &[&str]) -> HardwareLatency {
    HardwareLatency {
        send: "Out 1-2".into(),
        ret: "In 1".into(),
        round_trip_frames: 300,
        rate: 48_000,
        measured_at: 1,
        input_device: "Firefox".into(),
        input_ports: ports(input_ports),
    }
}

/// Rule 13: a measurement through a virtual input applies only over the ports it was measured
/// through; one without them (or with others) says "measure again".
#[test]
fn a_measurement_through_a_virtual_input_applies_only_over_its_ports() {
    let mut saved = Vec::new();
    let mut map = open(&mut saved, &["output_FL", "output_FR"]);
    map.set_hardware_latency(hw(&["output_FL", "output_FR"]));
    assert_eq!(map.hardware_latency("Out 1-2", "In 1", 48_000), Some(300));
    map.input_ports = ports(&["output_FR", "output_FL"]);
    assert_eq!(map.hardware_latency("Out 1-2", "In 1", 48_000), None, "after a reorder");
    assert!(map.hardware_latency_elsewhere("Out 1-2", "In 1", 48_000), "measure again");
    map.input_ports = ports(&["output_FL", "output_FR"]);
    map.hardware_latency = vec![hw(&[])];
    assert_eq!(map.hardware_latency("Out 1-2", "In 1", 48_000), None, "a figure without the mapping");
    // An External Instrument figure the same way.
    let port = crate::MidiOutPort { backend: "alsa".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() };
    let fig = |input_ports: &[&str]| crate::ExternalLatency {
        port_id: String::new(),
        port_name: "MODEL D".into(),
        ret: "In 1".into(),
        rate: 48_000,
        buffer: 256,
        frames: 955,
        spread_frames: 1,
        notes: 8,
        measured_at: 1,
        input_device: "Firefox".into(),
        input_ports: ports(input_ports),
    };
    map.set_external_latency(fig(&["output_FL", "output_FR"]));
    assert_eq!(map.external_measurement(&port, "In 1", 48_000, Some(256)).map(|l| l.frames), Some(955));
    map.input_ports = ports(&["output_FR", "output_FL"]);
    assert_eq!(map.external_measurement(&port, "In 1", 48_000, Some(256)), None, "after a reorder");
    assert!(map.external_latency_elsewhere(&port, "In 1"), "measure again");
    // Through a hardware group nothing changes: neither side has ports.
    let out = IoMap::for_device("JACK", "Scarlett", 0, 2);
    let (mut usb, _) = adopt_input(&out, &[], &[], "Firefox", 2);
    usb.set_hardware_latency(hw(&[]));
    assert_eq!(usb.hardware_latency("Out 1-2", "In 1", 48_000), Some(300));
}
