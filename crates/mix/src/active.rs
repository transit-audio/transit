//! A realtime engine's side of the plan contract: [`MixEngine::adopt_plan`] (validate, then swap)
//! and [`MixEngine::render_active`] (render the active plan's tables). Neither allocates, frees,
//! locks, makes a system call, waits or calls a plug-in lifecycle function (the native Sampler's
//! `set_sample` aside, which only stops voices and stores the shared sample).

use super::compile::SAMPLER;
use super::plan::{ActivePlan, Attach, CellKind, CellLoc, PlanTables, Shape};
use super::*;
use transit_rt::phase_marker::Phase;

/// The keys a pool block hands its plain path when the graph path fills the plan's own.
static NO_KEYS: KeyBank = KeyBank::EMPTY;

/// The Session of an engine without a plan.
static NO_METER: std::sync::LazyLock<StripMeter> = std::sync::LazyLock::new(StripMeter::default);
static NO_SESSION: std::sync::LazyLock<Arc<Session>> = std::sync::LazyLock::new(|| Arc::new(Session::default()));

/// The identity of an engine without a plan.
const NO_PLAN: PlanIdentity = PlanIdentity {
    generation: PlanGeneration::EMPTY,
    base: PlanGeneration::EMPTY,
    target: RenderTarget { device: DeviceGeneration(0), sample_rate: 0, max_block: 0, out_channels: 0, in_channels: 0 },
};

