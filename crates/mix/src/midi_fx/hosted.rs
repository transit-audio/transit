//! A hosted MIDI effect stage (ADR 0064 slice HM2): a VST3 or CLAP plug-in with a readable note
//! output as a stage of an instrument track's own MIDI effect chain.
//!
//! **The instance and its storage.** The instance is a plan cell of the instrument's strip
//! (`Strip::midi_fx`, slot [`crate::midi_fx_slot`]), made off the audio thread by the plan's
//! factory with note-output collection enabled before `prepare` ([`crate::create_instance_for`]).
//! Beside it, in the same cell, the compiler prepares a [`HostedStore`]: a two-channel scratch
//! buffer of the plan's largest block (4.4), the stage ledger ([`super::ledger`]), and room for one
//! block of the plug-in's output to be sorted. The host's chain keeps only the stage's small state
//! (`Hosted`): the instance it last saw, the epoch, the stage state.
//!
//! **Eligibility.** The stage uses the note-output seam only for an instance that reports a
//! readable note output (`Plugin::note_output`); one that reports none is silent, "Does not write
//! notes" (2.5). A default (generation 0, inactive) lifecycle is never taken as support.
//!
//! **Each block** ([`super::MidiFxHost::begin`]): the stage reads its instance's presence,
//! eligibility and `lifecycle()`. An instance arriving or leaving, entering or leaving a state the
//! user did not ask for (no instance, no readable note output, inactive after a refused
//! reactivation, failed), and a lifecycle generation other than the epoch's are each a host barrier
//! at offset 0 of the block, before its events, with the edit's steps (decision 6's table). A
//! barrier starts a new epoch with its reset pending: `restart_processing` is requested before the
//! stage's next actual call.
//!
//! **The call** ([`super::MidiFxHost::flush`]): the stage hands its input to the plug-in through
//! `Plugin::event` (events that waited between blocks first, at offset 0), processes the scratch
//! buffer marked silent (`set_input_silent`), and takes the block's output with its report. Output
//! is accepted only from calls that ran under the epoch's generation after the epoch's reset had
//! completed (in an earlier accepted call, or at the start of this one). An adapter holds the
//! instance's core across a block's calls, so they share one generation and only the first can
//! follow the reset: a block's output is accepted whole or discarded whole. A call under another
//! generation runs the barrier at offset 0 of this block, with an edit's pedal-ups, and starts a
//! new epoch for that generation (6.2). A block whose report carries a processing error is the
//! failing block (7.4): its whole output is discarded, the barrier runs at offset 0 (an edit too),
//! and the stage is silent until the next Stop. The accepted events are sorted by (offset, write
//! order) and matched by the ledger, which writes the stage's output.
//!
//! **Between blocks** the stage is never processed (4.6): an event that reaches it waits in its
//! carry and is handed to the plug-in at offset 0 of the next actual processing call, ahead of
//! that block's own input. A silent stage consumes its input. A bypassed stage passes its input
//! through, as any bypassed effect (7.1).

use transit_dsp::Plugin;
use transit_dsp::event::{Message, PluginEvent};
use transit_dsp::hosted_out::{HOSTED_OUT_CAP, HostedOut, OutForm, OutKind, OutValues};
use transit_dsp::midi_fx::TimedEvent;
use transit_model::Track;

use super::ledger::{Ledger, Sel, Tally};
use super::output::Output;

/// Whether `t` gets instances for the hosted MIDI effect stages of its own chain: an instrument
/// track with a plug-in instrument (a rack's chain takes none, 10.2; the chain feeds the
/// instrument's events).
#[must_use]
pub(crate) fn hosts_stages(t: &Track) -> bool {
    t.kind == transit_model::TrackKind::Instrument && t.instrument.as_ref().is_some_and(|i| i.plugin != transit_model::EXTERNAL_INSTRUMENT_PLUGIN)
}

/// A hosted stage's storage, made by the plan compiler with its instance (never on the audio
/// thread) and carried in the instance's cell.
pub(crate) struct HostedStore {
    /// The MIDI effect slot's uid the instance was made for.
    pub(crate) uid: u64,
    /// The plan's largest block: the scratch audio's length.
    max_block: usize,
    /// Two channels of `max_block`: the instance's audio, computed and not used (4.4).
    scratch: Vec<Vec<f32>>,
    /// One block's output events ([`HOSTED_OUT_CAP`], the adapter's buffer), to be sorted.
    outs: Vec<HostedOut>,
    pub(super) ledger: Ledger,
}

