//! The input half of the I/O table (ADR 0015, amendment for #431): per-side records, rule 9's
//! defaults that follow the width, rule 11's older tables, rule 10's unavailable inputs and rule
//! 13's measurement identity.
#![allow(clippy::unwrap_used)]

use super::*;
use crate::{ChannelFormat, ExternalLatency, HardwareLatency, InputUnavailable, MidiOutPort, input_channel, live_input_channel};

fn inputs(m: &IoMap) -> Vec<&str> {
    m.paths_of(IoDirection::Input).map(|p| p.name.as_str()).collect()
}

fn available(m: &IoMap) -> Vec<&str> {
    m.paths_of(IoDirection::Input).filter(|p| m.input_unavailable(p).is_none()).map(|p| p.name.as_str()).collect()
}

fn path(name: &str, direction: IoDirection, channels: Vec<u16>) -> IoPath {
    let format = ChannelFormat::for_channels(channels.len());
    IoPath { live: false, name: name.into(), direction, channels, format }
}

/// An output record as a new build makes it: outputs only.
fn output(system: &str, device: &str) -> IoMap {
    IoMap::for_device(system, device, 0, 2)
}

/// Opens `identity` at `width` over the saved records and keeps the record, as the app does.
fn open(out: &IoMap, saved: &mut Vec<InputMap>, identity: &str, width: u16) -> IoMap {
    let (map, record) = adopt_input(out, saved, &[], identity, width);
    saved.retain(|r| r.key() != record.key());
    saved.push(record);
    map
}

/// Saves the engine's edited table back into the input records.
fn keep(map: &IoMap, saved: &mut Vec<InputMap>) {
    let defaults_for = saved.iter().find(|r| r.device == map.input_device).map_or(0, |r| r.defaults_for);
    let record = map.input_record(defaults_for).unwrap();
    saved.retain(|r| r.key() != record.key());
    saved.push(record);
}

/// #759 case 2: 2 channels, then 1, then 2 ends with "In 1", "In 2" and "In 1-2" available and the
/// user's path kept; the narrower open deletes nothing.
#[test]
fn a_narrower_input_makes_paths_unavailable_and_widening_restores_them() {
    let out = output("JACK", "Headset");
    let mut saved = Vec::new();
    let mut map = open(&out, &mut saved, "Mic Pair", 2);
    assert_eq!(inputs(&map), ["In 1", "In 2", "In 1-2"]);
    map.add_path(path("Vox", IoDirection::Input, vec![1])).unwrap();
    keep(&map, &mut saved);
    let narrow = open(&out, &mut saved, "Mic Pair", 1);
    assert_eq!(inputs(&narrow), ["In 1", "In 2", "In 1-2", "Vox"], "definitions are kept");
    assert_eq!(available(&narrow), ["In 1"]);
    assert_eq!(narrow.input_state("In 1-2"), Some(Err(InputUnavailable::OutOfRange { opened: 1 })));
    assert_eq!(saved[0].defaults_for, 1);
    let wide = open(&out, &mut saved, "Mic Pair", 2);
    assert_eq!(inputs(&wide), ["In 1", "In 2", "In 1-2", "Vox"], "nothing duplicated");
    assert_eq!(available(&wide), ["In 1", "In 2", "In 1-2", "Vox"]);
}

/// A custom multichannel path keeps its definition through a shrink and is whole again after.
#[test]
fn shrinking_and_restoring_keeps_a_custom_multichannel_path() {
    let out = output("ASIO", "Interface");
    let mut saved = Vec::new();
    let mut map = open(&out, &mut saved, "Interface", 6);
    map.add_path(path("Drums", IoDirection::Input, vec![5, 2, 3, 4])).unwrap();
    keep(&map, &mut saved);
    let two = open(&out, &mut saved, "Interface", 2);
    assert_eq!(two.input_path("Drums").map(|p| p.channels.clone()), Some(vec![5, 2, 3, 4]));
    assert_eq!(two.input_state("Drums"), Some(Err(InputUnavailable::OutOfRange { opened: 2 })));
    assert_eq!(input_channel(Some(&two), "Drums", 0, 4), None, "never another channel");
    let six = open(&out, &mut saved, "Interface", 6);
    assert_eq!(six.available_input("Drums").map(|p| p.channels.clone()), Some(vec![5, 2, 3, 4]));
}

