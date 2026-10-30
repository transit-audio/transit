//! ADR 0018: the caller (standing in for the UI frame loop) never waits for plug-in code. Each test
//! polls once per "frame" and checks that no frame takes long while a fake plug-in sleeps in its
//! creation, destruction, or forever.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_plugin_host::{Config, Host, Status};

/// A frame may take this long at most. Load-tolerant: a starved runner can stall a poll for a
/// while, but a frame that waited for the plug-in takes its full 1 s sleep.
const FRAME_BUDGET: Duration = Duration::from_millis(400);
/// A fake plug-in's slow creation or destruction: well past the frame budget.
const SLOW: Duration = Duration::from_millis(1000);

/// A host whose requests and creations both wedge their thread after `timeout` (the wedge
/// mechanics; [`slow_host`] gives creations their own, longer budget, as the app's host does).
fn host(timeout: Duration) -> Host {
    Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() })
}

/// The app's shape (issue #82): a short request timeout and a longer creation budget.
fn slow_host(timeout: Duration, creation_timeout: Duration) -> Host {
    Host::new(Config { timeout, creation_timeout, inline: false, ..Config::default() })
}

/// Issue #82 (E48): a creation that runs past the request timeout is slow, not stuck. It never
/// reads stuck within its own budget, the thread is not marked wedged, the creation queued behind
/// it moves to a fresh thread, and the slow instance is delivered when it returns.
#[test]
fn a_creation_slower_than_the_request_timeout_is_delivered_and_hands_the_queue_over() {
    let host = slow_host(Duration::from_millis(200), Duration::from_secs(30));
    let (release, gate) = mpsc::channel::<()>();
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let slow = host.submit(move |ctx| {
        let _ = gate.recv(); // the heavy instrument, until the test lets it finish
        ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    // timing: stall-bound five seconds for the gated creation to start; its creation timeout is 30 seconds
    while !matches!(slow.poll(), Status::Waiting { running: true }) {
        assert!(Instant::now() < deadline, "the gated creation never started");
        std::thread::sleep(Duration::from_millis(1));
    }
    let behind_log = Arc::new(Log::default());
    let b = Arc::clone(&behind_log);
    let behind = host.submit(move |ctx| ctx.adopt(Fake::make(&b, Duration::ZERO, Duration::ZERO)));
    // Well past the request timeout: still loading, never stuck, nothing wedged.
    let until = Instant::now() + Duration::from_millis(800);
    // timing: stall-bound observe past the 200 ms request timeout; the gated creation has a separate 30-second budget
    while Instant::now() < until {
        assert!(matches!(slow.poll(), Status::Waiting { running: true }), "a slow creation is not stuck");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!((host.wedged(), host.wedges()), (0, 0), "a slow creation does not wedge its thread");
    let (status, _, _) = frames(&behind, Duration::from_secs(5));
    assert!(matches!(status, Status::Ready(_)), "the creation queued behind it ran on a fresh thread");
    // It finishes late: the instance is delivered, made on the slow thread.
    drop(release);
    let (status, _, _) = frames(&slow, Duration::from_secs(5));
    let Status::Ready(plugin) = status else { panic!("the late instance is delivered") };
    assert_ne!(log.thread_of("create"), behind_log.thread_of("create"), "two threads");
    drop(plugin);
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Issue #82: a creation past its own budget reads stuck (its thread is wedged), and if it returns
/// after all, the next poll delivers its instance: nothing is discarded by the host.
#[test]
fn a_creation_that_returns_after_reading_stuck_is_still_delivered() {
    let host = slow_host(Duration::from_millis(100), Duration::from_millis(300));
    let (release, gate) = mpsc::channel::<()>();
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let late = host.submit(move |ctx| {
        let _ = gate.recv();
        ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))
    });
    let started = Instant::now();
    let (status, _, _) = frames(&late, Duration::from_secs(5));
    assert!(matches!(status, Status::Stuck), "past the creation budget it reads stuck");
    assert!(started.elapsed() >= Duration::from_millis(250), "not before the creation budget ({:?})", started.elapsed());
    assert_eq!(host.wedged(), 1);
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    // timing: stall-bound five seconds for delivery after releasing the gate; the creation timeout has already fired
    let plugin = loop {
        match late.poll() {
            Status::Ready(p) => break p,
            Status::Stuck | Status::Waiting { .. } if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => panic!("the late instance is delivered"),
        }
    };
    // timing: stall-bound five seconds for the released thread to return; no short expiry determines success
    while host.wedged() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.wedged(), 0, "its thread returned");
    drop(plugin);
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Polls `pending` once per 5 ms frame until it is not waiting; returns the status, the frames
/// that ran meanwhile and the longest frame.
fn frames<T>(pending: &transit_plugin_host::Pending<T>, limit: Duration) -> (Status<T>, u32, Duration) {
    let (start, mut n, mut worst) = (Instant::now(), 0u32, Duration::ZERO);
    loop {
        let t = Instant::now();
        let status = pending.poll();
        worst = worst.max(t.elapsed());
        n += 1;
        if !matches!(status, Status::Waiting { .. }) || start.elapsed() > limit {
            return (status, n, worst);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn frames_advance_while_a_slow_instance_is_created_on_the_host_thread() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let pending = host.submit(move |ctx| {
        let p = Fake::make(&l, SLOW, Duration::ZERO);
        ctx.adopt(p)
    });
    assert!(matches!(pending.poll(), Status::Waiting { .. }), "the first frame sees it loading");
    let (status, n, worst) = frames(&pending, Duration::from_secs(5));
    let Status::Ready(plugin) = status else { panic!("not made") };
    // Not a frame-rate floor (#87, E49): one more frame than a blocked loop would have run.
    assert!(n >= 2, "only {n} frames ran during a 1 s creation");
    assert!(worst < FRAME_BUDGET, "a frame took {worst:?}");
    assert_ne!(log.thread_of("create"), Some(std::thread::current().id()), "made on the host thread");
    drop(plugin);
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_slow_destruction_runs_on_the_host_thread_and_the_drop_returns_at_once() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, SLOW))).wait(Duration::from_secs(5)).unwrap();
    let t = Instant::now();
    drop(plugin);
    assert!(t.elapsed() < FRAME_BUDGET, "the drop waited {:?} for the plug-in", t.elapsed());
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.thread_of("destroy").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(log.thread_of("destroy"), log.thread_of("create"), "destroyed on the thread that made it");
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_creation_that_never_returns_is_stuck_after_the_timeout_and_never_blocks_a_frame() {
    let host = host(Duration::from_millis(300));
    let (release, hang) = mpsc::channel::<()>();
    let stuck = host.submit(move |_| {
        let _ = hang.recv(); // "never" returns: until the test ends
    });
    // Queued behind the stuck one before anyone knew: it moves to the fresh thread.
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let behind = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO)));
    let (status, n, worst) = frames(&stuck, Duration::from_secs(5));
    assert!(matches!(status, Status::Stuck), "the hung creation reads as stuck, got {status:?}");
    // The property is "never blocks a frame", not a frame rate (#87, E49): a shared runner may
    // run few 5 ms frames in the 300 ms window. More than one frame ran (a blocked loop sees a
    // single poll, which returns only after the timeout, so `n >= 2` is the deterministic check)
    // and none stalled past the stall bound every frame test uses; a tighter 250 ms bound sat
    // under the rule's margin on a starved runner (E49, #226).
    assert!(n >= 2, "frames ran meanwhile ({n})");
    assert!(worst < FRAME_BUDGET, "a frame took {worst:?}");
    assert_eq!(host.wedged(), 1);
    let (status, _, _) = frames(&behind, Duration::from_secs(5));
    assert!(matches!(status, Status::Ready(_)), "the queued request ran on the fresh thread");
    // New requests go to the fresh thread too.
    let fresh = host.submit(|_| std::thread::current().id()).wait(Duration::from_secs(5)).unwrap();
    assert_eq!(Some(fresh), log.thread_of("create"));
    // Shutdown does not wait for the abandoned thread.
    let t = Instant::now();
    assert!(host.shutdown(Duration::from_secs(2)));
    assert!(t.elapsed() < Duration::from_secs(2));
    drop(release);
}