impl HostedStore {
    pub(crate) fn new(uid: u64, max_block: usize) -> Box<Self> {
        let max_block = max_block.max(1);
        Box::new(HostedStore {
            uid,
            max_block,
            scratch: vec![vec![0.0; max_block]; 2],
            outs: Vec::with_capacity(HOSTED_OUT_CAP),
            ledger: Ledger::new(),
        })
    }

    /// Test seam (feature `testing`): the store's state record. The scratch audio and the output
    /// buffer are unobservable: each is overwritten before it is read in every block.
    #[cfg(feature = "testing")]
    pub(crate) fn record_state(&self, w: &mut transit_state_record::Writer<'_>) {
        w.u64(self.uid);
        w.usize(self.max_block);
        self.ledger.record_state(w);
    }
}

/// A state the user did not ask for: the stage is silent (7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Silence {
    /// No instance in the plan yet (Loading), or its creation returned nothing.
    NoInstance,
    /// On a rack's chain, which takes no hosted stage (10.2).
    OnRack,
    /// The instance reports no readable note output (2.5).
    NoNotes,
    /// The instance is inactive after a refused (re)activation (decision 6).
    CouldNotRestart,
    /// A processing error, until the next Stop (7.4).
    Failed,
}

/// A hosted stage's state in the chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Hosted {
    /// The id is a VST3 one: a note-off with no note id is an anonymous single release (5.3).
    pub(super) vst3: bool,
    /// The stage is on a rack's chain.
    pub(super) on_rack: bool,
    /// The generation of the instance cell last seen (0: none).
    pub(super) inst: u64,
    /// The lifecycle generation the epoch expects (`None` before the first).
    pub(super) epoch: Option<u64>,
    /// The epoch's reset has completed before an accepted call's `process`.
    pub(super) reset_done: bool,
    /// `restart_processing` is to be requested before the next call.
    pub(super) reset_request: bool,
    /// A barrier ran since the ledger was last cleared.
    pub(super) clear_ledger: bool,
    /// `None`: running.
    pub(super) state: Option<Silence>,
    /// Failed until the next Stop (7.4).
    pub(super) failed: bool,
}

impl Hosted {
    pub(super) fn new(id: &str, on_rack: bool) -> Self {
        Hosted {
            vst3: id.starts_with(transit_vst3_host::ID_PREFIX),
            on_rack,
            inst: 0,
            epoch: None,
            reset_done: false,
            reset_request: true,
            clear_ledger: true,
            state: Some(if on_rack { Silence::OnRack } else { Silence::NoInstance }),
            failed: false,
        }
    }

    /// A host barrier: a new epoch with its reset pending, and the ledger cleared.
    pub(super) fn barrier(&mut self) {
        self.reset_done = false;
        self.reset_request = true;
        self.clear_ledger = true;
    }

    /// The block's check (see the module documentation): reads the instance in `cell`, updates
    /// the state, and says whether a barrier must run.
    pub(super) fn check(&mut self, cell: Option<(&dyn Plugin, u64)>) -> bool {
        let (inst, state, generation) = match cell {
            None => (0, Some(if self.on_rack { Silence::OnRack } else { Silence::NoInstance }), None),
            Some((p, g)) if p.note_output().is_none() => (g, Some(Silence::NoNotes), None),
            Some((p, g)) => {
                let lc = p.lifecycle();
                let state = if !lc.active {
                    Some(Silence::CouldNotRestart)
                } else if self.failed {
                    Some(Silence::Failed)
                } else {
                    None
                };
                (g, state, Some(lc.generation))
            }
        };
        let new_epoch = state.is_none() && generation.is_some() && generation != self.epoch;
        let barrier = inst != self.inst || state != self.state || new_epoch;
        self.inst = inst;
        self.state = state;
        if new_epoch {
            self.epoch = generation;
            self.barrier();
        }
        barrier
    }
}

/// What one hosted block produced for the chain's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct BlockOutcome {
    pub(super) tally: Tally,
    /// Output events discarded (a call not accepted, the failing block).
    pub(super) discarded: u64,
    /// Events the record's table drops, and native note-ons with an invalid address.
    pub(super) dropped: u64,
    /// The failing block (7.4): the barrier must run and the stage is failed.
    pub(super) failed: bool,
    /// A call ran under this generation, not the epoch's: the barrier must run (6.2).
    pub(super) new_generation: Option<u64>,
}

