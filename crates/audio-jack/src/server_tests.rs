//! Tests against a real JACK server through the real libjack (Linux, `libjack` feature). No
//! hardware is needed: a server with the dummy driver serves them,
//! `jackd -r -d dummy -r 48000 -p 128`.
//!
//! - `server_*` tests **skip at run time** (a line on stderr, then pass) when no server answers, so
//!   the Linux CI job and the WSL gate, which run no server, stay green. They never touch the
//!   server's settings and connect transit's ports only to each other, except the connection test,
//!   which plays silence to the default port group.
//! - `owned_server_*` tests are `#[ignore]`: they start, kill and restart a dummy server of their
//!   own, named by `JACK_DEFAULT_SERVER` (libjack reads it at client open), and skip without it:
//!   `JACK_DEFAULT_SERVER=transit-test cargo test -p transit-audio-jack --features libjack --
//!   --ignored owned_server_ --test-threads=1`.

use crate::JackBackend;
use crate::ServerState;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_audio_backend::{self as ab, Backend, Stream};

/// A backend over the real libjack, unshared (its own client), with short watcher timings.
fn real() -> JackBackend {
    JackBackend::with_api(Arc::new(crate::ffi::LibJack)).with_timing(Duration::from_millis(10), Duration::from_millis(100), Duration::from_secs(10))
}

/// The running server, or `None` after saying why the test is skipped.
fn server_or_skip(test: &str) -> Option<crate::Server> {
    match real().probe() {
        ServerState::Running(server) => Some(server),
        other => {
            eprintln!("{test}: skipped, no JACK server ({other:?}); start one with `jackd -r -d dummy -r 48000 -p 128`");
            None
        }
    }
}

