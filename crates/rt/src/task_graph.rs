//! Dependency-aware strip scheduling (issue #174, ADR 0029 amendment, slice 1): a per-plan task
//! table whose edges run from every sidechain key source to its keyed strip, the per-block run
//! state that publishes a task only when every input it needs is ready, and the driver that runs
//! one block of it through a [`WorkerSet`].
//!
//! **What the graph decides, and what it does not.** Only *when* a task is published. The slot
//! protocol ([`crate::strip_jobs`]), the bounded wait to the deadline, the per-block busy skip
//! and the admission rule of [`crate::worker_set`] are unchanged; the callback stays the only
//! publisher (the driver runs on it and is handed the [`WorkerSet`] by its [`GraphHost`]).
//!
//! **One block** ([`drive`]):
//!
//! 1. [`GraphRun::begin`]: every task's counter is set to its input count; the tasks with none
//!    are ready.
//! 2. The wave: each ready task is started ([`GraphHost::start`]): published to the workers,
//!    queued for the callback (a pinned task, rendered after the wake), taken as ready without a
//!    render, or found unavailable. Then the workers are woken, and the queued callback tasks are
//!    rendered.
//! 3. The sweep ([`GraphHost::poll`], which is [`WorkerSet::poll`]): each time a task resolves
//!    with a result for this block ([`Outcome::Mixed`] or [`Outcome::Inline`]) the host takes it
//!    home and checks its completion identity ([`GraphHost::take`]); each dependent whose
//!    counter reaches zero is started at once (a pinned one rendered right there) and the newly
//!    published are woken.
//!
//! **The failure rule.** A source that resolves without a usable result (an overrun of any cause,
//! a fault, an identity mismatch, or unavailable when started) is the *origin* of a miss: every
//! dependent still waiting, and theirs in turn, is skipped for the block
//! ([`WorkerSet::skip_upstream`], [`OverrunCause::UpstreamMissed`](crate::worker_set::OverrunCause))
//! with that origin, and never published; unrelated tasks keep rendering. A dependent that
//! becomes ready only after the deadline (its source resolved in the deadline phase) is not
//! published: [`WorkerSet::forgo`] counts it under its own cause (`Unstarted`). Nothing is
//! retried.
//!
//! **Bounded storage.** The table is built once per plan off the audio path; the run state holds
//! one counter, one state and at most one queued step per task, sized with the table. Nothing in
//! [`drive`] allocates. No `unsafe`.

use crate::worker_set::{BlockReport, Outcome, Poll, Published, WorkerSet};
use std::time::Instant;

/// FNV-1a offset basis (the edge hash).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One FNV-1a step over a word.
#[must_use]
pub const fn fnv(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(FNV_PRIME)
}

/// A plan's task table: per task its input count, its dependents and a hash of its input edges,
/// with the graph generation the compiler stamped on it. Built once off the audio path; read
/// only afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskGraph {
    generation: u64,
    /// Per task: how many tasks it waits for.
    inputs: Box<[u32]>,
    /// Per task, the start of its dependents in `deps`; one more entry than tasks.
    first: Box<[u32]>,
    /// The dependents of every task, task by task.
    deps: Box<[u32]>,
    /// Per task: a hash of its input edges (its sources, in order).
    edge_hash: Box<[u64]>,
}