/// A default the user removed stays removed while the width does not grow past `defaults_for`.
#[test]
fn a_removed_default_stays_removed_while_the_width_holds() {
    let out = output("ASIO", "Interface");
    let mut saved = Vec::new();
    let mut map = open(&out, &mut saved, "Interface", 2);
    map.remove_path("In 2").unwrap();
    keep(&map, &mut saved);
    assert_eq!(inputs(&open(&out, &mut saved, "Interface", 2)), ["In 1", "In 1-2"]);
    assert_eq!(inputs(&open(&out, &mut saved, "Interface", 2)), ["In 1", "In 1-2"]);
}

/// A failed input open calls nothing, so the records stay as they were (the app's half is in
/// `io_settings`); an input record is never made for an input that did not open.
#[test]
fn a_missing_record_and_an_explicitly_empty_one_differ() {
    let out = output("ASIO", "Interface");
    let mut old = IoMap::for_device("ASIO", "Interface", 2, 2);
    old.add_path(path("Mic", IoDirection::Input, vec![1])).unwrap();
    // Missing: the old table's input half (its key is the input's identity).
    let (map, record) = adopt_input(&out, &[], std::slice::from_ref(&old), "Interface", 2);
    assert_eq!(inputs(&map), ["In 1", "In 2", "In 1-2", "Mic"]);
    assert_eq!((record.defaults_for, record.inputs), (2, 2));
    // Explicitly empty: the user's empty table, never refilled, not even from the old half.
    let empty = InputMap { system: "ASIO".into(), device: "Interface".into(), inputs: 2, defaults_for: 2, paths: Vec::new() };
    let (map, record) = adopt_input(&out, std::slice::from_ref(&empty), std::slice::from_ref(&old), "Interface", 4);
    assert!(inputs(&map).is_empty() && map.lacks_input_paths());
    assert_eq!(record.defaults_for, 4);
}

/// #759 case 1: an older build saved 2 inputs and no input paths: the defaults are made once.
#[test]
fn an_old_table_with_inputs_and_no_input_paths_gets_its_defaults_once() {
    let old = IoMap { paths: IoMap::for_device("JACK", "Headset", 0, 2).paths, inputs: 2, ..IoMap::for_device("JACK", "Headset", 0, 2) };
    let (map, record) = adopt_input(&output("JACK", "Headset"), &[], std::slice::from_ref(&old), "Headset", 2);
    assert_eq!(inputs(&map), ["In 1", "In 2", "In 1-2"]);
    let mut edited = map.clone();
    edited.remove_path("In 1-2").unwrap();
    let kept = edited.input_record(record.defaults_for).unwrap();
    let (again, _) = adopt_input(&output("JACK", "Headset"), &[kept], std::slice::from_ref(&old), "Headset", 2);
    assert_eq!(inputs(&again), ["In 1", "In 2"], "once: the record now has defaults_for");
}

/// Rule 11, "No inference": a native WASAPI record is keyed by the render endpoint's name, and the
/// input is the paired capture endpoint: the old input half is not used for it.
#[test]
fn an_old_record_keyed_by_another_device_is_not_used_for_the_input() {
    let mut old = IoMap::for_device("WASAPI", "Speakers (Interface)", 2, 2);
    old.add_path(path("Mic", IoDirection::Input, vec![1])).unwrap();
    let (map, _) = adopt_input(&output("WASAPI", "Speakers (Interface)"), &[], std::slice::from_ref(&old), "{0.0.1.00000000}.{capture-a}", 2);
    assert_eq!(inputs(&map), ["In 1", "In 2", "In 1-2"], "the defaults, not the render record's input half");
}

