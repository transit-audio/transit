use super::*;
use crate::graph::task_identity;
use transit_rt::task_graph::{GraphHost, Rendered, Start, TaskGraph, Window};
use transit_rt::worker_set::Poll;

/// The driver's view of the pool for one block.
pub(super) struct Host<'a, 'b> {
    pub(super) pool: &'a mut StripPool,
    pub(super) p1: &'a mut [Strip],
    pub(super) events: &'a mut EngineEvents,
    pub(super) keys: &'a mut KeyBank,
    pub(super) graph: &'a TaskGraph,
    pub(super) layer_start: &'a [u16],
    pub(super) b: &'a PoolBlock<'b>,
    pub(super) g: u64,
    /// Strips rendered on the callback this block.
    pub(super) callback: u64,
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

    /// [`Host::fill`] for a strip about to be published: its keys are copied now, their early-key
    /// delay runs only when it renders (#174, found by #210: a strip the deadline takes back
    /// unrendered must leave its key lines untouched).
    fn fill_deferred(&mut self, t: usize) {
        let start = self.layer_start.get(t).map_or(0, |&s| usize::from(s));
        let p1: &[Strip] = self.p1;
        self.keys.fill_deferred(t..t + 1, |j| if j < start { p1.get(j) } else { None }, self.b.busses, self.b.frames);
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

    fn start(&mut self, t: usize, window: Window) -> Start {
        // Slice-1 policy until the tail host installs the section 11 start order.
        if window == Window::Closed {
            return Start::Forgo;
        }
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
        // Pending until it renders: on a worker or inline from its job, or here if it falls back
        // to the callback (`render_here` runs what is pending).
        self.fill_deferred(t);
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

    fn render_here(&mut self, t: usize) -> Rendered {
        let s: &Session = self.b.session;
        let Some(st) = self.p1.get_mut(t) else { return Rendered::Unusable };
        let Some(track) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t)) else { return Rendered::Usable };
        // A strip that fell back to the callback after its keys were filled for a worker: their
        // early-key delay runs now, as it renders.
        self.keys.apply_pending(Some(t));
        let b = self.b;
        let block = StripRender { s, busses: b.busses, live: b.live, pos: b.pos, frames: b.frames, any_solo: b.any_solo, keys: &*self.keys };
        guarded(st, |st| process_strip(track, st, &block));
        self.callback += 1;
        Rendered::Usable
    }

    fn poll(&mut self, deadline: Instant) -> Poll {
        // Test seam (issue #210): the deterministic deadline of a held block.
        #[cfg(feature = "testing")]
        let deadline = self.pool.gated_deadline(deadline);
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
