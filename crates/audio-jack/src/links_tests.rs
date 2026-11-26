//! ADR 0015 (#431) rule 6 over the fake server: links other clients made to transit's ports come
//! back after transit's own reopen. Each test runs the reopen operation as the app does
//! (`ui-studio`'s `audio_connection::replace`): the snapshot from the old Player's render stream on
//! this (control) thread before anything closes, the old streams dropped, the replacement opened in
//! the Player's duplex order and started, then the snapshot restored on the new render stream once.
//! The watcher's thread never ticks here (its period is an hour); the tests tick it themselves.

use crate::JackBackend;
use crate::api::JackApi;
use crate::fake::FakeServer;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend, Stream};

const HOUR: Duration = Duration::from_secs(3600);

fn manual(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(HOUR, HOUR, HOUR)
}

type Errors = Arc<Mutex<Vec<ab::StreamError>>>;

/// One Player's streams over the backend.
struct Open {
    capture: Box<dyn Stream>,
    render: Box<dyn Stream>,
    errors: Errors,
}

/// What the app chose in Audio Devices.
#[derive(Clone, Copy, Default)]
struct Choice<'a> {
    output: Option<&'a str>,
    input: Option<&'a str>,
    period: Option<u32>,
}

/// The Player's duplex open: the input resolved, capture opened, then render, then capture started,
/// then render.
fn open(backend: &JackBackend, choice: Choice<'_>) -> Result<Open, ab::OpenError> {
    let wish = ab::OutputWish { device: choice.output.map(str::to_owned), period: choice.period, ..Default::default() };
    let plan = backend.plan_render(&wish).map_err(ab::OpenError::Other)?;
    let input = ab::InputWish { device: choice.input.map(str::to_owned), rate: None, period: choice.period };
    let on = backend.duplex_capture_endpoint(&plan.endpoint, &input).map_err(ab::OpenError::Other)?;
    let request = ab::OpenRequest { format: backend.capture_format(&on, None).map_err(ab::OpenError::Other)?, ..plan.attempts[0] };
    let errors: Errors = Arc::default();
    let (a, b) = (Arc::clone(&errors), Arc::clone(&errors));
    let (capture, _) = backend.open_capture(&on, &request, Box::new(|_| {}), Box::new(move |e| a.lock().unwrap().push(e)))?;
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(move |e| b.lock().unwrap().push(e)))?;
    capture.start()?;
    render.start()?;
    Ok(Open { capture, render, errors })
}

/// The reopen operation: snapshot, close the old Player, open the new one, restore once.
fn reopen(backend: &JackBackend, old: Open, choice: Choice<'_>) -> (Result<Open, ab::OpenError>, Option<String>) {
    let snapshot = old.render.snapshot_links();
    drop(old);
    let new = open(backend, choice);
    let note = match (&new, snapshot) {
        (Ok(new), Some(snapshot)) => new.render.restore_links(snapshot),
        // A failed open abandons the operation: the snapshot is dropped here, unused.
        _ => None,
    };
    (new, note)
}

/// Every link of `before` is still in `after`: a restore removes nothing.
fn kept(before: &[(String, String)], after: &[(String, String)]) -> bool {
    before.iter().all(|link| after.contains(link))
}

/// A JACK 2 server with a music player's outputs and a recorder's inputs as other clients.
fn server() -> FakeServer {
    let server = FakeServer::jack2(48_000, 256);
    server.add_port("Player:out_L", true);
    server.add_port("Player:out_R", true);
    server.add_port("Recorder:in_1", false);
    server
}

fn own_links_made(server: &FakeServer, client: &str) -> bool {
    server.linked("system:capture_1", &format!("{client}:in_1"))
        && server.linked("system:capture_2", &format!("{client}:in_2"))
        && server.linked(&format!("{client}:out_1"), "system:playback_1")
        && server.linked(&format!("{client}:out_2"), "system:playback_2")
}

