//! Issue #205: giving up on a host thread stuck inside a plug-in. Its plug-ins are quarantined
//! (never called, freed or reused again, also once the call returns), the thread stops counting as
//! wedged, new plug-ins load on the fresh thread, and the thread's windows are asked to hide
//! without waiting for it. Fake plug-ins block on channels the test releases (E49).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use transit_plugin_host::{Config, Host, StuckCall, call};

fn host() -> Host {
    let timeout = Duration::from_millis(200);
    Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() })
}

/// Runs the watchdog once per 5 ms frame until `done` holds (a 5 s stall bound).
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
fn giving_up_quarantines_the_stuck_threads_plug_ins_and_new_ones_load_elsewhere() {
    let host = host();
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let old = log.thread_of("create");
    assert_eq!(host.give_up(), transit_plugin_host::GaveUp::default(), "nothing is stuck: nothing to give up on");
    // Its editor open never returns (a preset browse that hangs inside the plug-in).
    let (release, gate) = mpsc::channel::<()>();
    *log.open_gate.lock().unwrap() = Some(gate);
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    let stuck = watch(&host, |s| !s.is_empty());
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    // A request queued behind the stuck call before the user gave up.
    editor.set_param("gain", 0.5);
    let gave = host.give_up();
    assert_eq!(gave.plugins.iter().map(|p| &**p).collect::<Vec<_>>(), ["Fake"]);
    assert_eq!(gave.windows, 0, "the fake has no window");
    assert_eq!((host.wedged(), host.given_up()), (0, 1), "given up: no longer counted as not responding");
    assert!(host.stuck()[0].given_up, "the report says it was given up on");
    assert_eq!(host.give_up(), transit_plugin_host::GaveUp::default(), "once");
    // Its plug-ins answer at once, as unreachable.
    let mut reader = plugin.state_reader().unwrap();
    assert_eq!(reader.save_state_within(Duration::from_secs(60)), Err(transit_dsp::StateBusy));
    assert!(editor.available().is_err());
    // New plug-ins load on the fresh thread.
    let l = Arc::clone(&log);
    let fresh = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5));
    assert!(fresh.is_some(), "a new plug-in loads");
    let made = log.calls.lock().unwrap().iter().filter(|(c, _)| *c == "create").map(|(_, t)| *t).collect::<Vec<_>>();
    assert_ne!(made.last().copied(), old, "on another thread");
    // The call returns: the thread parks, and nothing queued for it runs.
    drop(release);
    let parked = watch(&host, <[StuckCall]>::is_empty);
    assert!(parked.is_empty(), "{parked:?}");
    assert_eq!(log.thread_of("editor_set_param"), None, "the request queued behind the call never ran");
    // Dropping the quarantined instance leaks it: its destruction is never sent to a thread.
    drop((reader, editor, plugin));
    assert_eq!(
        log.calls.lock().unwrap().iter().filter(|(c, t)| *c == "destroy" && Some(*t) == old).count(),
        0,
        "the quarantined instance is never destroyed"
    );
    drop(fresh);
    assert!(host.shutdown(Duration::from_secs(5)), "shutdown does not wait for the given-up thread");
}

#[test]
fn a_hung_creation_given_up_on_is_named_and_leaves_the_host_working() {
    let host = host();
    let (release, gate) = mpsc::channel::<()>();
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let hung = host.submit(move |ctx| {
        ctx.name_job("Guitar Rig 7");
        let _ = gate.recv();
        ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))
    });
    let stuck = watch(&host, |s| !s.is_empty());
    assert_eq!((&*stuck[0].plugin, stuck[0].call), ("Guitar Rig 7", call::CREATING));
    let gave = host.give_up();
    assert_eq!(gave.plugins.iter().map(|p| &**p).collect::<Vec<_>>(), ["Guitar Rig 7"]);
    assert_eq!(host.submit(|_| 7).wait(Duration::from_secs(5)), Some(7), "the host goes on");
    // The creation returns after all: its instance stays quarantined, never delivered.
    drop(release);
    assert!(hung.wait(Duration::from_secs(5)).is_none(), "a late instance of a given-up thread is not delivered");
    assert!(log.thread_of("create").is_some(), "it was made");
    assert_eq!(log.thread_of("destroy"), None, "and never freed");
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Windows: the stuck thread's editor window is asked to hide from the thread that gives up,
/// without waiting for the stuck one (the hide itself waits for that thread's next message; the
/// mechanics are tested in `transit_rt::editor_window`).
#[cfg(windows)]
#[test]
fn giving_up_asks_the_stuck_threads_windows_to_hide() {
    let host = host();
    let shown = host
        .submit(|_| {
            let win = transit_rt::editor_window::EditorWindow::new("Guitar Rig 7", 200.0, 100.0, None).unwrap();
            // A plug-in's editor window, owned by the host thread from now on (the handle has no
            // destructor: when it goes, the window stays open).
            win.raise();
        })
        .wait(Duration::from_secs(5));
    assert!(shown.is_some());
    let (release, gate) = mpsc::channel::<()>();
    let _hung = host.submit(move |_| {
        let _ = gate.recv();
    });
    let _ = watch(&host, |s| !s.is_empty());
    // Returning at all shows the call did not wait: the stuck thread retrieves no message.
    let gave = host.give_up();
    assert_eq!(gave.windows, 1, "{gave:?}");
    drop(release);
}
