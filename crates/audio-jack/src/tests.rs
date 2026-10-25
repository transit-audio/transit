//! Fake-server tests (ADR 0021): enumeration states, the client lifecycle, the achieved rate and
//! quantum, the duplex engine (order, conversion, chunking, stamps, the position), the watcher's
//! routing of shutdown, reconnect, rate and quantum change, panics and xruns, and an engine that
//! allocates nothing.

use crate::engine::{self, Engine, EngineSide, Shared};
use crate::fake::{FakeServer, InVecPlanes, VecPlanes};
use crate::{JackBackend, JackDevice, ServerState};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_audio_backend::{self as ab, Backend, Quality, SCRATCH_FRAMES, Stamp, Stream};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn backend(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(Duration::from_millis(2), Duration::from_millis(5), Duration::from_secs(2))
}

/// Waits up to two seconds for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

type Errors = Arc<Mutex<Vec<ab::StreamError>>>;

fn recorder() -> (Errors, ab::ErrorCallback) {
    let errors: Errors = Arc::default();
    let sink = Arc::clone(&errors);
    (errors, Box::new(move |e| sink.lock().unwrap().push(e)))
}

fn render_callback() -> ab::RenderCallback {
    Box::new(|_| {})
}

fn capture_callback() -> ab::CaptureCallback {
    Box::new(|_| {})
}

/// What the test callbacks saw: (callback, order index, frames, stamp, position) per block, and
/// the captured samples.
#[derive(Default)]
struct Seen {
    order: AtomicU64,
    blocks: Mutex<Vec<(&'static str, u64, usize, Stamp, Option<u64>)>>,
    captured: Mutex<Vec<(usize, Vec<f32>)>>,
}

/// A render callback that writes `value(frame, channel)` and a capture callback that keeps what it
/// read, both recording into `seen`.
fn recording(seen: &Arc<Seen>, value: fn(usize, usize) -> f32) -> (ab::RenderCallback, ab::CaptureCallback) {
    let (r, c) = (Arc::clone(seen), Arc::clone(seen));
    let render: ab::RenderCallback = Box::new(move |block| {
        let n = r.order.fetch_add(1, Ordering::SeqCst);
        r.blocks.lock().unwrap().push(("render", n, block.frames, block.stamp, block.device_position));
        let channels = usize::from(block.channels);
        let mut at = 0;
        block.render(&mut |chunk| {
            for (i, s) in chunk.iter_mut().enumerate() {
                *s = value(at + i / channels, i % channels);
            }
            at += chunk.len() / channels;
        });
    });
    let capture: ab::CaptureCallback = Box::new(move |block| {
        let n = c.order.fetch_add(1, Ordering::SeqCst);
        c.blocks.lock().unwrap().push(("capture", n, block.frames, block.stamp, block.device_position));
        block.chunks(&mut |offset, chunk| c.captured.lock().unwrap().push((offset, chunk.to_vec())));
    });
    (render, capture)
}

/// Opens capture then render on the first group (the Player's duplex order) with the given user
/// callbacks, each with an error recorder.
fn open_duplex_with(
    backend: &JackBackend,
    render: ab::RenderCallback,
    capture: ab::CaptureCallback,
) -> (Box<dyn Stream>, Box<dyn Stream>, Errors, Errors) {
    let render_plan = backend.plan_render(&ab::OutputWish::default()).expect("render plan");
    assert!(backend.capture_with_render(&render_plan.endpoint));
    let capture_request =
        ab::OpenRequest { format: backend.capture_format(&render_plan.endpoint, None).expect("capture format"), ..render_plan.attempts[0] };
    let (capture_errors, on_capture) = recorder();
    let (capture, _) = backend.open_capture(&render_plan.endpoint, &capture_request, capture, on_capture).expect("capture");
    let (render_errors, on_render) = recorder();
    let (render, _) = backend.open_render(&render_plan.endpoint, &render_plan.attempts[0], render, on_render).expect("render");
    (capture, render, capture_errors, render_errors)
}

fn open_duplex(backend: &JackBackend) -> (Box<dyn Stream>, Box<dyn Stream>, Errors, Errors) {
    open_duplex_with(backend, render_callback(), capture_callback())
}

#[test]
fn a_missing_library_is_reported_with_the_install_hint() {
    let backend = backend(&FakeServer::missing_library());
    let state = backend.probe();
    assert!(matches!(state, ServerState::LibraryMissing(_)), "{state:?}");
    assert_eq!(state.diagnostic().as_deref(), Some(crate::LIBRARY_MISSING));
    let error = backend.endpoints(ab::Direction::Output).unwrap_err();
    assert_eq!(error, ab::OpenError::DeviceNotAvailable(crate::LIBRARY_MISSING.into()));
    assert!(crate::LIBRARY_MISSING.contains("pipewire-jack") && crate::LIBRARY_MISSING.contains("pw-jack"));
    assert_eq!(backend.plan_render(&ab::OutputWish::default()).unwrap_err(), crate::LIBRARY_MISSING);
}

#[test]
fn no_server_is_reported_and_none_is_started() {
    let server = FakeServer::not_running();
    let backend = backend(&server);
    assert_eq!(backend.probe(), ServerState::NotRunning);
    assert_eq!(backend.probe().diagnostic().as_deref(), Some(crate::SERVER_NOT_RUNNING));
    assert_eq!(backend.endpoints(ab::Direction::Input).unwrap_err(), ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into()));
    assert_eq!(server.clients(<[_]>::len), 0, "no client exists without a server");
}

