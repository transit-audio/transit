//! The dependency graph through a real worker set (issue #174, ADR 0029 amendment section 8):
//! dependents published only when their sources are ready, the failure rule (a source late, busy
//! or unstarted skips its dependents, counted upstream-missed once per dependent with the origin,
//! while unrelated strips render), a stale result retired with its dependents skipped, a graph
//! change while a worker still holds an old task, and zero allocations on the callback.
//!
//! Timing: no assertion depends on the clock. A late or busy source is the fake runner's stall
//! (released by the test); the host's first poll waits on observable state (the stall entered,
//! the unrelated strips' slots `Done`) and then polls with a deadline already passed, so what is
//! resolved at that point is decided by construction, not by scheduling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{Behavior, Fake, STALL, set_of, wait_until};
use super::*;
use crate::task_graph::{GraphHost, GraphRun, Start, TaskGraph, drive};
use crate::testing::measure;

/// The test's side of the driver: the fake strips, the edges (to check the publication order),
/// and a record of what the driver asked for.
struct Host {
    set: WorkerSet,
    fake: Arc<Fake>,
    g: u64,
    edges: Vec<(usize, usize)>,
    /// Pinned to the callback.
    pinned: Vec<bool>,
    /// `take` reports a stale result for these.
    stale: Vec<bool>,
    /// Ready for dependents this block (taken, rendered here).
    done: Vec<bool>,
    /// Per strip: the identity it was started with (graph generation, edge hash, block).
    job: Vec<(u64, u64, u64)>,
    /// The table in effect (its generation and each task's edge hash).
    graph: Arc<TaskGraph>,
    started: Vec<usize>,
    taken: Vec<usize>,
    here: Vec<usize>,
    skipped: Vec<(usize, usize)>,
    /// Waited for before the block's first poll.
    gate: Option<fn(&Host) -> bool>,
    gated: bool,
    /// The deadline the host's polls use (`None`: the driver's).
    deadline: Option<Instant>,
    /// The driver's deadline (`None`: far away).
    drive_deadline: Option<Instant>,
    /// Waited for inside each render on the callback.
    here_gate: Option<fn(&Host) -> bool>,
}

impl Host {
    fn new(fake: &Arc<Fake>, workers: usize, graph: TaskGraph, edges: &[(usize, usize)]) -> Host {
        let n = fake.strips.len();
        Host {
            set: set_of(fake, workers),
            fake: Arc::clone(fake),
            g: 0,
            edges: edges.to_vec(),
            pinned: vec![false; n],
            stale: vec![false; n],
            done: vec![false; n],
            job: vec![(0, 0, 0); n],
            graph: Arc::new(graph),
            started: Vec::with_capacity(4 * n),
            taken: Vec::with_capacity(4 * n),
            here: Vec::with_capacity(4 * n),
            skipped: Vec::with_capacity(4 * n),
            gate: None,
            gated: false,
            deadline: None,
            drive_deadline: None,
            here_gate: None,
        }
    }

    /// One block: `begin_block`, then the driver with a far deadline (the host's own deadline,
    /// when set, is what its polls use).
    fn block(&mut self, run: &mut GraphRun) -> BlockReport {
        self.g += 1;
        assert!(self.set.begin_block(self.g));
        self.done.fill(false);
        self.started.clear();
        self.taken.clear();
        self.here.clear();
        self.skipped.clear();
        self.gated = false;
        let graph = Arc::clone(&self.graph);
        let deadline = self.drive_deadline.unwrap_or_else(|| Instant::now() + STALL);
        drive(self, &graph, run, deadline)
    }

    fn outcome(&self, s: usize) -> Outcome {
        self.set.outcome(s).unwrap()
    }

    fn slot_done(&self, s: usize) -> bool {
        self.set.jobs().slot(s).unwrap().state() == crate::strip_jobs::State::Done
    }

    fn at(list: &[usize], t: usize) -> usize {
        list.iter().position(|&x| x == t).unwrap_or_else(|| panic!("{t} not in {list:?}"))
    }
}

impl GraphHost for Host {
    fn set(&mut self) -> &mut WorkerSet {
        &mut self.set
    }

