//! Section 11: deterministic windows, usable results, soft edges and slot reuse.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::tests::{Fake, STALL, set_of};
use super::*;
use crate::task_graph::{EdgeKind, GraphHost, GraphRun, Rendered, Start, TaskGraph, Window, drive};

struct Host {
    set: WorkerSet,
    starts: Vec<Start>,
    usable: Vec<bool>,
    seen: Vec<(usize, Window)>,
    rendered: Vec<usize>,
    taken: Vec<usize>,
    skipped: Vec<(usize, usize)>,
}

impl Host {
    fn new(tasks: usize, slots: usize) -> Self {
        Self {
            set: set_of(&Fake::new(slots, Duration::ZERO), 0),
            starts: vec![Start::Callback; tasks],
            usable: vec![true; tasks],
            seen: Vec::new(),
            rendered: Vec::new(),
            taken: Vec::new(),
            skipped: Vec::new(),
        }
    }

    fn drive(&mut self, graph: &TaskGraph, closed: bool) -> BlockReport {
        assert!(self.set.begin_block(1));
        let deadline = if closed { Instant::now() } else { Instant::now() + STALL };
        drive(self, graph, &mut GraphRun::new(graph.tasks()), deadline)
    }
}

impl GraphHost for Host {
    fn set(&mut self) -> &mut WorkerSet {
        &mut self.set
    }
    fn start(&mut self, t: usize, window: Window) -> Start {
        self.seen.push((t, window));
        self.starts[t]
    }
    fn refused(&mut self, _: usize) {
        panic!("unexpected refusal");
    }
    fn render_here(&mut self, t: usize) -> Rendered {
        self.rendered.push(t);
        if self.usable[t] { Rendered::Usable } else { Rendered::Unusable }
    }
    fn poll(&mut self, _: Instant) -> Poll {
        // Step each worker to completion before the poll, with injected equal stamps.
        let shared = Arc::clone(&self.set.shared);
        while let Some((slot, token)) = shared.jobs.try_claim_next(0) {
            let stamp = Instant::now();
            shared.run_attempt(slot, token, || stamp);
        }
        self.set.poll(Instant::now(), |_| panic!("every published task already completed"))
    }
    fn take(&mut self, t: usize, _: Outcome) -> bool {
        self.taken.push(t);
        self.usable[t]
    }
    fn skipped(&mut self, t: usize, origin: usize) {
        self.skipped.push((t, origin));
    }
}

/// Mutation: making a soft edge hard skips task 1 or the commit after skipped task 2.
#[test]
fn r1_soft_failure_satisfies_a_dependent() {
    let graph =
        TaskGraph::with_slots(1, &[Some(2), Some(0), Some(1), None], &[(0, 1, EdgeKind::Soft), (0, 2, EdgeKind::Hard), (2, 3, EdgeKind::Soft)]);
    let mut host = Host::new(4, 3);
    host.usable[0] = false;
    host.usable[3] = false; // A control task succeeds even with this host result.
    let report = host.drive(&graph, true);
    assert!(host.rendered.contains(&1) && host.rendered.contains(&3));
    assert!(!host.rendered.contains(&2));
    assert_eq!(host.skipped, [(2, 0)]);
    assert_eq!(host.set.figures().strip(1).unwrap().upstream_origin, Some(2), "task origins map to slots too");
    assert_eq!((report.upstream_missed, host.set.stats().misuse), (1, 0));
}

/// Mutation: changing the legacy constructor's edge kinds or identity slots breaks equality.
#[test]
fn all_hard_identity_map_equivalence() {
    let edges = [(0, 2), (1, 2), (2, 3), (0, 2)];
    let explicit: Vec<_> = edges.iter().map(|&(a, b)| (a, b, EdgeKind::Hard)).collect();
    assert_eq!(TaskGraph::new(9, 4, &edges), TaskGraph::with_slots(9, &[Some(0), Some(1), Some(2), Some(3)], &explicit));
    let hard = TaskGraph::with_slots(9, &[Some(0), Some(1)], &[(0, 1, EdgeKind::Hard), (0, 1, EdgeKind::Soft)]);
    let soft = TaskGraph::with_slots(9, &[Some(0), Some(1)], &[(0, 1, EdgeKind::Soft)]);
    assert_eq!(hard.dependent_kinds(0), [EdgeKind::Hard]);
    assert_eq!(hard.edges(), 1);
    assert_ne!(hard.edge_hash(1), soft.edge_hash(1), "kinds participate in identity");
}

/// Mutation: forgoing before start loses the Closed-window Busy outcome and callback render.
#[test]
fn r2_closed_window_outcomes_forgo_and_busy_without_a_wake() {
    let graph = TaskGraph::with_slots(1, &[Some(1), Some(0), None], &[(0, 2, EdgeKind::Soft), (1, 2, EdgeKind::Soft)]);
    let mut host = Host::new(3, 2);
    assert!(host.set.begin_block(1));
    assert_eq!(host.set.publish(0), Ok(Published::Published));
    let shared = Arc::clone(&host.set.shared);
    let token = shared.jobs.slot(0).unwrap().try_claim().unwrap();
    host.starts[0] = Start::Forgo;
    host.starts[1] = Start::Busy;
    assert!(host.set.begin_block(2));
    let report = drive(&mut host, &graph, &mut GraphRun::new(3), Instant::now());
    assert_eq!(host.seen, [(0, Window::Closed), (1, Window::Closed), (2, Window::Closed)]);
    assert_eq!(host.rendered, [2]);
    assert_eq!(host.set.outcome(0), Some(Outcome::Overrun(OverrunCause::Busy)));
    assert_eq!(host.set.outcome(1), Some(Outcome::Overrun(OverrunCause::Unstarted)));
    assert_eq!(host.set.wake_cursor, 0);
    assert_eq!(report.overrun, 2);
    token.complete();
}