#[test]
fn a_refused_probe_client_says_why() {
    let backend = backend(&FakeServer::refusing("jack_client_open failed (status 0x21)"));
    let state = backend.probe();
    assert_eq!(state, ServerState::Failed("jack_client_open failed (status 0x21)".into()));
    assert_eq!(state.diagnostic().as_deref(), Some("JACK: jack_client_open failed (status 0x21)"));
}

#[test]
fn jack2_lists_the_system_group_with_its_ports_as_channels() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend(&server);
    let ServerState::Running(found) = backend.probe() else { panic!("not running") };
    assert_eq!((found.rate, found.buffer, found.devices.len()), (48_000, 128, 1));
    let outputs = backend.endpoints(ab::Direction::Output).unwrap();
    assert_eq!(outputs.len(), 1);
    let system = &outputs[0];
    assert_eq!((system.id.as_str(), system.name.as_str(), system.system.as_str()), ("system", "system", "JACK"));
    assert_eq!((system.inputs, system.outputs), (2, 2));
    let format = &system.formats[0];
    assert_eq!(
        (format.sample, format.channels, format.min_rate, format.max_rate, format.period),
        (ab::SampleFormat::F32, 2, 48_000, 48_000, Some((64, 2048)))
    );
    assert_eq!(backend.channel_names(system, ab::Direction::Input), ["capture_1", "capture_2"]);
    assert_eq!(backend.channel_names(system, ab::Direction::Output), ["playback_1", "playback_2"]);
    assert_eq!(server.open_clients(), 0, "the probe client is closed again");
    assert_eq!(server.clients(|c| c[0].name.clone()), crate::PROBE_NAME);
    assert!(backend.capture_precedes_render(system), "one process callback: capture, then render (K22)");
}

#[test]
fn pipewire_lists_one_entry_per_device_node() {
    let server = FakeServer::running(
        48_000,
        256,
        &[
            ("Scarlett 2i2 USB Pro:playback_AUX0", false),
            ("Scarlett 2i2 USB Pro:playback_AUX1", false),
            ("Built-in Audio Analog Stereo:playback_FL", false),
            ("Built-in Audio Analog Stereo:playback_FR", false),
            ("Scarlett 2i2 USB Pro:capture_AUX0", true),
            ("Scarlett 2i2 USB Pro:capture_AUX1", true),
        ],
    );
    let backend = backend(&server);
    let names = |direction| backend.endpoints(direction).unwrap().into_iter().map(|e| (e.name, e.inputs, e.outputs)).collect::<Vec<_>>();
    assert_eq!(
        names(ab::Direction::Output),
        [("Scarlett 2i2 USB Pro".to_string(), 2, 2), ("Built-in Audio Analog Stereo".to_string(), 0, 2)],
        "both nodes play, in the order their first port appeared"
    );
    assert_eq!(names(ab::Direction::Input), [("Scarlett 2i2 USB Pro".to_string(), 2, 2)], "only the Scarlett records");
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Built-in Audio Analog Stereo".into()), ..Default::default() }).unwrap();
    let device = plan.endpoint.handle::<JackDevice>().unwrap();
    assert_eq!(device.playback, ["Built-in Audio Analog Stereo:playback_FL", "Built-in Audio Analog Stereo:playback_FR"]);
    assert!(backend.plan_capture(&ab::InputWish { device: Some("Built-in Audio Analog Stereo".into()), ..Default::default() }).is_err());
    assert!(backend.plan_render(&ab::OutputWish { device: Some("USB".into()), ..Default::default() }).unwrap_err().contains("no port group"));
}

#[test]
fn the_server_is_named_from_the_library_version() {
    assert_eq!(crate::server_name(Some("1.9.20")), "JACK 1.9.20");
    assert_eq!(crate::server_name(Some("3.0.0.0 (using PipeWire 1.0.5)")), "PipeWire 1.0.5");
    assert_eq!(crate::server_name(Some("pipewire")), "PipeWire");
    assert_eq!(crate::server_name(None), "JACK");
    assert_eq!(crate::server_name(Some("  ")), "JACK");
    let server = FakeServer::jack2(48_000, 128);
    assert_eq!(backend(&server).label(), "Native JACK (JACK)");
    server.set_version("1.9.20");
    assert_eq!(backend(&server).label(), "Native JACK (JACK 1.9.20)");
    for (value, kept) in [(None, true), (Some("1"), true), (Some("0"), false), (Some(" Off "), false), (Some("false"), false), (Some("no"), false)] {
        assert_eq!(crate::auto_connect_from(value), kept, "{value:?}");
    }
}