impl TaskGraph {
    /// A table of `tasks` tasks stamped `generation`, with an edge `from -> to` for each pair.
    /// Only edges with `from < to < tasks` are kept (tasks are numbered in a render order, so a
    /// source always comes first; anything else could not be scheduled), each once. Allocates.
    #[must_use]
    pub fn new(generation: u64, tasks: usize, edges: &[(usize, usize)]) -> Self {
        let mut e: Vec<(usize, usize)> = edges.iter().copied().filter(|&(f, t)| f < t && t < tasks).collect();
        e.sort_unstable();
        e.dedup();
        let mut inputs = vec![0u32; tasks];
        let mut first = vec![0u32; tasks + 1];
        let mut edge_hash = vec![FNV_OFFSET; tasks];
        for &(f, t) in &e {
            inputs[t] = inputs[t].saturating_add(1);
            first[f + 1] = first[f + 1].saturating_add(1);
            // Sorted by source: each task's sources are hashed in ascending order.
            edge_hash[t] = fnv(edge_hash[t], u64::try_from(f).unwrap_or(u64::MAX).wrapping_add(1));
        }
        for i in 0..tasks {
            first[i + 1] = first[i + 1].saturating_add(first[i]);
        }
        let deps = e.iter().map(|&(_, t)| u32::try_from(t).unwrap_or(u32::MAX)).collect();
        TaskGraph { generation, inputs: inputs.into(), first: first.into(), deps, edge_hash: edge_hash.into() }
    }

    /// The graph generation the compiler stamped (new with every plan's table).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Tasks.
    #[must_use]
    pub fn tasks(&self) -> usize {
        self.inputs.len()
    }

    /// Edges.
    #[must_use]
    pub fn edges(&self) -> usize {
        self.deps.len()
    }

    /// How many tasks `task` waits for (0 for no such task).
    #[must_use]
    pub fn inputs(&self, task: usize) -> u32 {
        self.inputs.get(task).copied().unwrap_or(0)
    }

    /// The tasks that wait for `task` (empty for no such task).
    #[must_use]
    pub fn dependents(&self, task: usize) -> &[u32] {
        let (Some(&a), Some(&b)) = (self.first.get(task), self.first.get(task + 1)) else { return &[] };
        self.deps.get(a as usize..b as usize).unwrap_or(&[])
    }

    /// A hash of `task`'s input edges (its sources in order): part of its completion identity, so
    /// a changed edge set makes a result of the old one stale. The same for every task without
    /// inputs.
    #[must_use]
    pub fn edge_hash(&self, task: usize) -> u64 {
        self.edge_hash.get(task).copied().unwrap_or(FNV_OFFSET)
    }
}

/// Where a task stands in the open block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// Some input is not ready yet.
    Waiting,
    /// Every input was ready and the task was started (published, rendered on the callback,
    /// taken as ready, or found unavailable).
    Started,
    /// Skipped this block: an upstream task missed. The origin is the first task of the branch
    /// that missed on its own.
    Missed {
        /// The origin task.
        origin: u32,
    },
}

/// A step the run hands its driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Every input of the task is ready: start it.
    Ready(u32),
    /// The task is skipped: its `origin` missed.
    Missed { task: u32, origin: u32 },
}

/// One block's state of a [`TaskGraph`], owned by the callback. Sized for a table of up to its
/// capacity's tasks (allocated with the plan, off the audio path); reset by [`GraphRun::begin`].
#[derive(Clone, Debug, Default)]
pub struct GraphRun {
    remaining: Box<[u32]>,
    state: Box<[TaskState]>,
    /// The steps not yet handed out (a stack; each task is pushed at most once a block).
    steps: Box<[Step]>,
    steps_len: usize,
    /// Pinned tasks ready during the wave, rendered after the wake.
    later: Box<[u32]>,
    later_len: usize,
    /// Tasks of the open block.
    tasks: usize,
}