    fn start(&mut self, t: usize) -> Start {
        for &(f, to) in &self.edges {
            if to == t {
                assert!(self.done[f], "block {}: {t} started before its source {f} was ready", self.g);
            }
        }
        self.started.push(t);
        self.job[t] = (self.graph.generation(), self.graph.edge_hash(t), self.g);
        if self.set.is_busy(t) {
            Start::Busy
        } else if self.pinned[t] {
            Start::Callback
        } else {
            Start::Publish
        }
    }

    fn refused(&mut self, t: usize) {
        panic!("block {}: publish of {t} refused", self.g);
    }

    fn render_here(&mut self, t: usize) {
        if let Some(gate) = self.here_gate {
            wait_until("the pinned render's gate", || gate(self));
        }
        self.fake.execute(t, self.g, false);
        self.here.push(t);
        self.done[t] = true;
    }

    fn poll(&mut self, deadline: Instant) -> Poll {
        if !self.gated {
            self.gated = true;
            if let Some(gate) = self.gate {
                wait_until("the gate", || gate(self));
            }
        }
        let deadline = self.deadline.unwrap_or(deadline);
        let (fake, g) = (Arc::clone(&self.fake), self.g);
        self.set.poll(deadline, |s| fake.execute(s, g, false))
    }

    fn take(&mut self, t: usize, _: Outcome) -> bool {
        assert_eq!(self.fake.strips[t].last_gen.load(Ordering::Acquire), self.g, "{t} ran for this block");
        // The completion identity: this graph, this task's edges, this block.
        let ok = !self.stale[t] && self.job[t] == (self.graph.generation(), self.graph.edge_hash(t), self.g);
        self.taken.push(t);
        self.done[t] = ok;
        ok
    }

    fn skipped(&mut self, t: usize, origin: usize) {
        self.skipped.push((t, origin));
    }
}

fn host(fake: &Arc<Fake>, workers: usize, edges: &[(usize, usize)], generation: u64) -> Host {
    Host::new(fake, workers, TaskGraph::new(generation, fake.strips.len(), edges), edges)
}

fn sorted(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    v.sort_unstable();
    v
}

/// A chain and a join through two workers: every task runs, each only after its sources.
#[test]
fn dependents_are_published_only_after_their_sources() {
    let fake = Fake::new(6, Duration::ZERO);
    let mut host = host(&fake, 2, &[(0, 2), (1, 2), (2, 3)], 1);
    let mut run = GraphRun::new(6);
    for _ in 0..20 {
        let r = host.block(&mut run);
        assert_eq!((r.mixed + r.inline, r.overrun, r.upstream_missed), (6, 0, 0), "{r:?}");
        for s in 0..6 {
            assert!(matches!(host.outcome(s), Outcome::Mixed | Outcome::Inline));
        }
        let at = |t| Host::at(&host.started, t);
        assert!(at(2) > at(0) && at(2) > at(1) && at(3) > at(2), "started in dependency order: {:?}", host.started);
    }
    assert_eq!(host.set.stats().upstream_missed, 0);
    assert!(host.set.shutdown(STALL).is_empty());
    assert_eq!(host.set.jobs().faults(), 0);
}

/// With no worker, admitted strips render inline in dependency order (the callback alone).
#[test]
fn with_no_worker_the_graph_renders_inline_in_order() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut host = host(&fake, 0, &[(0, 1), (1, 3)], 1);
    for s in 0..4 {
        host.set.set_cost_estimate(s, Duration::from_nanos(1));
    }
    let mut run = GraphRun::new(4);
    let r = host.block(&mut run);
    assert_eq!((r.inline, r.overrun, r.upstream_missed), (4, 0, 0), "{r:?}");
    assert!(Host::at(&host.taken, 0) < Host::at(&host.taken, 1) && Host::at(&host.taken, 1) < Host::at(&host.taken, 3));
    assert!(host.set.shutdown(STALL).is_empty());
}