#[test]
fn a_requested_buffer_size_is_asked_of_the_server_and_achieved_follows_it() {
    let server = FakeServer::jack2(48_000, 64);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish { rates: vec![44_100], channels: 2, period: Some(128), ..Default::default() }).unwrap();
    assert_eq!(plan.attempts.len(), 1);
    assert_eq!(plan.attempts[0].period, ab::Period::Frames(128));
    let asked = ab::OpenRequest { format: ab::Format::new(44_100, 2, ab::SampleFormat::F32), ..plan.attempts[0] };
    let (errors, on_error) = recorder();
    let (stream, achieved) = backend.open_render(&plan.endpoint, &asked, render_callback(), on_error).unwrap();
    assert_eq!(achieved.format, ab::Format::new(48_000, 2, ab::SampleFormat::F32), "the server's rate, not 44.1 kHz");
    assert_eq!(server.quantum_requests(), [128], "the backend asked the server once");
    assert_eq!(achieved.period, ab::Period::Frames(128), "the server's new quantum");
    assert_eq!(achieved.buffer_frames, Some(128));
    assert_eq!(stream.thread_priority(), ab::WorkerPriority::Unobserved);
    drop(stream);
    assert!(errors.lock().unwrap().is_empty());
}

#[test]
fn a_quantum_the_server_already_runs_or_none_is_not_asked_for() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend(&server);
    for period in [Some(128), None] {
        let plan = backend.plan_render(&ab::OutputWish { channels: 2, period, ..Default::default() }).unwrap();
        let (_errors, on_error) = recorder();
        let (stream, achieved) = backend.open_render(&plan.endpoint, &plan.attempts[0], render_callback(), on_error).unwrap();
        assert_eq!(achieved.period, ab::Period::Frames(128));
        drop(stream);
    }
    assert!(server.quantum_requests().is_empty());
}

#[test]
fn a_server_that_refuses_the_quantum_still_opens_and_achieved_shows_the_mismatch() {
    let server = FakeServer::jack2(48_000, 64).refusing_quantum("the server will not change its buffer size");
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish { channels: 2, period: Some(128), ..Default::default() }).unwrap();
    let (errors, on_error) = recorder();
    let (stream, achieved) = backend.open_render(&plan.endpoint, &plan.attempts[0], render_callback(), on_error).unwrap();
    assert_eq!(server.quantum_requests(), [128]);
    assert_eq!(achieved.period, ab::Period::Frames(64), "the server's own quantum");
    assert_ne!(Some(128), achieved.period.frames(), "the request was not honored");
    drop(stream);
    assert!(errors.lock().unwrap().is_empty());
}

#[test]
fn the_buffer_list_offers_the_standard_sizes_around_the_running_quantum() {
    let server = FakeServer::jack2(48_000, 64);
    let jack = backend(&server);
    let endpoints = jack.endpoints(ab::Direction::Output).unwrap();
    assert_eq!(endpoints[0].formats[0].period, Some((64, 2048)));
    let odd = FakeServer::jack2(48_000, 4096);
    let endpoints = backend(&odd).endpoints(ab::Direction::Output).unwrap();
    assert_eq!(endpoints[0].formats[0].period, Some((64, 4096)), "the running size is always offered");
}

#[test]
fn duplex_shares_one_client_with_in_and_out_ports_connected_at_start() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let (capture, render, capture_errors, render_errors) = open_duplex(&backend);
    assert_eq!(server.open_clients(), 1, "both directions on one client");
    assert_eq!(backend.client_name().as_deref(), Some("transit"));
    let ports = server.clients(|c| c.last().unwrap().ports.clone());
    assert_eq!(
        ports,
        [
            ("transit:in_1".to_string(), true),
            ("transit:in_2".to_string(), true),
            ("transit:out_1".to_string(), false),
            ("transit:out_2".to_string(), false)
        ]
    );
    assert!(!server.clients(|c| c.last().unwrap().active), "nothing runs before a start");

    render.start().unwrap();
    let after_render = server.clients(|c| (c.last().unwrap().activations, c.last().unwrap().connections.clone()));
    assert_eq!(
        after_render,
        (1, vec![("transit:out_1".to_string(), "system:playback_1".to_string()), ("transit:out_2".to_string(), "system:playback_2".to_string())])
    );
    capture.start().unwrap();
    let connections = server.clients(|c| c.last().unwrap().connections.clone());
    assert_eq!(connections.len(), 4);
    assert!(connections.contains(&("system:capture_2".to_string(), "transit:in_2".to_string())));
    assert_eq!(server.clients(|c| c.last().unwrap().activations), 1, "activated once for both directions");
    assert_eq!(backend.with_client(|c| c.connections("transit:in_1")), Some(vec!["system:capture_1".to_string()]));

    let mut outputs = vec![vec![1.0f32; 256]; 2];
    assert!(server.cycle(256, &mut outputs, &[vec![0.5; 256], vec![0.5; 256]]));
    assert!(outputs.iter().all(|p| p.iter().all(|s| *s == 0.0)), "a render callback that writes nothing plays silence");
    assert_eq!((render.stats().unwrap().wakes, capture.stats().unwrap().wakes), (1, 1));

    drop(capture);
    assert_eq!(server.open_clients(), 1, "the render side keeps the client");
    drop(render);
    assert_eq!(server.open_clients(), 0, "the last stream closes the client");
    let closed = server.clients(|c| (c.last().unwrap().deactivations, c.last().unwrap().closed, c.last().unwrap().engine.is_none()));
    assert_eq!(closed, (1, true, true), "deactivated, closed, and the engine freed with the client");
    assert!(!backend.has_session());
    assert!(capture_errors.lock().unwrap().is_empty() && render_errors.lock().unwrap().is_empty());
}