impl GraphRun {
    /// State for tables of up to `capacity` tasks (allocates).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        GraphRun {
            remaining: vec![0; capacity].into(),
            state: vec![TaskState::Waiting; capacity].into(),
            steps: vec![Step::Ready(0); capacity].into(),
            steps_len: 0,
            later: vec![0; capacity].into(),
            later_len: 0,
            tasks: 0,
        }
    }

    /// The most tasks a table may have.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.state.len()
    }

    /// Opens a block of `graph`: every counter at its input count, every task waiting, the tasks
    /// without inputs ready (in task order). False when the table has more tasks than the run
    /// holds (nothing is set up; a caller error).
    pub fn begin(&mut self, graph: &TaskGraph) -> bool {
        let n = graph.tasks();
        if n > self.capacity() {
            return false;
        }
        self.tasks = n;
        self.steps_len = 0;
        self.later_len = 0;
        for t in 0..n {
            self.remaining[t] = graph.inputs(t);
            self.state[t] = TaskState::Waiting;
        }
        // Pushed in reverse, so the stack hands them out in task order.
        for t in (0..n).rev() {
            if self.remaining[t] == 0 {
                self.ready(t);
            }
        }
        true
    }

    /// Where `task` stands in the open block (`None` past the table).
    #[must_use]
    pub fn state(&self, task: usize) -> Option<TaskState> {
        (task < self.tasks).then(|| self.state[task])
    }

    fn push(&mut self, step: Step) {
        if let Some(s) = self.steps.get_mut(self.steps_len) {
            *s = step;
            self.steps_len += 1;
        }
    }

    fn ready(&mut self, task: usize) {
        if let Some(s) = self.state.get_mut(task) {
            *s = TaskState::Started;
            self.push(Step::Ready(u32::try_from(task).unwrap_or(u32::MAX)));
        }
    }

    /// `task` has its result for this block: each waiting dependent counts one input more ready,
    /// and is ready when it has them all.
    fn succeed(&mut self, graph: &TaskGraph, task: usize) {
        for &d in graph.dependents(task) {
            let d = d as usize;
            if self.state.get(d) != Some(&TaskState::Waiting) {
                continue;
            }
            self.remaining[d] = self.remaining[d].saturating_sub(1);
            if self.remaining[d] == 0 {
                self.ready(d);
            }
        }
    }

    /// `task` has no usable result for this block: every dependent still waiting misses, with
    /// `origin` (and theirs in turn, as each is handed out).
    fn fail(&mut self, graph: &TaskGraph, task: usize, origin: usize) {
        let origin = u32::try_from(origin).unwrap_or(u32::MAX);
        for &d in graph.dependents(task) {
            if let Some(s) = self.state.get_mut(d as usize)
                && *s == TaskState::Waiting
            {
                *s = TaskState::Missed { origin };
                self.push(Step::Missed { task: d, origin });
            }
        }
    }

    /// The next step, if any. A missed task's own dependents miss with the same origin.
    fn next(&mut self, graph: &TaskGraph) -> Option<Step> {
        self.steps_len = self.steps_len.checked_sub(1)?;
        let step = self.steps[self.steps_len];
        if let Step::Missed { task, origin } = step {
            self.fail(graph, task as usize, origin as usize);
        }
        Some(step)
    }

    fn defer(&mut self, task: usize) {
        if let Some(s) = self.later.get_mut(self.later_len) {
            *s = u32::try_from(task).unwrap_or(u32::MAX);
            self.later_len += 1;
        }
    }
}

/// How a ready task runs this block ([`GraphHost::start`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// Its inputs are written: the driver publishes it to the workers.
    Publish,
    /// Pinned to the callback (it reads the live input or the hardware): the driver has the
    /// host render it ([`GraphHost::render_here`]), after the wake when it is ready in the wave.
    Callback,
    /// Nothing to render: its result is ready as it is (dependents go ahead).
    Ready,
    /// A worker still holds it from an earlier block: the driver publishes it to record the
    /// skip (`Overrun(Busy)`) and its dependents miss.
    Busy,
    /// Unavailable for a reason of its own the host has counted (a suspect instance): its
    /// dependents miss.
    Unavailable,
}