#[test]
fn outside_links_come_back_after_a_reopen_for_a_quantum_change() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("transit:out_2", "Recorder:in_1");
    // PipeWire changes the quantum: the watcher's next tick asks for the reopen (ADR 0021).
    server.set_buffer_size(512);
    backend.tick();
    assert_eq!(*old.errors.lock().unwrap(), [ab::StreamError::ResetRequested; 2], "both directions asked to reopen");
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    assert_eq!(note, None, "everything came back");
    assert!(server.linked("Player:out_L", "transit:in_1") && server.linked("transit:out_2", "Recorder:in_1"));
    assert!(own_links_made(&server, "transit"), "transit's own links as before");
    assert_eq!(server.graph().len(), 6, "no link twice, nothing else");
}

#[test]
fn outside_links_come_back_after_reconnect() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("Player:out_R", "transit:in_2");
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    assert_eq!(note, None);
    assert!(server.linked("Player:out_L", "transit:in_1") && server.linked("Player:out_R", "transit:in_2"));
}

#[test]
fn apply_with_a_new_buffer_size_restores_the_links() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("transit:out_1", "Recorder:in_1");
    let (new, note) = reopen(&backend, old, Choice { period: Some(128), ..Default::default() });
    let _new = new.unwrap();
    assert_eq!(server.quantum_requests(), [128], "Apply asked the server for the new size");
    assert_eq!(note, None);
    assert!(server.linked("transit:out_1", "Recorder:in_1"));
}

/// PipeWire with a microphone and a headset: Apply moves the input from the headset to the
/// microphone, so `in_1` stands for another source and its outside link is not attached to it.
#[test]
fn apply_with_another_input_device_drops_the_input_links_and_says_so() {
    let server = FakeServer::running(
        48_000,
        256,
        &[
            ("Mic:capture_MONO", true),
            ("Headset:capture_FL", true),
            ("Headset:capture_FR", true),
            ("Headset:playback_FL", false),
            ("Headset:playback_FR", false),
        ],
    );
    server.add_port("Player:out_L", true);
    server.add_port("Recorder:in_1", false);
    let backend = manual(&server);
    let headset = Choice { output: Some("Headset"), input: Some("Headset"), period: None };
    let old = open(&backend, headset).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("transit:out_1", "Recorder:in_1");
    let (new, note) = reopen(&backend, old, Choice { input: Some("Mic"), ..headset });
    let _new = new.unwrap();
    let note = note.expect("a diagnostic");
    assert!(note.contains("in_1 from Player:out_L") && note.contains("the input device changed from Headset to Mic"), "{note}");
    assert!(!server.linked("Player:out_L", "transit:in_1"), "never an old patch on a different source");
    assert!(server.linked("Mic:capture_MONO", "transit:in_1"), "transit's own link to the new input");
    assert!(server.linked("transit:out_1", "Recorder:in_1"), "the output kept its meaning");
}

/// A device profile with fewer channels: the port beyond the new count loses its link, with the
/// diagnostic; the port that keeps its channel keeps its link.
#[test]
fn a_link_beyond_the_new_port_count_is_dropped_and_the_others_kept() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("Player:out_R", "transit:in_2");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    server.remove_physical("system:capture_2");
    let new = open(&backend, Choice::default()).unwrap();
    let note = new.render.restore_links(snapshot).expect("a diagnostic");
    assert!(note.contains("in_2 from Player:out_R (transit now has 1 input port)"), "{note}");
    assert!(server.linked("Player:out_L", "transit:in_1"));
}

#[test]
fn a_server_shutdown_restores_nothing() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.shutdown();
    assert!(old.render.snapshot_links().is_none(), "nothing is read once the server shut the client down");
    backend.tick();
    assert_eq!(*old.errors.lock().unwrap(), [ab::StreamError::DeviceNotAvailable; 2]);
    assert!(old.render.snapshot_links().is_none(), "nor after the dead client was closed");
    server.restart();
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    assert_eq!(note, None);
    assert!(!server.linked("Player:out_L", "transit:in_1"));
    assert!(!server.connects().iter().any(|(from, _)| from == "Player:out_L"), "no restore was even tried");
}

