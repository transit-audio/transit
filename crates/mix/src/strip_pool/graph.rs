//! The graph path of the strip fan-out (issue #174, ADR 0029 amendment, slice 1): a plan with
//! sidechain keys renders its phase-1 strips through the render workers as the plan's task graph
//! ([`crate::graph`]), driven by [`transit_rt::task_graph::drive`]. Taken only with
//! `TRANSIT_RENDER_GRAPH=1` (or [`MixEngine::force_graph`]), or while a strip of a keyed plan is
//! still away.
//!
//! **One block** (`StripPool::render_graph`, after `render_active` cleared the busses and aligned
//! the keys):
//!
//! 1. `begin_block(g)`; a strip whose worker has finished comes home with its keys; each home
//!    strip's deferred MIDI is delivered, as on the plain pool path.
//! 2. The driver starts every strip whose key sources are ready (at first, the strips with none):
//!    the callback fills its keys in the plan's bank from the sources' finished buffers (the
//!    early-key delay included, `KeyBank::fill`), sends it into its cell, lends it the keys, stamps
//!    its job with the graph generation and its task identity, and publishes it; a strip that
//!    reads the live input or the hardware renders on the callback instead, after the wake.
//! 3. Each strip the sweep resolves with a result comes home at once; its result is used only
//!    when its job's stamp is this graph's, this task's and this block's (else a counted
//!    conflict, silent). Then its ready dependents start, and the newly published are woken.
//! 4. A source with no usable result (an overrun of any cause, a fault, a stale stamp, a suspect
//!    instance, busy) skips its dependents for the block, and theirs: `Overrun(UpstreamMissed)`,
//!    counted per dependent with the origin. A skipped strip is never rendered later in the
//!    block: its keys are not filled (their lines neither written nor idled), its own lines are
//!    not run, it is silent and not metered, and its MIDI stays in its queue (or, away, goes to
//!    the ring), delivered at its next processed block as for an overrun.
//! 5. After the deadline the strips come home as on the plain path; the take journal and the
//!    overrun blocks count the strips' own overruns, not the upstream-missed skips.

use super::*;
use crate::graph::{StripGraph, task_identity};
use transit_rt::task_graph::{GraphHost, Start, TaskGraph, drive};
use transit_rt::worker_set::Poll;

/// The driver's view of the pool for one block.
struct Host<'a, 'b> {
    pool: &'a mut StripPool,
    p1: &'a mut [Strip],
    events: &'a mut EngineEvents,
    keys: &'a mut KeyBank,
    graph: &'a TaskGraph,
    layer_start: &'a [u16],
    b: &'a PoolBlock<'b>,
    g: u64,
    /// Strips rendered on the callback this block.
    callback: u64,
}

impl<'a> Host<'a, '_> {
    /// Strip `t`'s track when the plan renders it.
    fn track(&self, t: usize) -> Option<&'a Track> {
        let b: &'a PoolBlock<'_> = self.b;
        let s: &'a Session = b.session;
        self.p1.get(t).and_then(|st| s.tracks.get(usize::from(st.track))).filter(|t| live(t) && !is_aux(t))
    }

    /// Fills strip `t`'s keys from its sources, as the serial path's per-layer fill does: a source
    /// in an earlier layer gives its finished buffer, anything else silence.
    fn fill(&mut self, t: usize) {
        let start = self.layer_start.get(t).map_or(0, |&s| usize::from(s));
        let p1: &[Strip] = self.p1;
        self.keys.fill(t..t + 1, |j| if j < start { p1.get(j) } else { None }, self.b.busses, self.b.frames);
    }

    /// The identity `t`'s job must carry for this graph (its chain as it is home now).
    fn identity(&self, t: usize) -> u64 {
        self.p1.get(t).map_or(0, |st| task_identity(t, chain_identity(st), self.graph.edge_hash(t)))
    }

    /// Takes `t` back from its cell with its keys. False when the cell was not free (counted).
    fn home(&mut self, t: usize) -> bool {
        let Some(st) = self.p1.get_mut(t) else { return false };
        if !self.pool.bring_home(t, st, self.events) {
            return false;
        }
        self.pool.take_keys_back(t, self.keys);
        true
    }
}

impl GraphHost for Host<'_, '_> {
    fn set(&mut self) -> &mut WorkerSet {
        &mut self.pool.set
    }

