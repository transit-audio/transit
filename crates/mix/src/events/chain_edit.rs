//! Chain edits, the mix's side (ADR 0057, decision 2.13, slice P3a).
//!
//! In the block where a new MIDI effect chain is installed, before any of that block's events and
//! all at offset 0: (1) every voice from before ends: the host ends the notes it started (its `out`
//! table) and, at the first insertion, the mix ends every voice it gave the instrument directly,
//! through its own note table; (2) the host gives a pedal-up for every pedal the instrument holds
//! down; (3) nothing is pressed again: the mix's record of the pedal state it sent is set to up
//! and every holder is marked stale; (4) expression is kept: nothing else is sent.

use super::{InstrumentEvents, MAX_OPEN, Message};
use transit_dsp::{Plugin, Transport};
use transit_model::Track;
use transit_time::Samples;

impl InstrumentEvents {
    /// Start of a block for the track's MIDI effects ([`crate::midi_fx::MidiFxHost::begin`]),
    /// with a chain edit's steps when the chain changed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_fx(
        &mut self,
        plugin: &mut dyn Plugin,
        t: &Track,
        rack: Option<&Track>,
        pos: Samples,
        transport: &Transport,
        sample_rate: f64,
        frame: u64,
        frames: usize,
    ) {
        let Some(edit) = self.fx.begin(plugin, t, rack, pos, transport, sample_rate, frame, frames) else { return };
        if edit.first {
            self.end_direct_voices(plugin);
        }
        self.fx.release_pedals(plugin);
        self.press_nothing_again();
    }

    /// The first insertion's end through the mix's own table: a note-off by `give` for every voice
    /// it gave the instrument directly. An accepted one leaves the entry for its held key with no
    /// voice (its later release sends nothing); a refused one keeps the entry's retry, marked
    /// direct, so [`Self::retry_releases`] gives it to the instrument and never to the chain, until
    /// accepted. A release already being retried is marked direct and left to its retry.
    fn end_direct_voices(&mut self, plugin: &mut dyn Plugin) {
        for n in 0..MAX_OPEN {
            let Some(e) = self.notes.get(n).copied().filter(|e| e.used && e.voice >= 0) else { continue };
            if e.retry_release.is_some() {
                if let Some(e) = self.notes.get_mut(n) {
                    e.direct = true;
                }
                continue;
            }
            let ok = self.send_via(plugin, 0, e.source.channel, e.voice, Message::NoteOff { key: e.key, velocity: 0, id: e.sender }, true);
            if let Some(e) = self.notes.get_mut(n) {
                if ok {
                    e.voice = -1;
                } else {
                    e.retry_release = Some(0);
                    e.release_queued = true;
                    e.direct = true;
                }
            }
        }
    }

    /// The edit presses nothing again: the record of the pedal state sent is up for every pedal
    /// kind, and every holder is stale (it no longer counts; a new press counts as usual).
    fn press_nothing_again(&mut self) {
        self.sent = [[0; 4]; 16];
        self.holders = [[0; 4]; 16];
        for p in self.pedals.iter_mut().filter(|p| p.used && p.down) {
            p.stale = true;
        }
    }
}

#[cfg(test)]
mod tests;
