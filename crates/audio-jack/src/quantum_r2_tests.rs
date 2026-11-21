//! #760, second review round: loops the first fix left. An accepted buffer-size request the linked
//! graph does not run; outside links (restored by the reopen, or patched by hand) that move the
//! node to another quantum; a rate that depends on the links. Plus the shipping path's one reset
//! per direction for a change after the baseline is committed. Over the fake's PipeWire mode with
//! a driver running the client every graph step; the watcher is driven by hand.

use crate::JackBackend;
use crate::fake::FakeServer;
use crate::settle_tests::{Opened, STEP, backend_on, errors, open, session_opens};
use transit_audio_backend as ab;

/// The Player's automatic reopen allows this many attempts in a row (restated as in
/// `quantum_tests`).
const REOPEN_ATTEMPTS: usize = 3;

/// Graph steps (with a watcher tick after each) one open runs before the test looks again.
const RUN_STEPS: usize = 12;

fn desktop(rate: u32, buffer: u32) -> FakeServer {
    let server = FakeServer::pipewire(rate, buffer, (rate, buffer));
    server.driving();
    server
}

/// Runs the graph for [`RUN_STEPS`] steps with a watcher tick after each.
fn run(backend: &JackBackend, server: &FakeServer) {
    for _ in 0..RUN_STEPS {
        server.step(STEP);
        backend.tick();
    }
}

/// The reopen path as the backend sees it, with the link snapshot the app's `replace` carries:
/// snapshot the old client, drop it, open and start the new one, restore the snapshot on it. A
/// reset or a latched rebuild ends that open; at most [`REOPEN_ATTEMPTS`] reopens follow `first`.
/// Returns how many opens ended in a reset or a rebuild, and the open still running.
fn reopen_like_the_app(backend: &JackBackend, server: &FakeServer, first: Opened, period: Option<u32>) -> (usize, Option<Opened>) {
    let mut troubles = 0;
    let mut current = first;
    for _ in 0..REOPEN_ATTEMPTS {
        let snapshot = current.render.snapshot_links();
        drop(current);
        let opened = open(backend, period);
        opened.start().expect("the start");
        if let Some(snapshot) = snapshot {
            let _ = opened.render.restore_links(snapshot);
        }
        run(backend, server);
        if !opened.render.needs_rebuild() && !errors(&opened).contains(&ab::StreamError::ResetRequested) {
            return (troubles, Some(opened));
        }
        troubles += 1;
        current = opened;
    }
    (troubles, None)
}

/// Opens and starts the first client of a sequence and runs the graph.
fn first_open(backend: &JackBackend, server: &FakeServer, period: Option<u32>) -> Opened {
    let opened = open(backend, period);
    opened.start().expect("the start");
    run(backend, server);
    opened
}

#[test]
fn an_accepted_request_the_linked_graph_does_not_run_settles_on_what_runs_and_says_so() {
    // 2048 is chosen and accepted, but a global force pins the graph at 1024; every fresh client
    // first publishes 2048 (the review's scenario).
    let server = desktop(48_000, 1024);
    server.script_pipewire(|m| {
        m.pinned = Some(1024);
        m.stale_each = Some((48_000, 2048));
    });
    let backend = backend_on(&server);
    let first = first_open(&backend, &server, Some(2048));
    assert_eq!(first.render.period(), Some(1024), "the size the linked graph runs, not the activation figure");
    assert!(errors(&first).is_empty(), "no reset");
    assert!(first.render.period_declined(), "the request was not kept, said as declined");
    let (troubles, running) = reopen_like_the_app(&backend, &server, first, Some(2048));
    assert_eq!(troubles, 0);
    let running = running.expect("the device stays open");
    assert_eq!(running.render.period(), Some(1024));
    assert!(running.render.period_declined());
    assert_eq!(session_opens(&server), 2, "the first open and the test's own reopen");
}

