//! Delivery to the instrument (ADR 0057, slice P3b): the host owns what reaches the instrument
//! after the last stage.
//!
//! - **Tables.** `out` holds every note-on the host delivered and has not seen released; the
//!   refused table every note-on it did not deliver (the `out` table full, or the instrument
//!   refusing it), [`MIDI_FX_NOTES`] each; `given` the notes the chain was given. Every entry
//!   carries a 64-bit sequence number, never shared between live entries, so order is never by
//!   slot index; if the counter would run out, the live entries are renumbered in order.
//! - **Matching.** A note-off closes the oldest matching entry of `out` and the refused table
//!   together: one with a voice id the entry of its channel with that id only, one without the
//!   oldest anonymous entry of its channel and key. Closing an `out` entry gives the note-off to
//!   the instrument; closing a refused one, or closing nothing, consumes it (counted in
//!   `consumed_releases`). Note-offs only: pedal-ups and channel-mode controllers always pass.
//! - **Retries.** A release the instrument refuses goes to the retry buffer of
//!   [`MIDI_FX_RETRY`](super::MIDI_FX_RETRY) (past it, lost and counted in `retry_overflow`); a
//!   chain note-off frees its `out` entry as it enters, so it is owed once and a later note-off of
//!   its key claims the next entry. The buffer's releases are given at offset 0 of the next block,
//!   ahead of the block's events, directly: no device and not this matching.
//! - **The reset barrier.** A note-on that finds the refused table full triggers it (as does a
//!   release that finds a stage's buffer full, `output`): that note-on and every later note-on
//!   of the block are dropped with no record (the reset swallows their releases), releases still
//!   end the notes they match, and after the block's last event the host ends every `out` note,
//!   gives the safety controllers, resets every effect, empties every carry, clears the refused
//!   table and starts a new epoch, once for the whole chain ([`MidiFxHost::end_notes_at`]).

use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};

use super::{MIDI_FX_NOTES, MidiFxHost};

/// A note the host knows: by its voice id, else (anonymous) by channel and key, oldest first.
#[derive(Clone, Copy, Debug)]
pub(super) struct Note {
    pub(super) used: bool,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) voice: i32,
    /// For notes the chain was given: the chain edit it started under.
    pub(super) epoch: u32,
    /// Its place in the order of the host's notes (never shared between live entries).
    pub(super) seq: u64,
}

pub(super) const NO_NOTE: Note = Note { used: false, channel: 0, key: 0, voice: PluginEvent::NO_ID, epoch: 0, seq: 0 };

/// What made the block's reset barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trigger {
    /// A release a stage's buffer refused: no note-on of the block is delivered.
    Stage,
    /// A note-on that found the refused table full: no later note-on of the block is delivered.
    Delivery,
}

/// The oldest entry of `table` that `ev` (a note-off or poly pressure of `key`) matches: with a
/// voice id, the entry of its channel with that id; without, the oldest anonymous entry of its
/// channel and key.
pub(super) fn oldest(table: &[Note], ev: &PluginEvent, key: u8) -> Option<usize> {
    let channel = ev.channel & 0x0F;
    table
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.used
                && n.channel == channel
                && if ev.note_id != PluginEvent::NO_ID { n.voice == ev.note_id } else { n.voice == PluginEvent::NO_ID && n.key == key }
        })
        .min_by_key(|(_, n)| n.seq)
        .map(|(i, _)| i)
}

impl MidiFxHost {
    /// The next sequence number of the host's notes. No two live entries ever share one: when the
    /// 64-bit counter would run out, the live entries are renumbered first (`renumber`).
    pub(super) fn next_seq(&mut self) -> u64 {
        if self.seq == u64::MAX {
            self.renumber();
        }
        self.seq += 1;
        self.seq
    }