/// The server gives the replacement another name (another client took "transit" meanwhile): the
/// links land on the replacement's actual ports, a link between two of transit's own ports too.
#[test]
fn links_land_on_the_replacement_under_the_name_the_server_gave_it() {
    let server = server();
    let backend = manual(&server).without_auto_connect();
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("transit:out_1", "transit:in_2");
    server.outside_link("transit:out_2", "Recorder:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let squatter = server.open_client(crate::CLIENT_NAME).unwrap();
    assert_eq!(squatter.name(), "transit");
    let new = open(&backend, Choice::default()).unwrap();
    assert_eq!(backend.client_name().as_deref(), Some("transit-02"));
    assert_eq!(new.render.restore_links(snapshot), None);
    assert!(server.linked("Player:out_L", "transit-02:in_1"));
    assert!(server.linked("transit-02:out_1", "transit-02:in_2"));
    assert!(server.linked("transit-02:out_2", "Recorder:in_1"));
    assert_eq!(server.graph().len(), 3);
}

/// A link that is already there when the restore comes (the patchbay was faster) counts as
/// restored and is not asked for again, on a server that would duplicate it (PipeWire's libjack
/// makes a second link for a connect of an existing pair).
#[test]
fn a_link_that_already_exists_counts_as_restored_and_is_never_duplicated() {
    let server = server().duplicating_links();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("Player:out_R", "transit:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let new = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let before = server.graph();
    assert_eq!(new.render.restore_links(snapshot), None, "an existing link is not a failure");
    let after = server.graph();
    assert!(kept(&before, &after));
    assert_eq!(server.link_count("Player:out_L", "transit:in_1"), 1, "no duplicate on a duplicating server");
    assert_eq!(server.link_count("Player:out_R", "transit:in_1"), 1);
    let tried = server.connects().iter().filter(|(a, b)| a == "Player:out_L" && b == "transit:in_1").count();
    assert_eq!(tried, 0, "found among the port's links, so never asked for");
    assert!(own_links_made(&server, "transit"));
    assert_eq!(server.graph().len(), 6, "transit's own four and the two outside links, each once");
}

/// PipeWire's `jack.self-connect-mode` applies to a restored link (one end is transit's own port):
/// `ignore-*` answers 0 and makes no link, `fail-*` answers 1. The link is read back, so both are
/// failed restores, said once with the hint, and not retried.
#[test]
fn a_connect_the_server_ignores_or_refuses_is_a_failed_restore() {
    for mode in [crate::fake::SelfConnect::Ignore, crate::fake::SelfConnect::Fail] {
        let server = server();
        let backend = manual(&server);
        let old = open(&backend, Choice::default()).unwrap();
        server.outside_link("Player:out_L", "transit:in_1");
        let snapshot = old.render.snapshot_links().unwrap();
        drop(old);
        let new = open(&backend, Choice::default()).unwrap();
        server.set_self_connect(mode);
        let note = new.render.restore_links(snapshot).expect("a diagnostic");
        assert!(note.contains("in_1 from Player:out_L"), "{mode:?}: {note}");
        if mode == crate::fake::SelfConnect::Ignore {
            assert!(note.contains("the server did not make it; its self-connect setting may refuse links to transit's ports"), "{note}");
        } else {
            assert!(note.contains("jack_connect returned 1"), "{note}");
        }
        assert!(!server.linked("Player:out_L", "transit:in_1"));
        let tried = server.connects().iter().filter(|(a, _)| a == "Player:out_L").count();
        assert_eq!(tried, 1, "{mode:?}: asked once, not retried");
    }
}

/// The period of [`server`]'s graph in µs (256 frames at 48 kHz), the confirmation's step.
const PERIOD_US: u64 = 256 * 1_000_000 / 48_000;

/// JACK 2's double-buffered graph: `jack_connect` answers 0 at once, but the link shows only after
/// the server switched to the graph state the connect wrote, cycles later when a late client
/// postpones the switch. The restore reads the link again once per graph cycle until it shows:
/// restored, no diagnostic, nothing connected twice, and no longer waited than the cycles it took.
#[test]
fn a_link_the_graph_shows_cycles_after_the_connect_counts_as_restored() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("transit:out_1", "Recorder:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let new = open(&backend, Choice::default()).unwrap();
    server.delaying_links(3);
    let before = server.connects().len();
    let start = JackApi::now_us(&server);
    assert_eq!(new.render.restore_links(snapshot), None, "both came back; neither is called failed");
    assert!(server.linked("Player:out_L", "transit:in_1") && server.linked("transit:out_1", "Recorder:in_1"));
    assert_eq!(server.connects().len() - before, 2, "each asked for once; confirming connects nothing");
    assert_eq!(JackApi::now_us(&server) - start, 3 * PERIOD_US, "waited the three graph cycles and no more");
}

/// A link the server accepted and never shows (PipeWire's self-connect ignore mode, or a graph
/// that stays stalled) is a failed restore once the confirmation's limit has passed, not later: the
/// wait is bounded by `links::CONFIRM_LIMIT` plus at most one period.
#[test]
fn a_link_the_graph_never_shows_is_a_failed_restore_at_the_limit() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let new = open(&backend, Choice::default()).unwrap();
    server.delaying_links(u32::MAX);
    let start = JackApi::now_us(&server);
    let note = new.render.restore_links(snapshot).expect("a diagnostic");
    assert!(
        note.contains("in_1 from Player:out_L (the server did not make it; its self-connect setting may refuse links to transit's ports)"),
        "{note}"
    );
    let waited = JackApi::now_us(&server) - start;
    let limit = u64::try_from(crate::links::CONFIRM_LIMIT.as_micros()).unwrap();
    assert!((limit..limit + PERIOD_US).contains(&waited), "waited {waited} µs for a limit of {limit} µs");
    let tried = server.connects().iter().filter(|(a, _)| a == "Player:out_L").count();
    assert_eq!(tried, 1, "asked once, not retried while confirming");
}

/// JACK 2: another client makes the link between the restore's look and its connect; the connect
/// answers `EEXIST`, which counts as restored, and nothing is retried.
#[test]
fn eexist_from_the_connect_counts_as_restored() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let new = open(&backend, Choice::default()).unwrap();
    server.link_appears_on_connect("Player:out_L", "transit:in_1");
    assert_eq!(new.render.restore_links(snapshot), None, "EEXIST is not a failure");
    assert_eq!(server.link_count("Player:out_L", "transit:in_1"), 1);
    let tried = server.connects().iter().filter(|(a, b)| a == "Player:out_L" && b == "transit:in_1").count();
    assert_eq!(tried, 1, "asked once, answered EEXIST, not retried");
}

