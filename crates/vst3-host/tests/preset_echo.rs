//! Issue #651: a preset the user picks in a hosted plug-in's own editor must not come back to the
//! plug-in changed. The plug-in reports the preset's values (`performEdit`, or
//! `restartComponent(kParamValuesChanged)` and a re-read); the host hands them to the session as
//! plain values, and the session sends them back through `set_param` (`mix.insert_param`, then the
//! engine). The processor must get exactly the normalized values the plug-in reported (VST3 has
//! the host forward a controller's edits to the processor), and nothing may go back to the
//! controller: a plug-in that receives any other value, or a `setParamNormalized` it did not ask
//! for, marks its preset modified.
//!
//! The fixture's Gain logs every call the host makes that writes to it, and its "user" picks a
//! preset at the controller's next `getState` once a controller state armed the pick (see
//! `tests/fixture`, "call log and presets"): a main-thread call, as a click in its editor is.

#![cfg(not(target_os = "macos"))] // Editor handles drive windows on the process main thread on macOS.

mod common;

use transit_dsp::{Plugin, PluginEditor};

const BLOCK: usize = 64;

/// The tests of this file run one at a time: the fixture keeps its call log and its armed pick
/// process-wide.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The fixture's presets, as `tests/fixture` defines them: (parameter, normalized).
const PRESETS: [[(u32, f64); 3]; 2] = [[(7, 0.3), (3, 0.45), (12, 0.3)], [(7, 0.7), (3, 0.95), (12, 0.7)]];

/// A Gain state that arms a pick of preset `index`, announced by `performEdit` (`announce` 0) or
/// `kParamValuesChanged` (1): gain 0.5, Freq 0.5, no "preset" gain, no timer.
fn arming_state(index: u32, announce: u32) -> Vec<u8> {
    arming(1 + 2 * index + announce)
}

/// A Gain state that arms fixture code `code` (see `tests/fixture`, "call log and presets").
fn arming(code: u32) -> Vec<u8> {
    let comp = 0.5f64.to_le_bytes();
    let ctrl: Vec<u8> = [0.5f64, -1.0, -1.0, f64::from(code)].iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut out = b"SCV3".to_vec();
    out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    out.extend_from_slice(&comp);
    out.extend_from_slice(&(ctrl.len() as u32).to_le_bytes());
    out.extend_from_slice(&ctrl);
    out
}

/// The call log a saved state carries after the component's gain (see `tests/fixture`).
fn log_of(blob: Option<Vec<u8>>) -> Vec<String> {
    let blob = blob.expect("state");
    assert_eq!(&blob[..4], b"SCV3");
    let len = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;
    let comp = &blob[8..8 + len];
    String::from_utf8(comp[8..].to_vec()).expect("utf-8 log").lines().map(str::to_string).collect()
}

/// UI frames as the app runs them: the editor handle's idle (which services the plug-in's
/// requests), its edits into the session and back to the plug-in through `set_param` (the engine
/// clamps to the range, as `set_param` does itself), then an audio block.
fn frames(p: &mut Box<dyn Plugin>, ed: &mut Box<dyn PluginEditor>, n: usize) {
    for _ in 0..n {
        for (id, v) in ed.idle() {
            assert!(p.set_param(&id, v), "{id}");
        }
        p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    }
}

/// What the host did to the plug-in after a preset pick, against what the plug-in reported:
/// every write it did not ask for, and every value the processor got that differs from the
/// plug-in's (bit for bit). Empty when the host sent only what the plug-in reported.
fn unasked(log: &[String], preset: &[(u32, f64)]) -> Vec<String> {
    let mut bad = Vec::new();
    for line in log {
        let Some(rest) = line.strip_prefix("proc.point(") else {
            // A controller or component write, or a reactivation: nothing the pick asked for.
            bad.push(line.clone());
            continue;
        };
        let (id, rest) = rest.split_once(")=").expect("point line");
        let (value, _) = rest.split_once('@').expect("point offset");
        let (id, value): (u32, f64) = (id.parse().expect("id"), value.parse().expect("value"));
        match preset.iter().find(|(pid, _)| *pid == id) {
            Some(&(_, want)) if want.to_bits() == value.to_bits() => {}
            _ => bad.push(line.clone()),
        }
    }
    bad
}

