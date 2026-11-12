//! Section 11: deterministic windows, usable results, soft edges and slot reuse.
#![allow(clippy::unwrap_used, clippy::panic)]

use super::tests::{Fake, STALL, set_of};
use super::*;
use crate::task_graph::{StartAt, EdgeKind, GraphHost, GraphRun, Rendered, Start, TaskGraph, Window, drive};

struct Host {
    set: WorkerSet,
    starts: Vec<Start>,
    usable: Vec<bool>,
    seen: Vec<(usize, Window)>,
    rendered: Vec<usize>,
    taken: Vec<usize>,
    skipped: Vec<(usize, usize)>,
    /// Starts and callback renders, in call order.
    log: Vec<(&'static str, usize)>,
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
            log: Vec::new(),
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
    fn start(&mut self, t: usize, at: StartAt) -> Start {
        let window = at.window;
        self.seen.push((t, window));
        self.log.push(("start", t));
        self.starts[t]
    }
    fn refused(&mut self, _: usize) {
        panic!("unexpected refusal");
    }
    fn render_here(&mut self, t: usize) -> Rendered {
        self.rendered.push(t);
        self.log.push(("render", t));
        if self.usable[t] { Rendered::Usable } else { Rendered::Unusable }
    }
    fn poll(&mut self, deadline: Instant) -> Poll {
        // Step each worker to completion before the poll, with injected equal stamps.
        let shared = Arc::clone(&self.set.shared);
        while let Some((slot, token)) = shared.jobs.try_claim_next(0) {
            let stamp = Instant::now();
            shared.run_attempt(slot, token, || stamp);
        }
        self.set.poll(deadline, |_| panic!("every published task already completed"))
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

/// G4: in the sweep (the dependents of a task a worker finished), every ready task of the batch is
/// started, and the published ones woken, before the callback renders any of its own; the
/// callback's renders come in table order. Mutation: rendering a callback task inside the step
/// loop (as the wave did not) puts `render` between the starts and reverses the renders.
#[test]
fn g4_sweep_starts_the_whole_batch_before_a_callback_render_in_table_order() {
    let edges: Vec<_> = (1..5).map(|t| (0, t, EdgeKind::Soft)).collect();
    let graph = TaskGraph::with_slots(1, &[Some(0), Some(1), Some(2), Some(3), Some(4)], &edges);
    let mut host = Host::new(5, 5);
    host.starts = vec![Start::Publish, Start::Callback, Start::Publish, Start::Callback, Start::Publish];
    let report = host.drive(&graph, false);
    let last_start = host.log.iter().rposition(|e| e.0 == "start").unwrap();
    let first_render = host.log.iter().position(|e| e.0 == "render").unwrap();
    assert!(last_start < first_render, "a callback render ran before the batch was started: {:?}", host.log);
    assert_eq!(host.rendered, [1, 3], "callback work in table order");
    assert_eq!((report.mixed, host.set.stats().misuse), (3, 0));
}

/// G5: a published tail task the callback refuses by admission (barred after a recent overrun, or
/// a worst case longer than the time left) is taken before the deadline when the callback waits
/// for nothing a worker holds, and counted as a pre-deadline reclaim, not left to spin until the
/// deadline (where a worker could still claim it and silence the bus). Zero workers, a 60 s
/// deadline, no clock assertion. Mutation: dropping the tail rule spins to the deadline, which
/// then reclaims it (`unclaimed_at_deadline` 1, `late`).
#[test]
fn g5_an_unclaimed_tail_task_the_admission_refuses_is_taken_before_the_deadline() {
    for barred in [true, false] {
        let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
        assert!(set.begin_block(1));
        if barred {
            set.set_cost_estimate(0, Duration::from_nanos(1));
            set.bar(0);
        } else {
            set.set_cost_estimate(0, Duration::from_secs(3600));
        }
        set.set_inline_at_deadline(0);
        assert_eq!(set.publish(0), Ok(Published::Published));
        let mut inlined = 0;
        let poll = set.poll(Instant::now() + STALL, |_| inlined += 1);
        assert_eq!(poll, Poll::Resolved { strip: 0, outcome: Outcome::Inline, late: false }, "barred {barred}");
        let q = set.queue_snapshot();
        assert_eq!((inlined, q.inline_pre, q.unclaimed_at_deadline, q.deadline_reclaimed_tail), (1, 1, 0, 0), "barred {barred}");
        assert!(q.conserves());
        assert_eq!(set.poll(Instant::now() + STALL, |_| panic!("rendered twice")), Poll::Finished);
    }
}

/// G5 with the no-reclaim seam: the tail rule is a pre-deadline reclaim, so the seam that
/// suppresses those suppresses it; the deadline reclaim still renders the task (B2).
#[test]
fn g5_the_no_reclaim_seam_also_suppresses_the_tail_rule() {
    use super::seams::SlotOverrides;
    let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
    set.arm_slot_overrides(0, SlotOverrides { no_reclaim: true, ..SlotOverrides::default() });
    set.install_test_seams();
    assert!(set.begin_block(1));
    set.set_cost_estimate(0, Duration::from_secs(3600));
    set.set_inline_at_deadline(0);
    assert_eq!(set.publish(0), Ok(Published::Published));
    let mut inlined = 0;
    // A deadline already passed: no pre-deadline window at all, so nothing waits on the clock.
    let deadline = Instant::now();
    assert_eq!(set.poll(deadline, |_| inlined += 1), Poll::Resolved { strip: 0, outcome: Outcome::Inline, late: true });
    let q = set.queue_snapshot();
    assert_eq!((inlined, q.inline_pre, q.unclaimed_at_deadline, q.deadline_reclaimed_tail), (1, 0, 1, 1));
}


/// #215 item 1, through the race seam: a worker's claim and the callback's deadline reclaim of a
/// published tail task, in both orders. Worker first: the policy of `claimed_at_deadline` (G10:
/// `Late`, silent for the block, the result retired unread, never rendered by the callback).
/// Callback first: rendered inline at the deadline, and the worker's claim fails. Either way the
/// task is executed exactly once. Mutation: dropping the seam's call before the deadline reclaim
/// leaves the worker-first case without a claim (the token is `None`).
#[test]
fn a_claim_at_the_deadline_in_both_orders_executes_the_tail_task_exactly_once() {
    for worker_first in [true, false] {
        let mut set = set_of(&Fake::new(1, Duration::ZERO), 0);
        let shared = Arc::clone(&set.shared);
        assert!(set.begin_block(1));
        set.set_cost_estimate(0, Duration::from_nanos(1));
        set.set_inline_at_deadline(0);
        assert_eq!(set.publish(0), Ok(Published::Published));
        let mut inlined = 0u64;
        let mut token = None;
        let mut races = 0;
        // Passed before the poll reads the clock: only the deadline's reclaim is attempted.
        let deadline = Instant::now();
        let poll = set.poll_racing(
            deadline,
            |_| inlined += 1,
            |strip| {
                races += 1;
                if worker_first && strip == 0 {
                    token = shared.jobs.slot(0).unwrap().try_claim();
                }
            },
        );
        assert_eq!(races, 1, "one reclaim attempt, at the deadline");
        if worker_first {
            assert_eq!(poll, Poll::Resolved { strip: 0, outcome: Outcome::Overrun(OverrunCause::Late), late: true });
            assert_eq!(inlined, 0);
            // The worker finishes later; its result is retired unread at the next block.
            shared.run_attempt(0, token.take().expect("the worker claimed at the deadline"), Instant::now);
            assert!(set.begin_block(2));
            assert_eq!(set.stats().late_results, 1);
            assert!(set.inline_barred(0), "a late claim bars inline renders");
        } else {
            assert_eq!(poll, Poll::Resolved { strip: 0, outcome: Outcome::Inline, late: true });
            assert_eq!(inlined, 1);
            assert!(shared.jobs.slot(0).unwrap().try_claim().is_none(), "the worker's claim after the reclaim fails");
            assert_eq!(set.queue_snapshot().deadline_reclaimed_tail, 1);
        }
        let st = set.stats();
        assert_eq!(st.worker_runs + inlined, 1, "executed exactly once (worker first {worker_first})");
        assert_eq!(st.misuse, 0);
    }
}