#[test]
fn without_auto_connect_the_ports_are_left_unconnected() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server).without_auto_connect();
    let (capture, render, _, _) = open_duplex(&backend);
    render.start().unwrap();
    capture.start().unwrap();
    assert!(server.clients(|c| c.last().unwrap().connections.is_empty()));
    backend.with_client(|c| c.connect("transit:out_1", "transit:in_1")).unwrap().unwrap();
    assert_eq!(backend.with_client(|c| c.connections("transit:out_1")), Some(vec!["transit:in_1".to_string()]));
}

#[test]
fn one_cycle_runs_capture_then_render_and_converts_both_ways() {
    let server = FakeServer::jack2(48_000, 64);
    let backend = backend(&server);
    let seen = Arc::new(Seen::default());
    let (render_cb, capture_cb) = recording(&seen, |frame, channel| (frame * 10 + channel) as f32);
    let (capture, render, _, _) = open_duplex_with(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    let inputs = vec![(0..64).map(|i| i as f32).collect::<Vec<f32>>(), (0..64).map(|i| -(i as f32)).collect()];
    let mut outputs = vec![vec![9.0f32; 64]; 2];
    assert!(server.cycle(64, &mut outputs, &inputs));
    let blocks = seen.blocks.lock().unwrap().clone();
    assert_eq!(blocks.iter().map(|b| (b.0, b.1, b.2)).collect::<Vec<_>>(), [("capture", 0, 64), ("render", 1, 64)], "capture first, same cycle");
    assert_eq!(blocks[0].4, blocks[1].4, "both directions carry the same device position");
    let captured = seen.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, 0);
    let expected: Vec<f32> = (0..64).flat_map(|i| [i as f32, -(i as f32)]).collect();
    assert_eq!(captured[0].1, expected, "the input ports interleaved, frame by frame");
    assert_eq!(outputs[0], (0..64).map(|f| (f * 10) as f32).collect::<Vec<_>>(), "out_1 gets channel 0");
    assert_eq!(outputs[1], (0..64).map(|f| (f * 10 + 1) as f32).collect::<Vec<_>>(), "out_2 gets channel 1");
}

