//! #760, the report from a real PipeWire desktop (2026-11-21): every fresh client read one buffer
//! size right after activation and through the start, then ran another once its ports were
//! linked; the watcher took that as a change and reopened, and the reopened client read the first
//! figure again, without end. Over the fake's PipeWire mode with a driver running the client every
//! graph step; the watcher is driven by hand and time steps on the fake's clock.

use crate::JackBackend;
use crate::api::JackApi;
use crate::fake::FakeServer;
use crate::settle_tests::{Opened, STEP, backend_on, errors, open, session_opens};
use std::sync::{Arc, Mutex};
use transit_audio_backend::{self as ab, Backend, Stream};

/// The Player's automatic reopen allows this many attempts in a row (`transit-playback`'s
/// `REOPEN_ATTEMPTS`); this crate does not depend on it, so the figure is restated.
const REOPEN_ATTEMPTS: usize = 3;

/// Graph steps (with a watcher tick after each) one open runs before the test looks again.
const RUN_STEPS: usize = 12;

/// The tester's graph: 48 kHz, the server's default quantum 1024 (what an inactive client and the
/// probe read), a driver that runs the active client every graph step.
fn reported_desktop() -> FakeServer {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.driving();
    server
}

/// What the Player's reopen path does, as far as the backend sees it: open, start, run the graph;
/// a reset closes that open and the next one starts, at most [`REOPEN_ATTEMPTS`] times. Returns the
/// reset requests seen (one per open that asked, whichever direction) and the open still running,
/// if one is.
fn run_like_the_player(backend: &JackBackend, server: &FakeServer) -> (usize, Option<Opened>) {
    let mut resets = 0;
    for _ in 0..=REOPEN_ATTEMPTS {
        let opened = open(backend, None);
        opened.start().expect("the start");
        for _ in 0..RUN_STEPS {
            server.step(STEP);
            backend.tick();
        }
        if errors(&opened).contains(&ab::StreamError::ResetRequested) {
            resets += 1;
            continue;
        }
        return (resets, Some(opened));
    }
    (resets, None)
}

#[test]
fn a_stale_figure_after_activation_corrected_at_the_first_cycle_does_not_reopen() {
    // Every activation reads 2048 (the buffer size callback inside jack_activate too); the first
    // process cycle corrects it to the graph's 1024.
    let server = reported_desktop();
    server.script_pipewire(|m| m.stale_each = Some((48_000, 2048)));
    let backend = backend_on(&server);
    let (resets, running) = run_like_the_player(&backend, &server);
    assert_eq!(resets, 0, "no reset: the start settles on what the graph runs");
    let running = running.expect("the device stays open");
    assert_eq!(running.render.period(), Some(1024));
    assert_eq!(session_opens(&server), 1);
}

#[test]
fn a_graph_that_moves_a_few_cycles_in_reopens_once_then_stays_open() {
    // Every activation runs 2048 for its first cycles, then 1024 three graph steps in.
    let server = reported_desktop();
    server.script_pipewire(|m| m.moves_each = Some((2048, 3, 1024)));
    let backend = backend_on(&server);
    let (resets, running) = run_like_the_player(&backend, &server);
    // The first open settles on the first cycle after its links (2048); the move to 1024 is one
    // reset, and the reopen's window waits for the 1024 it remembers.
    assert_eq!(resets, 1, "one reopen, then the graph's size is remembered");
    assert!(resets < REOPEN_ATTEMPTS, "the attempts are not used up");
    let running = running.expect("the device stays open");
    assert_eq!(running.render.period(), Some(1024));
    assert!(errors(&running).is_empty());
}

/// Opens both directions with callbacks that record what they saw: every captured sample of the
/// first input, the capture block lengths, and, at each render, how many samples capture had
/// delivered by then (capture must precede render in each cycle).
struct Recording {
    capture: Box<dyn Stream>,
    render: Box<dyn Stream>,
    errors: crate::settle_tests::Errors,
    captured: Arc<Mutex<Vec<f32>>>,
    blocks: Arc<Mutex<Vec<usize>>>,
    seen_at_render: Arc<Mutex<Vec<usize>>>,
}

fn open_recording(backend: &JackBackend) -> Recording {
    let plan = backend.plan_render(&ab::OutputWish { channels: 2, ..Default::default() }).expect("render plan");
    let request = plan.attempts[0];
    let format = backend.capture_format(&plan.endpoint, Some(request.format.rate)).expect("capture format");
    let errors: crate::settle_tests::Errors = Arc::default();
    let (captured, blocks, seen_at_render) =
        (Arc::<Mutex<Vec<f32>>>::default(), Arc::<Mutex<Vec<usize>>>::default(), Arc::<Mutex<Vec<usize>>>::default());
    let (into, lengths) = (Arc::clone(&captured), Arc::clone(&blocks));
    let capture_cb: ab::CaptureCallback = Box::new(move |block| {
        lengths.lock().unwrap().push(block.frames);
        let channels = usize::from(block.channels);
        block.chunks(&mut |_, chunk| into.lock().unwrap().extend(chunk.iter().step_by(channels)));
    });
    let (delivered, at_render) = (Arc::clone(&captured), Arc::clone(&seen_at_render));
    let render_cb: ab::RenderCallback = Box::new(move |block| {
        at_render.lock().unwrap().push(delivered.lock().unwrap().len());
        block.render(&mut |chunk| chunk.fill(0.5));
    });
    let on_error = |errors: &crate::settle_tests::Errors| -> ab::ErrorCallback {
        let errors = Arc::clone(errors);
        Box::new(move |e| errors.lock().unwrap().push(e))
    };
    let (capture, _) = backend.open_capture(&plan.endpoint, &ab::OpenRequest { format, ..request }, capture_cb, on_error(&errors)).expect("capture");
    let (render, _) = backend.open_render(&plan.endpoint, &request, render_cb, on_error(&errors)).expect("render");
    Recording { capture, render, errors, captured, blocks, seen_at_render }
}

