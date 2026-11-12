//! Callback-owned MIX and commit tasks, and tail drive ownership cleanup.
use super::*;
use crate::graph::snapshots::Snapshots;
use crate::plan::PlanTables;
use crate::tail::{DeviceLatency, RenderPhases, fold_latency};
use transit_rt::task_graph::drive;

pub(super) struct TailWork<'a> {
    pub(super) phase1: usize,
    pub(super) others: &'a mut [Strip],
    pub(super) busses: &'a mut BusTable,
    pub(super) main: &'a mut [Vec<f32>],
    pub(super) main_fmt: ChannelFormat,
    pub(super) sinks: &'a mut [Sink],
    pub(super) status: &'a mut PlanStatus,
    pub(super) inputs: &'a mut Snapshots,
    pub(super) law: PanLaw,
    pub(super) host_frame: u64,
    pub(super) latency: DeviceLatency,
}

impl TailWork<'_> {
    pub(super) fn control(&mut self, task: usize, strips: &mut [Strip], pool: &StripPool, b: &PoolBlock<'_>) {
        if task == self.phase1 {
            let (p1, auxes) = strips.split_at_mut(self.phase1);
            self.latency = fold_latency(
                b.session,
                RenderPhases { p1, auxes, others: self.others, phase1: self.phase1 },
                self.busses,
                self.sinks,
                b.pos,
                !b.live.input.is_empty(),
                &|i| pool.preparation_blocked(i),
            );
            if self.latency.aborted {
                return;
            }
            for (i, st) in p1.iter_mut().enumerate() {
                self.commit(i, st, pool, b);
            }
        } else if task > self.phase1 && (task - self.phase1).is_multiple_of(2) && !self.latency.aborted {
            let slot = self.phase1 + (task - self.phase1 - 2) / 2;
            if let Some(st) = strips.get_mut(slot) {
                // The render stamp is set only by a usable direct or taken result of this block.
                if st.rendered.block == pool.generation {
                    self.commit(slot, st, pool, b);
                }
            }
        }
    }

    fn commit(&mut self, slot: usize, st: &mut Strip, pool: &StripPool, b: &PoolBlock<'_>) {
        if pool.is_away(slot) || pool.is_silent(slot) || st.panic.is_some() {
            return;
        }
        let Some(t) = b.session.tracks.get(usize::from(st.track)).filter(|t| live(t)) else { return };
        st.meter.measure(&st.buf, b.frames, st.gr, self.host_frame);
        st.meter.plugin_failures = st.failures;
        let need = route_into(
            t,
            st,
            b.pos,
            b.frames,
            MixOut {
                busses: self.busses,
                main: self.main,
                main_fmt: self.main_fmt,
                sinks: self.sinks,
                law: self.law,
                main_arrival: self.latency.main,
            },
        );
        self.status.note_capacity(need, st);
    }
}

impl StripPool {
    pub(crate) fn render_tail(
        &mut self,
        b: &PoolBlock<'_>,
        tables: &mut PlanTables,
        events: &mut EngineEvents,
        status: &mut PlanStatus,
        law: PanLaw,
        host_frame: u64,
    ) -> Option<DeviceLatency> {
        let g = self.generation + 1;
        if !self.set.begin_block(g) {
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let deadline = self.deadline(b);
        self.set.set_block_timing(deadline, self.host_period);
        self.generation = g;
        self.last_tail = true;
        self.tail_from = tables.phase1;
        let clock = BlockClock { frame: b.live.frame, frames: b.frames };
        self.returned.fill(false);
        for (i, st) in tables.strips.iter_mut().take(tables.aux_end).enumerate().take(self.capacity()) {
            self.silent[i] = false;
            self.on_callback[i] = false;
            if self.away[i] && !self.set.is_busy(i) && self.bring_home_ownership(i, st) {
                self.take_keys_back(i, &mut tables.keys);
                self.cells.with(i, |job| tables.inputs.take_back(i, &mut job.busses));
                self.returned[i] = true;
            }
        }
        crate::render_pool::panic_checkpoint(&mut tables.strips, Some(self));
        for (i, st) in tables.strips.iter_mut().take(tables.aux_end).enumerate().take(self.capacity()) {
            if !self.away[i] {
                if self.returned[i] {
                    self.apply_home_pending(i, st, events);
                }
                self.name(i, st);
                self.deliver(i, st, g, &clock, events, b.session);
            }
        }
        tables.prepare_block(b.session, b.pos, b.frames, b.sample_rate, !b.live.input.is_empty(), status, Some(self));
        let PlanTables { strips, phase1, aux_end, keys, inputs, graph, busses, main, main_fmt, sinks, .. } = tables;
        let (render, others) = strips.split_at_mut(*aux_end);
        let mut host = super::graph_host::Host {
            pool: self,
            p1: render,
            events,
            keys,
            graph: &graph.tail.graph,
            layer_start: &graph.layer_start,
            b,
            g,
            callback: 0,
            tail: Some(TailWork {
                phase1: *phase1,
                others,
                busses,
                main,
                main_fmt: *main_fmt,
                sinks,
                status,
                inputs,
                law,
                host_frame,
                latency: DeviceLatency { main: 0, device: 0, aborted: false },
            }),
        };
        drive(&mut host, &graph.tail.graph, &mut graph.tail.run, deadline);
        let callback = host.callback;
        let latency = host.tail.take().map(|t| t.latency);
        self.counters.callback_renders.fetch_add(callback, Ordering::Relaxed);
        for (i, st) in render.iter_mut().enumerate().take(self.capacity()) {
            let Some(outcome) = self.set.outcome(i) else { continue };
            match outcome {
                Outcome::Overrun(cause) => {
                    self.silent[i] = true;
                    if cause == OverrunCause::Unstarted && self.away[i] && self.bring_home(i, st, events) {
                        self.take_keys_back(i, keys);
                        self.cells.with(i, |job| inputs.take_back(i, &mut job.busses));
                    }
                }
                Outcome::Faulted => {
                    if self.away[i] && self.bring_home(i, st, events) {
                        self.take_keys_back(i, keys);
                        self.cells.with(i, |job| inputs.take_back(i, &mut job.busses));
                    }
                    self.silent[i] = true;
                    self.suspect_gen[i] = st.generation;
                    self.counters.faulted.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
            let n = self.live_len[i];
            if n > 0 && self.defer.defer_for(i, g, outcome, &self.live[i * LIVE..i * LIVE + n]) {
                self.live_len[i] = 0;
            }
        }
        if Instant::now().saturating_duration_since(deadline) > OVERSHOOT {
            self.note_host_overshoot();
        }
        self.mirror();
        for (k, skipped) in graph.tail_skipped.iter_mut().enumerate() {
            // `tail_skipped` is the serial oracle's no-execution flag. Late work did execute
            // and is discarded; only Busy, Unstarted and hard misses mean Skip (#210).
            *skipped = matches!(
                self.set.outcome(*phase1 + k),
                Some(Outcome::Overrun(OverrunCause::Busy | OverrunCause::Unstarted | OverrunCause::UpstreamMissed))
            );
        }
        crate::render_pool::panic_checkpoint(strips, Some(self));
        latency
    }
}