#[test]
fn a_block_larger_than_the_scratch_is_served_in_chunks() {
    let server = FakeServer::jack2(48_000, 64);
    let backend = backend(&server);
    let seen = Arc::new(Seen::default());
    let (render_cb, capture_cb) = recording(&seen, |frame, _| frame as f32);
    let (capture, render, _, _) = open_duplex_with(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    let frames = SCRATCH_FRAMES + 100;
    let inputs = vec![(0..frames).map(|i| i as f32).collect::<Vec<f32>>(); 2];
    let mut outputs = vec![vec![0.0f32; frames]; 2];
    assert!(server.cycle(u32::try_from(frames).unwrap(), &mut outputs, &inputs));
    let captured = seen.captured.lock().unwrap().clone();
    assert_eq!(captured.iter().map(|(o, c)| (*o, c.len())).collect::<Vec<_>>(), [(0, SCRATCH_FRAMES * 2), (SCRATCH_FRAMES, 200)]);
    assert_eq!(captured[1].1[0], SCRATCH_FRAMES as f32, "the second chunk starts where the first ended");
    assert!(outputs[0].iter().enumerate().all(|(i, s)| *s == i as f32), "every frame of every chunk rendered in place");
}

#[test]
fn stamps_follow_the_cycle_clock_and_the_position_unwraps() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend(&server);
    let seen = Arc::new(Seen::default());
    let (render_cb, capture_cb) = recording(&seen, |_, _| 0.0);
    let (capture, render, _, _) = open_duplex_with(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    server.set_frame_counter(u32::MAX - 127 - 128);
    let mut outputs = vec![vec![0.0f32; 128]; 2];
    let inputs = vec![vec![0.0f32; 128]; 2];
    for _ in 0..4 {
        assert!(server.cycle_at(128, 300, &mut outputs, &inputs));
    }
    let renders: Vec<_> = seen.blocks.lock().unwrap().iter().filter(|b| b.0 == "render").cloned().collect();
    let positions: Vec<u64> = renders.iter().map(|b| b.4.unwrap()).collect();
    let first = u64::from(u32::MAX - 255);
    assert_eq!(positions, [first, first + 128, first + 256, first + 384], "the 32-bit counter unwraps across its wrap");
    for (_, _, _, stamp, _) in &renders {
        assert_eq!(stamp.quality, Quality::BackendEstimate);
        let offset = i64::try_from(stamp.host_callback_ns).unwrap() - stamp.backend_callback_ns;
        assert!((300_000..300_000 + 1_000).contains(&offset), "the cycle started 300 µs before the arrival: {offset}");
        assert_eq!(stamp.backend_frame_ns, stamp.backend_callback_ns, "slice 2: no port latency yet");
    }
    assert!(renders.windows(2).all(|w| w[1].3.backend_callback_ns >= w[0].3.backend_callback_ns), "never backward");
    assert_eq!(render.stats().unwrap().position_jumps, 0);
}

#[test]
fn a_jump_an_xrun_and_a_quantum_change_each_invalidate_exactly_the_next_stamp() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend(&server);
    let seen = Arc::new(Seen::default());
    let (render_cb, capture_cb) = recording(&seen, |_, _| 0.0);
    let (capture, render, _, _) = open_duplex_with(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    let mut outputs = vec![vec![0.0f32; 256]; 2];
    let inputs = vec![vec![0.0f32; 256]; 2];
    let mut cycle = |frames| assert!(server.cycle(frames, &mut outputs, &inputs));
    cycle(128);
    cycle(128);
    server.set_frame_counter(1_000_000); // a skipped stretch
    cycle(128);
    cycle(128);
    server.xrun();
    cycle(128);
    cycle(128);
    cycle(256); // a new quantum
    cycle(256);
    let qualities: Vec<Quality> = seen.blocks.lock().unwrap().iter().filter(|b| b.0 == "capture").map(|b| b.3.quality).collect();
    use Quality::{BackendEstimate as Ok, Invalid};
    assert_eq!(qualities, [Ok, Ok, Invalid, Ok, Invalid, Ok, Invalid, Ok]);
    assert_eq!(render.stats().unwrap().position_jumps, 1, "the skipped stretch; a new quantum is not a jump");
    assert_eq!(capture.stats().unwrap().overloads, 1);
}

#[test]
fn a_stopped_direction_is_not_called_and_the_other_keeps_running() {
    let server = FakeServer::jack2(48_000, 64);
    let backend = backend(&server);
    let seen = Arc::new(Seen::default());
    let (render_cb, capture_cb) = recording(&seen, |_, _| 0.5);
    let (capture, render, _, _) = open_duplex_with(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    let mut outputs = vec![vec![0.0f32; 64]; 2];
    let inputs = vec![vec![0.0f32; 64]; 2];
    assert!(server.cycle(64, &mut outputs, &inputs));
    capture.stop().unwrap();
    assert!(server.cycle(64, &mut outputs, &inputs));
    let names: Vec<&str> = seen.blocks.lock().unwrap().iter().map(|b| b.0).collect();
    assert_eq!(names, ["capture", "render", "render"]);
    assert!(outputs[0].iter().all(|s| *s == 0.5), "render still plays");
    render.stop().unwrap();
    assert!(!server.cycle(64, &mut outputs, &inputs), "no direction runs: the client is deactivated");
}

#[test]
fn a_panicking_user_callback_is_silenced_and_reported_once() {
    let server = FakeServer::jack2(48_000, 64);
    let backend = backend(&server);
    let calls = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&calls);
    let render_cb: ab::RenderCallback = Box::new(move |block| {
        counter.fetch_add(1, Ordering::SeqCst);
        block.render(&mut |chunk| chunk.fill(1.0));
        panic!("a bug in the render callback");
    });
    let (capture, render, capture_errors, render_errors) = open_duplex_with(&backend, render_cb, capture_callback());
    render.start().unwrap();
    capture.start().unwrap();
    let mut outputs = vec![vec![0.0f32; 64]; 2];
    let inputs = vec![vec![0.0f32; 64]; 2];
    for _ in 0..3 {
        assert!(server.cycle(64, &mut outputs, &inputs));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "never called again");
    assert!(outputs.iter().all(|p| p.iter().all(|s| *s == 0.0)), "silence after the panic");
    wait_for("the panic report", || !render_errors.lock().unwrap().is_empty());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::Other]);
    assert!(capture_errors.lock().unwrap().is_empty(), "the capture side is unaffected");
    assert_eq!(render.stats().unwrap().faults, 1);
}