/// One block of a running hosted stage, after its input was handed to `p`: processes the scratch
/// audio, takes the output and writes what the epoch accepts through the ledger into `out`
/// (cleared first). On a failing block or a call under another generation nothing is written and
/// the outcome says so: the caller runs the barrier.
pub(super) fn process_block(
    h: &mut Hosted,
    p: &mut dyn Plugin,
    store: &mut HostedStore,
    ids: &mut u32,
    out: &mut Output,
    frames: u32,
) -> BlockOutcome {
    let mut o = BlockOutcome::default();
    let n = (frames as usize).min(store.max_block);
    for c in &mut store.scratch {
        c.iter_mut().take(n).for_each(|x| *x = 0.0);
    }
    p.set_input_silent(true);
    p.process(&mut store.scratch, n);
    store.outs.clear();
    let outs = &mut store.outs;
    let report = p.take_note_output(&mut |e| {
        if outs.len() < outs.capacity() {
            outs.push(e);
        }
        true
    });
    out.clear();
    if report.error {
        o.failed = true;
        o.discarded = store.outs.len() as u64;
        return o;
    }
    // Decision 6: a call's output counts only under the epoch's generation, after its reset. The
    // adapters hold the instance's core across a block's calls, so a block's calls share one
    // generation and only its first can follow the reset: the block's output is accepted whole or
    // discarded whole.
    let mut accepted = true;
    for rec in report.calls() {
        if Some(rec.generation) != h.epoch {
            o.new_generation = Some(rec.generation);
            accepted = false;
            break;
        }
        if !(h.reset_done || rec.reset_before) {
            accepted = false;
            break;
        }
        h.reset_done = true;
    }
    if !accepted {
        o.discarded = store.outs.len() as u64;
        return o;
    }
    // 5.2: event-time order, write order within an offset.
    store.outs.sort_unstable_by_key(|e| (e.offset, e.seq));
    store.ledger.owed(out);
    let last = frames.max(1) - 1;
    for e in &store.outs {
        translate(e, h.vst3, e.offset.min(last), &mut store.ledger, ids, out, &mut o);
    }
    o
}

/// A float velocity as the table says: `round(127 v)`, half away from zero, clamped to `min..=127`.
fn velocity(v: f64, min: u8) -> u8 {
    if !v.is_finite() {
        return min;
    }
    (127.0 * v).round().clamp(f64::from(min), 127.0) as u8
}

fn channel_of(c: i16) -> Option<u8> {
    u8::try_from(c).ok().filter(|c| *c < 16)
}

fn key_of(k: i16) -> Option<u8> {
    u8::try_from(k).ok().filter(|k| *k < 128)
}

/// A native release or pressure's selection (5.3): VST3 with no note id an anonymous single one;
/// CLAP with any wildcard field a wildcard one; otherwise identified. `None`: an address out of
/// range (dropped).
fn native_sel(e: &HostedOut, vst3: bool) -> Option<Sel> {
    if vst3 {
        let (channel, key) = (channel_of(e.channel)?, key_of(e.key)?);
        return Some(if e.note_id == -1 { Sel::Anonymous { channel, key } } else { Sel::Identified { channel, key, pid: e.note_id } });
    }
    let channel = if e.channel == -1 { None } else { Some(channel_of(e.channel)?) };
    let key = if e.key == -1 { None } else { Some(key_of(e.key)?) };
    let pid = (e.note_id != -1).then_some(e.note_id);
    Some(match (channel, key, pid) {
        (Some(channel), Some(key), Some(pid)) => Sel::Identified { channel, key, pid },
        (channel, key, pid) => Sel::Wildcard { channel, key, pid },
    })
}