/// What [`drive`] needs from its caller: the worker set, and the strips themselves.
pub trait GraphHost {
    /// The worker set (the single publisher's).
    fn set(&mut self) -> &mut WorkerSet;
    /// Every input of `task` is ready for this block: write its inputs (its keys, its cell) and
    /// say how it runs.
    fn start(&mut self, task: usize) -> Start;
    /// The set refused to publish `task` after [`Start::Publish`] (a protocol fault): take its
    /// inputs back; the driver renders it on the callback next.
    fn refused(&mut self, task: usize);
    /// Renders `task` on the callback now.
    fn render_here(&mut self, task: usize);
    /// One step of the block's wait: [`WorkerSet::poll`] with the host's inline render.
    fn poll(&mut self, deadline: Instant) -> Poll;
    /// `task` resolved with a result for this block (`Mixed` or `Inline`): take it home and check
    /// its completion identity. False when the result is not this task's for this graph and
    /// block (retired unread, counted by the host): its dependents miss.
    fn take(&mut self, task: usize, outcome: Outcome) -> bool;
    /// `task` is skipped for the block because `origin` missed.
    fn skipped(&mut self, task: usize, origin: usize);
}

/// Runs one block of `graph` through the host's worker set: the wave, the pinned tasks, then the
/// sweep to `deadline` (see the module docs). Call it after [`WorkerSet::begin_block`] and the
/// host's own preparation; returns the block's counts. Allocation-free.
pub fn drive(host: &mut impl GraphHost, graph: &TaskGraph, run: &mut GraphRun, deadline: Instant) -> BlockReport {
    if !run.begin(graph) {
        return host.set().block_report();
    }
    let mut d = Driver { wake: 0 };
    d.steps(host, graph, run, Phase::Wave);
    d.wake(host, deadline);
    // In task order; nothing is deferred outside the wave, so the list does not grow here.
    for k in 0..run.later_len {
        let t = run.later[k] as usize;
        host.render_here(t);
        run.succeed(graph, t);
        // A pinned render that ran past the deadline leaves no time for its dependents.
        let phase = if Instant::now() >= deadline { Phase::Late } else { Phase::Sweep };
        d.steps(host, graph, run, phase);
        d.wake(host, deadline);
    }
    run.later_len = 0;
    loop {
        match host.poll(deadline) {
            Poll::Finished => break,
            Poll::Resolved { strip, outcome, late } => {
                if matches!(outcome, Outcome::Mixed | Outcome::Inline) && host.take(strip, outcome) {
                    run.succeed(graph, strip);
                } else {
                    run.fail(graph, strip, strip);
                }
                d.steps(host, graph, run, if late { Phase::Late } else { Phase::Sweep });
                d.wake(host, deadline);
            }
        }
    }
    host.set().block_report()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The first wave: pinned tasks wait until the workers are woken.
    Wave,
    /// Inside the block: a ready task starts at once.
    Sweep,
    /// The deadline phase: a ready task is not published any more.
    Late,
}

struct Driver {
    /// Tasks published since the last wake.
    wake: usize,
}

impl Driver {
    fn steps(&mut self, host: &mut impl GraphHost, graph: &TaskGraph, run: &mut GraphRun, phase: Phase) {
        while let Some(step) = run.next(graph) {
            match step {
                Step::Missed { task, origin } => {
                    let (task, origin) = (task as usize, origin as usize);
                    let _ = host.set().skip_upstream(task, origin);
                    host.skipped(task, origin);
                }
                Step::Ready(t) => self.start(host, graph, run, t as usize, phase),
            }
        }
    }

    fn start(&mut self, host: &mut impl GraphHost, graph: &TaskGraph, run: &mut GraphRun, t: usize, phase: Phase) {
        if phase == Phase::Late {
            // Ready only in the deadline phase: no time is left, it is not published.
            let _ = host.set().forgo(t);
            run.fail(graph, t, t);
            return;
        }
        match host.start(t) {
            Start::Publish => match host.set().publish(t) {
                Ok(Published::Published) => self.wake += 1,
                Ok(Published::Skipped) => run.fail(graph, t, t),
                Err(_) => {
                    host.refused(t);
                    host.render_here(t);
                    run.succeed(graph, t);
                }
            },
            Start::Callback if phase == Phase::Wave => run.defer(t),
            Start::Callback => {
                host.render_here(t);
                run.succeed(graph, t);
            }
            Start::Ready => run.succeed(graph, t),
            Start::Busy => {
                let _ = host.set().publish(t);
                run.fail(graph, t, t);
            }
            Start::Unavailable => run.fail(graph, t, t),
        }
    }

