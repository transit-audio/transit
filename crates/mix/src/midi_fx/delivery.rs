//! Delivery to the instrument: the table of the notes the chain started (`out`), the notes it
//! was given (`given`), the retry buffer and the host's ending of its notes.

use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};

use super::{MIDI_FX_NOTES, MidiFxHost};

/// A note the host knows by its voice id (else by channel and key).
#[derive(Clone, Copy, Debug)]
pub(super) struct Note {
    pub(super) used: bool,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) voice: i32,
    /// For notes the chain was given: the chain edit it started under.
    pub(super) epoch: u32,
}

pub(super) const NO_NOTE: Note = Note { used: false, channel: 0, key: 0, voice: PluginEvent::NO_ID, epoch: 0 };

impl Note {
    pub(super) fn is(&self, ev: &PluginEvent, key: u8) -> bool {
        self.used && self.channel == ev.channel & 0x0F && if ev.note_id != PluginEvent::NO_ID { self.voice == ev.note_id } else { self.key == key }
    }
}

impl MidiFxHost {
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

    /// Gives one event to the instrument, keeping the table of the notes the chain started.
    pub(super) fn deliver(&mut self, plugin: &mut dyn Plugin, offset: u32, ev: &PluginEvent) {
        // Never emit an on we could not end explicitly after a release-only overflow or edit.
        if matches!(ev.message, Message::NoteOn { .. }) && self.out.iter().all(|n| n.used) {
            self.counts.overflow = self.counts.overflow.saturating_add(1);
            // `take` already counted the untracked note when the given table was full too.
            self.counts.refused_ons = self.counts.refused_ons.saturating_add(1);
            return;
        }
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
    pub(super) fn end_notes(&mut self, plugin: &mut dyn Plugin) {
        self.end_notes_at(plugin, 0, false);
    }

    pub(super) fn end_notes_at(&mut self, plugin: &mut dyn Plugin, offset: u32, overflow: bool) {
        for i in 0..MIDI_FX_NOTES {
            let Some(n) = self.out.get_mut(i).filter(|n| n.used) else { continue };
            let off = PluginEvent { channel: n.channel, note_id: n.voice, message: Message::NoteOff { key: n.key, velocity: 0, id: NoteId::NONE } };
            *n = NO_NOTE;
            let count = if overflow { &mut self.counts.released_on_overflow } else { &mut self.counts.released_on_edit };
            *count = count.saturating_add(1);
            if !self.give(plugin, offset, &off) {
                self.counts.refused = self.counts.refused.saturating_add(1);
                self.hold_for_retry(&off);
            }
        }
        if overflow {
            // A downstream effect may have carried the coalesced pedal-ups beyond this
            // block. Recovery cancels that carry, so deliver its safety controllers directly
            // as well: native offs alone do not end sustained MIDI voices.
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
        // Every note given so far is now from an earlier epoch: its release is swallowed. (Each
        // is set to the epoch just left, so a wrap of the counter cannot make one current again.)
        let previous = self.epoch;
        self.epoch = self.epoch.wrapping_add(1);
        for n in self.given.iter_mut().filter(|n| n.used) {
            n.epoch = previous;
        }
    }
}
