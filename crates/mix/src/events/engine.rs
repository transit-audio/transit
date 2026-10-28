//! The engine's side of instrument events: the epoch, the render clock, admission into an
//! instrument's queue ([`MixEngine::queue_event`]), revocation and the counters.

use super::*;

/// The engine's side: epoch, limits, the render clock, the transport and the counters.
pub(crate) struct EngineEvents {
    /// `None`: the engine takes no realtime events (offline engines).
    pub(crate) epoch: Option<EngineEpoch>,
    pub(crate) limits: EventLimits,
    /// The render frame of the next block's first sample.
    pub(crate) next_frame: u64,
    pub(crate) transport: Option<TransportEpoch>,
    pub(crate) status: EventStatus,
    tap: Vec<TapRecord>,
    /// Per port slot (see [`MAX_PORTS`]): the highest revoked generation.
    revoked: [Option<u64>; MAX_PORTS + 2],
}

impl EngineEvents {
    pub(crate) fn new(epoch: Option<EngineEpoch>, limits: EventLimits) -> Self {
        EngineEvents {
            epoch,
            limits,
            next_frame: 0,
            transport: None,
            status: EventStatus::default(),
            tap: Vec::new(),
            revoked: [None; MAX_PORTS + 2],
        }
    }
}

/// The instrument slot an event names, when it is a live, event-taking instrument.
fn instrument_of<'a>(
    session: &Session,
    strips: &'a mut [Strip],
    tracks: &[(transit_model::TrackId, u16)],
    target: &SlotBinding,
) -> Result<&'a mut PluginSlot, EventOutcome> {
    let i = tracks.binary_search_by_key(&target.0, |(t, _)| *t).ok().and_then(|i| tracks.get(i)).map(|(_, s)| usize::from(*s));
    match i.and_then(|i| strips.get_mut(i)) {
        Some(st) => instrument_in(session, st, target),
        None => Err(EventOutcome::NoInstrument),
    }
}

/// The instrument of `strip` when `target` names it and it is a live, event-taking instrument.
pub(crate) fn instrument_in<'a>(session: &Session, strip: &'a mut Strip, target: &SlotBinding) -> Result<&'a mut PluginSlot, EventOutcome> {
    let (_, slot, generation) = *target;
    let inactive = !plays(session, strip);
    let inst = strip.instrument.as_mut().filter(|_| slot == INSTRUMENT_SLOT);
    let Some(inst) = inst else { return Err(EventOutcome::NoInstrument) };
    if generation == SlotGen::VACANT || inst.generation != generation {
        return Err(EventOutcome::StaleBinding);
    }
    if inst.failed || inst.events.is_none() || inactive {
        return Err(EventOutcome::NoInstrument);
    }
    Ok(inst)
}

/// The strip's track is rendered (it is not inactive): its instrument processes its queue.
fn plays(session: &Session, strip: &Strip) -> bool {
    session.tracks.get(usize::from(strip.track)).is_some_and(|t| !t.inactive)
}

/// True for the messages the queue keeps a release slot for (a note-on with velocity 0 too).
fn is_release_message(m: &Message) -> bool {
    m.is_release() || matches!(m, Message::NoteOn { velocity: 0, .. })
}

impl EngineEvents {
    /// The checks that need no instrument, in order: engine epoch, then port revocation.
    pub(crate) fn precheck(&self, ev: &InstrumentEvent) -> Option<EventOutcome> {
        if self.epoch != Some(ev.engine) {
            return Some(EventOutcome::StaleEngine);
        }
        // Port generations only increase: one at or below the revoked one is gone for good.
        let Some(port) = Source::port_slot(ev.source.port) else { return Some(EventOutcome::PortOutOfRange) };
        if self.revoked.get(port).copied().flatten().is_some_and(|r| ev.source.generation <= r) {
            return Some(EventOutcome::RevokedSource);
        }
        None
    }

    /// The transport check for an event the engine takes without its instrument at hand (a
    /// strip a render worker holds, ADR 0029): `Err` when a non-release names an old transport.
    pub(crate) fn transport_ok(&self, ev: &InstrumentEvent) -> Result<(), EventOutcome> {
        if ev.transport.is_some_and(|t| Some(t) != self.transport) && !is_release_message(&ev.message) {
            return Err(EventOutcome::StaleTransport);
        }
        Ok(())
    }

