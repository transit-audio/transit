//! ADR 0021, amendment for #760, slice 1, over the fake's PipeWire and JACK 2 modes: the session's
//! figures are the active client's, a chosen buffer size under PipeWire is settled in the start's
//! request window, and a rate the caller did not prepare for latches one rebuild before any audio.
//! The watcher is driven by hand (its own thread never ticks); the window steps on the fake's
//! clock, never the wall clock.

use crate::JackBackend;
use crate::api::JackApi;
use crate::fake::{Event, FakeServer};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend, Stream};

/// The window's step (the watcher period), as the start takes it.
const STEP: Duration = crate::session::WATCH_PERIOD;

/// A backend whose watcher thread never ticks (the test calls [`JackBackend::tick`]).
fn backend_on(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(Duration::from_secs(3600), Duration::from_secs(3600), Duration::from_secs(1))
}

type Errors = Arc<Mutex<Vec<ab::StreamError>>>;

/// Both directions of one open, the errors either reported, and how often a user callback ran.
struct Opened {
    capture: Box<dyn Stream>,
    render: Box<dyn Stream>,
    errors: Errors,
    calls: Arc<AtomicU32>,
    achieved: ab::Achieved,
}

impl Opened {
    fn start(&self) -> Result<(), ab::OpenError> {
        self.capture.start()?;
        self.render.start()
    }
}

/// Plans the render on the default group with buffer size `period`, then opens capture and render
/// (the Player's duplex order), the capture at the format the backend gives for the render plan.
fn open(backend: &JackBackend, period: Option<u32>) -> Opened {
    let plan = backend.plan_render(&ab::OutputWish { channels: 2, period, ..Default::default() }).expect("render plan");
    let request = plan.attempts[0];
    let format = backend.capture_format(&plan.endpoint, Some(request.format.rate)).expect("capture format");
    assert_eq!(format.rate, request.format.rate, "the capture plan agrees with the render plan");
    let errors: Errors = Arc::default();
    let calls = Arc::new(AtomicU32::new(0));
    let on_error = |errors: &Errors| -> ab::ErrorCallback {
        let errors = Arc::clone(errors);
        Box::new(move |e| errors.lock().unwrap().push(e))
    };
    let counted = Arc::clone(&calls);
    let capture_cb: ab::CaptureCallback = Box::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let counted = Arc::clone(&calls);
    let render_cb: ab::RenderCallback = Box::new(move |block| {
        counted.fetch_add(1, Ordering::SeqCst);
        block.render(&mut |chunk| chunk.fill(0.5));
    });
    let capture_request = ab::OpenRequest { format, ..request };
    let (capture, _) = backend.open_capture(&plan.endpoint, &capture_request, capture_cb, on_error(&errors)).expect("capture");
    let (render, achieved) = backend.open_render(&plan.endpoint, &request, render_cb, on_error(&errors)).expect("render");
    Opened { capture, render, errors, calls, achieved }
}

/// The index of the open session client among every client the server opened (probes included).
fn session_client(server: &FakeServer) -> usize {
    server.clients(|c| c.iter().rposition(|c| c.name == crate::CLIENT_NAME && !c.closed)).expect("a session client")
}

/// Session clients opened so far (each open or reopen makes one).
fn session_opens(server: &FakeServer) -> usize {
    server.clients(|c| c.iter().filter(|c| c.name == crate::CLIENT_NAME).count())
}

fn errors(opened: &Opened) -> Vec<ab::StreamError> {
    opened.errors.lock().unwrap().clone()
}

#[test]
fn an_inactive_clients_request_is_not_the_baseline_and_no_reopen_follows() {
    // The graph runs 1024; the client's own node.latency request says 256 while it is inactive.
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.latency = Some((48_000, 256)));
    let backend = backend_on(&server);
    let opened = open(&backend, None);
    assert_eq!(server.inactive_reads(session_client(&server)), 0, "no rate or buffer getter on the session client before activation");
    assert_eq!(opened.achieved.format.rate, 48_000, "the provisional figures are the plan's");
    assert_eq!(opened.render.period(), None, "nothing settled before the start");
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(1024), "the active graph's buffer, not the inactive request");
    assert!(!opened.render.needs_rebuild());
    backend.tick();
    backend.tick();
    assert!(errors(&opened).is_empty(), "no reset follows the start");
    assert_eq!(session_opens(&server), 1, "no reopen");
    // Control: the inactive request is what an inactive client reports.
    let probe = backend.open_other_client("probe").unwrap();
    assert_eq!(probe.buffer_size(), 256);
}

#[test]
fn a_requested_size_arriving_in_the_window_is_the_baseline() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.force_delay = 2);
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    assert_eq!(server.quantum_requests(), [256], "every client asks, with no comparison to an inactive read");
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(256));
    assert!(!opened.render.period_declined());
    backend.tick();
    assert!(errors(&opened).is_empty(), "the arrival in the window is no reset");
}