/// A remote port that went away between the snapshot and the restore, or that came back as
/// another type, is a failed restore: logged once, said once, not connected and not retried.
#[test]
fn a_vanished_or_changed_remote_port_is_a_failed_restore_and_not_retried() {
    let server = server();
    server.add_port("Synth:out", true);
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("Synth:out", "transit:in_2");
    server.outside_link("transit:out_1", "Recorder:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    server.remove_port("Player:out_L");
    server.remove_port("Synth:out");
    server.add_midi_port("Synth:out", true);
    let new = open(&backend, Choice::default()).unwrap();
    let note = new.render.restore_links(snapshot).expect("a diagnostic");
    assert!(note.starts_with("JACK: 2 links made outside transit were not restored"), "{note}");
    assert!(
        note.contains("in_1 from Player:out_L (no audio port with that name and direction now)")
            && note.contains("in_2 from Synth:out (no audio port with that name and direction now)"),
        "{note}"
    );
    assert!(server.linked("transit:out_1", "Recorder:in_1"), "the other link came back");
    let tried = |port: &str| server.connects().iter().filter(|(a, _)| a == port).count();
    assert_eq!((tried("Player:out_L"), tried("Synth:out")), (0, 0), "checked before connecting, never asked");
    // The port comes back and the watcher ticks on: nothing tries again.
    server.add_port("Player:out_L", true);
    for _ in 0..5 {
        backend.tick();
    }
    assert!(!server.linked("Player:out_L", "transit:in_1"));
    assert_eq!(tried("Player:out_L"), 0);
    drop(new);
}

/// The replacement does not open: the operation is abandoned with its snapshot, nothing is
/// half-restored, and a later open restores nothing from it.
#[test]
fn a_failed_open_discards_the_snapshot() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.set_refusing(Some("refused for the test"));
    let (new, note) = reopen(&backend, old, Choice::default());
    assert!(new.is_err() && note.is_none());
    assert!(!server.connects().iter().any(|(from, _)| from == "Player:out_L"));
    server.set_refusing(None);
    let _later = open(&backend, Choice::default()).unwrap();
    assert!(!server.linked("Player:out_L", "transit:in_1"), "the abandoned snapshot is gone");
}