/// Mutation: using task numbers as slots would publish slot 1 and fail to map its resolution.
#[test]
fn mapped_worker_results_use_task_numbers_for_the_host() {
    let graph = TaskGraph::with_slots(1, &[None, Some(0), None], &[(0, 1, EdgeKind::Soft), (1, 2, EdgeKind::Hard)]);
    let mut host = Host::new(3, 1);
    host.starts[1] = Start::Publish;
    host.set.set_cost_estimate(0, Duration::from_nanos(1));
    host.drive(&graph, false);
    assert_eq!(host.taken, [1]);
    assert_eq!(host.rendered, [0, 2]);
    assert_eq!(host.set.stats().misuse, 0);
}

/// Mutation: a slotless call to publish, forgo or skip would increment misuse (capacity zero).
#[test]
fn r3_control_tasks_never_reach_the_worker_set() {
    let graph = TaskGraph::with_slots(1, &[None; 4], &[(0, 1, EdgeKind::Soft), (1, 2, EdgeKind::Soft), (2, 3, EdgeKind::Soft)]);
    let mut host = Host::new(4, 0);
    host.starts = vec![Start::Callback, Start::Ready, Start::Callback, Start::Ready];
    host.usable.fill(false);
    let report = host.drive(&graph, true);
    assert_eq!(host.seen.len(), 4, "control tasks always resolve successfully");
    assert_eq!(host.rendered, [0, 2]);
    assert_eq!(report, BlockReport::default());
    assert_eq!(host.set.stats().misuse, 0);
}

/// Mutation: permitting Publish in Closed removes this assertion.
#[test]
#[should_panic(expected = "a task attempted to publish in the Closed window")]
fn closed_window_never_publishes() {
    let mut host = Host::new(1, 1);
    host.starts[0] = Start::Publish;
    host.drive(&TaskGraph::new(1, 1, &[]), true);
}

/// Mutation: retaining the flag makes the reused phase-1 slot render at the deadline.
#[test]
fn r4_slot_reuse_clears_inline_at_deadline() {
    let mut set = set_of(&Fake::new(2, Duration::ZERO), 0);
    assert!(set.begin_block(1));
    set.set_inline_at_deadline(0);
    set.set_inline_at_deadline(1); // Even an unused slot must be cleared.
    assert_eq!(set.publish(0), Ok(Published::Published));
    assert!(matches!(set.poll(Instant::now(), |_| {}), Poll::Resolved { outcome: Outcome::Inline, .. }));
    assert!(set.begin_block(2));
    for slot in 0..2 {
        assert_eq!(set.publish(slot), Ok(Published::Published));
    }
    let report = set.finish(Instant::now(), |_| panic!("the phase-1 flag leaked"));
    assert_eq!(report.overrun, 2);
}

/// Mutation: updating only the EWMA, or seeding instead of recording, changes these exact values.
#[test]
fn t18_callback_cost_updates_both_worker_figures() {
    let mut set = set_of(&Fake::new(2, Duration::ZERO), 0);
    for ns in [6_400, 64] {
        set.record_callback_cost(0, Duration::from_nanos(ns));
        set.shared.record_cost(1, Duration::from_nanos(ns));
        assert_eq!(set.cost_estimate(0), set.cost_estimate(1));
        assert_eq!(set.worst_case(0), set.worst_case(1));
    }
    assert_eq!(set.cost_estimate(0), Some(Duration::from_nanos(4_816)));
    assert_eq!(set.worst_case(0), Some(Duration::from_nanos(6_375)));
    assert_eq!(set.shared.queue.counts.snapshot().tail_callback, 2);
}

/// Mutation: installing at each `begin_block` or retaining current overrides breaks this lifetime.
#[test]
fn seams_default_off_and_reset_per_host_block() {
    use super::seams::{CostClock, SlotOverrides};
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    let off = SlotOverrides::default();
    assert_eq!(set.slot_overrides(0), off);
    assert_eq!(set.cost_clock(), CostClock::Real);
    let on = SlotOverrides { budget_left: Some(Duration::from_nanos(42)), force_publish: true, no_reclaim: true };
    set.arm_slot_overrides(0, on);
    set.arm_cost_clock(CostClock::Fixed(Duration::from_nanos(6400)));
    assert_eq!(set.slot_overrides(0), off, "pending until host entry");
    set.install_test_seams();
    for block in 1..=2 {
        assert!(set.begin_block(block));
        assert_eq!(set.slot_overrides(0), on, "unchanged across sub-blocks");
    }
    let stamp = Instant::now();
    let cost = set.cost_clock().elapsed(stamp, stamp);
    assert_eq!(cost, Duration::from_nanos(6400));
    set.record_callback_cost(0, cost);
    assert!(!set.admits(0, stamp, stamp + STALL), "no pre-deadline reclaim");
    set.set_inline_at_deadline(0);
    assert_eq!(set.publish(0), Ok(Published::Published));
    assert!(matches!(set.poll(stamp, |_| {}), Poll::Resolved { outcome: Outcome::Inline, .. }));
    set.install_test_seams(); // A serial block still installs/expires overrides.
    assert_eq!(set.slot_overrides(0), off);
    assert_eq!(set.cost_clock(), CostClock::Real);
    set.install_test_seams();
    assert_eq!(set.slot_overrides(0), off);
}