#[test]
fn a_preset_picked_in_the_plugins_editor_reaches_the_processor_exactly_and_nothing_goes_back() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    // Settle: the fixture's setup edit goes through the session too; every curve gets sampled.
    frames(&mut p, &mut ed, 4);
    for (index, announce) in [(0u32, 0u32), (1, 1), (0, 1), (1, 0)] {
        let preset = &PRESETS[index as usize];
        let how = if announce == 0 { "performEdit" } else { "kParamValuesChanged" };
        // The host's own load, before the scenario: it only arms the pick.
        assert!(p.load_state(&arming_state(index, announce)));
        frames(&mut p, &mut ed, 2);
        // The user picks the preset (inside this getState); the log starts.
        assert!(reader.save_state().is_some());
        frames(&mut p, &mut ed, 4);
        // An autosave or backup reads the state; its own writes are in the log it returns.
        let mut log = log_of(reader.save_state());
        frames(&mut p, &mut ed, 2);
        log.extend(log_of(reader.save_state()));
        assert_eq!(
            unasked(&log, preset),
            Vec::<String>::new(),
            "preset {index} announced by {how}: the host sent back what the plug-in did not report; log: {log:#?}"
        );
        for &(pid, want) in preset {
            assert!(
                log.iter().any(|l| l.starts_with(&format!("proc.point({pid})="))),
                "preset {index} ({how}): the processor never got parameter {pid} = {want}; log: {log:#?}"
            );
        }
        assert_eq!(p.param("7"), Some((2.0 * preset[0].1) as f32), "preset {index} ({how}): Gain in the session");
    }
    // A value of the host's own (automation, a mapped control) still reaches the controller,
    // including one equal to a plain value the plug-in reported before. Here the session drops
    // the report (as while a session restore is pending), so it never echoes it. (Not 1.0: that
    // is the Gain the arming state loads, the plug-in's own value as the host read it, which the
    // session sending back is nothing new; #651, round 2.)
    assert!(p.load_state(&arming_state(0, 0)));
    frames(&mut p, &mut ed, 2);
    assert!(reader.save_state().is_some());
    let dropped = ed.idle();
    assert!(dropped.iter().any(|(id, _)| id == "7"), "{dropped:?}");
    for v in [1.2f32, (2.0 * PRESETS[0][0].1) as f32] {
        assert!(p.set_param("7", v));
        frames(&mut p, &mut ed, 2);
        let log = log_of(reader.save_state());
        assert!(log.iter().any(|l| l.starts_with("ctrl.setParamNormalized(7)=")), "the host's {v} reaches the controller; log: {log:#?}");
        assert!(log.iter().any(|l| l.starts_with("proc.point(7)=")), "and the processor; log: {log:#?}");
    }
    drop(reader);
    drop(ed);
    drop(p);
}

/// The session may send the same values again: a parameter map that grows re-sends every value
/// it stores (a hosted plug-in's map starts empty and grows with each parameter the plug-in
/// reports), and a later publication of the same values does the same. A repeat of the echo the
/// host already holds is nothing new: the processor got the plug-in's value once and nothing
/// goes to the controller.
#[test]
fn the_session_sending_its_echo_again_sends_nothing_more() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 4);
    for (index, announce) in [(0u32, 0u32), (1, 1), (0, 1), (1, 0)] {
        let preset = &PRESETS[index as usize];
        let how = if announce == 0 { "performEdit" } else { "kParamValuesChanged" };
        assert!(p.load_state(&arming_state(index, announce)));
        frames(&mut p, &mut ed, 2);
        assert!(reader.save_state().is_some());
        let edits = ed.idle();
        assert_eq!(edits.len(), preset.len(), "preset {index} ({how}): {edits:?}");
        for _ in 0..3 {
            for (id, v) in &edits {
                assert!(p.set_param(id, *v), "{id}");
            }
            p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
        }
        frames(&mut p, &mut ed, 2);
        let mut log = log_of(reader.save_state());
        frames(&mut p, &mut ed, 2);
        log.extend(log_of(reader.save_state()));
        assert_eq!(unasked(&log, preset), Vec::<String>::new(), "preset {index} ({how}): log: {log:#?}");
        for &(pid, _) in preset {
            let n = log.iter().filter(|l| l.starts_with(&format!("proc.point({pid})="))).count();
            assert_eq!(n, 1, "preset {index} ({how}): parameter {pid} reaches the processor once; log: {log:#?}");
        }
    }
    drop(reader);
    drop(ed);
    drop(p);
}

