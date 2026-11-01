use super::*;
use crate::graph::task_identity;
use transit_rt::task_graph::{GraphHost, Rendered, Start, TaskGraph, Window};
use transit_rt::worker_set::Poll;

/// One driver host. Tail controls have no slot; render tasks use their explicit slot.
pub(super) struct Host<'a, 'b> {
    pub(super) pool: &'a mut StripPool,
    pub(super) p1: &'a mut [Strip],
    pub(super) events: &'a mut EngineEvents,
    pub(super) keys: &'a mut KeyBank,
    pub(super) graph: &'a TaskGraph,
    pub(super) layer_start: &'a [u16],
    pub(super) b: &'a PoolBlock<'b>,
    pub(super) g: u64,
    pub(super) callback: u64,
    pub(super) tail: Option<super::tail::TailWork<'a>>,
}

impl Host<'_, '_> {
    fn track(&self, slot: usize) -> Option<&Track> {
        self.p1.get(slot).and_then(|st| self.b.session.tracks.get(usize::from(st.track)))
            .filter(|t| live(t) && (self.tail.is_some() || !is_aux(t)))
    }

    fn fill(&mut self, slot: usize, deferred: bool) {
        let start = self.layer_start.get(slot).map_or(slot, |&s| usize::from(s));
        let p1: &[Strip] = self.p1;
        let buses = self.tail.as_ref().map_or(self.b.busses, |tail| &*tail.busses);
        let source = |j| if j < start { p1.get(j) } else { None };
        if deferred { self.keys.fill_deferred(slot..slot + 1, source, buses, self.b.frames); }
        else { self.keys.fill(slot..slot + 1, source, buses, self.b.frames); }
    }

    fn identity(&self, task: usize, slot: usize) -> u64 {
        self.p1.get(slot).map_or(0, |st| task_identity(slot, chain_identity(st), self.graph.edge_hash(task)))
    }

    fn home(&mut self, slot: usize) -> bool {
        if self.pool.is_away(slot) {
            let Some(st) = self.p1.get_mut(slot) else { return false };
            if !self.pool.bring_home(slot, st, self.events) { return false; }
        }
        self.pool.take_keys_back(slot, self.keys);
        if let Some(tail) = self.tail.as_mut() {
            self.pool.cells.with(slot, |job| tail.inputs.take_back(slot, &mut job.busses));
        }
        true
    }

    fn aux(&self, slot: usize) -> bool { self.tail.is_some() && slot >= self.layer_start.len() }

    fn fits(&self, slot: usize, window: Window) -> bool {
        let left = self.pool.deadline(self.b).saturating_duration_since(Instant::now());
        #[cfg(feature = "testing")]
        let left = self.pool.set.slot_overrides(slot).budget_left.unwrap_or(left);
        let worst = self.pool.set.worst_case(slot).unwrap_or_default();
        let fit = !worst.is_zero() && worst <= left;
        #[cfg(feature = "testing")]
        let fit = fit || self.pool.set.slot_overrides(slot).force_publish;
        window == Window::Open && fit
    }
}

impl GraphHost for Host<'_, '_> {
    fn set(&mut self) -> &mut WorkerSet { &mut self.pool.set }

    fn start(&mut self, task: usize, window: Window) -> Start {
        let Some(slot) = self.graph.slot(task) else {
            if let Some(tail) = self.tail.as_mut() { tail.control(task, self.p1, self.pool, self.b); }
            return Start::Ready;
        };
        let Some(track) = self.track(slot) else { return Start::Ready };
        let pinned = reads_input(track, &self.p1[slot]);
        if slot >= self.pool.capacity() { self.fill(slot, false); return Start::Callback; }
        if self.pool.is_away(slot) {
            self.pool.silent[slot] = true;
            return if self.pool.set.is_busy(slot) { Start::Busy } else { Start::Unavailable };
        }
        if self.p1[slot].panic.is_some() || !self.pool.admit(slot, &self.p1[slot]) { return Start::Unavailable; }
        let aux = self.aux(slot);
        if aux && self.tail.as_ref().is_some_and(|t| t.latency.aborted) { return Start::Unavailable; }
        if pinned {
            self.fill(slot, false);
            self.pool.on_callback[slot] = true;
            return Start::Callback;
        }
        if !aux && window == Window::Closed { return Start::Forgo; }
        self.fill(slot, true);
        if aux && !self.fits(slot, window) {
            self.pool.on_callback[slot] = true;
            return Start::Callback;
        }
        if let Some(tail) = self.tail.as_mut() { tail.inputs.copy(slot, tail.busses, self.b.frames); }
        let (graph, identity) = (self.graph.generation(), self.identity(task, slot));
        if !self.pool.send(slot, &mut self.p1[slot], self.b) {
            return if self.pool.is_away(slot) { Start::Unavailable } else { Start::Callback };
        }
        let mut lent = false;
        let keys = &mut *self.keys;
        let tail = &mut self.tail;
        let borrowed = self.pool.cells.with(slot, |job| {
            lent = keys.lend(slot, &mut job.keys);
            if let Some(tail) = tail { lent &= tail.inputs.lend(slot, &mut job.busses); }
            job.graph = graph;
            job.task = identity;
        });
        if !(borrowed && lent) {
            self.pool.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            return if self.home(slot) { Start::Callback } else { Start::Unavailable };
        }
        if aux { self.pool.set.set_inline_at_deadline(slot); }
        Start::Publish
    }