#[test]
fn audio_after_the_startup_change_runs_at_the_graphs_size_with_capture_aligned() {
    let server = reported_desktop();
    server.script_pipewire(|m| m.stale_each = Some((48_000, 2048)));
    let backend = backend_on(&server);
    let opened = open_recording(&backend);
    opened.capture.start().unwrap();
    opened.render.start().unwrap();
    assert_eq!(opened.render.period(), Some(1024), "settled past the stale 2048");
    // Eight cycles of the graph's 1024 frames, the first input a ramp across them.
    let frames = 1024usize;
    let mut expected = Vec::new();
    for cycle in 0..8usize {
        let ramp: Vec<f32> = (0..frames).map(|i| (cycle * frames + i) as f32).collect();
        expected.extend_from_slice(&ramp);
        let inputs = vec![ramp, vec![-1.0; frames]];
        let mut outputs = vec![vec![9.0f32; frames]; 2];
        assert!(server.cycle(1024, &mut outputs, &inputs));
        assert!(outputs.iter().flatten().all(|s| *s == 0.5), "every output sample of cycle {cycle} is the render's");
        backend.tick();
    }
    assert_eq!(*opened.captured.lock().unwrap(), expected, "every captured sample, in order, none lost or repeated");
    assert_eq!(*opened.blocks.lock().unwrap(), vec![1024; 8], "one capture block per cycle at the graph's size");
    let at_render: Vec<usize> = (1..=8).map(|n| n * frames).collect();
    assert_eq!(*opened.seen_at_render.lock().unwrap(), at_render, "each cycle's capture is delivered before its render");
    assert!(opened.errors.lock().unwrap().is_empty(), "no reset, no fault");
    assert_eq!(session_opens(&server), 1, "no reopen");
}

#[test]
fn a_rate_change_after_the_start_still_reopens_at_the_new_rate() {
    let server = reported_desktop();
    let backend = backend_on(&server);
    let opened = open(&backend, None);
    opened.start().unwrap();
    server.set_sample_rate(44_100);
    backend.tick();
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2], "one reset, to both directions");
    assert_eq!(backend.remembered_rate(), Some(44_100));
    drop(opened);
    let reopened = open(&backend, None);
    assert_eq!(reopened.achieved.format.rate, 44_100, "the reopen plans at the new rate");
    reopened.start().unwrap();
    assert!(!reopened.render.needs_rebuild());
}

#[test]
fn the_period_hint_is_kept_from_what_ran_goes_stale_harmlessly_and_a_shutdown_clears_it() {
    let server = reported_desktop();
    let backend = backend_on(&server);
    assert_eq!(backend.remembered_period(), None);
    let opened = open(&backend, None);
    opened.start().unwrap();
    assert_eq!(backend.remembered_period(), Some(1024), "the settled size");
    // A change after the start is one reset, as before, and the size it found is remembered.
    server.set_buffer_size(512);
    backend.tick();
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2]);
    assert_eq!(backend.remembered_period(), Some(512));
    drop(opened);
    // The graph moved back to 1024 while transit had no client: the window waits for 512, finds
    // 1024 at its end, and that is the baseline; no reset follows.
    server.set_buffer_size(1024);
    let before = server.now_us();
    let opened = open(&backend, None);
    opened.start().unwrap();
    // A driven step is the wait (20 ms) and one cycle of 1024 frames (21.333 ms) on the fake clock.
    let (elapsed, step) = (server.now_us() - before, 20_000 + 1024 * 1_000_000 / 48_000);
    assert!((500_000..500_000 + step).contains(&elapsed), "the whole window, once ({elapsed} us)");
    assert_eq!(opened.render.period(), Some(1024));
    server.step(STEP);
    backend.tick();
    assert!(errors(&opened).is_empty());
    assert_eq!(backend.remembered_period(), Some(1024));
    server.shutdown();
    backend.tick();
    assert_eq!(backend.remembered_period(), None, "no hint survives a server shutdown");
}

#[test]
fn jack2_a_server_wide_buffer_change_reopens_once_and_runs_at_the_new_size() {
    let server = FakeServer::jack2(48_000, 256);
    server.driving();
    let backend = backend_on(&server);
    let opened = open(&backend, None);
    let before = server.now_us();
    opened.start().unwrap();
    assert_eq!(server.now_us(), before, "JACK 2 has no settling window");
    assert_eq!(opened.render.period(), Some(256));
    // Another client changes the server's buffer size for every client.
    server.set_buffer_size(128);
    backend.tick();
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2], "the reopen at the server's new size, as before");
    assert_eq!(backend.remembered_period(), None, "JACK 2 keeps no hint");
    drop(opened);
    let reopened = open(&backend, None);
    reopened.start().unwrap();
    assert_eq!(reopened.render.period(), Some(128));
    server.step(STEP);
    backend.tick();
    assert!(errors(&reopened).is_empty(), "running at the new size");
}