/// A link equal to one of transit's own configured links is transit's, whoever made it: it is not
/// saved, and the replacement makes it once, as its own. A link from the same device port to
/// another of transit's ports is an outside link and comes back.
#[test]
fn a_link_equal_to_transits_own_is_transits_and_others_from_the_device_are_outside() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("system:capture_2", "transit:in_2");
    server.outside_link("system:capture_1", "transit:in_2");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    let before = server.connects().len();
    let new = open(&backend, Choice::default()).unwrap();
    assert_eq!(new.render.restore_links(snapshot), None);
    let since: Vec<(String, String)> = server.connects()[before..].to_vec();
    let asked = |a: &str, b: &str| since.iter().filter(|(x, y)| x == a && y == b).count();
    assert_eq!(asked("system:capture_2", "transit:in_2"), 1, "made once, by the replacement's own links, never restored");
    assert_eq!(asked("system:capture_1", "transit:in_2"), 1, "the outside link restored");
    assert!(server.linked("system:capture_1", "transit:in_2") && own_links_made(&server, "transit"));
}

/// `TRANSIT_JACK_AUTOCONNECT=0`: transit makes no link of its own, and every link the user made
/// comes back, one from the device port transit would have linked included.
#[test]
fn with_auto_connect_off_outside_links_come_back_and_none_of_transits_are_made() {
    let server = server();
    let backend = manual(&server).without_auto_connect();
    let old = open(&backend, Choice::default()).unwrap();
    assert!(server.graph().is_empty(), "no link of transit's own");
    server.outside_link("system:capture_1", "transit:in_1");
    server.outside_link("Player:out_R", "transit:in_2");
    server.outside_link("transit:out_1", "system:playback_1");
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    assert_eq!(note, None);
    let mut graph = server.graph();
    graph.sort();
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    assert_eq!(
        graph,
        [pair("Player:out_R", "transit:in_2"), pair("system:capture_1", "transit:in_1"), pair("transit:out_1", "system:playback_1")],
        "exactly the user's links"
    );
}

/// The snapshot is consumed by a later client only, and only by this backend's streams.
#[test]
fn a_snapshot_restores_only_on_a_later_client() {
    let server = server();
    let backend = manual(&server);
    let open_now = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let snapshot = open_now.render.snapshot_links().unwrap();
    let before = server.connects().len();
    assert_eq!(open_now.render.restore_links(snapshot), None);
    assert_eq!(server.connects().len(), before, "not restored on the client it came from");
    let foreign: ab::LinkSnapshot = Box::new(42u32);
    assert_eq!(open_now.render.restore_links(foreign), None, "another backend's snapshot is discarded");
    assert_eq!(server.connects().len(), before);
    // Once the client was deactivated its links are gone: there is nothing to read.
    open_now.capture.stop().unwrap();
    open_now.render.stop().unwrap();
    assert!(open_now.render.snapshot_links().is_none());
}

