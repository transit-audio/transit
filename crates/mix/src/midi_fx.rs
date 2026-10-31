//! The MIDI effect chain of an instrument track (#201, ADR 0036), on the audio thread.
//!
//! Every event the mix gives an instrument leaves [`InstrumentEvents`](crate::events) through one
//! call (its `send`): live input, clips, the controller chase, the releases of a seek, a Stop or a
//! failure. While the track has MIDI effects that call hands the event to this host instead of the
//! plug-in. Within a block ([`MidiFxHost::begin`] to [`MidiFxHost::flush`]) the host collects the
//! block's events; right before the instrument processes, it runs them through the effects in
//! chain order and gives the instrument what comes out, at the offsets the effects wrote. Outside a
//! block (a Stop, a seek, a failure) an event runs through the chain at once, at offset 0.
//!
//! **Order and offsets.** Each effect gets its input sorted by offset (a stable sort: events at
//! one offset keep the order the mix gave them) and may move, add, remove or rewrite events. An
//! event an effect writes at or past the end of the block waits in that effect's carry and joins
//! the next effect's input (or the instrument's) in the block it is due, ahead of that block's
//! own events at its offset; one overdue (a block the strip did not render) lands at offset 0.
//!
//! **Bypass is transparent.** A bypassed effect is skipped: its input is its output, event for
//! event. A chain whose every effect is bypassed gives the instrument exactly the events, offsets
//! and order it would get with no chain.
//!
//! **Edits end the chain's notes.** Adding, removing, moving or bypassing an effect changes what
//! a note's release would go through, so at the first block after such an edit the host sends a
//! note-off for every note it started on the instrument (it tracks them), forgets carried events,
//! and swallows the later releases of notes that started before the edit. A parameter change is
//! not an edit: each effect pairs its own notes (Transpose remembers each note's shift). Stop and
//! panic do the same before the mix releases its sources.
//!
//! **Capacity.** A block takes at most [`MIDI_FX_BLOCK`] events into the chain and each effect
//! writes at most as many; each effect carries at most [`MIDI_FX_CARRY`] events to later blocks;
//! the host tracks [`MIDI_FX_NOTES`] sounding notes. Past those limits events are refused and
//! counted in [`MidiFxCounts`] (in the engine's [`EventStatus`](crate::EventStatus)). A release
//! the chain could not take is retried by the mix at the next block; one the instrument's queue
//! refuses is given again at offset 0 of the next block (up to [`MIDI_FX_RETRY`] at once); one a
//! full carry cannot hold takes the place of a carried non-release (a carry of releases alone
//! delivers it at the block's last offset). One path still drops: when an effect's output plus
//! its carried events due in the block exceed [`MIDI_FX_BLOCK`], the tail is dropped, releases
//! included. Transpose and Velocity never reach it (one event out per event in, no carry). Nothing here allocates, locks or logs: the
//! storage is made with the instrument's event state, off the audio thread, and a built-in effect
//! is a value made in place ([`Builtin`]).

use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::midi_fx::{Builtin, MidiOut, TimedEvent};
use transit_dsp::{Plugin, Transport};
use transit_model::{AutoParam, MAX_MIDI_EFFECTS, MidiEffectSlot, Track};
use transit_time::Samples;

use crate::note_view::NoteView;

/// Events one block takes into the chain, and each effect writes, at most.
pub const MIDI_FX_BLOCK: usize = 512;
/// Events each effect holds for later blocks at most.
pub const MIDI_FX_CARRY: usize = 64;
/// Notes the host tracks at once, both the ones it started on the instrument and the ones it was
/// given (more than an instrument holds open, 224).
pub const MIDI_FX_NOTES: usize = 256;
/// Releases from the chain that the instrument's own queue refused, held for the next block.
pub const MIDI_FX_RETRY: usize = 64;