    /// Queues `ev` on `inst`, after the binding checks: the transport check, the due frame (late,
    /// horizon) and the queue's own admission.
    pub(crate) fn admit_into(&mut self, inst: &mut PluginSlot, ev: &InstrumentEvent) -> (EventOutcome, Option<u64>) {
        let release = is_release_message(&ev.message);
        // A release is never refused for transport staleness: its note or pedal may already sound.
        // It is no longer the old transport's either (a later change does not cancel it).
        let mut transport = ev.transport;
        if transport.is_some_and(|t| Some(t) != self.transport) {
            if !release {
                return (EventOutcome::StaleTransport, None);
            }
            transport = None;
        }
        let next = self.next_frame;
        let horizon = next.saturating_add(u64::from(self.limits.max_future_frames));
        let (due, late, advanced) = match ev.due {
            Due::Next => (next, 0, 0),
            Due::Frame(RenderFrame(f)) if f < next => (next, next - f, 0),
            Due::Frame(RenderFrame(f)) if f > horizon => {
                if !release {
                    return (EventOutcome::TooEarly, Some(f));
                }
                (horizon, 0, f - horizon)
            }
            Due::Frame(RenderFrame(f)) => (f, 0, 0),
        };
        let Some(events) = inst.events.as_deref_mut() else { return (EventOutcome::NoInstrument, None) };
        let st = &mut self.status;
        let frames = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
        // A note-on with velocity 0 is a note-off (ingress normally does this already).
        let message = match ev.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        let outcome = match events.admit(due, ev.source, transport, message, st) {
            Admit::Queued if advanced > 0 => EventOutcome::ReleaseAdvanced { frames: frames(advanced) },
            Admit::Queued if late > 0 => EventOutcome::QueuedLate { frames: frames(late) },
            Admit::Queued => EventOutcome::Queued,
            Admit::Coalesced => EventOutcome::Coalesced,
            Admit::Unmatched => EventOutcome::Unmatched,
            Admit::Shed(s) => EventOutcome::Refused(s),
        };
        (outcome, Some(due))
    }

    /// Counts what became of `ev` and records it on the tap.
    fn record(&mut self, ev: &InstrumentEvent, outcome: EventOutcome, due: Option<u64>) {
        let st = &mut self.status;
        let bump = |n: &mut u64| *n = n.saturating_add(1);
        match outcome {
            EventOutcome::Queued => bump(&mut st.queued),
            EventOutcome::QueuedLate { frames } => {
                bump(&mut st.queued);
                bump(&mut st.late);
                st.max_late_frames = st.max_late_frames.max(frames);
                let b = (32 - frames.leading_zeros()).min(15) as usize;
                if let Some(n) = st.lateness_log2.get_mut(b) {
                    *n = n.saturating_add(1);
                }
            }
            EventOutcome::ReleaseAdvanced { .. } => {
                bump(&mut st.queued);
                bump(&mut st.releases_advanced);
            }
            EventOutcome::Coalesced => bump(&mut st.coalesced),
            EventOutcome::StaleEngine => bump(&mut st.refused_stale[0]),
            EventOutcome::RevokedSource | EventOutcome::PortOutOfRange => bump(&mut st.refused_revoked),
            EventOutcome::StaleBinding => bump(&mut st.refused_stale[1]),
            EventOutcome::StaleTransport => bump(&mut st.refused_stale[2]),
            EventOutcome::NoInstrument => bump(&mut st.refused_no_instrument),
            EventOutcome::TooEarly => bump(&mut st.refused_too_early),
            EventOutcome::Unmatched => bump(&mut st.unmatched),
            EventOutcome::Refused(_) => st.shed(&ev.message, ev.target),
            EventOutcome::Faulted => {}
        }
        if !matches!(outcome, EventOutcome::Queued | EventOutcome::QueuedLate { .. } | EventOutcome::ReleaseAdvanced { .. } | EventOutcome::Coalesced)
        {
            st.affected = Some(ev.target);
        }
        if self.tap.len() < self.tap.capacity() {
            let due = due.filter(|_| outcome.accepted()).map(RenderFrame);
            self.tap.push(TapRecord { source: ev.source, arrival: ev.arrival, due, message: ev.message, outcome });
        }
    }

    /// Admits an event taken while a render worker held its strip (ADR 0029, deferred MIDI) into
    /// `strip`'s instrument, now that the strip is back: the same checks and counters as
    /// [`MixEngine::queue_event`] (the tap records it a second time, with this outcome).
    pub(crate) fn readmit(&mut self, session: &Session, strip: &mut Strip, ev: &InstrumentEvent) -> EventOutcome {
        let (outcome, due) = match self.precheck(ev) {
            Some(o) => (o, None),
            None => match instrument_in(session, strip, &ev.target) {
                Ok(inst) => self.admit_into(inst, ev),
                Err(o) => (o, None),
            },
        };
        self.record(ev, outcome, due);
        outcome
    }