/// The window holds 25 steps of 20 ms: an arrival at the 25th is the baseline; one at the 26th
/// comes after readiness and is one reset, as any later change.
#[test]
fn an_arrival_just_before_the_window_ends_is_the_baseline_and_just_after_one_reset() {
    for (delay, baseline, resets) in [(25, 256, 0), (26, 1024, 1)] {
        let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
        server.script_pipewire(|m| m.force_delay = delay);
        let backend = backend_on(&server);
        let opened = open(&backend, Some(256));
        opened.start().unwrap();
        assert_eq!(opened.render.period(), Some(baseline), "arrival after {delay} steps");
        server.step(STEP);
        backend.tick();
        backend.tick();
        assert_eq!(errors(&opened), vec![ab::StreamError::ResetRequested; 2 * resets], "one reset (per direction) for a late arrival ({delay})");
    }
}

#[test]
fn a_request_that_never_arrives_leaves_the_running_size_and_reopens_do_not_loop() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.force_delay = 10_000);
    let backend = backend_on(&server);
    for open_number in 1..=3 {
        let opened = open(&backend, Some(256));
        opened.start().unwrap();
        assert_eq!(opened.render.period(), Some(1024), "the size that runs at the window's end");
        backend.tick();
        assert!(errors(&opened).is_empty(), "no reset, so no reopen ({open_number})");
    }
    assert_eq!(server.quantum_requests(), [256, 256, 256], "one request per client, also on a reopen");
    assert_eq!(session_opens(&server), 3, "only the opens the test made");
}

#[test]
fn a_declined_request_is_said_and_opens_no_window() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024)).refusing_quantum("jack_set_buffer_size returned 1");
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    assert!(opened.render.period_declined() && opened.capture.period_declined());
    let before = server.now_us();
    opened.start().unwrap();
    assert_eq!(server.now_us(), before, "no wait for a request that will not arrive");
    assert_eq!(opened.render.period(), Some(1024));
    drop(opened);
    // Control: an accepted request is not declined.
    let accepted = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    let opened = open(&backend_on(&accepted), Some(256));
    assert!(!opened.render.period_declined());
}

#[test]
fn a_competing_change_in_the_window_is_the_baseline_and_one_after_it_is_one_reset() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.force_delay = 10_000);
    // Another program forces 512 three steps into the window; transit's own request never arrives.
    server.schedule(3, Event::Quantum(512));
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(512), "the latest valid observation at the window's end");
    backend.tick();
    assert!(errors(&opened).is_empty());
    server.set_buffer_size(2048);
    backend.tick();
    backend.tick();
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2], "one reset after the window");
}

#[test]
fn a_callback_figure_published_during_the_read_is_kept_and_a_later_change_is_one_reset() {
    // The buffer size callback inside jack_activate reports 512; the getter afterwards reads 1024.
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.activation_buffer = Some(512));
    let backend = backend_on(&server);
    let opened = open(&backend, None);
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(512), "the callback's figure is not overwritten by the read");
    let shared = server.active_shared().expect("the active client's state");
    assert_eq!((shared.buffer_callbacks(), shared.buffer_changes()), (1, 0), "one callback inside jack_activate: the first figure, not a change");
    backend.tick();
    assert!(errors(&opened).is_empty());
    server.set_buffer_size(256);
    backend.tick();
    backend.tick();
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2], "a change after the read is one reset");
}

#[test]
fn a_rate_mismatch_latches_one_rebuild_before_any_audio_then_audio_runs() {
    // The listing reads the inactive default (48 kHz); the graph runs 44.1 kHz.
    let server = FakeServer::pipewire(44_100, 1024, (48_000, 1024));
    let backend = backend_on(&server);
    let first = open(&backend, None);
    assert_eq!(first.achieved.format.rate, 48_000, "prepared at the listing's rate");
    first.start().unwrap();
    assert!(first.render.needs_rebuild() && first.capture.needs_rebuild());
    assert_eq!(first.render.period(), None, "no committed buffer under the latch");
    let mut outputs = vec![vec![1.0f32; 1024]; 2];
    let inputs = vec![vec![0.25f32; 1024]; 2];
    assert!(server.cycle(1024, &mut outputs, &inputs));
    assert_eq!(first.calls.load(Ordering::SeqCst), 0, "neither user callback ran: capture not delivered");
    assert!(outputs.iter().flatten().all(|s| *s == 0.0), "the output carries silence");
    assert!(server.connects().is_empty(), "nothing connects under the latch");
    assert_eq!(backend.remembered_rate(), Some(44_100));
    drop(first);
    // The rebuild: planned at the remembered rate, it starts and plays.
    let second = open(&backend, None);
    assert_eq!(second.achieved.format.rate, 44_100, "plan, capture format and Achieved agree on the hint");
    second.start().unwrap();
    assert!(!second.render.needs_rebuild());
    assert_eq!(second.render.period(), Some(1024));
    let mut outputs = vec![vec![1.0f32; 1024]; 2];
    assert!(server.cycle(1024, &mut outputs, &inputs));
    assert_eq!(second.calls.load(Ordering::SeqCst), 2, "capture and render ran");
    assert!(outputs.iter().flatten().all(|s| *s == 0.5));
    assert_eq!(session_opens(&server), 2, "exactly one rebuild");
}