/// #431 review: two remote ports whose names differ only in bytes that are not UTF-8 collapse to
/// one name under a lossy copy. The one linked to transit (its name not UTF-8) is not saved, so the
/// restore never links the other one, which carries the lossy form as its real name.
#[test]
fn a_linked_port_whose_name_is_not_utf8_is_not_saved_and_never_confused_with_another() {
    let server = server();
    server.add_port("App:out_\u{FFFD}", true);
    server.add_port_with_invalid_name("App:out_<0xFF>", "App:out_\u{FFFD}", true);
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("App:out_<0xFF>", "transit:in_1");
    server.outside_link("Player:out_L", "transit:in_2");
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    let note = note.expect("a diagnostic");
    assert!(note.contains("in_1 from App:out_\u{FFFD} (the port's name is not valid UTF-8)"), "{note}");
    assert!(!server.linked("App:out_\u{FFFD}", "transit:in_1"), "never the other port");
    assert!(!server.connects().iter().any(|(a, _)| a.starts_with("App:out_")), "nothing asked for under that name");
    assert!(server.linked("Player:out_L", "transit:in_2"), "the other link came back");
}

/// #431 review: the server shuts down after the snapshot was read (while the old Player's editors
/// close) and is back before the replacement opens. The old client saw the shutdown, so the
/// snapshot is discarded: nothing is restored on the restarted server, and the user is told.
#[test]
fn a_server_shutdown_after_the_snapshot_restores_nothing() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let snapshot = old.render.snapshot_links().expect("read before the shutdown");
    server.shutdown();
    server.restart();
    drop(old);
    let new = open(&backend, Choice::default()).unwrap();
    let note = new.render.restore_links(snapshot).expect("a diagnostic");
    assert!(note.contains("in_1 from Player:out_L (the server stopped during the reopen)"), "{note}");
    assert!(!server.linked("Player:out_L", "transit:in_1"));
    assert!(!server.connects().iter().any(|(a, _)| a == "Player:out_L"), "no restore was tried");
}

/// #431 review: the server goes down during the reopen, after the old client closed: the
/// replacement does not open, the operation ends with its snapshot, and the open after the server's
/// return restores nothing.
#[test]
fn a_server_shutdown_during_the_reopen_restores_nothing() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    server.shutdown();
    assert!(open(&backend, Choice::default()).is_err(), "no server, no replacement");
    drop(snapshot);
    server.restart();
    let _later = open(&backend, Choice::default()).unwrap();
    assert!(!server.linked("Player:out_L", "transit:in_1"));
    assert!(!server.connects().iter().any(|(a, _)| a == "Player:out_L"));
}

#[test]
fn outside_links_come_back_after_a_reopen_for_a_rate_change() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("transit:out_1", "Recorder:in_1");
    server.set_sample_rate(44_100);
    backend.tick();
    assert_eq!(*old.errors.lock().unwrap(), [ab::StreamError::ResetRequested; 2], "the rate change asks to reopen");
    let (new, note) = reopen(&backend, old, Choice::default());
    let _new = new.unwrap();
    assert_eq!(note, None);
    assert!(server.linked("Player:out_L", "transit:in_1") && server.linked("transit:out_1", "Recorder:in_1"));
}

/// The same input device lists its capture ports in another order after the reopen: each of
/// transit's input ports now stands for another device channel, so its outside links are dropped
/// with the diagnostic, never moved onto another source. The output side kept its meaning.
#[test]
fn a_channel_reorder_on_the_same_device_drops_the_links_of_the_channels_that_moved() {
    let server = server();
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.outside_link("Player:out_R", "transit:in_2");
    server.outside_link("transit:out_1", "Recorder:in_1");
    let snapshot = old.render.snapshot_links().unwrap();
    drop(old);
    server.remove_physical("system:capture_1");
    server.add_physical("system:capture_1", true);
    let new = open(&backend, Choice::default()).unwrap();
    assert!(server.linked("system:capture_2", "transit:in_1"), "the device's order changed");
    let note = new.render.restore_links(snapshot).expect("a diagnostic");
    assert!(note.contains("in_1 from Player:out_L (in_1 now stands for another channel)"), "{note}");
    assert!(note.contains("in_2 from Player:out_R (in_2 now stands for another channel)"), "{note}");
    assert!(!server.linked("Player:out_L", "transit:in_1") && !server.linked("Player:out_R", "transit:in_2"));
    assert!(server.linked("transit:out_1", "Recorder:in_1"));
}