    fn refused(&mut self, task: usize) {
        if let Some(slot) = self.graph.slot(task) {
            self.pool.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            if self.home(slot) { self.pool.on_callback[slot] = true; }
        }
    }

    fn render_here(&mut self, task: usize) -> Rendered {
        let Some(slot) = self.graph.slot(task) else { return Rendered::Usable };
        if self.pool.is_away(slot) || self.p1.get(slot).is_none_or(|s| s.panic.is_some()) { return Rendered::Unusable; }
        let stamp = Stamp { graph: self.graph.generation(), task: self.identity(task, slot), block: self.g };
        let aux = self.aux(slot);
        let Some(st) = self.p1.get_mut(slot) else { return Rendered::Unusable };
        let s: &Session = self.b.session;
        let Some(track) = s.tracks.get(usize::from(st.track)).filter(|t| live(t)) else { return Rendered::Usable };
        self.keys.apply_pending(Some(slot));
        let start = Instant::now();
        let b = self.b;
        let busses = self.tail.as_ref().map_or(b.busses, |t| &*t.busses);
        let block = StripRender { s, busses, live: b.live, pos: b.pos, frames: b.frames, any_solo: b.any_solo, keys: &*self.keys };
        guarded(st, |st| process_strip(track, st, &block));
        st.rendered = stamp;
        let usable = st.panic.is_none();
        if aux {
            let elapsed = start.elapsed();
            #[cfg(feature = "testing")]
            let elapsed = self.pool.set.cost_clock().elapsed(start, start + elapsed);
            self.pool.set.record_callback_cost(slot, elapsed);
        }
        self.callback += 1;
        if !usable {
            if let Some(silent) = self.pool.silent.get_mut(slot) { *silent = true; }
            Rendered::Unusable
        } else { Rendered::Usable }
    }

    fn poll(&mut self, deadline: Instant) -> Poll {
        #[cfg(feature = "testing")]
        let deadline = self.pool.gated_deadline(deadline);
        let pool = &mut *self.pool;
        let (cells, g, marker) = (&pool.cells, self.g, self.b.live.marker);
        pool.set.poll(deadline, |i| {
            cells.with(i, |job| {
                job.render(g, marker);
                #[cfg(test)]
                super::tests::corrupt_stamp(i, &mut job.rendered);
            });
        })
    }

    fn take(&mut self, task: usize, _: Outcome) -> bool {
        let Some(slot) = self.graph.slot(task) else { return true };
        let mut stamp = Stamp::default();
        self.pool.cells.with(slot, |job| stamp = job.rendered);
        if !self.home(slot) { self.pool.silent[slot] = true; return false; }
        if self.p1[slot].panic.is_some() { self.pool.silent[slot] = true; return false; }
        if stamp != (Stamp { graph: self.graph.generation(), task: self.identity(task, slot), block: self.g }) {
            self.pool.counters.stale.fetch_add(1, Ordering::Relaxed);
            self.pool.silent[slot] = true;
            return false;
        }
        self.p1[slot].rendered = stamp;
        true
    }

    fn skipped(&mut self, task: usize, _origin: usize) {
        if let Some(slot) = self.graph.slot(task) && let Some(s) = self.pool.silent.get_mut(slot) { *s = true; }
    }
}