#[test]
fn a_request_overridden_after_the_start_reopens_once_and_the_reopen_settles_on_what_runs() {
    // 2048 is chosen and arrives at once; three steps in another program forces 1024 on every
    // client (the graph moves after the start).
    let server = desktop(48_000, 1024);
    server.script_pipewire(|m| m.moves_each = Some((2048, 3, 1024)));
    let backend = backend_on(&server);
    let first = open(&backend, Some(2048));
    first.start().unwrap();
    assert_eq!(first.render.period(), Some(2048));
    assert!(!first.render.period_declined());
    run(&backend, &server);
    assert_eq!(errors(&first), [ab::StreamError::ResetRequested; 2], "the move after the start is one reset");
    let (troubles, running) = reopen_like_the_app(&backend, &server, first, Some(2048));
    assert_eq!(troubles, 0, "the reopen waits for the size the graph ran, not the request again");
    let running = running.expect("the device stays open");
    assert_eq!(running.render.period(), Some(1024));
    assert!(running.render.period_declined(), "the request was not kept");
}

/// An outside link to `Mixer:in_FL` moves transit's node to a graph at 512; without it the graph
/// runs 1024.
fn mixer_desktop() -> FakeServer {
    let server = desktop(48_000, 1024);
    server.add_port("Mixer:in_FL", false);
    server.script_pipewire(|m| m.link_quantum = Some(("Mixer:in_FL".to_owned(), 512, 1024)));
    server
}

#[test]
fn a_restored_outside_link_that_moves_the_quantum_takes_part_before_the_baseline() {
    for auto_connect in [true, false] {
        let server = mixer_desktop();
        let backend = if auto_connect { backend_on(&server) } else { backend_on(&server).without_auto_connect() };
        let first = first_open(&backend, &server, None);
        assert_eq!(first.render.period(), Some(1024));
        // The user patches transit's first output into the mixer: the node moves to 512.
        server.outside_link("transit:out_1", "Mixer:in_FL");
        run(&backend, &server);
        assert_eq!(errors(&first), [ab::StreamError::ResetRequested; 2], "the patch is one reset ({auto_connect})");
        let (troubles, running) = reopen_like_the_app(&backend, &server, first, None);
        assert_eq!(troubles, 0, "the reopen settles with the restored link in place ({auto_connect})");
        let running = running.expect("the device stays open");
        assert_eq!(running.render.period(), Some(512));
        assert_eq!(server.link_count("transit:out_1", "Mixer:in_FL"), 1, "restored once, never twice ({auto_connect})");
    }
}

#[test]
fn a_rate_that_depends_on_the_links_converges_after_one_rebuild() {
    // A fresh unlinked client reads 48 kHz; once linked the graph runs 44.1 kHz.
    let server = desktop(48_000, 1024);
    server.script_pipewire(|m| m.rate_each = Some((48_000, 44_100)));
    let backend = backend_on(&server);
    let first = first_open(&backend, &server, None);
    assert!(first.render.needs_rebuild(), "prepared at 48 kHz, the linked graph runs 44.1 kHz");
    let (troubles, running) = reopen_like_the_app(&backend, &server, first, None);
    assert_eq!(troubles, 0, "the rebuild prepared at the remembered 44.1 kHz reaches the linked graph");
    let running = running.expect("the device runs");
    assert_eq!(running.achieved.format.rate, 44_100);
    assert!(errors(&running).is_empty());
}

#[test]
fn a_change_after_the_baseline_is_one_reset_per_direction_across_ticks() {
    let server = desktop(48_000, 1024);
    let backend = backend_on(&server);
    let opened = open(&backend, None);
    opened.start().unwrap();
    assert_eq!(opened.render.period(), Some(1024));
    server.set_buffer_size(256);
    for _ in 0..4 {
        backend.tick();
    }
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 2], "one per direction, however many ticks see it");
    server.set_buffer_size(512);
    for _ in 0..4 {
        backend.tick();
    }
    assert_eq!(errors(&opened), [ab::StreamError::ResetRequested; 4], "a further change is one more per direction");
    let stats = opened.render.stats().expect("stats");
    assert_eq!(stats.resets, 2, "the watcher counted two reset requests");
}

#[test]
fn an_abandoned_reopen_restores_nothing_on_a_later_open() {
    let server = mixer_desktop();
    let backend = backend_on(&server);
    let first = first_open(&backend, &server, None);
    server.outside_link("transit:out_1", "Mixer:in_FL");
    run(&backend, &server);
    // The reopen took its snapshot and then ended early: the snapshot is dropped with it.
    let snapshot = first.render.snapshot_links();
    assert!(snapshot.is_some());
    drop(snapshot);
    drop(first);
    let later = open(&backend, None);
    later.start().unwrap();
    assert!(!server.linked("transit:out_1", "Mixer:in_FL"), "nothing of the abandoned reopen's snapshot is restored");
}