    /// Renumbers the live entries of `out`, the refused table and `given` (at most 768) to 1..n in
    /// their existing order, in place, and continues the counter from n. Each step takes the
    /// smallest old number above the last one taken; a renumbered entry's new number is at most
    /// its old one, so it is never taken again. Allocation-free; quadratic in the live entries,
    /// once in 2^64 numbers.
    fn renumber(&mut self) {
        let mut taken: Option<u64> = None;
        let mut n = 0u64;
        loop {
            let next =
                self.out.iter().chain(&self.refusals).chain(&self.given).filter(|e| e.used && taken.is_none_or(|t| e.seq > t)).map(|e| e.seq).min();
            let Some(old) = next else { break };
            n += 1;
            for e in self.out.iter_mut().chain(self.refusals.iter_mut()).chain(self.given.iter_mut()).filter(|e| e.used && e.seq == old) {
                e.seq = n;
            }
            taken = Some(old);
        }
        self.seq = n;
    }

    /// Holds a refused release for the next block (counted when there is no room).
    pub(super) fn hold_for_retry(&mut self, ev: &PluginEvent) {
        match self.retry.get_mut(self.retry_len) {
            Some(slot) => {
                *slot = *ev;
                self.retry_len += 1;
            }
            None => self.counts.retry_overflow = self.counts.retry_overflow.saturating_add(1),
        }
    }

    /// The last block's refused releases, at offset 0, ahead of this block's events: directly to
    /// the instrument (their `out` entries were freed when they were refused). One refused again
    /// is held again, at or before the index being read, so nothing unread is overwritten.
    pub(super) fn give_retries(&mut self, plugin: &mut dyn Plugin) {
        let held = std::mem::take(&mut self.retry_len);
        for i in 0..held {
            let Some(e) = self.retry.get(i).copied() else { break };
            if !self.give(plugin, 0, &e) {
                self.counts.refused = self.counts.refused.saturating_add(1);
                self.hold_for_retry(&e);
            }
        }
    }

    /// Gives one event after the last stage to the instrument, keeping the delivery tables (see
    /// the module documentation).
    pub(super) fn deliver(&mut self, plugin: &mut dyn Plugin, offset: u32, ev: &PluginEvent) {
        match ev.message {
            Message::NoteOn { key, .. } => {
                if self.barrier.is_some() {
                    self.counts.dropped_ons = self.counts.dropped_ons.saturating_add(1);
                    return;
                }
                let Some(n) = self.out.iter().position(|n| !n.used) else {
                    self.counts.overflow = self.counts.overflow.saturating_add(1);
                    self.counts.refused_ons = self.counts.refused_ons.saturating_add(1);
                    self.record_refusal(ev, key);
                    return;
                };
                if !self.give(plugin, offset, ev) {
                    self.counts.refused = self.counts.refused.saturating_add(1);
                    self.record_refusal(ev, key);
                    return;
                }
                let seq = self.next_seq();
                if let Some(n) = self.out.get_mut(n) {
                    *n = Note { used: true, channel: ev.channel & 0x0F, key, voice: ev.note_id, epoch: 0, seq };
                }
            }
            Message::NoteOff { key, .. } => {
                let delivered = oldest(&self.out, ev, key);
                let refused = oldest(&self.refusals, ev, key);
                let seq = |table: &[Note], i: Option<usize>| i.and_then(|i| table.get(i)).map(|n| n.seq);
                match (seq(&self.out, delivered), seq(&self.refusals, refused)) {
                    (Some(d), r) if r.is_none_or(|r| d < r) => {
                        if let Some(n) = delivered.and_then(|i| self.out.get_mut(i)) {
                            *n = NO_NOTE;
                        }
                        if !self.give(plugin, offset, ev) {
                            self.counts.refused = self.counts.refused.saturating_add(1);
                            self.hold_for_retry(ev);
                        }
                    }
                    (_, Some(_)) => {
                        if let Some(n) = refused.and_then(|i| self.refusals.get_mut(i)) {
                            *n = NO_NOTE;
                        }
                        self.counts.consumed_releases = self.counts.consumed_releases.saturating_add(1);
                    }
                    _ => self.counts.consumed_releases = self.counts.consumed_releases.saturating_add(1),
                }
            }
            _ => {
                if !self.give(plugin, offset, ev) {
                    self.counts.refused = self.counts.refused.saturating_add(1);
                    if ev.is_release() {
                        self.hold_for_retry(ev);
                    }
                }
            }
        }
    }