/// Decision 3's table, after the ledger: one event the plug-in wrote, at `offset`.
fn translate(e: &HostedOut, vst3: bool, offset: u32, ledger: &mut Ledger, ids: &mut u32, out: &mut Output, o: &mut BlockOutcome) {
    let tally = &mut o.tally;
    let pass = |out: &mut Output, channel: u8, message: Message| {
        out.push(TimedEvent { offset, event: PluginEvent { channel, note_id: PluginEvent::NO_ID, message } });
    };
    match (e.form, e.kind, e.values) {
        (_, OutKind::Dropped(_), _) => o.dropped += 1,
        // A MIDI-byte event: seven-bit fields; 9n with velocity 0 is a release before the ledger
        // matches it (5.3, MIDI's rule).
        (OutForm::MidiBytes, kind, OutValues::Midi([status, d1, d2])) => {
            let (channel, d1, d2) = (status & 0x0F, d1 & 0x7F, d2 & 0x7F);
            match kind {
                OutKind::NoteOn if d2 == 0 => ledger.release(Sel::Anonymous { channel, key: d1 }, 0, offset, out, tally),
                OutKind::NoteOn => ledger.on(channel, d1, -1, d2, offset, ids, out, tally),
                OutKind::NoteOff => ledger.release(Sel::Anonymous { channel, key: d1 }, d2, offset, out, tally),
                OutKind::PolyPressure => ledger.pressure(Sel::Anonymous { channel, key: d1 }, d2, offset, out, tally),
                OutKind::Controller => pass(out, channel, Message::Control { cc: d1, value: d2 }),
                OutKind::Program => pass(out, channel, Message::Program { program: d1 }),
                OutKind::ChannelPressure => pass(out, channel, Message::ChannelPressure { value: d1 }),
                OutKind::PitchBend => pass(out, channel, Message::PitchBend { value: u16::from(d1) | (u16::from(d2) << 7) }),
                _ => o.dropped += 1,
            }
        }
        (OutForm::Native, OutKind::NoteOn, OutValues::Velocity(v)) => match (channel_of(e.channel), key_of(e.key)) {
            // A note-on needs a concrete channel and key; its id may be anything, -1 anonymous.
            (Some(channel), Some(key)) => ledger.on(channel, key, e.note_id, velocity(v, 1), offset, ids, out, tally),
            _ => o.dropped += 1,
        },
        (OutForm::Native, OutKind::NoteOff | OutKind::Choke, values) => {
            let v = if let OutValues::Velocity(v) = values { velocity(v, 0) } else { 0 };
            match native_sel(e, vst3) {
                Some(sel) => ledger.release(sel, v, offset, out, tally),
                None => o.dropped += 1,
            }
        }
        (OutForm::Native, OutKind::PolyPressure, OutValues::Pressure(p)) => match native_sel(e, vst3) {
            Some(sel) => ledger.pressure(sel, velocity(p, 0), offset, out, tally),
            None => o.dropped += 1,
        },
        (OutForm::Native, kind, OutValues::Controller { number, value }) => {
            let (Some(channel), value) = (channel_of(e.channel), u8::try_from(value).unwrap_or(0).min(127)) else {
                o.dropped += 1;
                return;
            };
            match kind {
                OutKind::Controller => pass(out, channel, Message::Control { cc: number & 0x7F, value }),
                OutKind::ChannelPressure => pass(out, channel, Message::ChannelPressure { value }),
                OutKind::Program => pass(out, channel, Message::Program { program: value }),
                _ => o.dropped += 1,
            }
        }
        _ => o.dropped += 1,
    }
}

/// Hands one input event to the hosted plug-in (counted when its queue refuses it).
pub(super) fn hand(p: &mut dyn Plugin, offset: u32, ev: &PluginEvent, refused: &mut u64) {
    if !p.event(offset, ev) {
        *refused += 1;
    }
}

impl super::MidiFxHost {
    /// The block's check of every hosted stage among the first `n` (the first `n_rack` the
    /// rack's), with the instances `cells` of the track's own chain: true when a barrier must run.
    pub(super) fn check_hosted(&mut self, cells: &[Option<crate::PluginSlot>], n: usize, n_rack: usize) -> bool {
        let mut barrier = false;
        for (i, s) in self.stages.iter_mut().enumerate().take(n) {
            let Some(s) = s.as_mut().filter(|s| !s.bypass) else { continue };
            let uid = s.uid;
            let Some(h) = s.hosted.as_mut() else { continue };
            let cell = i
                .checked_sub(n_rack)
                .filter(|_| !h.on_rack)
                .and_then(|k| cells.get(k))
                .and_then(Option::as_ref)
                .filter(|c| c.midi_stage.as_ref().is_some_and(|m| m.uid == uid))
                .map(|c| (&*c.plugin, c.generation.get()));
            barrier |= h.check(cell);
        }
        barrier
    }