/// Gate (deterministic late workers): a source late, then busy, skips its dependents (and theirs)
/// as upstream-missed with the source as the origin, once each per block; the source's own overrun
/// is counted once; unrelated strips render on the other worker. After the release the late
/// result is retired unread and everything renders again in order.
#[test]
fn a_late_then_busy_source_skips_its_branch_and_unrelated_strips_render() {
    let fake = Fake::new(5, Duration::ZERO);
    // 0 -> 1 -> 2; 3 and 4 unrelated.
    let mut host = host(&fake, 2, &[(0, 1), (1, 2)], 1);
    let mut run = GraphRun::new(5);
    fake.set(0, Behavior::Stall);
    // The source is inside its stall and the unrelated strips are done before the first poll,
    // whose deadline has already passed.
    host.gate = Some(|h| h.fake.stalled.load(Ordering::Acquire) == 1 && h.slot_done(3) && h.slot_done(4));
    host.deadline = Some(Instant::now());

    let r = host.block(&mut run);
    let missed = Outcome::Overrun(OverrunCause::UpstreamMissed);
    assert_eq!(host.outcome(0), Outcome::Overrun(OverrunCause::Late));
    assert_eq!((host.outcome(1), host.outcome(2)), (missed, missed));
    assert_eq!((host.outcome(3), host.outcome(4)), (Outcome::Mixed, Outcome::Mixed), "unrelated strips render");
    assert_eq!(sorted(host.skipped.clone()), [(1, 0), (2, 0)], "the origin is the source, also two levels down");
    assert_eq!((r.mixed, r.overrun, r.upstream_missed), (2, 1, 2), "{r:?}");
    for s in [1, 2] {
        assert_eq!(fake.strips[s].last_gen.load(Ordering::Acquire), 0, "{s} never ran");
        let f = host.set.figures().strip(s).unwrap();
        assert_eq!((f.upstream_missed, f.upstream_origin, f.overruns()), (1, Some(0), 0), "{s}: {f:?}");
    }
    let st = host.set.stats();
    assert_eq!((st.overruns, st.overruns_late, st.upstream_missed), (1, 1, 2), "one overrun at the origin: {st:?}");

    // Block 2: the worker still holds the source.
    let r = host.block(&mut run);
    assert_eq!(host.outcome(0), Outcome::Overrun(OverrunCause::Busy));
    assert_eq!((host.outcome(1), host.outcome(2)), (missed, missed));
    assert_eq!((host.outcome(3), host.outcome(4)), (Outcome::Mixed, Outcome::Mixed));
    assert_eq!((r.overrun, r.upstream_missed), (1, 2));
    let st = host.set.stats();
    assert_eq!((st.overruns, st.overruns_busy, st.upstream_missed), (2, 1, 4), "{st:?}");
    let top = host.set.figures().top_overruns::<5>();
    assert_eq!(top[0].map(|(i, f)| (i, f.overruns())), Some((0, 2)), "the origin first, with its own overruns: {top:?}");
    assert!(top.iter().flatten().any(|(i, f)| *i == 1 && f.upstream_missed == 2), "a dependent is listed by its upstream misses: {top:?}");

    // Released: the late result is retired unread at the next block, then the branch renders.
    fake.release(0);
    wait_until("the source's late result", || host.slot_done(0));
    host.gate = None;
    host.deadline = None;
    let r = host.block(&mut run);
    assert_eq!((r.mixed + r.inline, r.overrun, r.upstream_missed), (5, 0, 0), "{r:?}");
    assert_eq!(host.set.stats().late_results, 1, "retired unread");
    assert_eq!(fake.strips[2].last_gen.load(Ordering::Acquire), host.g);
    assert!(host.set.shutdown(STALL).is_empty());
    assert_eq!(host.set.jobs().faults(), 0);
}

/// Gate (deterministic): an unstarted source (no worker, no cost history, the deadline passed)
/// skips its dependent; pinned unrelated strips render on the callback. The dependent's MIDI is
/// deferred as an overrun's.
#[test]
fn an_unstarted_source_skips_its_dependent_and_pinned_strips_render() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut host = host(&fake, 0, &[(0, 1)], 1);
    host.pinned[2] = true;
    host.pinned[3] = true;
    host.deadline = Some(Instant::now());
    let mut run = GraphRun::new(4);
    let r = host.block(&mut run);
    assert_eq!(host.outcome(0), Outcome::Overrun(OverrunCause::Unstarted));
    assert_eq!(host.outcome(1), Outcome::Overrun(OverrunCause::UpstreamMissed));
    assert!(crate::midi_defer::should_defer(host.outcome(1)), "its events go to the ring, as an overrun's");
    assert_eq!(host.here, [2, 3], "the pinned strips rendered on the callback");
    assert_eq!(host.skipped, [(1, 0)]);
    assert_eq!((r.overrun, r.upstream_missed), (1, 1));
    assert_eq!(host.set.stats().overruns_unstarted, 1);
    assert!(host.set.shutdown(STALL).is_empty());
}