/// A plug-in may ask for a reactivation (`kLatencyChanged`) or a re-read of its parameter infos
/// (`kParamTitlesChanged`) along with a preset it loads. After either the host re-sends the values
/// it set, at the next block, before the session's echo of the preset can arrive; a value the
/// plug-in has replaced since must not be among them (the session's copy is stale until the echo
/// arrives), or the plug-in sees the host take its preset back.
#[test]
fn a_restart_asked_with_a_preset_re_sends_no_value_the_plugin_replaced() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 4);
    let preset = &PRESETS[0];
    for (flag, what) in [(16u32, "kLatencyChanged"), (32, "kParamTitlesChanged")] {
        for announce in [0u32, 1] {
            let how = if announce == 0 { "performEdit" } else { "kParamValuesChanged" };
            assert!(p.load_state(&arming(1 + announce + flag)));
            frames(&mut p, &mut ed, 2);
            // Values of the host's own, unlike the preset's, delivered before the pick.
            for (pid, v) in [("7", 1.0f32), ("3", 0.0), ("12", 1000.0)] {
                assert!(p.set_param(pid, v));
            }
            frames(&mut p, &mut ed, 3);
            // The pick and its restart request; the next idle services the restart and hands the
            // session the preset; a block runs before the session's echo comes back.
            assert!(reader.save_state().is_some());
            let edits = ed.idle();
            p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
            for (pid, v) in &edits {
                assert!(p.set_param(pid, *v), "{pid}");
            }
            frames(&mut p, &mut ed, 3);
            let mut log = log_of(reader.save_state());
            frames(&mut p, &mut ed, 2);
            log.extend(log_of(reader.save_state()));
            // The reactivation was asked for.
            let log: Vec<String> = log.into_iter().filter(|l| !l.starts_with("comp.setActive(")).collect();
            assert_eq!(unasked(&log, preset), Vec::<String>::new(), "{what} with a pick announced by {how}: log: {log:#?}");
            for &(pid, want) in preset {
                assert!(
                    log.iter().any(|l| l.starts_with(&format!("proc.point({pid})="))),
                    "{what} ({how}): the processor never got parameter {pid} = {want}; log: {log:#?}"
                );
            }
        }
    }
    drop(reader);
    drop(ed);
    drop(p);
}

/// A new instance restores a state the host made (a session reopened, a plug-in rebuilt), and the
/// session then sends the values it stores, which are the plug-in's own (they came from its
/// reports through the same conversion). Nothing goes back to the plug-in; a different value of the
/// host's own still does.
#[test]
fn a_restored_instance_gets_nothing_back_for_its_own_values() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    // The fixture's call log starts with a pick (on a first instance).
    let mut first = transit_vst3_host::create(&id).expect("instantiate");
    first.prepare(48_000.0, BLOCK, 2);
    assert!(first.load_state(&arming_state(0, 0)));
    assert!(first.state_reader().expect("state reader").save_state().is_some());
    drop(first);
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut reader = p.state_reader().expect("state reader");
    assert!(p.load_state(&arming(0)));
    // The host's own load is in the log; read it away.
    let _ = log_of(reader.save_state());
    let stored: Vec<(&str, f32)> = p.info().params.iter().map(|pi| (pi.id, p.param(pi.id).expect("value"))).collect();
    assert!(!stored.is_empty());
    for _ in 0..2 {
        for &(pid, v) in &stored {
            assert!(p.set_param(pid, v), "{pid}");
        }
        p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    }
    transit_vst3_host::service_requests();
    let log = log_of(reader.save_state());
    assert_eq!(log, Vec::<String>::new(), "the session's copy of the plug-in's own values went back to it");
    // A value of the host's own still reaches the processor and the controller.
    assert!(p.set_param("7", 1.5));
    p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    transit_vst3_host::service_requests();
    let log = log_of(reader.save_state());
    assert!(log.iter().any(|l| l == "proc.point(7)=0.75@0"), "{log:#?}");
    assert!(log.iter().any(|l| l == "ctrl.setParamNormalized(7)=0.75"), "{log:#?}");
    drop(reader);
    drop(p);
}