fn wait_for(what: &str, limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

type Errors = Arc<Mutex<Vec<ab::StreamError>>>;

fn recorder() -> (Errors, ab::ErrorCallback) {
    let errors: Errors = Arc::default();
    let sink = Arc::clone(&errors);
    (errors, Box::new(move |e| sink.lock().unwrap().push(e)))
}

/// Opens capture, then render, on the first port group, with these callbacks.
fn open_duplex(
    backend: &JackBackend,
    render: ab::RenderCallback,
    capture: ab::CaptureCallback,
) -> (Box<dyn Stream>, Box<dyn Stream>, ab::Achieved, Errors, Errors) {
    let plan = backend.plan_render(&ab::OutputWish::default()).expect("a port group with playback ports");
    let capture_request = ab::OpenRequest { format: backend.capture_format(&plan.endpoint, None).expect("capture format"), ..plan.attempts[0] };
    let (capture_errors, on_capture) = recorder();
    let (capture, _) = backend.open_capture(&plan.endpoint, &capture_request, capture, on_capture).expect("capture opens");
    let (render_errors, on_render) = recorder();
    let (render, achieved) = backend.open_render(&plan.endpoint, &plan.attempts[0], render, on_render).expect("render opens");
    (capture, render, achieved, capture_errors, render_errors)
}

fn silent() -> (ab::RenderCallback, ab::CaptureCallback) {
    (Box::new(|_| {}), Box::new(|_| {}))
}

#[test]
fn server_open_registers_ports_and_connects_them_to_the_device() {
    let Some(server) = server_or_skip("server_open_registers_ports_and_connects_them_to_the_device") else { return };
    let backend = real();
    let (render_cb, capture_cb) = silent();
    let (capture, render, achieved, capture_errors, render_errors) = open_duplex(&backend, render_cb, capture_cb);
    assert_eq!((achieved.format.rate, achieved.period), (server.rate, ab::Period::Frames(server.buffer)), "the server's rate and quantum");
    render.start().unwrap();
    capture.start().unwrap();
    let name = backend.client_name().expect("a client");
    assert!(name.starts_with(crate::CLIENT_NAME), "{name}");
    let group = backend.plan_render(&ab::OutputWish::default()).unwrap().endpoint.handle::<crate::JackDevice>().unwrap().clone();
    let out_1 = backend.with_client(|c| c.connections(&format!("{name}:out_1"))).unwrap();
    assert_eq!(out_1, [group.playback[0].clone()], "out_1 feeds the device's first playback port");
    if let Some(first_capture) = group.capture.first() {
        let in_1 = backend.with_client(|c| c.connections(&format!("{name}:in_1"))).unwrap();
        assert_eq!(in_1, std::slice::from_ref(first_capture), "in_1 reads the device's first capture port");
    }
    wait_for("process callbacks", Duration::from_secs(10), || render.stats().unwrap().wakes > 10 && capture.stats().unwrap().wakes > 10);
    drop(capture);
    drop(render);
    assert!(!backend.has_session());
    assert!(capture_errors.lock().unwrap().is_empty() && render_errors.lock().unwrap().is_empty());
}

/// What the loopback test's callbacks saw: per capture block its device position and samples;
/// whether every capture ran before the render of its cycle.
#[derive(Default)]
struct Loop {
    /// (device position, channels, interleaved samples) per capture block.
    captured: Mutex<Vec<(u64, usize, Vec<f32>)>>,
    last_capture: AtomicU64,
    out_of_order: AtomicBool,
}

/// The test signal of frame `position`: unique per frame for 2^20 frames, never zero.
fn signal(position: u64) -> f32 {
    (position % (1 << 20) + 1) as f32
}

#[test]
fn server_loopback_renders_the_signal_and_captures_it_aligned() {
    let Some(_server) = server_or_skip("server_loopback_renders_the_signal_and_captures_it_aligned") else { return };
    let backend = real().without_auto_connect();
    let seen = Arc::new(Loop::default());
    let (r, c) = (Arc::clone(&seen), Arc::clone(&seen));
    let render_cb: ab::RenderCallback = Box::new(move |block| {
        let position = block.device_position.expect("a device position");
        if r.last_capture.load(Ordering::SeqCst) != position + 1 {
            r.out_of_order.store(true, Ordering::SeqCst);
        }
        let channels = usize::from(block.channels);
        let mut at = position;
        block.render(&mut |chunk| {
            for (i, s) in chunk.iter_mut().enumerate() {
                // Channel 1 carries the negated signal, so a crossed channel shows.
                let v = signal(at + (i / channels) as u64);
                *s = if i % channels == 0 { v } else { -v };
            }
            at += (chunk.len() / channels) as u64;
        });
    });
    let capture_cb: ab::CaptureCallback = Box::new(move |block| {
        let position = block.device_position.expect("a device position");
        c.last_capture.store(position + 1, Ordering::SeqCst);
        let mut samples = Vec::new();
        block.chunks(&mut |_, chunk| samples.extend_from_slice(chunk));
        c.captured.lock().unwrap().push((position, usize::from(block.channels), samples));
    });
    let (capture, render, achieved, _, render_errors) = open_duplex(&backend, render_cb, capture_cb);
    let channels = usize::from(achieved.format.channels).min(2);
    render.start().unwrap();
    capture.start().unwrap();
    let name = backend.client_name().unwrap();
    for i in 1..=channels {
        backend.with_client(|c| c.connect(&format!("{name}:out_{i}"), &format!("{name}:in_{i}"))).unwrap().expect("loop connects");
    }
    wait_for("200 cycles", Duration::from_secs(10), || capture.stats().unwrap().wakes > 200);
    drop(capture);
    drop(render);
    assert!(!seen.out_of_order.load(Ordering::SeqCst), "every cycle ran its capture before its render, at one position");
    let captured = seen.captured.lock().unwrap();
    let ab::Period::Frames(period) = achieved.period else { panic!("JACK reports its quantum") };
    // The lag of every looped frame: the capture position minus the rendered position it holds.
    let mut lags = std::collections::BTreeSet::new();
    let mut looped = 0usize;
    for (position, in_channels, samples) in captured.iter() {
        assert!(*in_channels >= channels, "capture carries the looped channels");
        for (frame, values) in samples.chunks(*in_channels).enumerate() {
            let v = values[0];
            if v == 0.0 {
                continue; // before the loop was connected
            }
            // Channel 2 is connected a moment after channel 1: silent until then.
            if channels > 1 && values[1] != 0.0 {
                assert_eq!(values[1], -v, "channel 2 carries channel 2");
            }
            let rendered = (v as u64) - 1;
            let at = (position + frame as u64) % (1 << 20);
            lags.insert((at + (1 << 20) - rendered) % (1 << 20));
            looped += 1;
        }
    }
    assert!(looped > 100 * 64, "the loop carried the signal ({looped} frames)");
    let lags: Vec<u64> = lags.into_iter().collect();
    let period = u64::from(period);
    assert_eq!(lags.len(), 1, "one constant lag, the capture aligned frame for frame: {lags:?}");
    assert_eq!(
        lags[0], period,
        "the loop through JACK's graph lags exactly one period (all four quanta measured so under JACK 2): {} frames",
        lags[0]
    );
    eprintln!("loopback lag: {} frames (period {period})", lags[0]);
    assert!(render_errors.lock().unwrap().is_empty());
}

#[test]
fn server_a_stalled_cycle_is_counted_as_an_xrun() {
    let Some(_server) = server_or_skip("server_a_stalled_cycle_is_counted_as_an_xrun") else { return };
    let backend = real().without_auto_connect();
    let stall = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stall);
    // A test-only hook: one render callback that sleeps far past the cycle.
    let render_cb: ab::RenderCallback = Box::new(move |_| {
        if flag.swap(false, Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(60));
        }
    });
    let (capture, render, _, _, render_errors) = open_duplex(&backend, render_cb, Box::new(|_| {}));
    render.start().unwrap();
    capture.start().unwrap();
    wait_for("cycles", Duration::from_secs(10), || render.stats().unwrap().wakes > 20);
    // A non-real-time dummy server under load reports a stray xrun now and then: count from here.
    let before = render.stats().unwrap().overloads;
    // A loaded machine can delay the dummy driver itself by seconds (its own "XRun = 4228404 usec"
    // in the server log): stall again each second until the report comes, for up to 20 s.
    let deadline = Instant::now() + Duration::from_secs(20);
    while render.stats().unwrap().overloads == before {
        assert!(Instant::now() < deadline, "timed out waiting for the server's xrun report");
        stall.store(true, Ordering::SeqCst);
        let tick = Instant::now() + Duration::from_secs(1);
        while render.stats().unwrap().overloads == before && Instant::now() < tick {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(render_errors.lock().unwrap().is_empty(), "an xrun is not an error: {:?}", render_errors.lock().unwrap());
    assert!(backend.has_session());
}

/// A dummy server of the test's own, named by `JACK_DEFAULT_SERVER`; killed on drop.
struct OwnedServer {
    name: String,
    child: Option<std::process::Child>,
}

static OWNED: Mutex<()> = Mutex::new(());

impl OwnedServer {
    fn named(test: &str) -> Option<Self> {
        let Ok(name) = std::env::var("JACK_DEFAULT_SERVER") else {
            eprintln!("{test}: skipped, set JACK_DEFAULT_SERVER to a name for the test's own dummy server");
            return None;
        };
        Some(Self { name, child: None })
    }

    fn start(&mut self, period: u32) {
        let child = std::process::Command::new("jackd")
            .args(["-n", &self.name, "-r", "-d", "dummy", "-r", "48000", "-p", &period.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("jackd starts (jackd2 installed?)");
        self.child = Some(child);
        wait_for("the owned server", Duration::from_secs(10), || matches!(real().probe(), ServerState::Running(_)));
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for OwnedServer {
    fn drop(&mut self) {
        self.kill();
    }
}

#[test]
#[ignore]
fn owned_server_a_quantum_change_requests_a_reopen_at_the_new_quantum() {
    let _one = OWNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut server) = OwnedServer::named("owned_server_a_quantum_change_requests_a_reopen_at_the_new_quantum") else { return };
    server.start(128);
    let backend = real().without_auto_connect();
    let (render_cb, capture_cb) = silent();
    let (capture, render, achieved, capture_errors, render_errors) = open_duplex(&backend, render_cb, capture_cb);
    assert_eq!(achieved.period, ab::Period::Frames(128));
    render.start().unwrap();
    capture.start().unwrap();
    wait_for("cycles", Duration::from_secs(10), || render.stats().unwrap().wakes > 10);
    let mut other = backend.open_other_client("transit-test-quantum").expect("a second client");
    other.set_buffer_size(256).expect("the server takes the new quantum");
    wait_for("the reset request", Duration::from_secs(10), || {
        !render_errors.lock().unwrap().is_empty() && !capture_errors.lock().unwrap().is_empty()
    });
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::ResetRequested]);
    assert_eq!(render.stats().unwrap().resets, 1);
    // The owner's reopen: the new streams report the new quantum.
    drop(capture);
    drop(render);
    let (render_cb, capture_cb) = silent();
    let (capture, render, achieved, _, _) = open_duplex(&backend, render_cb, capture_cb);
    assert_eq!(achieved.period, ab::Period::Frames(256), "reopened at the server's new quantum");
    render.start().unwrap();
    capture.start().unwrap();
    wait_for("cycles at 256", Duration::from_secs(10), || render.stats().unwrap().wakes > 10);
    drop((capture, render));
    other.close().unwrap();
}

#[test]
#[ignore]
fn owned_server_a_shutdown_is_device_loss_and_the_server_back_is_a_reopen() {
    let _one = OWNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut server) = OwnedServer::named("owned_server_a_shutdown_is_device_loss_and_the_server_back_is_a_reopen") else { return };
    server.start(128);
    let backend = real().without_auto_connect();
    let (render_cb, capture_cb) = silent();
    let (capture, render, _, capture_errors, render_errors) = open_duplex(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    wait_for("cycles", Duration::from_secs(10), || render.stats().unwrap().wakes > 10);
    server.kill();
    wait_for("the device-lost report", Duration::from_secs(5), || {
        render_errors.lock().unwrap().first() == Some(&ab::StreamError::DeviceNotAvailable)
    });
    assert_eq!(*capture_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable]);
    assert_eq!(render.stats().unwrap().faults, 1);
    server.start(128);
    wait_for("the reopen request", Duration::from_secs(10), || render_errors.lock().unwrap().len() == 2);
    assert_eq!(*render_errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable, ab::StreamError::ResetRequested]);
    drop(capture);
    drop(render);
    let (render_cb, capture_cb) = silent();
    let (capture, render, _, _, _) = open_duplex(&backend, render_cb, capture_cb);
    render.start().unwrap();
    capture.start().unwrap();
    wait_for("cycles after the reopen", Duration::from_secs(10), || render.stats().unwrap().wakes > 10);
}