/// Rule 6: transit never disconnects. The function table and the client trait have no disconnect
/// call (`jack_disconnect`, `jack_port_disconnect`), so only a deactivation or a close (the
/// server's own disconnect of an inactive client) ever removes one of transit's links.
#[test]
fn the_function_table_binds_no_disconnect() {
    let banned = [concat!("jack_", "disconnect"), concat!("jack_port_", "disconnect"), concat!("fn ", "disconnect")];
    for (file, text) in [("sys.rs", include_str!("sys.rs")), ("ffi.rs", include_str!("ffi.rs")), ("api.rs", include_str!("api.rs"))] {
        for word in banned {
            assert!(!text.contains(word), "{file} names {word}");
        }
    }
}

#[test]
fn a_name_pattern_matches_only_the_name() {
    assert_eq!(crate::links::exact_pattern("system:capture_1"), "^system:capture_1$");
    assert_eq!(crate::links::exact_pattern("App (2):out.L [x]*"), r"^App \(2\):out\.L \[x\]\*$");
    assert_eq!(crate::links::exact_pattern(r"a\b|c^d$e+f?g{1}"), r"^a\\b\|c\^d\$e\+f\?g\{1\}$");
}

/// #760 slice 1 review: a replacement client that latched a rebuild (the graph runs another rate
/// than it was prepared for) is silent and about to be replaced, so it restores nothing; it keeps
/// the snapshot, and the reopen that replaces it hands it on, so the rebuilt client restores it.
#[test]
fn a_client_that_latched_a_rebuild_restores_no_links_and_the_rebuilt_client_does() {
    let server = FakeServer::pipewire(48_000, 256, (48_000, 256));
    server.add_port("Player:out_L", true);
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    // The graph moves to 44.1 kHz: the replacement, planned at the remembered 48 kHz, latches.
    server.set_sample_rate(44_100);
    let (latched, note) = reopen(&backend, old, Choice::default());
    let latched = latched.unwrap();
    assert!(latched.render.needs_rebuild());
    assert_eq!(note, None, "nothing failed: nothing was tried");
    assert!(!server.linked("Player:out_L", "transit:in_1"), "the latched client restored nothing");
    assert!(server.connects().iter().all(|(a, _)| a != "Player:out_L"), "not even asked for");
    // The automatic reopen takes the held snapshot back; the rebuilt client restores it.
    let (rebuilt, note) = reopen(&backend, latched, Choice::default());
    let rebuilt = rebuilt.unwrap();
    assert!(!rebuilt.render.needs_rebuild());
    assert_eq!(note, None);
    assert!(server.linked("Player:out_L", "transit:in_1"), "the rebuilt client restored the outside link");
}

/// #760: a reopen of the latched client that is refused after it read the snapshot (the app's
/// `replace` returns early when the old Player refuses to close its editors or input) drops its
/// copy; the latched client still holds the snapshot, so the next reopen restores the links.
#[test]
fn a_refused_reopen_of_a_latched_client_leaves_its_held_links_for_the_next_reopen() {
    let server = FakeServer::pipewire(48_000, 256, (48_000, 256));
    server.add_port("Player:out_L", true);
    let backend = manual(&server);
    let old = open(&backend, Choice::default()).unwrap();
    server.outside_link("Player:out_L", "transit:in_1");
    server.set_sample_rate(44_100);
    let (latched, _) = reopen(&backend, old, Choice::default());
    let latched = latched.unwrap();
    assert!(latched.render.needs_rebuild());
    // The refused reopen: read, then dropped with the call; the latched client stays open.
    let refused = latched.render.snapshot_links();
    assert!(refused.is_some(), "the latched client hands out what it holds");
    drop(refused);
    let (rebuilt, note) = reopen(&backend, latched, Choice::default());
    let rebuilt = rebuilt.unwrap();
    assert!(!rebuilt.render.needs_rebuild());
    assert_eq!(note, None);
    assert!(server.linked("Player:out_L", "transit:in_1"), "the outside link survived the refused reopen");
}