#[test]
fn an_open_after_a_start_or_a_second_open_of_one_direction_is_refused() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], render_callback(), Box::new(|_| {})).unwrap();
    let again = backend.open_render(&plan.endpoint, &plan.attempts[0], render_callback(), Box::new(|_| {}));
    assert!(matches!(again, Err(ab::OpenError::InUse(_))));
    render.start().unwrap();
    let late = backend.open_capture(&plan.endpoint, &plan.attempts[0], capture_callback(), Box::new(|_| {}));
    assert!(matches!(late, Err(ab::OpenError::Other(ref text)) if text.contains("before starting")), "{:?}", late.err());
    render.stop().unwrap();
    assert_eq!(server.clients(|c| c.last().unwrap().deactivations), 1, "stopping the only direction deactivates");
    assert!(render.start().is_err(), "a stopped stream is reopened, not restarted");
    drop(render);
    assert_eq!(server.open_clients(), 0);
}

#[test]
fn an_open_without_a_server_fails_and_leaves_no_client() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    server.shutdown();
    let opened = backend.open_render(&plan.endpoint, &plan.attempts[0], render_callback(), Box::new(|_| {}));
    assert_eq!(opened.err(), Some(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())));
    assert!(!backend.has_session());
}

#[test]
fn a_server_shutdown_is_reported_once_and_the_client_is_closed_on_the_watcher() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let (capture, render, capture_errors, render_errors) = open_duplex(&backend);
    capture.start().unwrap();
    render.start().unwrap();
    server.shutdown();
    wait_for("the shutdown report", || !render_errors.lock().unwrap().is_empty() && !capture_errors.lock().unwrap().is_empty());
    wait_for("the dead client's close", || server.open_clients() == 0);
    // More watcher periods pass, the server stays down: still one report each.
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable]);
    assert_eq!(*capture_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable]);
    let closed_on = server.clients(|c| c.iter().find(|c| c.name == "transit").and_then(|c| c.closed_on.clone()));
    assert_eq!(closed_on.as_deref(), Some(crate::session::WATCHER_NAME), "closed off the handler, on the watcher");
    assert_eq!(render.stats().unwrap().faults, 1);
    assert!(render.start().is_err(), "nothing restarts a dead client");
    drop(capture);
    drop(render);
    assert!(!backend.has_session());
    assert_eq!(server.clients(|c| c.iter().find(|c| c.name == "transit").unwrap().deactivations), 0, "a dead client is closed, not deactivated");
}

#[test]
fn when_the_server_comes_back_the_watcher_requests_a_reopen() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let (capture, render, capture_errors, render_errors) = open_duplex(&backend);
    capture.start().unwrap();
    render.start().unwrap();
    server.shutdown();
    wait_for("the shutdown report", || render_errors.lock().unwrap().len() == 1);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(render_errors.lock().unwrap().len(), 1, "no reset while the server is down");
    server.restart();
    wait_for("the reopen request", || render_errors.lock().unwrap().len() == 2 && capture_errors.lock().unwrap().len() == 2);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable, ab::StreamError::ResetRequested]);
    assert_eq!(*capture_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable, ab::StreamError::ResetRequested]);
    assert_eq!(render.stats().unwrap().resets, 1);
    // The owner's reopen: drop and open again, on the server that is back.
    drop(capture);
    drop(render);
    let (capture, render, _, _) = open_duplex(&backend);
    render.start().unwrap();
    capture.start().unwrap();
    assert_eq!(server.open_clients(), 1);
}

#[test]
fn the_watcher_gives_up_after_its_window_and_ends_with_the_session() {
    let server = FakeServer::jack2(48_000, 256);
    let backend =
        JackBackend::with_api(Arc::new(server.clone())).with_timing(Duration::from_millis(2), Duration::from_millis(2), Duration::from_millis(20));
    let (capture, render, _, render_errors) = open_duplex(&backend);
    capture.start().unwrap();
    render.start().unwrap();
    server.shutdown();
    wait_for("the shutdown report", || render_errors.lock().unwrap().len() == 1);
    std::thread::sleep(Duration::from_millis(60));
    server.restart();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable], "the window passed: the user reconnects");
}

#[test]
fn an_error_callback_may_stop_its_stream() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let slot: Arc<Mutex<Option<crate::session::JackStream>>> = Arc::default();
    let calls = Arc::new(AtomicU32::new(0));
    let (inner, counter) = (Arc::clone(&slot), Arc::clone(&calls));
    let on_error: ab::ErrorCallback = Box::new(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        if let Some(stream) = inner.lock().unwrap().as_ref() {
            stream.stop().unwrap();
        }
    });
    let pending = crate::session::Pending::Render(render_callback());
    let (render, _) = backend.open_side(&plan.endpoint, &plan.attempts[0], pending, on_error).unwrap();
    render.start().unwrap();
    *slot.lock().unwrap() = Some(render);
    server.set_sample_rate(44_100);
    wait_for("the reset report", || calls.load(Ordering::SeqCst) == 1);
    wait_for("the deactivation the stop made", || server.clients(|c| c.last().unwrap().deactivations) == 1);
    slot.lock().unwrap().take();
    assert_eq!(server.open_clients(), 0);
}