    fn start(&mut self, t: usize) -> Start {
        let Some(track) = self.track(t) else {
            // Not rendered by the plan (as the serial path, which still fills its keys).
            self.fill(t);
            return Start::Ready;
        };
        let reads = reads_input(track, &self.p1[t]);
        if t >= self.pool.capacity() {
            // Past the cells (not taken: `render_active` keeps such plans serial).
            self.fill(t);
            return Start::Callback;
        }
        if self.pool.is_away(t) {
            // A worker still holds it (the per-block rule), or a refused borrow kept it away.
            self.pool.silent[t] = true;
            return if self.pool.set.is_busy(t) { Start::Busy } else { Start::Unavailable };
        }
        if reads {
            self.fill(t);
            self.pool.on_callback[t] = true;
            return Start::Callback;
        }
        if !self.pool.admit(t, &self.p1[t]) {
            return Start::Unavailable;
        }
        self.fill(t);
        let (graph, task) = (self.graph.generation(), self.identity(t));
        if !self.pool.send(t, &mut self.p1[t], self.b) {
            self.pool.on_callback[t] = true;
            return Start::Callback;
        }
        let mut lent = false;
        let keys = &mut *self.keys;
        let borrowed = self.pool.cells.with(t, |job| {
            job.graph = graph;
            job.task = task;
            lent = keys.lend(t, &mut job.keys);
        });
        if !(borrowed && lent) {
            // The job could not take the keys (a protocol fault): render it here instead.
            self.pool.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            self.home(t);
            self.pool.on_callback[t] = true;
            return Start::Callback;
        }
        Start::Publish
    }

    fn refused(&mut self, t: usize) {
        self.pool.counters.conflicts.fetch_add(1, Ordering::Relaxed);
        self.home(t);
        self.pool.on_callback[t] = true;
    }

    fn render_here(&mut self, t: usize) {
        let s: &Session = self.b.session;
        let Some(st) = self.p1.get_mut(t) else { return };
        let Some(track) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t)) else { return };
        let b = self.b;
        let block = StripRender { s, busses: b.busses, live: b.live, pos: b.pos, frames: b.frames, any_solo: b.any_solo, keys: &*self.keys };
        guarded(st, |st| process_strip(track, st, &block));
        self.callback += 1;
    }

    fn poll(&mut self, deadline: Instant) -> Poll {
        let pool = &mut *self.pool;
        let (cells, g, marker) = (&pool.cells, self.g, self.b.live.marker);
        pool.set.poll(deadline, |i| {
            // Reclaimed by the callback: no worker can hold the cell. On the callback: the strip
            // marks the phase marker as a serial render does.
            cells.with(i, |job| {
                job.render(g, marker);
                #[cfg(test)]
                super::tests::corrupt_stamp(i, &mut job.rendered);
            });
        })
    }

    fn take(&mut self, t: usize, _: Outcome) -> bool {
        let mut stamp = Stamp::default();
        self.pool.cells.with(t, |job| stamp = job.rendered);
        if !self.home(t) {
            self.pool.silent[t] = true;
            return false;
        }
        // The completion identity: this graph, this task (its chain and its edges), this block.
        if stamp != (Stamp { graph: self.graph.generation(), task: self.identity(t), block: self.g }) {
            self.pool.counters.stale.fetch_add(1, Ordering::Relaxed);
            self.pool.silent[t] = true;
            return false;
        }
        true
    }

    fn skipped(&mut self, t: usize, _origin: usize) {
        if let Some(s) = self.pool.silent.get_mut(t) {
            *s = true;
        }
    }
}

