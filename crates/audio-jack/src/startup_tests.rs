//! ADR 0021, amendment for #760, slice 2, over the fake server: the watcher's one startup check of
//! transit's own links, and the display-only lines (the graph, the latency reported at startup,
//! PipeWire's sentence). The watcher's own thread never ticks; the tests tick it.

use crate::JackBackend;
use crate::devices::ServerState;
use crate::fake::{FakeServer, Read, SelfConnect};
use std::sync::Arc;
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend, Stream};

const HOUR: Duration = Duration::from_secs(3600);

fn manual(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(HOUR, HOUR, HOUR)
}

struct Open {
    capture: Box<dyn Stream>,
    render: Box<dyn Stream>,
    achieved: ab::Achieved,
}

/// The Player's duplex open, not started.
fn open(backend: &JackBackend) -> Open {
    let plan = backend.plan_render(&ab::OutputWish { channels: 2, ..Default::default() }).unwrap();
    let request = ab::OpenRequest { format: backend.capture_format(&plan.endpoint, None).unwrap(), ..plan.attempts[0] };
    let (capture, _) = backend.open_capture(&plan.endpoint, &request, Box::new(|_| {}), Box::new(|_| {})).unwrap();
    let (render, achieved) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {})).unwrap();
    Open { capture, render, achieved }
}

fn started(backend: &JackBackend) -> Open {
    let open = open(backend);
    open.capture.start().unwrap();
    open.render.start().unwrap();
    open
}

/// `n` process cycles of `frames` on the active client (no port buffers).
fn cycles(server: &FakeServer, n: usize, frames: u32) {
    for _ in 0..n {
        assert!(server.cycle(frames, &mut [], &[]));
    }
}

fn link_lines(open: &Open) -> Vec<String> {
    open.render.device_lines().into_iter().filter(|l| l.starts_with("Link from")).collect()
}

#[test]
fn transits_links_are_checked_once_and_each_one_not_observed_is_one_line() {
    // allow: every link made, no line.
    let server = FakeServer::jack2(48_000, 256);
    let backend = manual(&server);
    let open = started(&backend);
    cycles(&server, 2, 256);
    backend.tick();
    assert!(link_lines(&open).is_empty(), "{:?}", open.render.device_lines());
    drop(open);
    // ignore: the connects answer 0 and make no link: each pair is named.
    server.set_self_connect(SelfConnect::Ignore);
    let open = started(&backend);
    cycles(&server, 2, 256);
    backend.tick();
    let lines = link_lines(&open);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[0], "Link from system:capture_1 to transit:in_1 was not observed at the startup check.");
    assert_eq!(lines[2], "Link from transit:out_1 to system:playback_1 was not observed at the startup check.");
    drop(open);
    // fail: the connects answer 1, which the line adds.
    server.set_self_connect(SelfConnect::Fail);
    let open = started(&backend);
    cycles(&server, 2, 256);
    backend.tick();
    assert_eq!(link_lines(&open)[0], "Link from system:capture_1 to transit:in_1 was not observed at the startup check. jack_connect returned 1.");
}

#[test]
fn the_check_is_historical_and_reads_once_per_start() {
    let server = FakeServer::jack2(48_000, 256);
    // The links show only five graph cycles after the connects: still missing at the check.
    server.delaying_links(5);
    let backend = manual(&server);
    let open = started(&backend);
    let connects = server.connects().len();
    cycles(&server, 2, 256);
    backend.tick();
    let reads = server.link_reads();
    assert_eq!(reads, 4, "one read per asked pair");
    assert_eq!(link_lines(&open).len(), 4, "not seen at the check");
    // The links show afterwards: the lines stay (historical); no read, no connect again.
    for _ in 0..3 {
        server.step(Duration::from_millis(5));
    }
    assert!(server.linked("transit:out_1", "system:playback_1"));
    backend.tick();
    backend.tick();
    assert_eq!(link_lines(&open).len(), 4, "a link that appears later does not update the check");
    assert_eq!(server.link_reads(), reads, "read once");
    assert_eq!(server.connects().len(), connects, "nothing retried");
    // A link removed after the check makes no new line.
    server.unlink("transit:out_1", "system:playback_1");
    backend.tick();
    assert_eq!(link_lines(&open).len(), 4);
}

/// #760 round 3: the server shows a link a connect made from a later graph cycle on, so the check
/// waits for a process cycle that began after the connects to complete: a link that shows at the
/// next cycle is no line.
#[test]
fn a_link_that_shows_at_the_next_cycle_is_not_called_missing() {
    let server = FakeServer::jack2(48_000, 256);
    server.delaying_links(1);
    let backend = manual(&server);
    let open = started(&backend);
    backend.tick();
    assert_eq!(server.link_reads(), 0, "no cycle since the connects: the check waits");
    cycles(&server, 1, 256);
    backend.tick();
    assert_eq!(server.link_reads(), 0, "the first cycle may still run");
    cycles(&server, 1, 256);
    backend.tick();
    assert_eq!(server.link_reads(), 4, "checked once the cycle after the connects completed");
    assert!(link_lines(&open).is_empty(), "{:?}", open.render.device_lines());
}

