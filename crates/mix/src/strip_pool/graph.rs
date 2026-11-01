//! The graph path of the strip fan-out (issue #174, ADR 0029 amendment, slice 1): a plan with
//! sidechain keys renders its phase-1 strips through the render workers as the plan's task graph
//! ([`crate::graph`]), driven by [`transit_rt::task_graph::drive`]. The default for a keyed plan
//! on the pool path (since 2026-10-29); not taken with `TRANSIT_RENDER_GRAPH=0` (or
//! [`MixEngine::force_graph`]`(false)`), where a keyed plan renders serially, unless a strip of a
//! keyed plan is still away.
//!
//! **One block** (`StripPool::render_graph`, after `render_active` cleared the busses and aligned
//! the keys):
//!
//! 1. `begin_block(g)`; a strip whose worker has finished comes home with its keys; each home
//!    strip's deferred MIDI is delivered, as on the plain pool path.
//! 2. The driver starts every strip whose key sources are ready (at first, the strips with none):
//!    the callback copies its keys into the plan's bank from the sources' finished buffers
//!    (`KeyBank::fill_deferred`: the early-key delay is left pending and runs when the strip
//!    renders, `KeyBank::apply_pending`, so a strip taken back unrendered leaves its key lines
//!    untouched), sends it into its cell, lends it the keys, stamps
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

use super::graph_host::Host;
use super::*;
use crate::graph::StripGraph;
use transit_rt::task_graph::drive;

impl StripPool {
    /// A refused block opening or an oversized keyed plan never renders an away spare.
    pub(crate) fn recover_phase1(&mut self, b: &PoolBlock<'_>, strips: &mut [Strip], keys: &mut KeyBank, graph: &mut StripGraph) {
        for i in 0..strips.len() {
            let unavailable = self.preparation_blocked(i)
                || strips[i].panic.is_some()
                || keys.sources_of(i).any(|j| self.is_silent(j) || self.is_away(j) || graph.recovery_skipped.get(j) == Some(&true));
            if let Some(skipped) = graph.recovery_skipped.get_mut(i) {
                *skipped = unavailable;
            }
            if let Some(silent) = self.silent.get_mut(i) {
                *silent = unavailable;
            }
            if unavailable {
                continue;
            }
            let start = graph.layer_start(i);
            let (before, rest) = strips.split_at_mut(i);
            keys.fill(i..i + 1, |j| if j < start { before.get(j) } else { None }, b.busses, b.frames);
            if let Some(st) = rest.first_mut()
                && let Some(t) = b.session.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t))
            {
                guarded(st, |st| {
                    process_strip(
                        t,
                        st,
                        &StripRender { s: b.session, busses: b.busses, live: b.live, pos: b.pos, frames: b.frames, any_solo: b.any_solo, keys },
                    )
                });
                if st.panic.is_some() {
                    if let Some(skipped) = graph.recovery_skipped.get_mut(i) {
                        *skipped = true;
                    }
                    if let Some(silent) = self.silent.get_mut(i) {
                        *silent = true;
                    }
                }
            }
        }
    }
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
        self.last_tail = false;
        let s: &Session = b.session;
        let g = self.generation + 1;
        let clock = BlockClock { frame: b.live.frame, frames: b.frames };
        let cells = self.away.len().min(p1.len());
        if !self.set.begin_block(g) {
            // Refused (a protocol fault): render what is home serially in table order (a source
            // renders before its keyed strips), leave the rest silent.
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            self.recover_phase1(b, p1, keys, graph);
            self.note_held(s);
            return;
        }
        self.generation = g;
        self.set.set_block_timing(self.deadline(b), self.host_period);
        self.returned.fill(false);
        for (i, st) in p1.iter_mut().enumerate().take(cells) {
            self.silent[i] = false;
            self.on_callback[i] = false;
            // A worker finished since the last block: its (late) result was retired unread.
            if self.away[i] && !self.set.is_busy(i) && self.bring_home_ownership(i, st) {
                self.take_keys_back(i, keys);
                self.returned[i] = true;
            }
        }
        crate::render_pool::panic_checkpoint(p1, Some(self));
        for (i, st) in p1.iter_mut().enumerate().take(cells) {
            if !self.away[i] {
                if self.returned[i] {
                    self.apply_home_pending(i, st, events);
                }
                self.name(i, st);
                self.deliver(i, st, g, &clock, events, s);
            }
        }
        let deadline = self.deadline(b);
        let StripGraph { graph: table, run, layer_start, .. } = graph;
        let (_report, callback) = {
            let mut host = Host {
                pool: &mut *self,
                p1: &mut *p1,
                events: &mut *events,
                keys: &mut *keys,
                graph: table,
                layer_start,
                b,
                g,
                callback: 0,
                tail: None,
            };
            let report = drive(&mut host, table, run, deadline);
            (report, host.callback)
        };
        if Instant::now().saturating_duration_since(deadline) > OVERSHOOT {
            self.note_host_overshoot();
        }
        self.counters.callback_renders.fetch_add(callback, Ordering::Relaxed);
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
        self.mirror();
    }
}