    /// Wakes the workers for what was published since the last wake, unless the deadline has
    /// passed (a worker that started one now would hold it into the next block).
    fn wake(&mut self, host: &mut impl GraphHost, deadline: Instant) {
        let n = std::mem::take(&mut self.wake);
        if n > 0 && Instant::now() < deadline {
            host.set().wake(n);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn the_table_keeps_forward_edges_once_and_hashes_each_task_s_sources() {
        let g = TaskGraph::new(7, 5, &[(0, 2), (1, 2), (0, 2), (2, 4), (3, 1), (4, 9), (2, 2)]);
        assert_eq!((g.generation(), g.tasks(), g.edges()), (7, 5, 3), "backward, self, out-of-range and repeated edges dropped");
        assert_eq!((0..5).map(|t| g.inputs(t)).collect::<Vec<_>>(), [0, 0, 2, 0, 1]);
        assert_eq!(g.dependents(0), [2]);
        assert_eq!(g.dependents(1), [2]);
        assert_eq!(g.dependents(2), [4]);
        assert!(g.dependents(3).is_empty() && g.dependents(9).is_empty());
        assert_eq!(g.edge_hash(0), g.edge_hash(3), "no inputs, one hash");
        assert_ne!(g.edge_hash(2), g.edge_hash(4), "different sources, different hash");
        let other = TaskGraph::new(8, 5, &[(1, 2), (2, 4)]);
        assert_ne!(g.edge_hash(2), other.edge_hash(2), "a changed edge set changes the identity");
        assert_eq!(g.edge_hash(4), other.edge_hash(4), "an unchanged edge set keeps it");
    }

    #[test]
    fn a_run_hands_out_ready_tasks_and_propagates_a_miss_with_its_origin() {
        // 0 -> 1 -> 2, 0 -> 3, 4 alone.
        let g = TaskGraph::new(1, 5, &[(0, 1), (1, 2), (0, 3)]);
        let mut run = GraphRun::new(8);
        assert!(run.begin(&g));
        assert_eq!(run.next(&g), Some(Step::Ready(0)));
        assert_eq!(run.next(&g), Some(Step::Ready(4)));
        assert_eq!(run.next(&g), None);
        run.succeed(&g, 4);
        assert_eq!(run.next(&g), None, "4 has no dependents");
        run.fail(&g, 0, 0);
        let mut missed = Vec::new();
        while let Some(s) = run.next(&g) {
            missed.push(s);
        }
        missed.sort_by_key(|s| match s {
            Step::Missed { task, .. } | Step::Ready(task) => *task,
        });
        assert_eq!(missed, [Step::Missed { task: 1, origin: 0 }, Step::Missed { task: 2, origin: 0 }, Step::Missed { task: 3, origin: 0 }]);
        assert_eq!(run.state(2), Some(TaskState::Missed { origin: 0 }), "transitively, with the first origin");
        assert!(!GraphRun::new(2).begin(&g), "a table larger than the run is refused");
    }

    #[test]
    fn a_task_waits_for_every_input() {
        let g = TaskGraph::new(1, 3, &[(0, 2), (1, 2)]);
        let mut run = GraphRun::new(3);
        assert!(run.begin(&g));
        while run.next(&g).is_some() {}
        run.succeed(&g, 0);
        assert_eq!(run.next(&g), None, "one of two inputs");
        assert_eq!(run.state(2), Some(TaskState::Waiting));
        run.succeed(&g, 1);
        assert_eq!(run.next(&g), Some(Step::Ready(2)));
        run.fail(&g, 2, 2);
        assert_eq!(run.next(&g), None);
    }
}