    /// Stage `i` of the chain, a hosted one that is not bypassed, in `run` (see the module
    /// documentation): between blocks its input waits; in a block it is handed to the instance
    /// with what waited, the block is processed, and the stage's output replaces `a`.
    /// True when a barrier ran in the block: an edit (decision 6's table), whose pedal-ups this has
    /// given and whose remaining step (the mix presses nothing again) the caller takes.
    pub(super) fn run_hosted(
        &mut self,
        instrument: &mut dyn Plugin,
        cells: &mut [Option<crate::PluginSlot>],
        i: usize,
        frames: u32,
        block: bool,
    ) -> bool {
        let len = self.a_len.min(super::MIDI_FX_BLOCK);
        let (n_rack, clock) = (self.n_rack, self.clock);
        let Some(super::Stage { uid, hosted: Some(h), carry, carry_len, .. }) = self.stages.get_mut(i).and_then(Option::as_mut) else { return false };
        if !block {
            // 4.6: never processed between blocks; the event waits for the next actual call.
            if h.state.is_none() {
                for e in self.a.get(..len).unwrap_or_default() {
                    match carry.get_mut(*carry_len) {
                        Some(c) => {
                            *c = super::Carried { due: clock, event: e.event };
                            *carry_len += 1;
                        }
                        None => self.counts.carry_overflow = self.counts.carry_overflow.saturating_add(1),
                    }
                }
            }
            self.a_len = 0;
            return false;
        }
        let uid = *uid;
        let cell = i
            .checked_sub(n_rack)
            .filter(|_| h.state.is_none())
            .and_then(|k| cells.get_mut(k))
            .and_then(Option::as_mut)
            .filter(|c| c.midi_stage.as_ref().is_some_and(|m| m.uid == uid));
        let Some(crate::PluginSlot { plugin: p, midi_stage: Some(store), .. }) = cell else {
            // Silent (7.2), or no instance in this block: its input and what waited are consumed.
            *carry_len = 0;
            self.a_len = 0;
            return false;
        };
        if h.clear_ledger {
            store.ledger.clear();
            h.clear_ledger = false;
        }
        if h.reset_request {
            p.restart_processing();
            h.reset_request = false;
            h.reset_done = false;
        }
        let mut refused = 0;
        // What waited between blocks first, at offset 0, then the block's own input.
        let end = clock.saturating_add(u64::from(frames));
        let mut k = 0;
        while k < *carry_len {
            let Some(c) = carry.get(k).copied() else { break };
            if c.due < end {
                hand(&mut **p, 0, &c.event, &mut refused);
                carry.copy_within(k + 1..*carry_len, k);
                *carry_len -= 1;
            } else {
                k += 1;
            }
        }
        for e in self.a.get(..len).unwrap_or_default() {
            hand(&mut **p, e.offset, &e.event, &mut refused);
        }
        self.a_len = 0;
        let o = process_block(h, &mut **p, store, &mut self.next_host, &mut self.b, frames);
        if o.failed {
            h.failed = true;
            h.state = Some(Silence::Failed);
        }
        let c = &mut self.counts;
        let add = |a: &mut u64, b: u64| *a = a.saturating_add(b);
        add(&mut c.unmatched_releases, o.tally.unmatched);
        add(&mut c.refused_ons, o.tally.full);
        add(&mut c.rejected_unrecorded, o.tally.unrecorded);
        add(&mut c.hosted_dropped, o.dropped);
        add(&mut c.hosted_discarded, o.discarded);
        add(&mut c.hosted_refused, refused);
        c.add(&self.b.counts);
        if o.failed || o.new_generation.is_some() {
            // The failing block (7.4) or a call under another generation (6.2): the barrier at
            // offset 0 of this block, before any output of it reaches downstream, with an edit's
            // pedal-ups; a new epoch.
            self.end_notes_at(instrument, 0, false);
            self.release_pedals(instrument);
            if let (Some(g), Some(h)) = (o.new_generation, self.stages.get_mut(i).and_then(Option::as_mut).and_then(|s| s.hosted.as_mut())) {
                h.epoch = Some(g);
            }
            return true;
        }
        if self.b.release_refused {
            // The reset barrier: no note-on of this block is delivered (ADR 0057 P3b).
            self.barrier = Some(super::Trigger::Stage);
        }
        let mut n = 0;
        for (slot, e) in self.a.iter_mut().zip(self.b.events()) {
            *slot = e;
            n += 1;
        }
        self.a_len = n;
        if let Some(a) = self.a.get_mut(..n) {
            super::sort_by_offset(a);
        }
        false
    }
}