    /// The (port, generation) of every revoked port slot, for a strip that was away when the
    /// revocation came.
    pub(crate) fn revocations(&self) -> impl Iterator<Item = (u16, u64)> + '_ {
        self.revoked.iter().enumerate().filter_map(|(slot, r)| {
            let port = match slot {
                s if s < MAX_PORTS => u16::try_from(s).ok()?,
                s if s == MAX_PORTS => Source::UI,
                _ => Source::CLIP,
            };
            r.map(|g| (port, g))
        })
    }
}

impl MixEngine {
    /// A realtime engine ([`MixEngine::realtime`]) that takes instrument events stamped with
    /// `epoch` ([`MixEngine::queue_event`]). Playback creates the epoch before construction and
    /// gives every engine (device or rate) incarnation a new one. Engines made any other way
    /// refuse every event with [`EventOutcome::StaleEngine`].
    ///
    /// Events reach instruments only through the plan path: adopt a plan and render with
    /// [`MixEngine::render_active`], which delivers them and advances [`MixEngine::next_frame`].
    /// Before the first adoption every event is refused ([`EventOutcome::NoInstrument`]).
    #[must_use]
    pub fn realtime_with_events(target: crate::RenderTarget, epoch: EngineEpoch, limits: EventLimits) -> Self {
        let mut eng = MixEngine::realtime(target);
        eng.events = EngineEvents::new(Some(epoch), limits);
        eng
    }

    /// Audio thread, between blocks: validates `ev` (engine epoch, then binding, then transport
    /// epoch) and queues it on its instrument. Never allocates, locks or logs.
    ///
    /// For an instrument whose strip a render worker still holds (an overrun, ADR 0029) the
    /// event is checked as far as it can be without the instrument (epoch, port, binding,
    /// transport) and taken into the strip's deferred-MIDI buffer: `Queued`. The strip's next
    /// processed block admits it into the instrument's queue, in order, before the strip renders.
    pub fn queue_event(&mut self, ev: &InstrumentEvent) -> EventOutcome {
        let (outcome, due) = self.admit(ev);
        self.events.record(ev, outcome, due);
        outcome
    }

    /// The outcome and the frame it was (or would have been) queued for. Order: engine epoch,
    /// port revocation, plan, binding, then transport (non-releases only).
    fn admit(&mut self, ev: &InstrumentEvent) -> (EventOutcome, Option<u64>) {
        if let Some(o) = self.events.precheck(ev) {
            return (o, None);
        }
        // No adopted plan: there is no instrument to take it.
        let Some(plan) = self.plan.as_deref_mut() else { return (EventOutcome::NoInstrument, None) };
        if plan.faulted {
            return (EventOutcome::Faulted, None);
        }
        if let Some(pool) = self.pool.as_mut()
            && let Some(i) = plan.tables.strip_of(ev.target.0)
            && pool.parks(i)
        {
            let home = plan.tables.strips.get(i).map(|st| st.instrument.as_ref().map(|inst| inst.generation));
            return pool.park(i, ev, &self.events, self.max_block, home);
        }
        match instrument_of(&plan.session, &mut plan.tables.strips, &plan.tables.tracks, &ev.target) {
            Ok(inst) => self.events.admit_into(inst, ev),
            Err(o) => (o, None),
        }
    }

    /// Audio thread, with every seek, start, stop and loop wrap: the current transport epoch.
    /// Queued events derived from another transport are cancelled (counted
    /// `cancelled_transport`); their releases are kept, and a release that arrives later with an
    /// old epoch is still admitted.
    pub fn set_transport_epoch(&mut self, epoch: TransportEpoch) {
        if self.events.transport == Some(epoch) {
            return;
        }
        self.events.transport = Some(epoch);
        for e in self.instrument_events() {
            e.transport_changed(epoch);
        }
        // A strip a render worker holds applies it when it is back (ADR 0029).
        if let Some(pool) = self.pool.as_mut() {
            pool.mark_away(crate::strip_pool::PENDING_TRANSPORT);
        }
    }