#[test]
fn the_remembered_rate_bounds_rebuilds_goes_stale_and_is_cleared_by_a_shutdown() {
    let server = FakeServer::pipewire(44_100, 1024, (48_000, 1024));
    let backend = backend_on(&server);
    let mut rebuilds = 0;
    let mut open_until_running = |expected: u32| {
        for _ in 0..3 {
            let opened = open(&backend, None);
            opened.start().unwrap();
            if !opened.render.needs_rebuild() {
                assert_eq!(opened.achieved.format.rate, expected);
                return opened;
            }
            rebuilds += 1;
        }
        panic!("rebuilt in a loop");
    };
    drop(open_until_running(44_100));
    drop(open_until_running(44_100));
    // The graph moves to 96 kHz while transit has no client: the hint is stale, one rebuild.
    server.set_sample_rate(96_000);
    let running = open_until_running(96_000);
    assert_eq!(rebuilds, 2, "one for the first open, one for the stale hint, none while stable");
    // A server shutdown clears the hint.
    server.shutdown();
    backend.tick();
    assert_eq!(backend.remembered_rate(), None);
    drop(running);
    // A fresh backend (a second instance) remembers nothing and plans at the listing's rate.
    server.restart();
    let fresh = backend_on(&server);
    let plan = fresh.plan_render(&ab::OutputWish { channels: 2, ..Default::default() }).unwrap();
    assert_eq!(plan.attempts[0].format.rate, 48_000);
}

#[test]
fn a_rate_change_in_the_window_ends_it_and_latches_the_rebuild() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.force_delay = 10);
    server.schedule(3, Event::Rate(96_000));
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    let before = server.now_us();
    opened.start().unwrap();
    assert_eq!(server.now_us() - before, 3 * 20_000, "the window ended at the rate change");
    assert!(opened.render.needs_rebuild());
    assert_eq!(opened.calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.remembered_rate(), Some(96_000));
}

#[test]
fn a_shutdown_in_the_window_aborts_the_start() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.script_pipewire(|m| m.force_delay = 10);
    server.schedule(3, Event::Shutdown);
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    let client = session_client(&server);
    assert_eq!(opened.capture.start(), Err(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())));
    assert_eq!(opened.render.period(), None);
    assert_eq!(server.clients(|c| c[client].deactivations), 1, "the client is deactivated again");
    assert_eq!(opened.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_stale_read_after_activation_costs_one_reset_and_first_cycle_init_none() {
    // The position right after activation is stale (256); the first cycle runs the graph's 1024.
    for first_cycle in [false, true] {
        let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
        server.script_pipewire(|m| {
            m.stale = Some((48_000, 256));
            m.activation_buffer = Some(256);
        });
        server.schedule(2, Event::Cycle(1024));
        let backend = if first_cycle { backend_on(&server).with_first_cycle_init() } else { backend_on(&server) };
        let opened = open(&backend, None);
        opened.start().unwrap();
        assert_eq!(opened.calls.load(Ordering::SeqCst), 0, "a cycle before the start finished reaches no user callback");
        if first_cycle {
            assert_eq!(opened.render.period(), Some(1024), "the first cycle's nframes");
        } else {
            assert_eq!(opened.render.period(), Some(256), "the stale read");
            server.step(STEP);
            server.step(STEP);
        }
        backend.tick();
        backend.tick();
        let resets = if first_cycle { 0 } else { 2 };
        assert_eq!(errors(&opened).len(), resets, "first-cycle init: {first_cycle}");
    }
}

#[test]
fn first_cycle_init_fails_the_start_visibly_without_a_cycle() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    let backend = backend_on(&server).with_first_cycle_init();
    let opened = open(&backend, None);
    let error = opened.capture.start().unwrap_err();
    assert_eq!(error, ab::OpenError::Other("JACK: no process cycle arrived within 500 ms of activation".into()));
    assert_eq!(opened.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn jack2_keeps_its_request_and_reads_and_a_rate_change_at_activation_rebuilds() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend_on(&server);
    let opened = open(&backend, Some(256));
    assert_eq!(server.quantum_requests(), [256], "asked once, as before");
    assert!(server.inactive_reads(session_client(&server)) > 0, "JACK 2 reads the inactive client, as before");
    assert_eq!(opened.achieved.period, ab::Period::Frames(256), "the server applied it at once");
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(256));
    assert!(!opened.render.needs_rebuild());
    drop(opened);
    // The rate changes between the open and the start: the active client's rate takes the
    // mismatch path.
    let opened = open(&backend, None);
    server.set_sample_rate(44_100);
    opened.start().unwrap();
    assert!(opened.render.needs_rebuild());
    assert_eq!(backend.remembered_rate(), None, "JACK 2 keeps no hint");
}