/// Rule 8, "Clashes", and rule 10: an input path with an output path's name is unavailable on every
/// backend: it monitors silence and never falls back to a channel number.
#[test]
fn an_input_path_that_clashes_with_an_output_path_is_unavailable() {
    let mut out = output("ASIO", "Interface");
    out.add_path(path("Cue", IoDirection::Output, vec![1])).unwrap();
    let record = InputMap {
        system: "ASIO".into(),
        device: "Mic Pre".into(),
        inputs: 2,
        defaults_for: 2,
        paths: vec![path("cue", IoDirection::Input, vec![1]), path("In 1", IoDirection::Input, vec![0])],
    };
    let (map, _) = adopt_input(&out, std::slice::from_ref(&record), &[], "Mic Pre", 2);
    assert_eq!(map.input_state("Cue"), Some(Err(InputUnavailable::Clash)));
    assert_eq!(map.path("Cue").map(|p| p.direction), Some(IoDirection::Output), "the output keeps its name");
    assert_eq!(input_channel(Some(&map), "Cue", 0, 1), None);
    assert_eq!(live_input_channel(Some(&map), "Cue", 0, 1, 2), None, "silent, never channel 1");
    assert!(!map.lacks_input_paths(), "In 1 is available");
    // Renaming the input side ends the clash.
    let mut fixed = map.clone();
    fixed.rename_path_in("Cue", Some(IoDirection::Input), "Talkback").unwrap();
    assert_eq!(fixed.available_input("Talkback").map(|p| p.channels.clone()), Some(vec![1]));
    assert_eq!(fixed.path("Cue").map(|p| p.direction), Some(IoDirection::Output));
}

/// Rule 14: Add default paths fills what is missing, never duplicates a renamed default, never
/// changes a definition, and a second click adds nothing.
#[test]
fn add_default_paths_is_idempotent_and_skips_renamed_defaults() {
    let mut map = IoMap::for_device("ASIO", "Interface", 4, 2);
    map.rename_path("In 1", "Vox").unwrap();
    map.remove_path("In 3-4").unwrap();
    map.remove_path("In 4").unwrap();
    map.set_channels("In 2", vec![3]).unwrap();
    let before: Vec<IoPath> = map.paths.clone();
    assert_eq!(map.add_default_inputs(map.inputs), 1, "In 2 by its name is present and reads channel 4, so In 4 is too; In 3-4 is back");
    assert!(map.input_path("In 3-4").is_some() && map.input_path("In 4").is_none());
    assert!(before.iter().all(|p| map.paths.contains(p)), "no definition changed");
    assert_eq!(map.input_path("In 2").map(|p| p.channels.clone()), Some(vec![3]));
    assert!(map.input_path("In 1").is_none(), "Vox reads channel 1 already");
    let once = map.paths.clone();
    assert_eq!(map.add_default_inputs(map.inputs), 0);
    assert_eq!(map.paths, once);
}

fn hw(input_device: &str, frames: u32) -> HardwareLatency {
    HardwareLatency { send: "Out 3-4".into(), ret: "In 1".into(), round_trip_frames: frames, rate: 48_000, measured_at: 1, input_device: input_device.into() }
}

/// #689 and rule 13: a figure measured through input A is not applied through B; it stays for A.
/// An older figure (no input recorded) applies only where the record's key is the input's identity.
#[test]
fn a_measurement_applies_only_through_the_input_it_returned_through() {
    let mut out = IoMap::for_device("WASAPI", "Speakers", 0, 4);
    out.set_hardware_latency(hw("capture-a", 300));
    let through = |id: &str| adopt_input(&out, &[], &[], id, 2).0;
    assert_eq!(through("capture-a").hardware_latency("Out 3-4", "In 1", 48_000), Some(300));
    let b = through("capture-b");
    assert_eq!(b.hardware_latency("Out 3-4", "In 1", 48_000), None);
    assert!(b.hardware_latency_elsewhere("Out 3-4", "In 1", 48_000), "measure again");
    let mut both = b.clone();
    both.set_hardware_latency(hw("capture-b", 410));
    assert_eq!(both.hardware_latency.len(), 2, "A's figure stays for when A is open");
    assert_eq!(both.hardware_latency("Out 3-4", "In 1", 48_000), Some(410));
    // An older figure: established on ASIO (output, input and driver are one), not on WASAPI.
    let mut asio = IoMap::for_device("ASIO", "Interface", 0, 4);
    asio.hardware_latency.push(hw("", 512));
    assert_eq!(adopt_input(&asio, &[], &[], "Interface", 2).0.hardware_latency("Out 3-4", "In 1", 48_000), Some(512));
    let mut wasapi = out.clone();
    wasapi.hardware_latency = vec![hw("", 512)];
    let other = adopt_input(&wasapi, &[], &[], "capture-a", 2).0;
    assert_eq!(other.hardware_latency("Out 3-4", "In 1", 48_000), None, "equal names elsewhere are not enough");
    assert!(other.hardware_latency_elsewhere("Out 3-4", "In 1", 48_000));
    assert_eq!(out.compose(None).hardware_latency("Out 3-4", "In 1", 48_000), None, "no input open: nothing returns");
}