impl StripPool {
    /// Takes back the keys strip `i`'s job holds (nothing when it holds none). Its cell must be
    /// free (the strip home or coming home).
    pub(crate) fn take_keys_back(&self, i: usize, keys: &mut KeyBank) {
        if !self.cells.with(i, |job| keys.take_back(i, &mut job.keys)) {
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// An aux keyed from a phase-1 strip that missed was skipped for the block (the graph's rule
    /// on the serial tail, slice 1): counted with the upstream-missed skips.
    pub(crate) fn note_tail_skip(&self) {
        self.counters.upstream_tail.fetch_add(1, Ordering::Relaxed);
    }

    /// Phase 1 of a keyed plan's block through the worker set as `graph` (see the module docs).
    /// The plan must have no more phase-1 strips than the pool has cells. Allocation-free; never
    /// waits on a worker past the deadline.
    pub(crate) fn render_graph(
        &mut self,
        b: &PoolBlock<'_>,
        p1: &mut [Strip],
        events: &mut EngineEvents,
        keys: &mut KeyBank,
        graph: &mut StripGraph,
    ) {
        let s: &Session = b.session;
        let g = self.generation + 1;
        let clock = BlockClock { frame: b.live.frame, frames: b.frames };
        let cells = self.away.len().min(p1.len());
        if !self.set.begin_block(g) {
            // Refused (a protocol fault): render what is home serially in table order (a source
            // renders before its keyed strips), leave the rest silent.
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            for i in 0..p1.len() {
                if self.is_away(i) {
                    continue;
                }
                let start = graph.layer_start(i);
                let (done, rest) = p1.split_at_mut(i);
                keys.fill(i..i + 1, |j| if j < start { done.get(j) } else { None }, b.busses, b.frames);
                let Some(st) = rest.first_mut() else { continue };
                if let Some(t) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t)) {
                    let block = StripRender { s, busses: b.busses, live: b.live, pos: b.pos, frames: b.frames, any_solo: b.any_solo, keys: &*keys };
                    guarded(st, |st| process_strip(t, st, &block));
                }
            }
            return;
        }
        self.generation = g;
        for (i, st) in p1.iter_mut().enumerate().take(cells) {
            self.silent[i] = false;
            self.on_callback[i] = false;
            // A worker finished since the last block: its (late) result was retired unread.
            if self.away[i] && !self.set.is_busy(i) && self.bring_home(i, st, events) {
                self.take_keys_back(i, keys);
            }
            if !self.away[i] {
                self.deliver(i, st, g, &clock, events, s);
            }
        }
        let deadline = self.deadline(b);
        let StripGraph { graph: table, run, layer_start, .. } = graph;
        let (report, callback) = {
            let mut host =
                Host { pool: &mut *self, p1: &mut *p1, events: &mut *events, keys: &mut *keys, graph: table, layer_start, b, g, callback: 0 };
            let report = drive(&mut host, table, run, deadline);
            (report, host.callback)
        };
        if Instant::now().saturating_duration_since(deadline) > OVERSHOOT {
            self.counters.overshoot.fetch_add(1, Ordering::Relaxed);
        }
        self.counters.callback_renders.fetch_add(callback, Ordering::Relaxed);
        let mut overruns = 0u32;
        for (i, st) in p1.iter_mut().enumerate().take(cells) {
            let Some(outcome) = self.set.outcome(i) else { continue };
            match outcome {
                // Home since the sweep (silent when its stamp did not match).
                Outcome::Mixed | Outcome::Inline => {}
                Outcome::Overrun(cause) => {
                    self.silent[i] = true;
                    match cause {
                        // Taken back unrendered (or never sent, when it became ready too late):
                        // home, its queue keeps this block's events.
                        OverrunCause::Unstarted => {
                            if self.away[i] && self.bring_home(i, st, events) {
                                self.take_keys_back(i, keys);
                            }
                        }
                        // The worker keeps it; the next blocks skip it until it is done.
                        OverrunCause::Late | OverrunCause::Busy => {}
                        // Never sent: home, its queue keeps this block's events.
                        OverrunCause::UpstreamMissed => {}
                    }
                    // Counted once, at the origin: a skip for a source is not an overrun of its own.
                    if cause != OverrunCause::UpstreamMissed {
                        overruns += 1;
                    }
                }
                Outcome::Faulted => {
                    if self.bring_home(i, st, events) {
                        self.take_keys_back(i, keys);
                    }
                    self.silent[i] = true;
                    self.suspect_gen[i] = st.generation;
                    self.counters.faulted.fetch_add(1, Ordering::Relaxed);
                }
                Outcome::NotPublished | Outcome::Pending => {}
            }
            // `Busy` and the upstream-missed of an away strip defer what was parked for it;
            // a home strip's events are already in its queue.
            let n = self.live_len[i];
            if n > 0 && self.defer.defer_for(i, g, outcome, &self.live[i * LIVE..i * LIVE + n]) {
                self.live_len[i] = 0;
            }
        }
        self.last_overruns = overruns;
        self.mirror(report.overrun > 0);
    }
}