/// A pinned source renders on the callback first, and its dependent is published after it.
#[test]
fn a_pinned_source_unlocks_its_dependent() {
    let fake = Fake::new(3, Duration::ZERO);
    let mut host = host(&fake, 1, &[(0, 1), (1, 2)], 1);
    host.pinned[0] = true;
    host.pinned[2] = true;
    let mut run = GraphRun::new(3);
    let r = host.block(&mut run);
    assert_eq!(host.here, [0, 2], "the pinned source, then the pinned dependent of a worker strip");
    assert_eq!((r.mixed + r.inline, r.upstream_missed), (1, 0));
    assert!(host.set.shutdown(STALL).is_empty());
}

/// A result whose completion identity does not match is retired unread by the host (counted
/// there), and its dependents are skipped with it as the origin.
#[test]
fn a_stale_result_skips_its_dependents() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut host = host(&fake, 2, &[(0, 1), (1, 2)], 1);
    host.stale[0] = true;
    let mut run = GraphRun::new(4);
    let r = host.block(&mut run);
    assert_eq!(host.taken.iter().filter(|&&t| t == 0).count(), 1);
    assert_eq!(sorted(host.skipped.clone()), [(1, 0), (2, 0)]);
    assert_eq!((host.outcome(1), host.outcome(3)), (Outcome::Overrun(OverrunCause::UpstreamMissed), Outcome::Mixed));
    assert_eq!(r.upstream_missed, 2);
    assert!(host.set.shutdown(STALL).is_empty());
}

/// Gate: a graph change while a worker still holds a task of the old graph. The task is busy
/// under the new graph (its new dependent skipped, its old one now independent and rendered);
/// its late result is retired unread at the next block, and its first result under the new graph
/// carries the new graph's identity.
#[test]
fn a_graph_change_while_a_worker_holds_an_old_task() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut host = host(&fake, 2, &[(0, 1)], 1);
    let mut run = GraphRun::new(4);
    fake.set(0, Behavior::Stall);
    host.gate = Some(|h| h.fake.stalled.load(Ordering::Acquire) == 1 && h.slot_done(2) && h.slot_done(3));
    host.deadline = Some(Instant::now());
    host.block(&mut run);
    assert_eq!(host.outcome(0), Outcome::Overrun(OverrunCause::Late));
    assert_eq!(host.job[0].0, 1, "started under the old graph");

    let new = [(0, 2)];
    host.graph = Arc::new(TaskGraph::new(2, 4, &new));
    host.edges = new.to_vec();
    host.gate = Some(|h| h.slot_done(1) && h.slot_done(3));
    host.block(&mut run);
    assert_eq!(host.outcome(0), Outcome::Overrun(OverrunCause::Busy));
    assert_eq!(host.outcome(1), Outcome::Mixed, "no longer a dependent: renders");
    assert_eq!(host.outcome(2), Outcome::Overrun(OverrunCause::UpstreamMissed));
    assert_eq!(host.skipped, [(2, 0)]);

    fake.release(0);
    wait_until("the old task's result", || host.slot_done(0));
    host.gate = None;
    host.deadline = None;
    let r = host.block(&mut run);
    assert_eq!(host.set.stats().late_results, 1, "the old graph's result retired unread");
    assert_eq!((r.mixed + r.inline, r.upstream_missed), (4, 0));
    assert_eq!(host.job[0], (2, host.graph.edge_hash(0), host.g), "rendered under the new graph");
    assert!(Host::at(&host.taken, 0) < host.taken.len(), "taken once, with the new identity");
    assert!(host.set.shutdown(STALL).is_empty());
    assert_eq!(host.set.jobs().faults(), 0);
}

