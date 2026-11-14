//! Issue #651, CLAP side: a value the plug-in reports itself (an edit in its own editor, such as a
//! preset it loads, as an output parameter event; or a change it announces with
//! `params.rescan(CLAP_PARAM_RESCAN_VALUES)`) goes to the session as an `f32`, and the session
//! sends it back through `set_param` (`mix.insert_param`, then the engine). The plug-in already
//! has that value: nothing may go back to it, least of all the `f32` rounding of its own `f64`,
//! which a plug-in takes as a change and marks its preset modified.
//!
//! The fixture's gain logs every parameter event the host sends it (see `tests/fixture`).

#![cfg(not(target_os = "macos"))] // Editor handles run on the process main thread on macOS.

mod common;

use transit_dsp::{Plugin, PluginEditor};

const BLOCK: usize = 64;

/// The tests of this file run one at a time: the fixture keeps its call log and its armed pick
/// process-wide.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// UI frames as the app runs them: the editor handle's idle, its edits into the session and back
/// to the plug-in through `set_param`, then an audio block.
fn frames(p: &mut Box<dyn Plugin>, ed: &mut Box<dyn PluginEditor>, n: usize) {
    for _ in 0..n {
        for (id, v) in ed.idle() {
            assert!(p.set_param(&id, v), "{id}");
        }
        p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    }
}

/// The parameter events the gain fixture logged since the last read (after its two f64s).
fn log_of(blob: Option<Vec<u8>>) -> Vec<String> {
    let blob = blob.expect("state");
    String::from_utf8(blob[16..].to_vec()).expect("utf-8 log").lines().map(str::to_string).collect()
}

#[test]
fn a_value_the_plugin_reports_is_not_sent_back_to_it() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    common::fixture_dir();
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("the gain fixture has an editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 2);
    assert!(p.load_state(b"CMD:log-start"));
    // The user picks a preset in its editor: Gain 0.3, reported as an output parameter event.
    assert!(p.load_state(b"CMD:edit:0.3"));
    frames(&mut p, &mut ed, 4);
    assert_eq!(p.param("7"), Some(0.3f64 as f32), "the session holds the preset's Gain");
    // A change it announces with a rescan: Gain 0.7.
    assert!(!p.load_state(b"CMD:rescan-values:0.7"));
    frames(&mut p, &mut ed, 4);
    assert_eq!(p.param("7"), Some(0.7f64 as f32), "the session holds the re-read Gain");
    let log = log_of(reader.save_state());
    assert_eq!(log, Vec::<String>::new(), "the host sent the plug-in's own values back to it");
    // A value of the host's own (automation, a mapped control) still reaches it, including one
    // equal to a value the plug-in reported before. Here the session drops the report (as while a
    // session restore is pending), so it never echoes it.
    assert!(p.load_state(b"CMD:edit:0.3"));
    p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
    let dropped = ed.idle();
    assert_eq!(dropped, vec![("7".to_string(), 0.3f64 as f32)]);
    for v in [1.0f32, 0.3f64 as f32] {
        assert!(p.set_param("7", v));
        frames(&mut p, &mut ed, 1);
        let log = log_of(reader.save_state());
        assert_eq!(log, vec![format!("param(7)={:?}@0", f64::from(v))], "the host's {v} reaches the plug-in");
    }
    drop(reader);
    drop(ed);
    drop(p);
}

/// The session may send the same values again: a parameter map that grows re-sends every value
/// it stores (a hosted plug-in's map starts empty and grows with each parameter the plug-in
/// reports), and a later publication of the same values does the same. A repeat of the echo the
/// host already holds is nothing new: nothing goes to the plug-in.
#[test]
fn the_session_sending_its_echo_again_sends_nothing() {
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    common::fixture_dir();
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.gain").expect("gain fixture");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("the gain fixture has an editor handle");
    let mut reader = p.state_reader().expect("state reader");
    frames(&mut p, &mut ed, 2);
    assert!(p.load_state(b"CMD:log-start"));
    for (cmd, want) in [("CMD:edit:0.3", 0.3f64), ("CMD:rescan-values:0.7", 0.7)] {
        let _ = p.load_state(cmd.as_bytes());
        p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
        let edits = ed.idle();
        assert_eq!(edits, vec![("7".to_string(), want as f32)], "{cmd}");
        for _ in 0..3 {
            assert!(p.set_param("7", want as f32));
            p.process(&mut vec![vec![0.5f32; BLOCK]; 2], BLOCK);
        }
        frames(&mut p, &mut ed, 2);
        let log = log_of(reader.save_state());
        assert_eq!(log, Vec::<String>::new(), "{cmd}: the host sent the plug-in's own value back to it");
    }
    drop(reader);
    drop(ed);
    drop(p);
}
