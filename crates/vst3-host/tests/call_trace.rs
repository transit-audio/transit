//! Issue #651: the host call trace. While debug logging is on, every call between the host and a
//! plug-in that can bear on the plug-in's "modified" state is written to the log (target
//! `transit_vst3_host::trace`) with the instance, a sequence number, the thread, the parameter and
//! the full value with its bits; while it is off nothing is recorded. This binary installs a
//! logger that keeps those lines, runs the fixture Gain through each call kind, and checks the
//! lines come in order. One test only: the trace and the logger are process-wide.

#![cfg(not(target_os = "macos"))] // Editor handles drive windows on the process main thread on macOS.

mod common;

use std::sync::Mutex;
use transit_dsp::{Plugin, PluginEditor};

const BLOCK: usize = 64;
const TARGET: &str = "transit_vst3_host::trace";

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Keep;

impl log::Log for Keep {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.target() == TARGET
    }
    fn log(&self, r: &log::Record<'_>) {
        if r.target() == TARGET {
            LINES.lock().unwrap().push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}

fn take() -> Vec<String> {
    std::mem::take(&mut *LINES.lock().unwrap())
}

/// A Gain state that arms a pick of preset `index` at the controller's next `getState`, announced
/// by `performEdit` (`announce` 0) or `kParamValuesChanged` (1) (see `tests/fixture`).
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

/// UI frames as the app runs them: the editor handle's idle, its edits into the session and back
/// through `set_param`, the request service, then an audio block.
fn frames(p: &mut Box<dyn Plugin>, ed: &mut Box<dyn PluginEditor>, n: usize) {
    for _ in 0..n {
        for (id, v) in ed.idle() {
            assert!(p.set_param(&id, v), "{id}");
        }
        transit_vst3_host::service_requests();
        p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    }
}

/// Each pattern matches a later line than the one before (every pattern is a list of parts the
/// line must all contain).
fn in_order(lines: &[String], patterns: &[&[&str]]) {
    let mut at = 0;
    for p in patterns {
        match lines.iter().skip(at).position(|l| p.iter().all(|part| l.contains(part))) {
            Some(k) => at += k + 1,
            None => panic!("no line with {p:?} after line {at}; lines: {lines:#?}"),
        }
    }
}

/// The sequence numbers of the lines, which must rise.
fn rising(lines: &[String]) {
    let seqs: Vec<u64> = lines.iter().filter_map(|l| l.strip_prefix("vst3 #")?.split(' ').next()?.parse().ok()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "sequence numbers rise: {seqs:?}");
}

#[test]
fn the_trace_records_each_call_kind_in_order_and_nothing_while_debug_logging_is_off() {
    log::set_logger(&Keep).expect("the only logger of this binary");
    log::set_max_level(log::LevelFilter::Info);
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    let id = found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture gain").id;
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 4);
    // Off: a pick, a host value and a save write nothing.
    assert!(p.load_state(&arming_state(0, 0)));
    assert!(reader.save_state().is_some());
    assert!(p.set_param("7", 1.5));
    frames(&mut p, &mut ed, 3);
    assert!(reader.save_state().is_some());
    assert_eq!(take(), Vec::<String>::new(), "nothing is traced while debug logging is off");

    // On.
    log::set_max_level(log::LevelFilter::Debug);
    // A value of the host's own: the processor gets it in the next block, the controller at the
    // next service.
    assert!(p.set_param("7", 1.0));
    frames(&mut p, &mut ed, 2);
    // A pick announced by edits, then one announced by a restart and a re-read; each comes back
    // from the session as the echo.
    assert!(p.load_state(&arming_state(1, 0)));
    frames(&mut p, &mut ed, 1);
    assert!(reader.save_state().is_some());
    frames(&mut p, &mut ed, 3);
    assert!(p.load_state(&arming_state(0, 1)));
    frames(&mut p, &mut ed, 1);
    assert!(reader.save_state().is_some());
    frames(&mut p, &mut ed, 3);
    // A pick through the extended handlers: grouped edits, setDirty, a program-list change.
    assert!(p.load_state(&arming(1 + 2 + 64)));
    frames(&mut p, &mut ed, 1);
    assert!(reader.save_state().is_some());
    frames(&mut p, &mut ed, 3);
    // A reactivation: setProcessing and setActive.
    p.prepare(48_000.0, BLOCK, 2);
    frames(&mut p, &mut ed, 1);
    // The last block's records are written by the next drain.
    transit_vst3_host::service_requests();
    let lines = take();
    rising(&lines);
    in_order(
        &lines,
        &[
            // The host value 1.0 (Gain's 0.5): converted and queued; the next service shows it to
            // the controller, the next block delivers it.
            &["thread=audio", "set_param id=7", "plain=1.0 ", "a host value", "value=0.5 bits=0x3fe0000000000000"],
            &["thread=service", "controller.setParamNormalized id=7 value=0.5 ", "(host value)"],
            &["thread=audio", "processor point id=7 offset=0 value=0.5 "],
            // The host's own load before the pick: component state, then the controller's.
            &["thread=audio", "component.setState bytes=8 ok"],
            &["thread=audio", "controller.setComponentState bytes=8"],
            &["thread=audio", "controller.setState bytes=32 ok"],
            // The pick inside the controller's getState: the edits as the plug-in made them.
            &["thread=ui", "component.getState"],
            &["thread=ui", "beginEdit id=7"],
            &["thread=ui", "performEdit id=7 value=0.7 bits=0x3fe6666666666666"],
            &["thread=ui", "endEdit id=7"],
            &["thread=ui", "controller.getState"],
            // The value handed to the session, its echo, and the exact value at the processor.
            &["thread=ui", "report to the session id=7 value=0.7 ", "plain=1.4 "],
            &["thread=audio", "set_param id=7 plain=1.4 ", "the session's echo", "value=0.7 bits=0x3fe6666666666666"],
            &["thread=audio", "processor point id=7 offset=0 value=0.7 bits=0x3fe6666666666666"],
            // The second pick: a restart with its flags by name, the re-read values reported.
            &["thread=ui", "restartComponent flags=0x4 (kParamValuesChanged)"],
            &["report to the session id=7 value=0.3 "],
            &["thread=audio", "set_param id=7", "the session's echo", "value=0.3 bits=0x3fd3333333333333"],
            &["thread=audio", "processor point id=7 offset=0 value=0.3 bits=0x3fd3333333333333"],
            // The third pick, through the extended handlers.
            &["thread=ui", "IComponentHandler2::startGroupEdit"],
            &["thread=ui", "performEdit id=7 value=0.7 "],
            &["thread=ui", "IComponentHandler2::finishGroupEdit"],
            &["thread=ui", "IComponentHandler2::setDirty state=1"],
            &["thread=ui", "IUnitHandler::notifyProgramListChange list=1 program=1"],
            // The reactivation.
            &["thread=audio", "processor.setProcessing(0)"],
            &["thread=audio", "component.setActive(0)"],
            &["thread=audio", "component.setActive(1)"],
            &["thread=audio", "re-send of a value the host set", "id=7"],
            &["thread=audio", "processor.setProcessing(1)"],
        ],
    );
    assert!(lines.iter().all(|l| l.starts_with("vst3 ") && l.contains(" inst=")), "{lines:#?}");
    log::set_max_level(log::LevelFilter::Info);
    drop(reader);
    drop(ed);
    drop(p);
}