#[test]
fn a_rate_or_quantum_change_requests_a_reset_once_and_an_xrun_is_an_overload() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let (capture, render, capture_errors, render_errors) = open_duplex(&backend);
    capture.start().unwrap();
    render.start().unwrap();
    for _ in 0..3 {
        server.xrun();
    }
    std::thread::sleep(Duration::from_millis(20));
    assert!(render_errors.lock().unwrap().is_empty(), "an xrun is not an error");
    let stats = render.stats().unwrap();
    assert_eq!((stats.overloads, stats.resets, stats.faults), (3, 0, 0));

    server.set_buffer_size(512);
    wait_for("the quantum reset", || !render_errors.lock().unwrap().is_empty() && !capture_errors.lock().unwrap().is_empty());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::ResetRequested], "once per new quantum");
    let shared = server.active_shared().unwrap();
    assert_eq!((shared.buffer_size(), shared.buffer_changes()), (512, 1));

    server.set_sample_rate(44_100);
    wait_for("the rate reset", || render_errors.lock().unwrap().len() == 2 && capture_errors.lock().unwrap().len() == 2);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::ResetRequested; 2], "once per new rate");
    assert_eq!(render.stats().unwrap().resets, 2);
    assert_eq!(server.open_clients(), 1, "a reset leaves the client to the owner's reopen");
}

#[test]
fn the_engine_and_the_handlers_allocate_nothing() {
    let shared = Arc::new(Shared::new(48_000, 256));
    let mut engine = Engine::new(Arc::clone(&shared));
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let sum = Arc::new(AtomicU64::new(0));
    let total = Arc::clone(&sum);
    engine.render = Some(EngineSide::new(
        Box::new(|block: &mut ab::RenderBlock<'_>| block.render(&mut |chunk| chunk.fill(0.25))),
        format,
        ab::HostOrigin::default(),
    ));
    engine.capture = Some(EngineSide::new(
        Box::new(move |block: &mut ab::CaptureBlock<'_>| {
            block.chunks(&mut |_, chunk| {
                total.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            })
        }),
        format,
        ab::HostOrigin::default(),
    ));
    shared.render.set_state(engine::STARTED);
    shared.capture.set_state(engine::STARTED);
    let mut outputs = vec![vec![1.0f32; 8192 * 2]; 2];
    let inputs = vec![vec![0.5f32; 8192 * 2]; 2];
    let ((), stats) = measure(|| {
        let mut frames = 16u32;
        let mut position = 0u32;
        for cycle in 0..10_000u32 {
            let clock =
                engine::Clock { current_frames: position, current_usecs: 1_000 + u64::from(cycle) * 100, now_usecs: 1_050 + u64::from(cycle) * 100 };
            let at = engine::Cycle { frames, arrival: Instant::now(), clock: Some(clock) };
            engine.process(at, &mut VecPlanes(&mut outputs), &InVecPlanes(&inputs));
            position = position.wrapping_add(frames);
            if cycle % 100 == 0 {
                shared.on_xrun();
                shared.on_buffer_size(frames);
                shared.on_sample_rate(48_000);
            }
            frames = if frames >= 8192 * 2 { 16 } else { frames * 2 };
        }
        shared.on_shutdown();
    });
    assert!(stats.is_zero(), "the callbacks allocated: {stats:?}");
    assert_eq!(shared.cycles(), 10_000);
    assert_eq!(shared.last_frames(), 16, "the 10,000th cycle starts the 16..16384 sequence again");
    assert!(shared.shutdown());
    assert!(sum.load(Ordering::Relaxed) > 0);
    assert!(outputs.iter().all(|p| p[..16].iter().all(|s| *s == 0.25)));
}

/// The real libjack with no server running (it never starts one): what enumeration says. Not run
/// in CI; by hand on Linux, with and without libjack on the loader path and no server:
/// `cargo test -p transit-audio-jack --features libjack -- --ignored libjack_ --nocapture`.
#[test]
#[ignore]
#[cfg(all(target_os = "linux", feature = "libjack"))]
fn libjack_without_a_server_reports_the_library_or_the_server() {
    let state = JackBackend::shared().probe();
    println!("{state:?}: {:?}", state.diagnostic());
    assert!(matches!(state, ServerState::LibraryMissing(_) | ServerState::NotRunning), "{state:?}");
    let error = JackBackend::shared().endpoints(ab::Direction::Output).unwrap_err();
    assert!(matches!(error, ab::OpenError::DeviceNotAvailable(_)), "{error:?}");
}