/// The same for External Instrument figures (ADR 0020 decision 4's lookup).
#[test]
fn an_external_instrument_figure_applies_only_through_its_input() {
    let port = MidiOutPort { backend: "winmm".into(), name: "MODEL D".into(), ordinal: 0, id: String::new() };
    let fig = ExternalLatency {
        port_id: String::new(),
        port_name: "MODEL D".into(),
        ret: "In 1".into(),
        rate: 48_000,
        buffer: 64,
        frames: 955,
        spread_frames: 1,
        notes: 8,
        measured_at: 1,
        input_device: "group-a".into(),
    };
    let mut out = IoMap::for_device("JACK", "Headset", 0, 2);
    out.set_external_latency(fig);
    let a = adopt_input(&out, &[], &[], "group-a", 1).0;
    assert_eq!(a.external_measurement(&port, "In 1", 48_000, Some(64)).map(|l| l.frames), Some(955));
    let b = adopt_input(&out, &[], &[], "group-b", 1).0;
    assert_eq!(b.external_measurement(&port, "In 1", 48_000, Some(64)), None);
    assert!(b.external_latency_elsewhere(&port, "In 1"));
}

/// Saving splits the composed table: the output record keeps an older build's input half exactly
/// as it was read (or none), the input record holds the input paths, and composing them again
/// gives the same table.
#[test]
fn saving_splits_the_table_and_keeps_an_old_input_half_as_read() {
    let mut old = IoMap::for_device("JACK", "Headset", 2, 2);
    old.add_path(path("Old Mic", IoDirection::Input, vec![1])).unwrap();
    let (mut map, record) = adopt_input(&output("JACK", "Headset"), &[], &[], "USB Mic", 1);
    map.add_path(path("Cue", IoDirection::Output, vec![1])).unwrap();
    let kept = map.output_record(Some(&old));
    assert_eq!(kept.inputs, 2);
    assert_eq!(
        kept.paths_of(IoDirection::Input).cloned().collect::<Vec<_>>(),
        old.paths_of(IoDirection::Input).cloned().collect::<Vec<_>>()
    );
    assert!(kept.path("Cue").is_some() && kept.input_device.is_empty());
    let fresh = map.output_record(None);
    assert_eq!((fresh.inputs, fresh.paths_of(IoDirection::Input).count()), (0, 0), "a new record has no input half");
    let input = map.input_record(record.defaults_for).unwrap();
    assert_eq!(input.key(), ("JACK", "USB Mic"));
    assert_eq!(fresh.compose(Some(&input)), map);
    assert!(serde_json::to_string(&fresh).is_ok_and(|s| !s.contains("input_device")), "no new field in an output record");
}

/// Removing an input path takes the figures that came back through it on this input only: another
/// input's path of the same name keeps its measurement.
#[test]
fn removing_an_input_path_keeps_another_inputs_figures() {
    let mut out = IoMap::for_device("JACK", "HDMI", 0, 4);
    out.set_hardware_latency(hw("group-a", 300));
    out.set_hardware_latency(hw("group-b", 400));
    let (mut b, _) = adopt_input(&out, &[], &[], "group-b", 2);
    b.remove_path("In 1").unwrap();
    assert_eq!(b.hardware_latency.len(), 1, "B's figure went with its path");
    assert_eq!(b.hardware_latency[0].input_device, "group-a", "A's stays");
}
