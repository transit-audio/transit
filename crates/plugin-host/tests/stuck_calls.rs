//! Issue #205: a host thread stuck in a plug-in call is named. The watchdog's report says which
//! plug-in and which call did not return, for the UI's message and the diagnostics report. Fake
//! plug-ins block on a channel the test releases (E49), never on a sleep.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use transit_plugin_host::{Config, Host, StuckCall, call};

fn host(timeout: Duration) -> Host {
    Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() })
}

/// Runs the watchdog once per 5 ms frame until `done` holds of what it reports (at most 5 s, a
/// stall bound: the blocked call is released by the test, not by time).
fn watch(host: &Host, done: impl Fn(&[StuckCall]) -> bool) -> Vec<StuckCall> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        host.check();
        let stuck = host.stuck();
        if done(&stuck) || Instant::now() > deadline {
            return stuck;
        }
        // timing: stall-bound frame pacing while the fake plug-in is held by its channel
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_hung_editor_open_is_named_with_its_plug_in_and_its_call() {
    let host = host(Duration::from_millis(200));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    assert!(host.stuck().is_empty(), "nothing is stuck yet");
    let (release, gate) = mpsc::channel::<()>();
    *log.open_gate.lock().unwrap() = Some(gate);
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    let stuck = watch(&host, |s| !s.is_empty());
    let [s] = stuck.as_slice() else { panic!("one stuck call: {stuck:?}") };
    assert_eq!((&*s.plugin, s.call), ("Fake", call::EDITOR_OPEN), "{s:?}");
    assert_eq!(s.thread, "plugin-host-0");
    assert!(s.stuck_for >= Duration::from_millis(200), "past the budget: {s:?}");
    assert!(s.line().starts_with("Plug-in host: Fake stuck in opening its editor for "), "{}", s.line());
    assert_eq!(host.wedged(), 1);
    drop(release);
    let after = watch(&host, <[StuckCall]>::is_empty);
    assert!(after.is_empty(), "the call returned: nothing is stuck ({after:?})");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_hung_creation_is_named_with_the_plug_in_it_was_making() {
    let host = host(Duration::from_millis(200));
    let (release, gate) = mpsc::channel::<()>();
    let creating = host.submit(move |ctx| {
        // The session's id; no instance of the type was made, so the id is the name.
        ctx.name_job("clap:com.example.never-returns");
        let _ = gate.recv();
    });
    let stuck = watch(&host, |s| !s.is_empty());
    let [s] = stuck.as_slice() else { panic!("one stuck call: {stuck:?}") };
    assert_eq!((&*s.plugin, s.call), ("clap:com.example.never-returns", call::CREATING), "{s:?}");
    // The fresh thread that took over is not stuck: only the wedged one is listed.
    let fresh = host.submit(|_| ()).wait(Duration::from_secs(5));
    assert!(fresh.is_some(), "new creations run on the fresh thread");
    assert_eq!(host.stuck().len(), 1);
    drop(release);
    drop(creating);
    let after = watch(&host, <[StuckCall]>::is_empty);
    assert!(after.is_empty(), "{after:?}");
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_hung_preset_load_is_named_as_a_preset_job() {
    let host = host(Duration::from_millis(200));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let (release, hang) = mpsc::channel::<()>();
    *log.hang.lock().unwrap() = Some(hang);
    let mut reader = plugin.state_reader().unwrap();
    let job = reader.begin_load(b"hang".to_vec());
    let stuck = watch(&host, |s| !s.is_empty());
    let [s] = stuck.as_slice() else { panic!("one stuck call: {stuck:?}") };
    assert_eq!((&*s.plugin, s.call), ("Fake", call::PRESET_JOB), "{s:?}");
    drop(release);
    drop(job);
    let after = watch(&host, <[StuckCall]>::is_empty);
    assert!(after.is_empty(), "{after:?}");
    drop((reader, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}