/// An engine with a started render callback writing 1.0 and a started capture callback that keeps
/// what it saw (frames in the block, samples per chunk), over `shared`.
fn started_engine(shared: &Arc<Shared>) -> (Engine, Arc<Mutex<Vec<f32>>>) {
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let mut engine = Engine::new(Arc::clone(shared));
    let kept = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&kept);
    engine.render = Some(EngineSide::new(
        Box::new(|block: &mut ab::RenderBlock<'_>| block.render(&mut |chunk| chunk.fill(1.0))),
        format,
        ab::HostOrigin::default(),
    ));
    engine.capture = Some(EngineSide::new(
        Box::new(move |block: &mut ab::CaptureBlock<'_>| block.chunks(&mut |_, chunk| sink.lock().unwrap().extend_from_slice(chunk))),
        format,
        ab::HostOrigin::default(),
    ));
    shared.render.set_state(engine::STARTED);
    shared.capture.set_state(engine::STARTED);
    (engine, kept)
}

fn cycle(frames: usize) -> engine::Cycle {
    engine::Cycle { frames: u32::try_from(frames).unwrap(), arrival: Instant::now(), clock: None }
}

#[test]
fn ports_shorter_than_the_block_are_never_read_or_written_past_their_end() {
    // 40 samples for a 64-frame block, and a block of two chunks over ports that end inside the
    // first chunk (nothing is left for the second): the copy stops at each port's own length.
    for (frames, len) in [(64, 40), (SCRATCH_FRAMES + 100, SCRATCH_FRAMES - 5), (SCRATCH_FRAMES + 100, SCRATCH_FRAMES + 10)] {
        let shared = Arc::new(Shared::new(48_000, 64));
        let (mut engine, kept) = started_engine(&shared);
        let mut outputs = vec![vec![9.0f32; len]; 2];
        let inputs = vec![vec![0.5f32; len]; 2];
        engine.process(cycle(frames), &mut VecPlanes(&mut outputs), &InVecPlanes(&inputs));
        assert!(!shared.render.panicked() && !shared.capture.panicked(), "an out-of-range copy panics the callback ({frames} frames, {len} samples)");
        assert!(outputs.iter().all(|p| p.len() == len && p.iter().all(|s| *s == 1.0)), "every sample of the short port rendered, none beyond");
        let kept = kept.lock().unwrap();
        assert_eq!(kept.len(), frames * 2, "the capture block is still the full {frames} frames");
        assert!(
            kept.chunks(2).enumerate().all(|(i, f)| f.iter().all(|s| *s == if i < len { 0.5 } else { 0.0 })),
            "the short input is read to its end, zeros after"
        );
    }
}

#[test]
fn ports_longer_than_the_block_keep_their_tail() {
    let shared = Arc::new(Shared::new(48_000, 64));
    let (mut engine, kept) = started_engine(&shared);
    let mut outputs = vec![vec![9.0f32; 100]; 2];
    let inputs = vec![vec![0.5f32; 100]; 2];
    engine.process(cycle(64), &mut VecPlanes(&mut outputs), &InVecPlanes(&inputs));
    assert!(outputs.iter().all(|p| p[..64].iter().all(|s| *s == 1.0) && p[64..].iter().all(|s| *s == 9.0)), "frames 64.. are not ours");
    assert_eq!(kept.lock().unwrap().len(), 64 * 2);
}

#[test]
fn an_engine_panic_outside_the_user_callbacks_leaves_no_direction_in_flight() {
    for capture_first in [true, false] {
        let shared = Arc::new(Shared::new(48_000, 64));
        let (mut engine, _) = started_engine(&shared);
        if !capture_first {
            engine.capture = None;
        }
        engine.fault = true;
        let mut outputs = vec![vec![0.0f32; 64]; 2];
        let inputs = vec![vec![0.0f32; 64]; 2];
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine.process(cycle(64), &mut VecPlanes(&mut outputs), &InVecPlanes(&inputs));
        }));
        assert!(result.is_err(), "the fault unwinds out of the engine");
        // A stuck mark would make each later stop wait out its whole quiesce limit.
        assert!(shared.capture.quiesce(Duration::ZERO) && shared.render.quiesce(Duration::ZERO), "no direction left in flight");
    }
}

#[test]
fn a_panic_while_the_engine_is_busy_releases_the_busy_flag() {
    let busy = engine::BusyFlag::default();
    let first = busy.try_take().expect("free at first");
    assert!(busy.try_take().is_none(), "taken while held");
    drop(first);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = busy.try_take().expect("free again");
        assert!(busy.is_taken());
        panic!("a fake engine panic while the flag is held");
    }));
    assert!(result.is_err());
    assert!(!busy.is_taken(), "the unwind released the flag");
    assert!(busy.try_take().is_some(), "the next cycle takes it: no permanent silence");
}