/// `forgo` and `skip_upstream` decide a strip without publishing it; each is refused for a strip
/// already decided.
#[test]
fn forgo_and_skip_upstream_record_their_causes() {
    let fake = Fake::new(4, Duration::ZERO);
    let mut set = set_of(&fake, 0);
    assert!(set.begin_block(1));
    assert_eq!(set.forgo(0), Ok(Outcome::Overrun(OverrunCause::Unstarted)));
    assert_eq!(set.skip_upstream(1, 0), Ok(Outcome::Overrun(OverrunCause::UpstreamMissed)));
    assert_eq!(set.publish(1), Err(PublishRefused::Twice));
    assert_eq!(set.forgo(0), Err(PublishRefused::Twice));
    assert_eq!(set.skip_upstream(9, 0), Err(PublishRefused::OutOfRange));
    assert_eq!(set.publish(2), Ok(Published::Published));
    let r = set.finish(Instant::now(), |_| {});
    assert_eq!((r.overrun, r.upstream_missed), (2, 1), "the forgone strip and the unstarted one; the skip apart: {r:?}");
    let st = set.stats();
    assert_eq!((st.overruns_unstarted, st.upstream_missed, st.misuse), (2, 1, 3));
    assert!(set.shutdown(STALL).is_empty());
}

/// Gate: zero allocations and frees on the callback over graph blocks with overruns,
/// upstream-missed skips, stale results, pinned and inline renders.
#[test]
fn the_graph_driver_allocates_nothing() {
    let fake = Fake::new(6, Duration::ZERO);
    let mut host = host(&fake, 1, &[(0, 2), (1, 2), (2, 3)], 1);
    host.pinned[5] = true;
    for s in 0..6 {
        host.set.set_cost_estimate(s, Duration::from_nanos(1));
    }
    let mut run = GraphRun::new(6);
    host.block(&mut run);
    let ((), stats) = measure(|| {
        for b in 0..60u64 {
            host.stale[1] = b % 3 == 0;
            // A deadline already passed every fourth block: whatever is pending is an overrun.
            host.deadline = (b % 4 == 0).then(Instant::now);
            host.block(&mut run);
        }
    });
    assert!(stats.is_zero(), "the callback allocated or freed: {stats:?}");
    assert!(host.set.stats().upstream_missed > 0, "the skip path ran");
    wait_until("the worker", || host.set.jobs().counts().claimed == 0);
    assert!(host.set.shutdown(STALL).is_empty());
}

/// Review finding 2 (#174): nothing is published after the deadline. A pinned strip renders on
/// the callback until the worker's source is done and the deadline has passed; then the source's
/// result (taken past the deadline) and the pinned strip each have a dependent ready, and neither
/// is started: both are forgone as `Unstarted`, never published, so no worker can claim them.
#[test]
fn dependents_ready_after_the_deadline_are_forgone_unpublished() {
    let fake = Fake::new(4, Duration::ZERO);
    // 0 (worker) -> 1, 2 (pinned) -> 3.
    let mut host = host(&fake, 1, &[(0, 1), (2, 3)], 1);
    host.pinned[2] = true;
    let mut run = GraphRun::new(4);
    host.drive_deadline = Some(Instant::now() + Duration::from_millis(20));
    host.here_gate = Some(|h| h.slot_done(0) && h.drive_deadline.is_some_and(|d| Instant::now() >= d));
    let r = host.block(&mut run);
    assert_eq!(host.outcome(0), Outcome::Mixed, "the source rendered");
    assert_eq!(host.here, [2], "the pinned strip rendered");
    for t in [1, 3] {
        assert_eq!(host.outcome(t), Outcome::Overrun(OverrunCause::Unstarted), "{t}: forgone");
        assert!(!host.started.contains(&t), "{t} was never started (filled, sent or published): {:?}", host.started);
        assert_eq!(fake.strips[t].last_gen.load(Ordering::Acquire), 0, "{t} never ran");
    }
    assert_eq!((r.overrun, r.upstream_missed), (2, 0), "{r:?}");
    assert_eq!(host.set.stats().overruns_unstarted, 2);
    assert!(host.set.shutdown(STALL).is_empty());
    assert_eq!(host.set.jobs().faults(), 0);
}