/// The chain's counters, cumulative for the instrument's life (read through
/// [`MixEngine::event_status`](crate::MixEngine::event_status)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MidiFxCounts {
    /// Events a full block buffer refused: the chain's input (the mix retries a refused release)
    /// or an effect's output.
    pub overflow: u64,
    /// Events due in a later block that a full carry could not hold, or that a release displaced
    /// from it (a release is never dropped for room).
    pub carry_overflow: u64,
    /// Notes the host ended because the chain changed under them, or at a Stop.
    pub released_on_edit: u64,
    /// Carried events forgotten at a chain edit or a Stop.
    pub carry_dropped: u64,
    /// Notes started on the instrument, or given to the chain, that a full note table could not
    /// track.
    pub untracked: u64,
    /// Events the instrument's own queue refused. A refused release is given again at offset 0 of
    /// the next block (up to [`MIDI_FX_RETRY`] held at once; past that it is lost and counted in
    /// `retry_overflow`).
    pub refused: u64,
    pub retry_overflow: u64,
    /// Events the instrument accepted that a full note view (#242, [`crate::note_view`]) could
    /// not keep for the track's inserts that read notes; the view ended with an all-notes-off.
    pub note_view_dropped: u64,
}

impl MidiFxCounts {
    pub(crate) fn add(&mut self, o: &MidiFxCounts) {
        let add = |a: &mut u64, b: u64| *a = a.saturating_add(b);
        add(&mut self.overflow, o.overflow);
        add(&mut self.carry_overflow, o.carry_overflow);
        add(&mut self.released_on_edit, o.released_on_edit);
        add(&mut self.carry_dropped, o.carry_dropped);
        add(&mut self.untracked, o.untracked);
        add(&mut self.refused, o.refused);
        add(&mut self.retry_overflow, o.retry_overflow);
        add(&mut self.note_view_dropped, o.note_view_dropped);
    }
}

#[derive(Clone, Copy, Debug)]
struct Carried {
    due: u64,
    event: PluginEvent,
}

const NO_CARRY: Carried = Carried { due: 0, event: TimedEvent::EMPTY.event };

struct Stage {
    uid: u64,
    /// FNV-1a of the plug-in id: an id change is an edit.
    plugin: u64,
    bypass: bool,
    /// `None`: an id this build does not know, which passes events through.
    fx: Option<Builtin>,
    carry: [Carried; MIDI_FX_CARRY],
    carry_len: usize,
}

/// A note the host knows by its voice id (else by channel and key).
#[derive(Clone, Copy, Debug)]
struct Note {
    used: bool,
    channel: u8,
    key: u8,
    voice: i32,
    /// For notes the chain was given: the chain edit it started under.
    epoch: u32,
}

const NO_NOTE: Note = Note { used: false, channel: 0, key: 0, voice: PluginEvent::NO_ID, epoch: 0 };

impl Note {
    fn is(&self, ev: &PluginEvent, key: u8) -> bool {
        self.used && self.channel == ev.channel & 0x0F && if ev.note_id != PluginEvent::NO_ID { self.voice == ev.note_id } else { self.key == key }
    }
}