#[cfg(test)]
std::thread_local! {
    /// Test hook: the commit step at which an adoption unwinds (0: never).
    pub(crate) static FAULT_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test hook: unwind at commit step `step` when a test asked for it.
#[cfg(test)]
#[allow(clippy::panic)]
fn fault_point(step: usize) {
    if FAULT_AT.get() == step {
        panic!("injected fault at commit step {step}");
    }
}

#[cfg(not(test))]
fn fault_point(_step: usize) {}

/// The shape of a live strip's state, `None` when it holds none.
fn strip_shape(st: &Strip) -> Option<Shape> {
    (st.generation != SlotGen::VACANT).then(|| Shape {
        kind: CellKind::Strip,
        ch: u16::try_from(st.buf.len()).unwrap_or(u16::MAX),
        layout: st.layout,
    })
}

fn slot_shape(kind: CellKind, s: &PluginSlot) -> Shape {
    Shape { kind, ch: u16::try_from(s.ch).unwrap_or(u16::MAX), layout: s.layout }
}

/// Moves a strip's running state into a new strip shell: note chasing, mute, gain
/// reduction, latency, failures, meter, and its delay lines whole (with their contents and
/// capacity) where the shell holds placeholders for them. A shell with real (larger) lines keeps
/// them. Only swaps and copies; what the shell had goes back with the old strip.
fn move_strip(old: &mut Strip, new: &mut Strip) {
    new.chase = old.chase;
    new.muted = old.muted;
    new.gr = old.gr;
    new.latency = old.latency;
    new.failures = old.failures;
    for (a, b) in [(&mut old.tap, &mut new.tap), (&mut old.tap_pre, &mut new.tap_pre)] {
        if b.is_placeholder() {
            std::mem::swap(a, b);
        } else {
            // Longer lines prepared by the plan: the delay continues from the old line's history.
            b.take_history(a);
        }
    }
    new.edges = old.edges;
    // The delays before keyed inserts continue from the old lines' history, insert by insert.
    for k in new.key_in.iter_mut() {
        if let Some(o) = old.key_in.iter().find(|o| o.slot == k.slot) {
            k.line.take_history(&o.line);
        }
    }
    if old.meter.peaks.len() == new.meter.peaks.len() {
        std::mem::swap(&mut old.meter, &mut new.meter);
    }
    new.generation = old.generation;
    old.generation = SlotGen::VACANT;
}

/// Validates `plan` against the active plan without changing either (beyond the plan's claim
/// scratch).
fn validate(active: &ActivePlan, plan: &mut PreparedPlan) -> Result<(), RejectReason> {
    if plan.id.base != active.id.generation || plan.id.generation <= active.id.generation {
        return Err(RejectReason::BaseMismatch);
    }
    if plan.id.target != active.id.target {
        return Err(RejectReason::TargetMismatch);
    }
    let old = &active.tables;
    let Some(new) = plan.tables.as_ref() else {
        if !plan.moves.is_empty() {
            return Err(RejectReason::MoveOutOfRange);
        }
        if !plan.attach.is_empty() {
            return Err(RejectReason::AttachInvalid);
        }
        return Ok(());
    };
    if plan.src_claimed.len() != old.cells.len() || plan.dst_claimed.len() != new.cells.len() {
        return Err(RejectReason::MoveOutOfRange);
    }
    for mv in &plan.moves {
        let (from, to) = (usize::from(mv.from), usize::from(mv.to));
        let (Some(src), Some(dst)) = (old.cells.get(from), new.cells.get(to)) else { return Err(RejectReason::MoveOutOfRange) };
        let (Some(sc), Some(dc)) = (plan.src_claimed.get(from).copied(), plan.dst_claimed.get(to).copied()) else {
            return Err(RejectReason::MoveOutOfRange);
        };
        if sc {
            return Err(RejectReason::DuplicateSource);
        }
        if dc {
            return Err(RejectReason::DuplicateDestination);
        }
        if let (Some(s), Some(d)) = (plan.src_claimed.get_mut(from), plan.dst_claimed.get_mut(to)) {
            *s = true;
            *d = true;
        }
        if src.shape.kind != mv.kind || dst.shape.kind != mv.kind {
            return Err(RejectReason::ShapeMismatch);
        }
        let (live, vacant) = if mv.kind == CellKind::Strip {
            let s = old.strips.get(usize::from(src.strip)).filter(|_| usize::from(src.strip) == from);
            let d = new.strips.get(usize::from(dst.strip)).filter(|_| usize::from(dst.strip) == to);
            let (Some(s), Some(d)) = (s, d) else { return Err(RejectReason::MoveOutOfRange) };
            let shell = Shape { kind: CellKind::Strip, ch: u16::try_from(d.buf.len()).unwrap_or(u16::MAX), layout: d.layout };
            if shell != dst.shape {
                return Err(RejectReason::ShapeMismatch);
            }
            (strip_shape(s).map(|sh| (sh, s.generation)), d.generation == SlotGen::VACANT)
        } else {
            let (Some(s), Some(d)) = (old.slot(src), new.slot(dst)) else { return Err(RejectReason::MoveOutOfRange) };
            (s.as_ref().map(|s| (slot_shape(mv.kind, s), s.generation)), d.is_none())
        };
        let Some((shape, generation)) = live else { return Err(RejectReason::SourceNotLive) };
        if generation != mv.expect {
            return Err(RejectReason::SlotGenMismatch);
        }
        if !vacant {
            return Err(RejectReason::DestinationNotVacant);
        }
        if shape != dst.shape {
            return Err(RejectReason::ShapeMismatch);
        }
    }
    for a in &plan.attach {
        let (cell, from_move, sample) = match a {
            Attach::Applied { cell, from_move, storage } => {
                if !plan::needs_storage(storage.len()) {
                    return Err(RejectReason::AttachInvalid);
                }
                (*cell, *from_move, false)
            }
            Attach::Sample { cell, from_move, .. } => (*cell, *from_move, true),
        };
        let Some(loc) = new.cells.get(usize::from(cell)) else { return Err(RejectReason::AttachInvalid) };
        let kind_ok = if sample { loc.shape.kind == CellKind::Instrument } else { matches!(loc.shape.kind, CellKind::Insert | CellKind::Instrument) };
        if !kind_ok {
            return Err(RejectReason::AttachInvalid);
        }
        // The slot that will be live there after the moves: the one its move brings (named by
        // index, O(1)), or the fresh one already in the new tables.
        let moved = plan.dst_claimed.get(usize::from(cell)).copied().unwrap_or(false);
        let slot = match (moved, from_move) {
            (true, Some(i)) => plan
                .moves
                .get(usize::from(i))
                .filter(|m| m.to == cell)
                .and_then(|m| old.cells.get(usize::from(m.from)))
                .and_then(|l| old.slot(l))
                .and_then(Option::as_ref),
            (false, None) => new.slot(loc).and_then(Option::as_ref),
            _ => None,
        };
        let Some(slot) = slot else { return Err(RejectReason::AttachInvalid) };
        if slot_shape(loc.shape.kind, slot) != loc.shape || (sample && slot.id != SAMPLER) {
            return Err(RejectReason::AttachInvalid);
        }
    }
    Ok(())
}

/// Swaps the cell `src` of `old` with the cell `dst` of `new`.
fn swap_cells(kind: CellKind, old: &mut PlanTables, src: &CellLoc, new: &mut PlanTables, dst: &CellLoc) {
    if kind == CellKind::Strip {
        if let (Some(a), Some(b)) = (old.strips.get_mut(usize::from(src.strip)), new.strips.get_mut(usize::from(dst.strip))) {
            move_strip(a, b);
        }
    } else if let (Some(a), Some(b)) = (old.slot_mut(src), new.slot_mut(dst)) {
        std::mem::swap(a, b);
    }
}

impl MixEngine {
    /// A realtime engine for `target` (control thread). It renders plans adopted with
    /// [`MixEngine::adopt_plan`] through [`MixEngine::render_active`]; its generation starts at
    /// [`PlanGeneration::EMPTY`] with nothing to render.
    #[must_use]
    pub fn realtime(target: RenderTarget) -> Self {
        let target = target.normalized();
        let mut eng = MixEngine::new(target.block());
        eng.plan = Some(Box::new(ActivePlan {
            id: PlanIdentity { generation: PlanGeneration::EMPTY, base: PlanGeneration::EMPTY, target },
            session: Arc::new(Session::default()),
            tables: PlanTables::default(),
            status: PlanStatus::default(),
            faulted: false,
        }));
        eng
    }

    /// Audio thread: adopts the plan in `env` in place, at most once per envelope. It validates
    /// first and changes nothing when it refuses (the envelope keeps its entire input); then it
    /// only swaps values between the engine and the envelope, which afterwards holds everything
    /// the plan displaced. No allocation, free, lock, system call or lifecycle call; the work is
    /// bounded by the cells of the old and the new plan.
    pub fn adopt_plan(&mut self, env: &mut PlanEnvelope) -> Adoption {
        if env.state != EnvelopeState::Pending {
            return Adoption::AlreadyServiced;
        }
        let Some(active) = self.plan.as_deref_mut() else {
            env.state = EnvelopeState::Rejected { reason: RejectReason::TargetMismatch, active: PlanGeneration::EMPTY };
            return Adoption::Rejected(RejectReason::TargetMismatch);
        };
        let reject = |env: &mut PlanEnvelope, reason, active: &ActivePlan| {
            env.state = EnvelopeState::Rejected { reason, active: active.id.generation };
            Adoption::Rejected(reason)
        };
        if active.faulted {
            return reject(env, RejectReason::Faulted, active);
        }
        // Every strip must be home: a strip a render worker holds cannot move (ADR 0029). Playback
        // defers adoption until then ([`MixEngine::render_workers_settled`]).
        if let Some(pool) = self.pool.as_mut() {
            if !pool.settle(&mut active.tables.strips, &mut active.tables.keys, &mut self.events, &active.session, self.max_block) {
                return reject(env, RejectReason::StripsInFlight, active);
            }
            // Deferred MIDI goes into the instances now, so it moves with them.
            pool.flush_parked(&mut active.tables.strips, &mut self.events, &active.session, self.max_block);
        }
        if let Err(reason) = validate(active, &mut env.plan) {
            return reject(env, reason, active);
        }
        // Commit. Every step is a complete swap; the plan never leaves its envelope, so an unwind
        // at any point leaves each owner either here or there. `faulted` stays set if one does.
        active.faulted = true;
        env.committing = true;
        let replaced = active.id.generation;
        let plan = &mut env.plan;
        if let Some(new) = plan.tables.as_mut() {
            for mv in plan.moves.iter() {
                let (Some(src), Some(dst)) = (active.tables.cells.get(usize::from(mv.from)).copied(), new.cells.get(usize::from(mv.to)).copied())
                else {
                    continue;
                };
                swap_cells(mv.kind, &mut active.tables, &src, new, &dst);
                fault_point(1);
            }
            for a in plan.attach.iter_mut() {
                let cell = match a {
                    Attach::Applied { cell, .. } | Attach::Sample { cell, .. } => *cell,
                };
                let Some(loc) = new.cells.get(usize::from(cell)).copied() else { continue };
                let Some(Some(slot)) = new.slot_mut(&loc) else { continue };
                match a {
                    Attach::Applied { storage, .. } => {
                        std::mem::swap(&mut slot.applied.heap, storage);
                        // The next block treats the map as new: every value is pushed again.
                        slot.applied.len = usize::MAX;
                    }
                    Attach::Sample { audio, wrapper, .. } => {
                        std::mem::swap(&mut slot.sample_audio, audio);
                        std::mem::swap(&mut slot.sample_wrapper, wrapper);
                    }
                }
                fault_point(2);
            }
            fault_point(3);
            std::mem::swap(&mut active.tables, new);
            // The displaced instruments' queued events retire with them (F10).
            MixEngine::retire_events(&mut self.events.status, &mut new.strips);
        }
        std::mem::swap(&mut active.session, &mut plan.session);
        fault_point(4);
        // An instrument on a track that became inactive keeps no pending events (F10).
        MixEngine::void_silent_instruments(active);
        // Last: the native Sampler takes its new sample. The old (audio, wrapper) pair is already
        // in the envelope, so the wrapper the Sampler drops here is never the last owner.
        for a in plan.attach.iter() {
            let Attach::Sample { cell, .. } = a else { continue };
            let Some(loc) = active.tables.cells.get(usize::from(*cell)).copied() else { continue };
            if let Some(Some(slot)) = active.tables.slot_mut(&loc)
                && slot.id == SAMPLER
            {
                slot.plugin.set_sample(slot.sample_wrapper.clone());
            }
        }
        active.id = plan.id;
        active.status = PlanStatus::default();
        active.faulted = false;
        env.state = EnvelopeState::Returned { replaced };
        Adoption::Adopted
    }

    /// The active plan's identity ([`PlanGeneration::EMPTY`] before the first adoption).
    #[must_use]
    pub fn active(&self) -> &PlanIdentity {
        self.plan.as_deref().map_or(&NO_PLAN, |p| &p.id)
    }

    /// Delay compensation of `bus` in the active plan, as of the last block (0 for an unknown bus).
    #[must_use]
    pub fn bus_latency_active(&self, bus: BusId) -> usize {
        self.plan.as_deref().and_then(|p| p.tables.busses.bus(bus)).map_or(0, |b| b.arrival)
    }

    /// Copies the first `frames` of `bus` as the active plan rendered them into `out` (non-finite
    /// samples become silence; an unknown bus stays silent). No allocation.
    pub fn copy_bus_active(&self, bus: BusId, frames: usize, out: &mut [Vec<f32>]) {
        for o in out.iter_mut() {
            o.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        let Some(src) = self.plan.as_deref().and_then(|p| p.tables.busses.bus(bus)) else { return };
        for (o, c) in out.iter_mut().zip(src.buf.iter()) {
            for (d, v) in o.iter_mut().zip(c.iter()).take(frames) {
                *d = if v.is_finite() { *v } else { 0.0 };
            }
        }
    }

    /// The active plan's Session.
    #[must_use]
    pub fn session(&self) -> &Arc<Session> {
        self.plan.as_deref().map_or(&NO_SESSION, |p| &p.session)
    }

    /// Each strip's meter in the active plan, by track.
    pub fn meters(&self) -> impl Iterator<Item = (TrackId, &StripMeter)> {
        self.plan.as_deref().into_iter().flat_map(|p| p.tables.strips.iter().map(|st| (st.id, &st.meter)))
    }

    /// The main mix's meter in the active plan.
    #[must_use]
    pub fn active_main_meter(&self) -> &StripMeter {
        self.plan.as_deref().map_or(&*NO_METER, |p| &p.tables.main_meter)
    }

    /// What the audio thread noticed since the last adoption, in fixed storage.
    #[must_use]
    pub fn status(&self) -> PlanStatus {
        let Some(p) = self.plan.as_deref() else { return PlanStatus::default() };
        let mismatch = p.tables.plugins().fold(0u32, |n, s| n.saturating_add(s.applied.mismatch));
        PlanStatus { applied_mismatch: mismatch, ..p.status }
    }

    /// True after an adoption unwound: the engine may be half-adopted, renders silence and
    /// adopts nothing. Tear it down with [`MixEngine::into_retired`].
    #[must_use]
    pub fn faulted(&self) -> bool {
        self.plan.as_deref().is_some_and(|p| p.faulted)
    }

    /// Control thread, after the audio side is quiescent (also after a fault): everything the
    /// engine owns, to drop there.
    pub fn into_retired(mut self) -> Retired {
        // The render workers stop here, off the audio thread, and every strip comes home.
        if let Some(pool) = self.pool.take() {
            match self.plan.as_deref_mut() {
                Some(p) => pool.retire(&mut p.tables.strips, &p.session),
                None => pool.retire(&mut [], &Session::default()),
            }
        }
        match self.plan.take() {
            Some(p) => {
                let p = *p;
                Retired { session: Some(p.session), tables: Some(p.tables), _attach: Box::new([]) }
            }
            None => Retired { session: None, tables: None, _attach: Box::new([]) },
        }
    }

    /// Audio thread: renders `frames` (≤ max block) samples of the active plan starting at `pos`
    /// into `out`. Silence before the first adoption and after a fault.
    pub fn render_active(&mut self, pos: Samples, frames: usize, out: &mut [Vec<f32>]) {
        // The block's start, which the render workers' deadline is measured from (ADR 0029).
        let start = self.pool.is_some().then(std::time::Instant::now);
        let callback_deadline = self.block_deadline.take();
        let frames = frames.min(self.max_block);
        let Some(plan) = self.plan.as_deref_mut().filter(|p| !p.faulted) else {
            self.events.next_frame = self.events.next_frame.saturating_add(frames as u64);
            for o in out.iter_mut() {
                o.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
            return;
        };
        let ActivePlan { id, session, tables, status, .. } = plan;
        let sample_rate = id.target.sample_rate;
        let s: &Session = session;
        // A seek or loop wrap releases what the clips play; live sources keep sounding (F10).
        if pos != self.last_end && !self.monitor_only {
            for st in tables.strips.iter_mut() {
                if let Some(i) = &mut st.instrument {
                    match i.events.as_deref_mut() {
                        Some(e) => e.release_clips(&mut *i.plugin),
                        None => i.plugin.all_notes_off(),
                    }
                }
                st.chase = true;
            }
            if let Some(pool) = self.pool.as_mut() {
                pool.mark_away(strip_pool::PENDING_SEEK);
            }
        }
        let frame = self.events.next_frame;
        self.events.next_frame = frame.saturating_add(frames as u64);
        self.last_end = pos.saturating_add(frames as i64);
        if let Some(m) = self.marker.as_deref() {
            m.enter(Phase::Mixdown, None);
        }
        tables.clear_block(frames);
        // A strip a render worker holds keeps the delays it went away with (its keys are lent).
        let pool_view = self.pool.as_ref();
        tables.align_keys(s, pos, !self.input.is_empty(), status, &|i| pool_view.is_some_and(|p| p.is_away(i)));
        let PlanTables { strips, phase1, aux_end, layers, keys, graph, busses, main, main_fmt, main_meter, sinks, io, .. } = tables;
        let (phase1, aux_end) = (*phase1, *aux_end);
        let main_fmt = *main_fmt;
        let any_solo = s.tracks.iter().any(|t| t.mixer.solo && !t.inactive);
        let track = |st: &Strip| s.tracks.get(usize::from(st.track));
        let live_in = LiveInput {
            input: &self.input,
            monitor_only: self.monitor_only,
            recording: self.recording,
            no_suspend: self.no_suspend,
            frame,
            io: Some(&**io),
            marker: self.marker.as_deref(),
        };
        let mark = |phase: Phase, strip: Option<u32>| {
            if let Some(m) = live_in.marker {
                m.enter(phase, strip);
            }
        };
        let n_strips = strips.len();
        let (p1, rest) = strips.split_at_mut(phase1.min(n_strips));
        let (auxes, others) = rest.split_at_mut(aux_end.saturating_sub(phase1).min(rest.len()));
        // Phase 1: independent strips. They stay in the table; nothing to restore on an unwind.
        // A strip's panic is kept in the strip (`guarded`); the first one unwinds out of here, the
        // others stay in the table and go back to the control side with it.
        //
        // With render workers (the default, ADR 0029) and no sidechain key, the strips go through the
        // worker set instead, under the block's deadline; a strip a worker still holds comes back
        // that way whatever the switches say. A plan with keys goes through them as a task graph
        // (#174, the default) while the graph switch is on ([`MixEngine::force_graph`];
        // `TRANSIT_RENDER_GRAPH=0` turns it off) and no more phase-1 strips than the pool has
        // cells; else serially, as before.
        let keyed = !keys.is_empty();
        let pooled = match (self.pool.as_mut(), start) {
            (Some(pool), Some(start))
                if on_audio_thread()
                    && (pool.any_away()
                        || (!self.serial
                            && !MixEngine::serial_forced()
                            && (!keyed || (MixEngine::graph_enabled() && p1.len() <= pool.capacity()))
                            && p1.len() >= PARALLEL_STRIPS)) =>
            {
                let block = strip_pool::PoolBlock {
                    session: &*session,
                    busses: &*busses,
                    // The plain path runs only for a plan without keys; the graph path fills and
                    // reads the plan's own.
                    keys: &NO_KEYS,
                    live: &live_in,
                    pos,
                    frames,
                    any_solo,
                    sample_rate,
                    start,
                    budget: self.strip_deadline,
                    deadline: callback_deadline,
                };
                if keyed {
                    pool.render_graph(&block, p1, &mut self.events, keys, graph);
                } else {
                    pool.render(&block, p1, &mut self.events);
                }
                true
            }
            _ => false,
        };
        if !pooled {
            // Whatever is still parked for a home strip is delivered now, as the pool path does.
            if let Some(pool) = self.pool.as_mut() {
                pool.deliver_home(p1, &mut self.events, s, frame, frames);
            }
            let busses: &BusTable = busses;
            #[cfg(feature = "testing")]
            let muted = self.mutes.any();
            // One `run_strips` per layer: a strip keyed from another renders after it, and its key
            // is copied from the finished source first. Without keys this is the one call.
            let mut from = 0;
            for end in layers.iter().map(|&e| usize::from(e)).chain(std::iter::once(p1.len())) {
                let end = end.min(p1.len());
                if end <= from {
                    continue;
                }
                let (done, cur) = p1.split_at_mut(from);
                // Test seam (issue #210): a muted serial block, strip by strip; a skipped strip's
                // keys are not filled and it does not render.
                #[cfg(feature = "testing")]
                if muted {
                    for (k, st) in cur.iter_mut().take(end - from).enumerate() {
                        if self.mutes.skips(from + k) {
                            continue;
                        }
                        keys.fill(from + k..from + k + 1, |i| done.get(i), busses, frames);
                        let keys: &KeyBank = keys;
                        if let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) {
                            guarded(st, |st| process_strip(t, st, &StripRender { s, busses, live: &live_in, pos, frames, any_solo, keys }));
                        }
                    }
                    from = end;
                    continue;
                }
                keys.fill(from..end, |i| done.get(i), busses, frames);
                let keys: &KeyBank = keys;
                run_strips(cur.get_mut(..end - from).unwrap_or(&mut []), !self.serial, |st| {
                    if let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) {
                        guarded(st, |st| process_strip(t, st, &StripRender { s, busses, live: &live_in, pos, frames, any_solo, keys }));
                    }
                });
                from = end;
            }
        }
        if let Some(panic) = p1.iter_mut().find_map(|st| st.panic.take()) {
            std::panic::resume_unwind(panic);
        }
        mark(Phase::Mixdown, None);
        let DeviceLatency { main: main_arrival, device } =
            fold_latency(s, RenderPhases { p1, auxes, others, phase1 }, busses, sinks, pos, !self.input.is_empty());
        self.main_arrival = main_arrival;
        self.latency = device;
        let law = self.pan_law;
        let pool = if pooled { self.pool.as_ref() } else { None };
        for (i, st) in p1.iter_mut().enumerate() {
            let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) else { continue };
            // An overrun, a faulted or a suspect strip is silence for the block (ADR 0029).
            if pool.is_some_and(|p| p.is_silent(i) || p.is_away(i)) {
                continue;
            }
            #[cfg(feature = "testing")]
            if self.mutes.silent(i) {
                continue;
            }
            st.meter.measure(&st.buf, frames, st.gr);
            st.meter.plugin_failures = st.failures;
            let need = route_into(t, st, pos, frames, MixOut { busses, main, main_fmt, sinks, law, main_arrival });
            status.note_capacity(need, st);
        }
        // Phase 2: auxes in dependency order (they read busses).
        for k in 0..auxes.len() {
            let (before, rest) = auxes.split_at_mut(k);
            let Some(st) = rest.first_mut() else { continue };
            let Some(t) = track(st).filter(|t| live(t)) else { continue };
            let at = phase1.saturating_add(k);
            // The graph's rule on the serial tail (#174): an aux keyed from a phase-1 strip that
            // missed this block, or from an earlier aux skipped by this rule, is skipped (silent, not
            // metered, its lines neither run nor idled). A skipped aux's buffer still holds the last
            // block it ran, so a later aux keyed from it must not run either.
            let skipped = pool.is_some_and(|p| {
                let tail = &graph.tail_skipped;
                keys.sources_of(at).any(|j| match j.checked_sub(phase1) {
                    None => p.is_silent(j) || p.is_away(j),
                    Some(a) => a < k && tail.get(a).copied().unwrap_or(false),
                })
            });
            // Test seam (issue #210): a muted serial block applies the same rule to its mutes.
            #[cfg(feature = "testing")]
            let skipped = skipped
                || (self.mutes.any()
                    && keys.sources_of(at).any(|j| match j.checked_sub(phase1) {
                        None => self.mutes.silent(j),
                        Some(a) => a < k && graph.tail_skipped.get(a).copied().unwrap_or(false),
                    }));
            if let Some(f) = graph.tail_skipped.get_mut(k) {
                *f = skipped;
            }
            if skipped {
                if let Some(p) = pool {
                    p.note_tail_skip();
                }
                continue;
            }
            let strip = |i: usize| match i.checked_sub(phase1) {
                None => p1.get(i),
                Some(j) => before.get(j).filter(|b| track(b).is_some_and(live)),
            };
            keys.fill(at..at + 1, strip, busses, frames);
            process_strip(t, st, &StripRender { s, busses, live: &live_in, pos, frames, any_solo, keys });
            st.meter.measure(&st.buf, frames, st.gr);
            st.meter.plugin_failures = st.failures;
            let need = route_into(t, st, pos, frames, MixOut { busses, main, main_fmt, sinks, law, main_arrival });
            status.note_capacity(need, st);
        }
        // Master faders process the main mix, in track order.
        let transport = transport_at(s, pos, !self.monitor_only, self.recording);
        for st in others.iter_mut() {
            let Some(t) = track(st).filter(|t| t.kind == TrackKind::Master && !t.inactive) else { continue };
            mark(Phase::Masters, Some(u32::from(st.track)));
            process_master(t, st, main, pos, frames, &transport, self.no_suspend);
            st.meter.measure(main, frames, 0.0);
            st.meter.plugin_failures = st.failures;
        }
        mark(Phase::Mixdown, None);
        if self.metronome && !self.monitor_only && s.edit.click && (!s.edit.flag("click.only_during_record") || self.recording) {
            click::render(s, pos, frames, main, self.metronome_gain);
        }
        if let Some(c) = self.count_in {
            click::render_count_in(s, c, frames, main, self.metronome_gain);
        }
        mark(Phase::Output, None);
        for (c, o) in out.iter_mut().enumerate() {
            let src = main.get(c);
            for i in 0..frames {
                let v = src.and_then(|m| m.get(i)).copied().unwrap_or(0.0);
                if let Some(d) = o.get_mut(i) {
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
        // The sinks' channels follow Main's, in table order: the device callback plays each on its
        // path's device channels (`plan_sinks` gives the layout both sides share).
        let mut at = main.len();
        for k in sinks.iter() {
            for src in &k.buf {
                if let Some(o) = out.get_mut(at) {
                    for (d, v) in o.iter_mut().zip(src.iter()).take(frames) {
                        *d = if v.is_finite() { *v } else { 0.0 };
                    }
                }
                at += 1;
            }
        }
        main_meter.measure(main, frames, 0.0);
        #[cfg(feature = "testing")]
        self.mutes.clear();
    }
}

impl PlanTables {
    fn clear_block(&mut self, frames: usize) {
        let PlanTables { busses, main, sinks, .. } = self;
        for (_, b) in busses.0.iter_mut() {
            b.clear(frames);
        }
        for c in main.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        for k in sinks.iter_mut() {
            k.clear(frames);
        }
    }

    /// `away`: the strip at that table index is held by a render worker (its keys and its
    /// chain are in its cell): its delays stay as they were until it is home.
    fn align_keys(&mut self, s: &Session, pos: Samples, duplex: bool, status: &mut PlanStatus, away: &dyn Fn(usize) -> bool) {
        let PlanTables { strips, phase1, aux_end, keys, busses, sinks, .. } = self;
        let (phase1, aux_end) = (*phase1, *aux_end);
        let track = |st: &Strip| s.tracks.get(usize::from(st.track));
        // Sidechain keys (ADR 0015 section 4): a key and the signal it meets must come from the
        // same source sample time, but a strip's output latency is known only after phase 1. So
        // with keys in the plan, the latencies of this block (each strip's inserts, as the
        // forward pass below counts them) are walked once before it, in render order, to give
        // every key edge its delay (`KeyBank::align`). The busses' marks are cleared after.
        if !keys.is_empty() {
            let pdc = s.edit.delay_compensation;
            let mut walked = 0usize;
            for at in 0..aux_end.min(strips.len()) {
                let (before, rest) = strips.split_at_mut(at);
                let Some(st) = rest.first_mut() else { continue };
                let Some(t) = track(st) else { continue };
                let aux = is_aux(t);
                let alive = live(t);
                if at < phase1 && (!alive || aux) {
                    continue;
                }
                let mut arrive = 0usize;
                if aux && let Route::Bus(b) = &t.mixer.input {
                    arrive = busses.bus(*b).map_or(0, |x| x.arrival);
                }
                if alive && !away(at) {
                    let from = |sp: &KeySpec| match sp.from {
                        KeyEnd::Strip(j) => before.get(usize::from(j)).filter(|b| track(b).is_some_and(live)).map_or(0, |b| b.pdc_out),
                        KeyEnd::Bus(b) => {
                            if aux {
                                busses.bus(b).map_or(0, |x| x.arrival)
                            } else {
                                0
                            }
                        }
                    };
                    let short = keys.align(t, st, key::KeyAlignment { at, arrive, pos, pdc, duplex }, &from);
                    status.note_capacity(short, st);
                }
                pdc_visit(t, st, at, alive, PdcWalk { busses, sinks, main_arrival: &mut walked, pdc });
            }
            for (_, b) in busses.0.iter_mut() {
                b.arrival = 0;
                b.reader = usize::MAX;
            }
            for k in sinks.iter_mut() {
                k.arrival = 0;
            }
        }
    }
}

impl PlanStatus {
    fn note_capacity(&mut self, need: usize, st: &Strip) {
        if need > 0 {
            self.capacity_exceeded = self.capacity_exceeded.saturating_add(1);
            self.required_latency = self.required_latency.max(u32::try_from(need).unwrap_or(u32::MAX));
            self.affected = Some((st.id, st.generation));
        }
    }
}

struct RenderPhases<'a> {
    p1: &'a mut [Strip],
    auxes: &'a mut [Strip],
    others: &'a mut [Strip],
    phase1: usize,
}

struct DeviceLatency {
    main: usize,
    device: usize,
}

/// Walk the rendered strips and pending aux/master inserts, then align every sink to the device.
fn fold_latency(s: &Session, phases: RenderPhases<'_>, busses: &mut BusTable, sinks: &mut [Sink], pos: Samples, duplex: bool) -> DeviceLatency {
    let RenderPhases { p1, auxes, others, phase1 } = phases;
    let track = |st: &Strip| s.tracks.get(usize::from(st.track));
    let pdc = s.edit.delay_compensation;
    if pdc {
        for st in auxes.iter_mut() {
            if let Some(t) = track(st).filter(|t| !t.inactive) {
                note_insert_latency(t, st, pos, duplex);
            }
        }
    }
    let mut lmax_m = 0usize;
    if pdc {
        for st in others.iter_mut() {
            if let Some(t) = track(st).filter(|t| t.kind == TrackKind::Master && !t.inactive) {
                note_insert_latency(t, st, pos, duplex);
                lmax_m = lmax_m.saturating_add(st.latency);
            }
        }
    }
    // Forward pass in render order (the table order): phase-1 strips, then the auxes.
    let mut main_arrival = 0usize;
    for (at, st) in p1.iter_mut().enumerate() {
        if let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) {
            pdc_visit(t, st, at, true, PdcWalk { busses, sinks, main_arrival: &mut main_arrival, pdc });
        }
    }
    // Inactive auxes still mark the bus they read (see `pdc_visit`).
    for (k, st) in auxes.iter_mut().enumerate() {
        if let Some(t) = track(st) {
            pdc_visit(t, st, phase1.saturating_add(k), live(t), PdcWalk { busses, sinks, main_arrival: &mut main_arrival, pdc });
        }
    }
    // One device latency for every sink (ADR 0015 section 6): Main's (its sources' arrival plus
    // the master inserts') or the slowest path's, whichever is longer. Each sink is padded to
    // it, Main to it less its inserts, so stems and cue mixes leave the device with Main. With
    // no sinks this is Main's own arrival, as before.
    let device = sinks.iter().fold(main_arrival.saturating_add(lmax_m), |l, k| l.max(k.arrival));
    main_arrival = device.saturating_sub(lmax_m);
    for k in sinks.iter_mut() {
        k.arrival = device;
    }
    DeviceLatency { main: main_arrival, device }
}

/// The render workers (issue #124, ADR 0029): the default, started by the live engine's owner.
impl MixEngine {
    /// Control thread: starts this engine's render workers (cores minus one, at most
    /// [`MAX_RENDER_WORKERS`], each promoted once at start unless [`RENDER_NO_PRIORITY_ENV`] is
    /// set) with a cell for each of the first `strips` strips (at most 256; strips past them
    /// render on the callback). Returns how many workers run (0: none, the strips render
    /// serially: one core, the OS refused a thread, or the audio thread called). The workers
    /// render only while [`MixEngine::serial_forced`] is off, and stop at
    /// [`MixEngine::into_retired`]; never drop an engine with workers on the audio thread.
    pub fn start_render_workers(&mut self, strips: usize) -> usize {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        let workers = cores.saturating_sub(1).min(MAX_RENDER_WORKERS);
        if workers == 0 {
            return self.render_workers();
        }
        self.start_render_workers_with(strips, workers)
    }

    /// [`MixEngine::start_render_workers`] with exactly `workers` threads; 0 makes the pool path
    /// with no worker at all, where the callback renders inline every strip the admission rule
    /// lets it (a strip needs a cost history first: [`MixEngine::seed_strip_costs`]), so a test
    /// can force an outcome by construction, never by scheduling. For tests.
    #[doc(hidden)]
    pub fn start_render_workers_with(&mut self, strips: usize, workers: usize) -> usize {
        if self.pool.is_none() && !on_audio_thread() {
            // The task graph's switch is read here too, once per process (#174).
            crate::graph::init_graph_switch();
            let promote = !render_pool::env_flag(RENDER_NO_PRIORITY_ENV, "SOUNDCRAFT_RENDER_NO_PRIORITY");
            // The environment is read here, once per process (never on the audio thread). The
            // device's callback is not known yet: the engine's largest block at its plan's rate
            // stands in until playback hands the device's (`set_device_block`, #172).
            let overridden = render_pool::strip_deadline_override();
            let (frames, rate) = self.stand_in_block();
            match strip_pool::StripPool::new(
                workers.min(MAX_RENDER_WORKERS),
                strips.clamp(1, strip_pool::POOL_STRIPS),
                promote,
                overridden,
                frames,
                rate,
            ) {
                Ok(pool) => {
                    log::info!(
                        "mix: {workers} render workers ({}); strip deadline {}{}",
                        if promote { "promoted" } else { "default priority" },
                        strip_deadline_text(u64::from(pool.deadline_percent()), u64::from(frames), u64::from(rate), overridden.is_some()),
                        if overridden.is_some() { "" } else { ", until the device's buffer size is known" }
                    );
                    self.pool = Some(pool);
                }
                Err(e) => log::warn!("mix: no render workers ({e}); the audio thread renders strips itself"),
            }
        }
        self.render_workers()
    }

    /// Seeds every pool strip's cost history (its estimate and its worst case) with `cost`, as if
    /// each had been rendered once in that time; zero clears it (ADR 0029, deviation 16: a strip
    /// with no history is never rendered inline). A strip whose chain changes afterwards (a plug-in
    /// loaded or replaced, a track moved) starts again with none. Between blocks; nothing without
    /// a pool (a pool with zero workers is one). For tests and the bench.
    #[doc(hidden)]
    pub fn seed_strip_costs(&mut self, cost: std::time::Duration) {
        if let Some(pool) = self.pool.as_mut() {
            pool.seed_costs(cost);
        }
    }

    /// Render worker threads (0: none).
    #[must_use]
    pub fn render_workers(&self) -> usize {
        self.pool.as_ref().map_or(0, strip_pool::StripPool::workers)
    }

    /// The strips' deadline, percent of the block period (#172): the render workers' share, the
    /// [`STRIP_DEADLINE_PERCENT_ENV`] override they were started with, else
    /// [`default_strip_deadline_percent`] of the device's callback
    /// ([`MixEngine::set_device_block`]; the engine's largest block at its plan's rate before it).
    /// Without workers, the rule at that stand-in (it then decides nothing). Playback computes
    /// [`MixEngine::set_block_deadline`] with it. Allocation-free; any thread.
    #[must_use]
    pub fn strip_deadline_percent(&self) -> u32 {
        self.pool.as_ref().map_or_else(
            || {
                let (frames, rate) = self.stand_in_block();
                default_strip_deadline_percent(frames, rate)
            },
            strip_pool::StripPool::deadline_percent,
        )
    }

    /// The device callback the share stands in with before the device's first: the engine's
    /// largest block at its plan's rate (48 kHz without one).
    fn stand_in_block(&self) -> (u32, u32) {
        let rate = self.plan.as_ref().map_or(0, |p| p.id.target.sample_rate);
        (u32::try_from(self.max_block).unwrap_or(u32::MAX), if rate == 0 { 48_000 } else { rate })
    }

    /// Replaces the render workers' [`STRIP_DEADLINE_PERCENT_ENV`] override (`None`: the rule
    /// decides) and resolves the share again for the device's callback, so a test's expectation
    /// does not depend on the process environment. Nothing without workers. For tests.
    #[doc(hidden)]
    pub fn set_strip_deadline_override(&mut self, overridden: Option<u32>) {
        if let Some(pool) = self.pool.as_mut() {
            pool.set_deadline_override(overridden);
        }
    }

    /// Audio thread (or any owner), before a block: the device's callback is `frames` long at
    /// `rate` Hz. The render workers' share follows its period ([`default_strip_deadline_percent`]:
    /// 150% at 1.5 ms and below, 100% above) unless [`STRIP_DEADLINE_PERCENT_ENV`] overrode it; a
    /// new length or rate resolves the share again, the same pair costs one compare.
    /// Allocation-free, no environment or clock read. Nothing without workers; zero is ignored.
    pub fn set_device_block(&mut self, frames: u32, rate: u32) {
        if let Some(pool) = self.pool.as_mut() {
            pool.set_device_block(frames, rate);
        }
    }

    /// What each render worker's promotion achieved, by worker (`None` before it ran, or with
    /// promotion off). For diagnostics and tests; allocates.
    #[must_use]
    pub fn render_worker_promotions(&self) -> Vec<Option<transit_rt::Promotion>> {
        self.pool.as_ref().map(strip_pool::StripPool::promotions).unwrap_or_default()
    }

    /// A reader of the render workers' stream faults (overruns by cause, late results, deferred
    /// MIDI overflows and silences, suspect strips), for the diagnostics report; `None` without
    /// workers. Take it off the audio thread.
    #[must_use]
    pub fn strip_faults(&self) -> Option<StripFaultsReader> {
        self.pool.as_ref().map(strip_pool::StripPool::reader)
    }

    /// Audio thread, between blocks: brings home every strip whose render worker is done, and
    /// returns whether every strip is home. A plan is adopted only then (a strip a worker holds
    /// cannot move); playback defers the adoption while this is false. Always true without
    /// workers. Allocation-free and non-blocking.
    pub fn render_workers_settled(&mut self) -> bool {
        match (self.pool.as_mut(), self.plan.as_deref_mut()) {
            (Some(pool), Some(p)) => pool.settle(&mut p.tables.strips, &mut p.tables.keys, &mut self.events, &p.session, self.max_block),
            _ => true,
        }
    }

    /// Strips that were silence in the last block because of an overrun (0 without workers): the
    /// take journal marks the block when this is not zero.
    #[must_use]
    pub fn block_overruns(&self) -> u32 {
        self.pool.as_ref().map_or(0, strip_pool::StripPool::last_overruns)
    }

    /// Overrides the strips' deadline, measured from the block's start (default: the pool's share
    /// of the block's period, [`MixEngine::strip_deadline_percent`]). For tests and the bench: a
    /// long budget makes the pool path wait for every worker, a short one forces overruns.
    #[doc(hidden)]
    pub fn set_strip_deadline(&mut self, budget: Option<std::time::Duration>) {
        self.strip_deadline = budget;
    }

    /// Audio thread, before a block: the strips' deadline for the next `render_active`, measured
    /// from the device callback's entry (playback passes callback entry + (blocks rendered in
    /// this callback + 1) x block period x [`MixEngine::strip_deadline_percent`] / 100, through
    /// [`strip_deadline_share`]). A late wake or the callback's own work before the render then
    /// shortens the strips' budget instead of moving the deadline. Without it
    /// the deadline is measured from `render_active`'s entry. Taken by that one block.
    pub fn set_block_deadline(&mut self, deadline: Option<std::time::Instant>) {
        self.block_deadline = deadline;
    }
}
