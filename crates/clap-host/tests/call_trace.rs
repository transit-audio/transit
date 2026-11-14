//! Issue #651, CLAP side: the host call trace. While debug logging is on, every call between the
//! host and a plug-in that can bear on the plug-in's "modified" state is written to the log
//! (target `transit_clap_host::trace`) with the instance, a sequence number, the thread, the
//! parameter and the full value with its bits; while it is off nothing is recorded. This binary
//! installs a logger that keeps those lines, runs the fixture gain through each call kind, and
//! checks the lines come in order. One test only: the trace and the logger are process-wide.

#![cfg(not(target_os = "macos"))] // Editor handles run on the process main thread on macOS.

mod common;

use std::sync::Mutex;
use transit_dsp::{Plugin, PluginEditor};

const BLOCK: usize = 64;
const TARGET: &str = "transit_clap_host::trace";

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

/// UI frames as the app runs them: the editor handle's idle, its edits into the session and back
/// through `set_param`, the request service, then an audio block.
fn frames(p: &mut Box<dyn Plugin>, ed: &mut Box<dyn PluginEditor>, n: usize) {
    for _ in 0..n {
        for (id, v) in ed.idle() {
            assert!(p.set_param(&id, v), "{id}");
        }
        transit_clap_host::service_requests();
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

#[test]
fn the_trace_records_each_call_kind_in_order_and_nothing_while_debug_logging_is_off() {
    log::set_logger(&Keep).expect("the only logger of this binary");
    log::set_max_level(log::LevelFilter::Info);
    common::fixture_dir();
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("the gain fixture has an editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 2);
    // Off: an edit, a rescan, a host value and a save write nothing.
    assert!(p.load_state(b"CMD:edit:0.4"));
    frames(&mut p, &mut ed, 3);
    let _ = p.load_state(b"CMD:rescan-values:0.6");
    assert!(p.set_param("7", 0.9));
    frames(&mut p, &mut ed, 3);
    assert!(reader.save_state().is_some());
    assert_eq!(take(), Vec::<String>::new(), "nothing is traced while debug logging is off");

    // On.
    log::set_max_level(log::LevelFilter::Debug);
    // A value of the host's own.
    assert!(p.set_param("7", 1.0));
    frames(&mut p, &mut ed, 1);
    // An edit in the plug-in's editor (an output event), handed to the session and echoed.
    assert!(p.load_state(b"CMD:edit:0.3"));
    frames(&mut p, &mut ed, 3);
    // A change announced with a rescan.
    let _ = p.load_state(b"CMD:rescan-values:0.7");
    frames(&mut p, &mut ed, 3);
    assert!(reader.save_state().is_some());
    // A reactivation.
    p.prepare(48_000.0, BLOCK, 2);
    frames(&mut p, &mut ed, 1);
    transit_clap_host::service_requests();
    let lines = take();
    let seqs: Vec<u64> = lines.iter().filter_map(|l| l.strip_prefix("clap #")?.split(' ').next()?.parse().ok()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "sequence numbers rise: {seqs:?}");
    in_order(
        &lines,
        &[
            &["thread=audio", "set_param id=7 plain=1.0 ", "a host value", "value=1.0 bits=0x3ff0000000000000"],
            &["thread=audio", "input param value id=7 time=0 value=1.0 "],
            &["thread=audio", "state.load bytes=12"],
            &["thread=audio", "output param value id=7 value=0.3 bits=0x3fd3333333333333"],
            &["thread=ui", "report to the session id=7 value=0.3 ", "plain=0.3 "],
            &["thread=audio", "set_param id=7 plain=0.3 ", "the session's echo, nothing sent"],
            &["thread=audio", "params.rescan flags=0x1 (CLAP_PARAM_RESCAN_VALUES)"],
            &["report to the session id=7 value=0.7 "],
            &["thread=audio", "set_param id=7 plain=0.7 ", "the session's echo, nothing sent"],
            &["thread=ui", "state.save bytes="],
            &["thread=audio", "stop_processing"],
            &["thread=audio", "deactivate"],
            &["thread=audio", "activate"],
            &["thread=audio", "start_processing"],
        ],
    );
    assert!(
        !lines.iter().any(|l| l.contains("input param value id=7") && (l.contains("value=0.30000001") || l.contains("value=0.69999998"))),
        "{lines:#?}"
    );
    log::set_max_level(log::LevelFilter::Info);
    drop(reader);
    drop(ed);
    drop(p);
}