/// The MIDI effect chain of one instrument slot. See the module documentation.
pub(crate) struct MidiFxHost {
    stages: [Option<Stage>; 2 * MAX_MIDI_EFFECTS],
    /// Effects in the chain (the first `len` stages).
    len: usize,
    /// Within a block: events are collected, not run at once.
    capturing: bool,
    frames: u32,
    /// The render frame of the current block's start (of the next block's, between blocks): carried
    /// events are due against it.
    clock: u64,
    a: [TimedEvent; MIDI_FX_BLOCK],
    a_len: usize,
    b: [TimedEvent; MIDI_FX_BLOCK],
    /// Notes the host started on the instrument.
    out: [Note; MIDI_FX_NOTES],
    /// Notes the chain was given and has not seen released.
    given: [Note; MIDI_FX_NOTES],
    /// Used entries of `given`.
    given_len: usize,
    /// Refused releases, given again at the start of the next block.
    retry: [PluginEvent; MIDI_FX_RETRY],
    retry_len: usize,
    epoch: u32,
    pub(crate) counts: MidiFxCounts,
    /// What the instrument accepted this block, for the inserts that read notes (#242). Kept here
    /// because every event to the instrument leaves through this host or past it
    /// ([`MidiFxHost::give`]).
    pub(crate) notes: NoteView,
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn key_of(m: &Message) -> Option<u8> {
    match *m {
        Message::NoteOn { key, .. } | Message::NoteOff { key, .. } | Message::PolyPressure { key, .. } => Some(key),
        _ => None,
    }
}

/// A stable, allocation-free sort by offset (blocks are short and nearly sorted).
fn sort_by_offset(events: &mut [TimedEvent]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events.get(j - 1).zip(events.get(j)).is_some_and(|(x, y)| x.offset > y.offset) {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// The value of MIDI effect `index`'s parameter `id` at `pos`: its automation lane while the
/// track reads automation and the lane has points, else the stored value.
fn param_value(t: &Track, index: usize, fx: &MidiEffectSlot, id: &str, pos: Samples) -> Option<f32> {
    let slot = AutoParam::midi_effect_slot(index)?;
    let lane = t
        .automation
        .iter()
        .filter(|_| t.mixer.automation_mode.reads())
        .find(|l| matches!(&l.param, AutoParam::Plugin { slot: s, param } if *s == slot && param == id))
        .filter(|l| !l.points.is_empty());
    match lane {
        Some(l) => Some(l.value_at(pos, 0.0)),
        None => fx.params.get(id).copied(),
    }
}

impl MidiFxHost {
    /// Made off the audio thread, with the instrument's event state.
    pub(crate) fn new() -> Box<Self> {
        Box::new(MidiFxHost {
            stages: std::array::from_fn(|_| None),
            len: 0,
            capturing: false,
            frames: 0,
            clock: 0,
            a: [TimedEvent::EMPTY; MIDI_FX_BLOCK],
            a_len: 0,
            b: [TimedEvent::EMPTY; MIDI_FX_BLOCK],
            out: [NO_NOTE; MIDI_FX_NOTES],
            given: [NO_NOTE; MIDI_FX_NOTES],
            given_len: 0,
            retry: [TimedEvent::EMPTY.event; MIDI_FX_RETRY],
            retry_len: 0,
            epoch: 0,
            counts: MidiFxCounts::default(),
            notes: NoteView::new(),
        })
    }

    /// Events go through the host: it has effects, carried events, or releases to swallow.
    pub(crate) fn active(&self) -> bool {
        self.len > 0 || self.given_len > 0 || self.pending()
    }

    /// Carried events wait for a later block.
    pub(crate) fn pending(&self) -> bool {
        self.retry_len > 0 || self.stages.iter().flatten().any(|s| s.carry_len > 0)
    }

    /// Start of a block of `frames` at render frame `frame`: the chain follows `t.midi_effects` (an
    /// edit ends the chain's notes on `plugin`, see the module documentation), each effect gets its
    /// parameter values at `pos` and the transport, and events are collected until
    /// [`MidiFxHost::flush`].
    pub(crate) fn begin(
        &mut self,
        plugin: &mut dyn Plugin,
        t: &Track,
        rack: Option<&Track>,
        pos: Samples,
        transport: &Transport,
        frame: u64,
        frames: usize,
    ) {
        // A member of a rack runs the rack's chain first, then its own (ADR 0036, racks): the
        // rack's effects feed every member the same way. Both run in full: the rack's up to
        // `MAX_MIDI_EFFECTS`, then the member's up to as many.
        let n_rack = rack.map_or(0, |r| r.midi_effects.len().min(MAX_MIDI_EFFECTS));
        let n = n_rack + t.midi_effects.len().min(MAX_MIDI_EFFECTS);
        let entry = |i: usize| match rack.filter(|_| i < n_rack) {
            Some(r) => r.midi_effects.get(i).map(|w| (r, i, w)),
            None => t.midi_effects.get(i - n_rack).map(|w| (t, i - n_rack, w)),
        };
        let same = n == self.len
            && (0..n).zip(self.stages.iter()).all(|(i, s)| {
                entry(i)
                    .is_some_and(|(_, _, w)| s.as_ref().is_some_and(|s| s.uid == w.uid.get() && s.bypass == w.bypass && s.plugin == fnv(&w.plugin)))
            });
        if !same {
            self.end_notes(plugin);
            for (i, slot) in self.stages.iter_mut().enumerate() {
                let Some((_, _, w)) = entry(i).filter(|_| i < n) else {
                    *slot = None;
                    continue;
                };
                let fx = Builtin::new(&w.plugin);
                match slot {
                    Some(s) => {
                        s.uid = w.uid.get();
                        s.plugin = fnv(&w.plugin);
                        s.bypass = w.bypass;
                        s.fx = fx;
                        s.carry_len = 0;
                    }
                    None => {
                        *slot = Some(Stage {
                            uid: w.uid.get(),
                            plugin: fnv(&w.plugin),
                            bypass: w.bypass,
                            fx,
                            carry: [NO_CARRY; MIDI_FX_CARRY],
                            carry_len: 0,
                        });
                    }
                }
            }
            self.len = n;
        }
        for (i, s) in self.stages.iter_mut().enumerate().take(n) {
            let Some((owner, index, w)) = entry(i) else { continue };
            let Some(fx) = s.as_mut().filter(|s| !s.bypass).and_then(|s| s.fx.as_mut()) else { continue };
            let fx = fx.effect();
            for p in fx.info().params {
                if let Some(v) = param_value(owner, index, w, p.id, pos) {
                    fx.set_param(p.id, v);
                } else if fx.param(p.id) != Some(p.default) {
                    fx.set_param(p.id, p.default);
                }
            }
            fx.set_transport(transport);
        }
        self.frames = u32::try_from(frames).unwrap_or(u32::MAX).max(1);
        // A block the strip did not render (a render worker held it) leaves carried events overdue:
        // they land at offset 0.
        self.clock = frame;
        self.capturing = true;
    }

    /// Takes one event from the mix: collected within a block, else run through the chain now
    /// and given to `plugin` at offset 0. False when it could not be taken (counted; the mix
    /// retries a release).
    pub(crate) fn take(&mut self, plugin: &mut dyn Plugin, offset: u32, ev: &PluginEvent) -> bool {
        // The release of a note that started before a chain edit: the edit already ended it.
        let off = matches!(ev.message, Message::NoteOff { .. });
        let known = key_of(&ev.message)
            .filter(|_| self.given_len > 0 && !matches!(ev.message, Message::NoteOn { .. }))
            .and_then(|key| self.given.iter().position(|n| n.is(ev, key)));
        if let Some(n) = known
            && self.given.get(n).is_some_and(|n| n.epoch != self.epoch)
        {
            if off {
                self.forget_given(n);
            }
            return true;
        }
        if self.capturing {
            let Some(slot) = self.a.get_mut(self.a_len) else {
                self.counts.overflow = self.counts.overflow.saturating_add(1);
                return false;
            };
            *slot = TimedEvent { offset: offset.min(self.frames - 1), event: *ev };
            self.a_len += 1;
        } else {
            let Some(slot) = self.a.first_mut() else { return false };
            *slot = TimedEvent { offset: 0, event: *ev };
            self.a_len = 1;
            self.run(plugin, 1, false);
        }
        if let Some(n) = known.filter(|_| off) {
            self.forget_given(n);
        }
        if let Message::NoteOn { key, .. } = ev.message {
            match self.given.iter().position(|n| !n.used) {
                Some(n) => {
                    if let Some(n) = self.given.get_mut(n) {
                        *n = Note { used: true, channel: ev.channel & 0x0F, key, voice: ev.note_id, epoch: self.epoch };
                        self.given_len += 1;
                    }
                }
                None => self.counts.untracked = self.counts.untracked.saturating_add(1),
            }
        }
        true
    }

    fn forget_given(&mut self, n: usize) {
        if let Some(n) = self.given.get_mut(n).filter(|n| n.used) {
            *n = NO_NOTE;
            self.given_len = self.given_len.saturating_sub(1);
        }
    }

    /// End of the block's collection: the block's events through the chain, then to `plugin`,
    /// right before it processes.
    pub(crate) fn flush(&mut self, plugin: &mut dyn Plugin) {
        if !self.capturing {
            return;
        }
        self.capturing = false;
        let frames = self.frames;
        self.run(plugin, frames, true);
        self.clock = self.clock.saturating_add(u64::from(frames));
    }

    /// Stop or panic: ends every note the chain started and forgets what it carries and holds;
    /// the releases the mix sends next for the chain's input are swallowed.
    pub(crate) fn panic(&mut self, plugin: &mut dyn Plugin) {
        if self.active() {
            self.end_notes(plugin);
        }
    }

    /// Runs `a` (`a_len` events at offsets below `frames`) through the chain and gives the result
    /// to `plugin`. `block`: a rendered block, whose carried events come due (else an event run
    /// between blocks, which only adds to the carries).
    fn run(&mut self, plugin: &mut dyn Plugin, frames: u32, block: bool) {
        if block {
            // Last block's refused releases first, at offset 0. One refused again is held again,
            // at or before the index being read, so nothing unread is overwritten.
            let held = std::mem::take(&mut self.retry_len);
            for i in 0..held {
                let Some(e) = self.retry.get(i).copied() else { break };
                self.deliver(plugin, 0, &e);
            }
        }
        let len = self.a_len.min(MIDI_FX_BLOCK);
        if let Some(a) = self.a.get_mut(..len) {
            sort_by_offset(a);
        }
        let end = self.clock.saturating_add(u64::from(frames));
        for stage in self.stages.iter_mut().take(self.len).flatten() {
            let Some(fx) = stage.fx.as_mut().filter(|_| !stage.bypass) else { continue };
            // The effect's output, then this block's carried events ahead of it at their offset.
            let mut out = MidiOut::new(&mut self.b);
            fx.effect().process(self.a.get(..self.a_len).unwrap_or_default(), &mut out, frames);
            let (written, refused) = (out.len(), out.refused());
            self.counts.overflow = self.counts.overflow.saturating_add(u64::from(refused));
            let mut n = 0usize;
            if block {
                let mut k = 0;
                while k < stage.carry_len {
                    let Some(c) = stage.carry.get(k).copied() else { break };
                    if c.due < end {
                        if let Some(slot) = self.a.get_mut(n) {
                            *slot =
                                TimedEvent { offset: u32::try_from(c.due.saturating_sub(self.clock)).unwrap_or(0).min(frames - 1), event: c.event };
                            n += 1;
                        } else {
                            self.counts.overflow = self.counts.overflow.saturating_add(1);
                        }
                        stage.carry.copy_within(k + 1..stage.carry_len, k);
                        stage.carry_len -= 1;
                    } else {
                        k += 1;
                    }
                }
            }
            for e in self.b.iter().take(written) {
                let mut e = *e;
                if e.offset >= frames {
                    let due = self.clock.saturating_add(u64::from(e.offset));
                    if let Some(c) = stage.carry.get_mut(stage.carry_len) {
                        *c = Carried { due, event: e.event };
                        stage.carry_len += 1;
                        continue;
                    }
                    self.counts.carry_overflow = self.counts.carry_overflow.saturating_add(1);
                    if !e.event.is_release() {
                        continue;
                    }
                    // A release takes the place of a carried non-release (its note-on, perhaps):
                    // delivered early it could end up before its own note-on and leave it hanging.
                    let held = stage.carry.get(..stage.carry_len).unwrap_or_default().iter().position(|c| !c.event.is_release());
                    if let Some(c) = held.and_then(|k| stage.carry.get_mut(k)) {
                        *c = Carried { due, event: e.event };
                        continue;
                    }
                    // Every carried event is a release, so this one's note-on is not among them.
                    e.offset = frames - 1;
                }
                match self.a.get_mut(n) {
                    Some(slot) => {
                        *slot = e;
                        n += 1;
                    }
                    None => self.counts.overflow = self.counts.overflow.saturating_add(1),
                }
            }
            self.a_len = n;
            if let Some(a) = self.a.get_mut(..n) {
                sort_by_offset(a);
            }
        }
        let n = self.a_len;
        self.a_len = 0;
        for i in 0..n {
            let Some(e) = self.a.get(i).copied() else { break };
            self.deliver(plugin, e.offset, &e.event);
        }
    }

    /// Holds a refused release for the next block (counted when there is no room).
    fn hold_for_retry(&mut self, ev: &PluginEvent) {
        match self.retry.get_mut(self.retry_len) {
            Some(slot) => {
                *slot = *ev;
                self.retry_len += 1;
            }
            None => self.counts.retry_overflow = self.counts.retry_overflow.saturating_add(1),
        }
    }

    /// Gives one event to the instrument and, when it accepts it, keeps it in the note view (#242).
    /// Every event the mix gives an instrument goes through here: the chain's deliveries and
    /// releases, and the note table's own when the track has no MIDI effects.
    pub(crate) fn give(&mut self, plugin: &mut dyn Plugin, offset: u32, ev: &PluginEvent) -> bool {
        let ok = plugin.event(offset, ev);
        if ok {
            let lost = self.notes.record(offset, ev);
            self.counts.note_view_dropped = self.counts.note_view_dropped.saturating_add(lost);
        }
        ok
    }

    /// Gives one event to the instrument, keeping the table of the notes the chain started.
    fn deliver(&mut self, plugin: &mut dyn Plugin, offset: u32, ev: &PluginEvent) {
        if !self.give(plugin, offset, ev) {
            self.counts.refused = self.counts.refused.saturating_add(1);
            if ev.is_release() {
                self.hold_for_retry(ev);
            }
            return;
        }
        match ev.message {
            Message::NoteOn { key, .. } => match self.out.iter().position(|n| !n.used) {
                Some(n) => {
                    if let Some(n) = self.out.get_mut(n) {
                        *n = Note { used: true, channel: ev.channel & 0x0F, key, voice: ev.note_id, epoch: 0 };
                    }
                }
                None => self.counts.untracked = self.counts.untracked.saturating_add(1),
            },
            Message::NoteOff { key, .. } => {
                if let Some(n) = self.out.iter_mut().find(|n| n.is(ev, key)) {
                    *n = NO_NOTE;
                }
            }
            _ => {}
        }
    }

    /// Ends the chain's notes at the instrument, forgets the carries and the effects' state, and
    /// starts a new edit epoch: the given notes' later releases are swallowed.
    fn end_notes(&mut self, plugin: &mut dyn Plugin) {
        for i in 0..MIDI_FX_NOTES {
            let Some(n) = self.out.get_mut(i).filter(|n| n.used) else { continue };
            let off = PluginEvent { channel: n.channel, note_id: n.voice, message: Message::NoteOff { key: n.key, velocity: 0, id: NoteId::NONE } };
            *n = NO_NOTE;
            self.counts.released_on_edit = self.counts.released_on_edit.saturating_add(1);
            if !self.give(plugin, 0, &off) {
                self.counts.refused = self.counts.refused.saturating_add(1);
                self.hold_for_retry(&off);
            }
        }
        for s in self.stages.iter_mut().flatten() {
            self.counts.carry_dropped = self.counts.carry_dropped.saturating_add(s.carry_len as u64);
            s.carry_len = 0;
            if let Some(fx) = s.fx.as_mut() {
                fx.effect().reset();
            }
        }
        // Every note given so far is now from an earlier epoch: its release is swallowed. (Each
        // is set to the epoch just left, so a wrap of the counter cannot make one current again.)
        let previous = self.epoch;
        self.epoch = self.epoch.wrapping_add(1);
        for n in self.given.iter_mut().filter(|n| n.used) {
            n.epoch = previous;
        }
    }
}

#[cfg(test)]
mod tests;