/// With no process cycle completing within the bound, no link is called missing: one neutral
/// line says the check could not run, and the latency is still read.
#[test]
fn with_no_cycle_within_the_bound_the_check_says_it_could_not_run() {
    let server = FakeServer::jack2(48_000, 256);
    server.set_self_connect(SelfConnect::Ignore);
    let backend = manual(&server);
    let open = started(&backend);
    for _ in 1..crate::startup::CHECK_TICKS {
        backend.tick();
    }
    assert!(link_lines(&open).is_empty() && !open.render.device_lines().iter().any(|l| l.starts_with("The startup link check")), "not yet");
    backend.tick();
    let lines = open.render.device_lines();
    assert_eq!(server.link_reads(), 0, "nothing was read");
    assert!(link_lines(&open).is_empty(), "no link is called missing: {lines:?}");
    assert_eq!(lines.iter().filter(|l| l.as_str() == crate::startup::LINK_CHECK_SKIPPED).count(), 1, "{lines:?}");
    assert_eq!(
        crate::startup::LINK_CHECK_SKIPPED,
        "The startup link check could not run: no process cycle completed within about 2 seconds of the start."
    );
    assert!(lines.iter().any(|l| l.starts_with("Latency reported at startup:")), "{lines:?}");
    backend.tick();
    assert_eq!(open.render.device_lines(), lines, "said once");
}

/// #760 round 3: the server can shut the client down while the check reads (the shutdown callback
/// takes no lock of transit's); a shutdown seen after the reads publishes nothing of them.
#[test]
fn a_shutdown_during_the_startup_check_publishes_nothing_read() {
    // During the link reads (links refused, so lines would be made).
    let server = FakeServer::jack2(48_000, 256);
    server.set_self_connect(SelfConnect::Ignore);
    let backend = manual(&server);
    let open = started(&backend);
    cycles(&server, 2, 256);
    server.shutdown_during(Read::Links);
    backend.tick();
    assert!(server.link_reads() > 0, "the reads ran");
    let lines = open.render.device_lines();
    assert!(!lines.iter().any(|l| l.starts_with("Link from") || l.starts_with("Latency reported")), "{lines:?}");
    drop(open);
    // During the latency reads (auto-connect off: nothing to check, the latency is read at once).
    let server = FakeServer::jack2(48_000, 256);
    let backend = manual(&server).without_auto_connect();
    let open = started(&backend);
    server.shutdown_during(Read::Latency);
    backend.tick();
    assert!(server.latency_reads() > 0, "the reads ran");
    assert!(!open.render.device_lines().iter().any(|l| l.starts_with("Latency reported")), "{:?}", open.render.device_lines());
}

/// #760 round 3: a rescan reads the listing through the active session client; a shutdown during
/// those reads discards them, and the listing is taken afresh (here: no server).
#[test]
fn a_shutdown_while_a_rescan_reads_the_session_client_discards_the_listing() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = manual(&server);
    let _open = started(&backend);
    server.shutdown_during(Read::PhysicalPorts);
    assert_eq!(backend.probe(), ServerState::NotRunning, "not a listing built from a dead client's answers");
}

#[test]
fn without_auto_connect_nothing_is_asked_and_nothing_read() {
    let server = FakeServer::jack2(48_000, 256);
    let backend = manual(&server).without_auto_connect();
    let open = started(&backend);
    backend.tick();
    assert_eq!(server.link_reads(), 0);
    assert!(link_lines(&open).is_empty());
}

#[test]
fn the_latency_reported_at_startup_is_read_once_per_session_and_labeled_as_reported() {
    let server = FakeServer::jack2(48_000, 256);
    server.set_latency("system:playback_1", true, (256, 256));
    server.set_latency("system:playback_2", true, (256, 512));
    server.set_latency("system:capture_1", false, (128, 128));
    server.unreported_latency("system:capture_2");
    let backend = manual(&server);
    let open = started(&backend);
    assert_eq!(open.render.device_lines(), ["Graph: 48000 Hz, buffer 256 (reported by transit's active client)"], "nothing read before the tick");
    // The buffer is observed once a process cycle ran with it.
    cycles(&server, 2, 256);
    assert_eq!(open.render.device_lines(), ["Graph: 48000 Hz (reported), buffer 256 (observed)"]);
    backend.tick();
    assert_eq!(
        open.render.device_lines(),
        [
            "Graph: 48000 Hz (reported), buffer 256 (observed)",
            "Latency reported at startup: output 256–512 frames across 2 ports (5.3–10.7 ms), input 128–128 frames across 1 port (2.7–2.7 ms), 1 of 2 ports not reported; not measured, Measure measures the loop"
        ]
    );
    assert_eq!(server.latency_reads(), 4, "one read per device port");
    backend.tick();
    assert_eq!(server.latency_reads(), 4, "once per session");
    // Display only: nothing reaches the stream's latency or `Achieved`.
    assert_eq!((open.render.latency(), open.achieved.latency), (None, None));
    assert_eq!(open.capture.device_lines(), open.render.device_lines(), "one session, one set of lines");
}

#[test]
fn a_library_without_the_latency_call_says_not_reported_and_pipewire_adds_its_sentence() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    server.without_latency_call();
    let backend = manual(&server);
    let open = started(&backend);
    cycles(&server, 2, 1024);
    backend.tick();
    assert_eq!(
        open.render.device_lines(),
        [
            "Graph: 48000 Hz (reported), buffer 1024 (observed)",
            "Latency reported at startup: output not reported, input not reported; not measured, Measure measures the loop",
            crate::startup::PIPEWIRE_STAGES,
        ]
    );
    assert_eq!(
        crate::startup::PIPEWIRE_STAGES,
        "PipeWire may perform rate conversion and add buffering. JACK does not identify those stages separately or establish which delays are included in these reported ranges."
    );
}

#[test]
fn a_latched_client_checks_and_reads_nothing() {
    let server = FakeServer::pipewire(44_100, 1024, (48_000, 1024));
    let backend = manual(&server);
    let open = started(&backend);
    assert!(open.render.needs_rebuild());
    backend.tick();
    assert_eq!((server.link_reads(), server.latency_reads()), (0, 0));
    assert!(open.render.device_lines().is_empty());
}
