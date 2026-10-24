//! Fake-server tests of slice 1 (ADR 0021): enumeration states, the client lifecycle, the achieved
//! rate and quantum, the watcher's routing of shutdown, rate change and xruns, and a callback shell
//! that allocates nothing.

use crate::engine::{self, Shared};
use crate::fake::{FakeServer, VecPlanes};
use crate::{JackBackend, JackDevice, ServerState};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_audio_backend::{self as ab, Backend, Stream};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn backend(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_watch_period(Duration::from_millis(2))
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

/// Opens capture then render on the first group (the Player's duplex order), each with an error recorder.
fn open_duplex(backend: &JackBackend) -> (Box<dyn Stream>, Box<dyn Stream>, Errors, Errors) {
    let render_plan = backend.plan_render(&ab::OutputWish::default()).expect("render plan");
    assert!(backend.capture_with_render(&render_plan.endpoint));
    let capture_request =
        ab::OpenRequest { format: backend.capture_format(&render_plan.endpoint, None).expect("capture format"), ..render_plan.attempts[0] };
    let (capture_errors, on_capture) = recorder();
    let (capture, _) = backend.open_capture(&render_plan.endpoint, &capture_request, capture_callback(), on_capture).expect("capture");
    let (render_errors, on_render) = recorder();
    let (render, _) = backend.open_render(&render_plan.endpoint, &render_plan.attempts[0], render_callback(), on_render).expect("render");
    (capture, render, capture_errors, render_errors)
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
        (ab::SampleFormat::F32, 2, 48_000, 48_000, Some((128, 128)))
    );
    assert_eq!(backend.channel_names(system, ab::Direction::Input), ["capture_1", "capture_2"]);
    assert_eq!(backend.channel_names(system, ab::Direction::Output), ["playback_1", "playback_2"]);
    assert_eq!(server.open_clients(), 0, "the probe client is closed again");
    assert_eq!(server.clients(|c| c[0].name.clone()), crate::PROBE_NAME);
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
fn achieved_reports_the_servers_rate_and_quantum_not_the_request() {
    let server = FakeServer::jack2(48_000, 128);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish { rates: vec![44_100], channels: 2, period: Some(64), ..Default::default() }).unwrap();
    assert_eq!(plan.attempts.len(), 1);
    let asked = ab::OpenRequest { format: ab::Format::new(44_100, 2, ab::SampleFormat::F32), period: ab::Period::Frames(64), ..plan.attempts[0] };
    let (errors, on_error) = recorder();
    let (stream, achieved) = backend.open_render(&plan.endpoint, &asked, render_callback(), on_error).unwrap();
    assert_eq!(achieved.format, ab::Format::new(48_000, 2, ab::SampleFormat::F32), "the server's rate, not 44.1 kHz");
    assert_eq!(achieved.period, ab::Period::Frames(128), "the server's quantum, not 64");
    assert_eq!(achieved.buffer_frames, Some(128));
    assert_eq!(stream.thread_priority(), ab::WorkerPriority::Unobserved);
    drop(stream);
    assert!(errors.lock().unwrap().is_empty());
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

    let mut outputs = vec![vec![1.0f32; 256]; 2];
    server.cycle(256, &mut outputs);
    assert!(outputs.iter().all(|p| p.iter().all(|s| *s == 0.0)), "slice 1 writes silence");
    assert_eq!(render.stats().unwrap().wakes, 1);

    drop(capture);
    assert_eq!(server.open_clients(), 1, "the render side keeps the client");
    drop(render);
    assert_eq!(server.open_clients(), 0, "the last stream closes the client");
    let closed = server.clients(|c| (c.last().unwrap().deactivations, c.last().unwrap().closed));
    assert_eq!(closed, (1, true));
    assert!(!backend.has_session());
    assert!(capture_errors.lock().unwrap().is_empty() && render_errors.lock().unwrap().is_empty());
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
    // More watcher periods pass: still one report each.
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable]);
    assert_eq!(*capture_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable]);
    let closed_on = server.clients(|c| c.last().unwrap().closed_on.clone());
    assert_eq!(closed_on.as_deref(), Some(crate::session::WATCHER_NAME), "closed off the handler, on the watcher");
    assert_eq!(render.stats().unwrap().faults, 1);
    assert!(render.start().is_err(), "nothing restarts a dead client");
    drop(capture);
    drop(render);
    assert!(!backend.has_session());
    assert_eq!(server.clients(|c| c.last().unwrap().deactivations), 0, "a dead client is closed, not deactivated");
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
    let (render, _) = backend.open_side(&plan.endpoint, &plan.attempts[0], ab::Direction::Output, on_error).unwrap();
    render.start().unwrap();
    *slot.lock().unwrap() = Some(render);
    server.set_sample_rate(44_100);
    wait_for("the reset report", || calls.load(Ordering::SeqCst) == 1);
    wait_for("the deactivation the stop made", || server.clients(|c| c.last().unwrap().deactivations) == 1);
    slot.lock().unwrap().take();
    assert_eq!(server.open_clients(), 0);
}

#[test]
fn a_rate_change_requests_a_reset_once_and_an_xrun_is_an_overload() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = backend(&server);
    let (capture, render, capture_errors, render_errors) = open_duplex(&backend);
    capture.start().unwrap();
    render.start().unwrap();
    for _ in 0..3 {
        server.xrun();
    }
    server.set_buffer_size(512);
    std::thread::sleep(Duration::from_millis(20));
    assert!(render_errors.lock().unwrap().is_empty(), "an xrun or a quantum change is not an error");
    let stats = render.stats().unwrap();
    assert_eq!((stats.overloads, stats.resets, stats.faults), (3, 0, 0));
    let shared = server.active_shared().unwrap();
    assert_eq!((shared.buffer_size(), shared.buffer_changes()), (512, 1));

    server.set_sample_rate(44_100);
    wait_for("the reset report", || !render_errors.lock().unwrap().is_empty() && !capture_errors.lock().unwrap().is_empty());
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::ResetRequested], "once per new rate");
    assert_eq!(*capture_errors.lock().unwrap(), [ab::StreamError::ResetRequested]);
    assert_eq!(render.stats().unwrap().resets, 1);
    assert_eq!(server.open_clients(), 1, "a reset leaves the client to the owner's reopen");
}

#[test]
fn the_process_shell_and_the_handlers_allocate_nothing() {
    let shared = Shared::new(48_000, 256);
    let mut outputs = vec![vec![1.0f32; 8192]; 8];
    let ((), stats) = measure(|| {
        let mut frames = 16u32;
        for cycle in 0..10_000u32 {
            engine::process(&shared, frames, &mut VecPlanes(&mut outputs));
            if cycle % 100 == 0 {
                shared.on_xrun();
                shared.on_buffer_size(frames);
                shared.on_sample_rate(48_000);
            }
            frames = if frames >= 8192 { 16 } else { frames * 2 };
        }
        shared.on_shutdown();
    });
    assert!(stats.is_zero(), "the callbacks allocated: {stats:?}");
    assert_eq!((shared.cycles(), shared.last_frames()), (10_000, 8192));
    assert!(shared.shutdown());
    assert!(outputs.iter().all(|p| p.iter().all(|s| *s == 0.0)));
}

/// The real libjack with no server running (it never starts one): what enumeration says. Not run
/// in CI; by hand on Linux, with and without libjack on the loader path:
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