#[test]
fn state_restore_and_save_round_trip_through_the_host_thread() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host
        .submit(move |ctx| {
            let mut p = Fake::make(&l, Duration::ZERO, Duration::ZERO);
            assert!(p.load_state(b"stored"));
            ctx.adopt(p)
        })
        .wait(Duration::from_secs(5))
        .unwrap();
    let mut reader = plugin.state_reader().expect("a state reader");
    assert_eq!(reader.save_state().as_deref(), Some(&b"stored"[..]));
    let host_thread = log.thread_of("create");
    assert_eq!(log.thread_of("load_state"), host_thread);
    assert_eq!(log.thread_of("save_state"), host_thread, "the state was read on the host thread");
    assert_ne!(host_thread, Some(std::thread::current().id()));
    drop((reader, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_state_read_from_a_wedged_thread_answers_at_once() {
    let host = host(Duration::from_millis(200));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let (release, hang) = mpsc::channel::<()>();
    // A hang on the instance's own thread (another job, a window message, a tick).
    let stuck = host.submit(move |_| {
        let _ = hang.recv();
    });
    let (status, _, _) = frames(&stuck, Duration::from_secs(5));
    assert!(matches!(status, Status::Stuck));
    let t = Instant::now();
    assert_eq!(plugin.state_reader().unwrap().save_state(), None);
    assert!(t.elapsed() < FRAME_BUDGET, "the wedged thread was waited for");
    // A save's read (issue #56) gives up at once too, however long it may wait for a dialog.
    let t = Instant::now();
    assert_eq!(plugin.state_reader().unwrap().save_state_within(Duration::from_secs(60)), Err(transit_dsp::StateBusy));
    assert!(t.elapsed() < FRAME_BUDGET, "the wedged thread was waited for by a save");
    let mut editor = plugin.editor().unwrap();
    assert!(editor.open().unwrap_err().contains("not responding"));
    drop(release);
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn editor_requests_run_on_the_host_thread_and_report_back() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().expect("an editor");
    let t = Instant::now();
    editor.open().unwrap();
    assert!(t.elapsed() < FRAME_BUDGET);
    assert!(editor.is_open(), "reads open while the request is on its way");
    // The host thread's tick idles the editor; its edit comes back through `idle`.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut edits = Vec::new();
    while edits.is_empty() && Instant::now() < deadline {
        edits = editor.idle();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(edits, vec![("gain".to_string(), 0.25)]);
    editor.set_param("gain", 0.5);
    editor.close();
    assert!(!editor.is_open());
    let host_thread = log.thread_of("create");
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.thread_of("editor_close").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    for call in ["editor_open", "editor_idle", "editor_set_param", "editor_close"] {
        assert_eq!(log.thread_of(call), host_thread, "{call} ran on the host thread");
    }
    assert_eq!(*log.editor_params.lock().unwrap(), vec![("gain".to_string(), 0.5)]);
    assert!(!editor.is_open(), "a tick does not reopen a closed editor");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Issue #67: an editor open is timed on the host's clock, request to window. A clock that moves
/// one second per reading makes the figures exact without a sleep: the request is read once, the
/// host thread reads the start and the end.
#[test]
fn an_editor_open_reports_its_queue_and_window_time_on_the_host_clock() {
    let base = Instant::now();
    let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let t = Arc::clone(&ticks);
    let clock: transit_plugin_host::Clock = Arc::new(move || base + Duration::from_secs(t.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let host = Host::new(Config { clock, inline: false, ..Config::default() });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().unwrap();
    assert_eq!(editor.take_open_report(), None, "nothing before an open");
    editor.open().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut report = None;
    while report.is_none() && Instant::now() < deadline {
        report = editor.take_open_report();
        std::thread::sleep(Duration::from_millis(2));
    }
    let report = report.expect("the open finished");
    assert_eq!((report.queued, report.opening), (Duration::from_secs(1), Duration::from_secs(1)));
    assert_eq!(editor.take_open_report(), None, "reported once");
    assert_eq!(editor.opening_for(), None, "no open is pending");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// The Opening state counts up while the open waits behind a busy host thread, and the wait shows
/// as the queued time.
#[test]
fn an_open_waiting_behind_a_busy_host_thread_counts_up_on_the_clock() {
    let base = Instant::now();
    let now = Arc::new(Mutex::new(base));
    let n = Arc::clone(&now);
    let clock: transit_plugin_host::Clock = Arc::new(move || *n.lock().unwrap());
    let host = Host::new(Config { clock, inline: false, ..Config::default() });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().unwrap();
    // The thread is held by an earlier job until the test lets it go.
    let (release, held) = mpsc::channel::<()>();
    let blocker = host.submit(move |_| held.recv().is_ok());
    editor.open().unwrap();
    assert_eq!(editor.opening_for(), Some(Duration::ZERO));
    *now.lock().unwrap() = base + Duration::from_millis(4_500);
    assert_eq!(editor.opening_for(), Some(Duration::from_millis(4_500)));
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut report = None;
    while report.is_none() && Instant::now() < deadline {
        report = editor.take_open_report();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(report.expect("the open finished").queued, Duration::from_millis(4_500));
    drop(blocker);
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// A failed open reports no "opened" time; its error is what the status line gets.
#[test]
fn a_failed_open_has_no_open_report_and_keeps_its_error() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host
        .submit(move |ctx| ctx.adopt(Box::new(Fake { log: l, destroy: Duration::ZERO, editor_fails: true })))
        .wait(Duration::from_secs(5))
        .unwrap();
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut error = None;
    while error.is_none() && Instant::now() < deadline {
        error = editor.take_error();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(error.as_deref(), Some("no window today"));
    assert_eq!(editor.take_open_report(), None);
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// A host thread that wedges while an open waits behind its job leaves no "Opening… N s" counting.
#[test]
fn opening_for_stops_when_the_host_thread_wedges() {
    let base = Instant::now();
    let now = Arc::new(Mutex::new(base));
    let n = Arc::clone(&now);
    let clock: transit_plugin_host::Clock = Arc::new(move || *n.lock().unwrap());
    let host = Host::new(Config {
        timeout: Duration::from_millis(100),
        creation_timeout: Duration::from_millis(100),
        clock,
        inline: false,
        ..Config::default()
    });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().unwrap();
    let (release, held) = mpsc::channel::<()>();
    let blocker = host.submit(move |_| held.recv().is_ok());
    editor.open().unwrap();
    assert_eq!(editor.opening_for(), Some(Duration::ZERO));
    let deadline = Instant::now() + Duration::from_secs(5);
    while editor.opening_for().is_some() && Instant::now() < deadline {
        host.check();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(editor.opening_for(), None, "the wedged thread's open is not counted");
    release.send(()).unwrap();
    drop(blocker);
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn an_editor_that_fails_to_open_reports_why_once() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host
        .submit(move |ctx| {
            let p = Box::new(Fake { log: l, destroy: Duration::ZERO, editor_fails: true });
            ctx.adopt(p)
        })
        .wait(Duration::from_secs(5))
        .unwrap();
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut error = None;
    while error.is_none() && Instant::now() < deadline {
        error = editor.take_error();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(error.as_deref(), Some("no window today"));
    assert!(!editor.is_open());
    assert_eq!(editor.take_error(), None, "reported once");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn a_request_dropped_before_it_starts_never_runs() {
    let host = host(Duration::from_secs(10));
    let (release, hang) = mpsc::channel::<()>();
    let busy = host.submit(move |_| {
        let _ = hang.recv();
    });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let cancelled = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO)));
    drop(cancelled);
    drop(release);
    assert!(matches!(frames(&busy, Duration::from_secs(5)).0, Status::Ready(())));
    host.submit(|_| ()).wait(Duration::from_secs(5)).unwrap();
    assert_eq!(log.thread_of("create"), None, "the cancelled creation never ran");
    assert!(host.shutdown(Duration::from_secs(5)));
}

#[test]
fn inline_mode_runs_on_the_calling_thread_and_keeps_the_instance_unwrapped() {
    let host = Host::new(Config { inline: true, ..Config::default() });
    assert!(host.is_inline());
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let pending = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO)));
    let Status::Ready(mut plugin) = pending.poll() else { panic!("inline jobs finish at once") };
    assert_eq!(log.thread_of("create"), Some(std::thread::current().id()));
    plugin.editor().unwrap().open().unwrap();
    assert_eq!(log.thread_of("editor_open"), Some(std::thread::current().id()), "the editor is the plug-in's own");
    drop(plugin);
    assert_eq!(log.thread_of("destroy"), Some(std::thread::current().id()));
}

/// Issue #36: closing the app while a plug-in is being created does not hang. The creation is
/// not stuck yet (the timeout is long), and shutdown still returns within its own bound.
#[test]
fn shutdown_during_a_creation_returns_within_its_bound() {
    let host = host(Duration::from_secs(60));
    let (release, hang) = mpsc::channel::<()>();
    let creating = host.submit(move |_| {
        let _ = hang.recv();
    });
    while !matches!(creating.poll(), Status::Waiting { running: true }) {
        // timing: sleep-ok polls until the creation is running; the 2 s shutdown bound is a stall bound
        std::thread::sleep(Duration::from_millis(1));
    }
    let t = Instant::now();
    assert!(!host.shutdown(Duration::from_millis(500)), "the busy thread did not finish");
    assert!(t.elapsed() < Duration::from_secs(2), "shutdown waited {:?}", t.elapsed());
    assert!(matches!(host.submit(|_| ()).poll(), Status::Failed), "no new requests after shutdown");
    drop(release);
}

/// B1 of the ADR 0018 review: a thread that was slow, not stuck (a 12 s sampler restore, a modal
/// dialog), is responsive again when its call returns. Its instances' live states are read and
/// their editors open; new creations stay on the thread that took over.
#[test]
fn a_thread_that_returns_after_the_timeout_serves_its_instances_again() {
    let host = host(Duration::from_millis(300));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host
        .submit(move |ctx| {
            let mut p = Fake::make(&l, Duration::ZERO, Duration::ZERO);
            assert!(p.load_state(b"live"));
            ctx.adopt(p)
        })
        .wait(Duration::from_secs(5))
        .unwrap();
    let old = log.thread_of("create");
    let long = host.submit(|_| std::thread::sleep(Duration::from_millis(900)));
    let (status, _, _) = frames(&long, Duration::from_secs(5));
    assert!(matches!(status, Status::Stuck), "past the timeout it reads stuck, got {status:?}");
    assert_eq!(host.wedged(), 1);
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.wedged() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.wedged(), 0, "the thread returned: it is no longer wedged");
    assert_eq!(host.wedges(), 1);
    // The live state, read on the old thread (a save after this carries it, not the stored one).
    *log.state.lock().unwrap() = b"edited after the stall".to_vec();
    assert_eq!(plugin.state_reader().unwrap().save_state().as_deref(), Some(&b"edited after the stall"[..]));
    assert_eq!(log.calls.lock().unwrap().iter().rev().find(|(c, _)| *c == "save_state").map(|(_, t)| *t), old);
    let mut editor = plugin.editor().unwrap();
    editor.open().expect("the editor opens again");
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.thread_of("editor_open").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(log.thread_of("editor_open"), old);
    assert!(editor.is_open());
    // New creations go to the thread that took over.
    let fresh = host.submit(|_| std::thread::current().id()).wait(Duration::from_secs(5)).unwrap();
    assert_ne!(Some(fresh), old);
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// An editor open queued behind a call that then wedges its thread does not read open forever;
/// it opens once the thread returns.
#[test]
fn an_open_queued_on_a_thread_that_wedges_is_not_reported_open_until_it_runs() {
    let host = host(Duration::from_millis(300));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let (release, hang) = mpsc::channel::<()>();
    let stuck = host.submit(move |_| {
        let _ = hang.recv();
    });
    while !matches!(stuck.poll(), Status::Waiting { running: true }) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    assert!(editor.is_open(), "on its way");
    assert!(matches!(frames(&stuck, Duration::from_secs(5)).0, Status::Stuck));
    assert!(!editor.is_open(), "a wedged thread's pending open does not read open");
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !editor.is_open() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(editor.is_open(), "it opens when the thread returns");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Issue #56 (b): the host thread drops its job receiver before its message queue (the COM
/// apartment) ends, so a destroy job sent after shutdown is never run: the instance is leaked by
/// its sender and the drop returns at once.
#[test]
fn a_job_sent_after_shutdown_is_never_run() {
    let host = host(Duration::from_secs(10));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    assert!(host.shutdown(Duration::from_secs(5)));
    let t = Instant::now();
    drop(plugin);
    assert!(t.elapsed() < FRAME_BUDGET, "the drop waited {:?}", t.elapsed());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(log.thread_of("destroy"), None, "the destroy job sent after shutdown never ran");
    assert!(matches!(host.submit(|_| ()).poll(), Status::Failed), "no job runs after shutdown");
}

/// Issue #68: the header strip's model goes to the editor on the host thread, only when it
/// changes, and what the user did there comes back through the proxy; a wedged thread disables it.
#[test]
fn the_header_reaches_the_editor_on_the_host_thread_and_its_actions_come_back() {
    use transit_dsp::{EditorHeader, HeaderAction};
    let host = host(Duration::from_millis(300));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().unwrap();
    assert_eq!(editor.available(), Ok(()));
    let header = EditorHeader { title: "Drums · Insert A · Fake".into(), user: vec!["Kit".into()], ..EditorHeader::default() };
    let t = Instant::now();
    editor.set_header(&header);
    editor.set_header(&header);
    assert!(t.elapsed() < FRAME_BUDGET, "never waits for the plug-in");
    let deadline = Instant::now() + Duration::from_secs(5);
    while log.thread_of("set_header").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(log.thread_of("set_header"), log.thread_of("create"), "on the host thread");
    assert_eq!(*log.headers.lock().unwrap(), vec![header.clone()], "sent once: the second was the same");
    log.actions.lock().unwrap().push(HeaderAction::User(0));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut actions = Vec::new();
    while actions.is_empty() && Instant::now() < deadline {
        actions = editor.take_header_actions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(actions, vec![HeaderAction::User(0)], "taken from the editor by the host thread's tick");
    // A wedged thread: the strip is disabled with the reason, at once.
    let (release, hang) = mpsc::channel::<()>();
    let stuck = host.submit(move |_| {
        let _ = hang.recv();
    });
    assert!(matches!(frames(&stuck, Duration::from_secs(5)).0, Status::Stuck));
    let t = Instant::now();
    let why = editor.available().unwrap_err();
    assert!(why.contains("not responding"), "{why}");
    assert!(t.elapsed() < FRAME_BUDGET);
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while editor.available().is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(editor.available(), Ok(()), "available again once the thread returns");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Issue #213: the state reader says whether its plug-in can be reached: yes, no while the host
/// thread is wedged (the unresponsive marker's host-thread cause), yes again once it returns.
#[test]
fn the_state_reader_reports_a_wedged_host_thread_and_its_return() {
    let host = host(Duration::from_millis(300));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let reader = plugin.state_reader().unwrap();
    assert_eq!(reader.available(), Ok(()));
    let (release, hang) = mpsc::channel::<()>();
    let stuck = host.submit(move |_| {
        let _ = hang.recv();
    });
    assert!(matches!(frames(&stuck, Duration::from_secs(5)).0, Status::Stuck));
    let t = Instant::now();
    assert!(reader.available().unwrap_err().contains("not responding"));
    assert!(t.elapsed() < FRAME_BUDGET);
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while reader.available().is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reader.available(), Ok(()), "reachable again once the thread returns");
    drop((reader, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Issue #68: a preset save reads the state and a preset load loads it into the live instance,
/// both on the host thread and polled once per frame; a wedged thread fails them at once.
#[test]
fn preset_state_jobs_run_on_the_host_thread_and_never_block_a_frame() {
    use transit_dsp::JobPoll;
    fn finish<T>(job: &mut transit_dsp::StateJob<T>) -> (JobPoll<T>, Duration) {
        let (deadline, mut worst) = (Instant::now() + Duration::from_secs(5), Duration::ZERO);
        loop {
            let t = Instant::now();
            let r = job.poll();
            worst = worst.max(t.elapsed());
            if !matches!(r, JobPoll::Waiting) || Instant::now() > deadline {
                return (r, worst);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let host = host(Duration::from_millis(400));
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host
        .submit(move |ctx| {
            let mut p = Fake::make(&l, Duration::ZERO, Duration::ZERO);
            assert!(p.load_state(b"before"));
            ctx.adopt(p)
        })
        .wait(Duration::from_secs(5))
        .unwrap();
    let mut reader = plugin.state_reader().unwrap();
    let (saved, worst) = finish(&mut reader.begin_save());
    assert_eq!(saved, JobPoll::Ready(Some(b"before".to_vec())));
    assert!(worst < FRAME_BUDGET, "a poll took {worst:?}");
    let (loaded, _) = finish(&mut reader.begin_load(b"my kit".to_vec()));
    assert_eq!(loaded, JobPoll::Ready(Some(vec![("len".to_string(), 6.0)])), "the values after the load come back");
    assert_eq!(*log.state.lock().unwrap(), b"my kit");
    assert_eq!(log.thread_of("live_load"), log.thread_of("create"), "loaded on the host thread");
    let (refused, _) = finish(&mut reader.begin_load(b"refuse".to_vec()));
    assert_eq!(refused, JobPoll::Ready(None), "a refused load says so");
    // Wedged: both fail at once, with the reason.
    let (release, hang) = mpsc::channel::<()>();
    let stuck = host.submit(move |_| {
        let _ = hang.recv();
    });
    assert!(matches!(frames(&stuck, Duration::from_secs(5)).0, Status::Stuck));
    let t = Instant::now();
    assert!(matches!(reader.begin_save().poll(), JobPoll::Failed(e) if e.contains("not responding")));
    assert!(matches!(reader.begin_load(b"x".to_vec()).poll(), JobPoll::Failed(e) if e.contains("not responding")));
    assert!(t.elapsed() < FRAME_BUDGET);
    drop(release);
    // A load queued behind one that hangs (on the instance's own thread) fails as soon as the
    // thread is wedged, and is cancelled: it does not load later behind the UI's back.
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.wedged() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (release, hang) = mpsc::channel::<()>();
    *log.hang.lock().unwrap() = Some(hang);
    let mut hanging = reader.begin_load(b"hang".to_vec());
    let mut queued = reader.begin_load(b"late".to_vec());
    let (r, worst) = finish(&mut queued);
    assert!(matches!(&r, JobPoll::Failed(e) if e.contains("not responding")), "{r:?}");
    assert!(worst < FRAME_BUDGET);
    assert!(matches!(finish(&mut hanging).0, JobPoll::Failed(_)), "the hung load reads failed too");
    drop((queued, hanging));
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.wedged() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (after, _) = finish(&mut reader.begin_save());
    assert_eq!(after, JobPoll::Ready(Some(b"hang".to_vec())), "the cancelled load never ran");
    drop((reader, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}

/// Review B1 of #82: a save during a slow load. Five instances already live on the host thread,
/// then a heavy instrument's creation holds it (past the request timeout, within its creation
/// budget). The save's reads share one deadline (#56): the save gives up within about one
/// request timeout in all, not one per instance, and each plug-in keeps its stored state.
#[test]
fn a_save_during_a_slow_creation_waits_about_one_timeout_not_one_per_instance() {
    let timeout = Duration::from_millis(300);
    let host = slow_host(timeout, Duration::from_secs(30));
    let log = Arc::new(Log::default());
    let mut plugins: Vec<_> = (0..5)
        .map(|_| {
            let l = Arc::clone(&log);
            host.submit(move |ctx| {
                let mut p = Fake::make(&l, Duration::ZERO, Duration::ZERO);
                assert!(p.load_state(b"stored"));
                ctx.adopt(p)
            })
            .wait(Duration::from_secs(5))
            .unwrap()
        })
        .collect();
    let (release, gate) = mpsc::channel::<()>();
    let l = Arc::clone(&log);
    let slow = host.submit(move |ctx| {
        let _ = gate.recv();
        ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))
    });
    while !matches!(slow.poll(), Status::Waiting { running: true }) {
        std::thread::sleep(Duration::from_millis(1));
    }
    // A save: each live instance read with what is left of the shared 500 ms deadline.
    let deadline = Instant::now() + Duration::from_millis(500);
    let t = Instant::now();
    let results: Vec<_> =
        plugins.iter_mut().map(|p| p.state_reader().unwrap().save_state_within(deadline.saturating_duration_since(Instant::now()))).collect();
    let took = t.elapsed();
    assert!(results.iter().all(|r| *r == Err(transit_dsp::StateBusy)), "the held thread cannot answer: {results:?}");
    assert!(took < timeout * 3, "the save waited {took:?}: one request timeout per instance would be {:?}", timeout * 5);
    drop(release);
    assert!(slow.wait(Duration::from_secs(5)).is_some(), "the slow creation is still delivered");
    drop(plugins);
    assert!(host.shutdown(Duration::from_secs(5)));
}