    /// A note-on the host did not deliver: recorded, so its note-off is consumed; with the
    /// refused table full, the block's reset barrier instead (the note-on dropped, no record).
    fn record_refusal(&mut self, ev: &PluginEvent, key: u8) {
        let Some(n) = self.refusals.iter().position(|n| !n.used) else {
            self.barrier = Some(Trigger::Delivery);
            self.counts.dropped_ons = self.counts.dropped_ons.saturating_add(1);
            return;
        };
        let seq = self.next_seq();
        if let Some(n) = self.refusals.get_mut(n) {
            *n = Note { used: true, channel: ev.channel & 0x0F, key, voice: ev.note_id, epoch: 0, seq };
        }
    }

    /// Ends the chain's notes at the instrument, forgets the carries and the effects' state, clears
    /// the refused table and starts a new edit epoch: the given notes' later releases are swallowed
    /// (a chain edit, Stop or panic).
    pub(super) fn end_notes(&mut self, plugin: &mut dyn Plugin) {
        self.end_notes_at(plugin, 0, false);
    }

    /// [`Self::end_notes`] at `offset`; `barrier`: the block's reset barrier, which also gives the
    /// safety controllers (CC 64, 66, 67, 69, 120, 121 and 123 on every channel) after the note-offs.
    /// The stage buffers need no emptying: each is cleared before it is written.
    pub(super) fn end_notes_at(&mut self, plugin: &mut dyn Plugin, offset: u32, barrier: bool) {
        for i in 0..MIDI_FX_NOTES {
            let Some(n) = self.out.get_mut(i).filter(|n| n.used) else { continue };
            let off = PluginEvent { channel: n.channel, note_id: n.voice, message: Message::NoteOff { key: n.key, velocity: 0, id: NoteId::NONE } };
            *n = NO_NOTE;
            let count = if barrier { &mut self.counts.released_on_overflow } else { &mut self.counts.released_on_edit };
            *count = count.saturating_add(1);
            if !self.give(plugin, offset, &off) {
                self.counts.refused = self.counts.refused.saturating_add(1);
                self.hold_for_retry(&off);
            }
        }
        if barrier {
            self.counts.safety_resets = self.counts.safety_resets.saturating_add(1);
            // Native note-offs alone do not end sustained MIDI voices, and a pedal-up the chain
            // carried past this block is forgotten with the carries below.
            for channel in 0..16 {
                for cc in [64, 66, 67, 69, 120, 121, 123] {
                    let ev = PluginEvent { channel, note_id: PluginEvent::NO_ID, message: Message::Control { cc, value: 0 } };
                    if !self.give(plugin, offset, &ev) {
                        self.counts.refused = self.counts.refused.saturating_add(1);
                        self.hold_for_retry(&ev);
                    }
                }
            }
        }
        for s in self.stages.iter_mut().flatten() {
            self.counts.carry_dropped = self.counts.carry_dropped.saturating_add(s.carry_len as u64);
            s.carry_len = 0;
            if let Some(fx) = s.fx.as_mut() {
                fx.effect().reset();
            }
        }
        // A refusal recorded for a note of the old chain would outlive its device: its note-off
        // never comes, and oldest-first matching would consume a new note's against it.
        self.refusals = [NO_NOTE; MIDI_FX_NOTES];
        // Every note given so far is now from an earlier epoch: its release is swallowed. (Each
        // is set to the epoch just left, so a wrap of the counter cannot make one current again.)
        let previous = self.epoch;
        self.epoch = self.epoch.wrapping_add(1);
        for n in self.given.iter_mut().filter(|n| n.used) {
            n.epoch = previous;
        }
    }
}