    /// Audio thread, when a port generation retires (unplug): cancels that source's queued
    /// events, releases its notes and pedals at offset 0 of the next block (a pedal goes up at
    /// the plug-in only when it was the last holder), and returns bend and pressure to neutral
    /// where it was the last writer. Bounded: instruments × (512 + 224 + 32). Counted `revoked`.
    ///
    /// The port stays revoked: a later event from this generation or an older one (one still in
    /// flight in playback's lane) is refused with [`EventOutcome::RevokedSource`] until a newer
    /// generation of the port appears.
    pub fn revoke_source(&mut self, port: u16, generation: u64) {
        let mut done = 0u64;
        if let Some(p) = self.plan.as_deref_mut() {
            for st in p.tables.strips.iter_mut() {
                if let Some(inst) = st.instrument.as_mut()
                    && let Some(e) = inst.events.as_deref_mut()
                {
                    done = done.saturating_add(e.release_sender(&mut *inst.plugin, port, generation));
                }
            }
        }
        self.events.status.revoked = self.events.status.revoked.saturating_add(done);
        if let Some(r) = Source::port_slot(port).and_then(|i| self.events.revoked.get_mut(i)) {
            *r = Some(r.map_or(generation, |r| r.max(generation)));
        }
        // A strip a render worker holds releases every revoked source when it is back.
        if let Some(pool) = self.pool.as_mut() {
            pool.mark_away(crate::strip_pool::PENDING_REVOKE);
        }
    }

    /// The wake contract: true while any instrument still has something to deliver or release:
    /// queued events (future ones included), sounding notes, held pedals, or reserved releases.
    /// A stopped host keeps processing while this holds. Allocation-free; bounded by the
    /// instruments × (512 + 224 + 32) entries.
    ///
    /// It covers undelivered events, holds and reservations only. `false` does not mean the
    /// engine is quiet: release envelopes, one-shots and insert or delay tails still need
    /// processing after the last release is delivered; the caller decides when to idle (see
    /// [`Plugin::tail_samples`] and [`Plugin::process_status`]).
    ///
    /// A strip a render worker still holds (an overrun, ADR 0029) counts as pending.
    #[must_use]
    pub fn has_pending_events(&self) -> bool {
        self.pool.as_ref().is_some_and(crate::strip_pool::StripPool::any_away)
            || self.plan.as_deref().is_some_and(|p| {
                p.tables.strips.iter().filter_map(|st| st.instrument.as_ref()).filter_map(|i| i.events.as_deref()).any(InstrumentEvents::busy)
            })
    }

    /// The render frame of the next block's first sample.
    #[must_use]
    pub fn next_frame(&self) -> RenderFrame {
        RenderFrame(self.events.next_frame)
    }

    /// The event counters: the engine's and every instrument's. An instrument whose strip a
    /// render worker holds right now (an overrun, ADR 0029) is left out until it is back.
    #[must_use]
    pub fn event_status(&self) -> EventStatus {
        let mut s = self.events.status;
        if let Some(p) = self.plan.as_deref() {
            for e in p.tables.strips.iter().filter_map(|st| st.instrument.as_ref()).filter_map(|i| i.events.as_deref()) {
                s.absorb(&e.status);
            }
        }
        s
    }

    /// Control thread: record the next `capacity` submitted events (G5 boundary tests). Replaces
    /// what was recorded.
    pub fn install_event_tap(&mut self, capacity: usize) {
        self.events.tap = Vec::with_capacity(capacity);
    }

    /// What the tap recorded, in submission order.
    #[must_use]
    pub fn event_tap(&self) -> &[TapRecord] {
        &self.events.tap
    }

    /// Every instrument's event state in the active plan.
    fn instrument_events(&mut self) -> impl Iterator<Item = &mut InstrumentEvents> {
        self.plan.iter_mut().flat_map(|p| p.tables.strips.iter_mut()).filter_map(|st| st.instrument.as_mut()).filter_map(|i| i.events.as_deref_mut())
    }

    /// After an adoption: an instrument whose track is now inactive will not process its queue,
    /// so the queue is voided (counted) and what it holds is released at the plug-in.
    pub(crate) fn void_silent_instruments(plan: &mut crate::plan::ActivePlan) {
        let session: &Session = &plan.session;
        for st in plan.tables.strips.iter_mut() {
            let playing = plays(session, st);
            if let Some(inst) = st.instrument.as_mut()
                && let Some(e) = inst.events.as_deref_mut()
                && (!playing || inst.failed)
            {
                e.void(&mut *inst.plugin);
            }
        }
    }

    /// After an adoption: the displaced instruments' queues retire with them (counted) and their
    /// counters fold into the engine's.
    pub(crate) fn retire_events(status: &mut EventStatus, displaced: &mut [Strip]) {
        for e in displaced.iter().filter_map(|st| st.instrument.as_ref()).filter_map(|i| i.events.as_deref()) {
            let [n, r] = e.queued_counts();
            status.absorb(&e.status);
            status.retired_with_instance[0] = status.retired_with_instance[0].saturating_add(n);
            status.retired_with_instance[1] = status.retired_with_instance[1].saturating_add(r);
        }
    }
}
