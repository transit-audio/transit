//! Test-only (feature `testing`): the owning pass-through, a fixture of the host slices' tests
//! (ADR 0057, decision 13, "Host slices"; slice P3a). Not in the registry and never in the app.
//!
//! It owns every note it lets through, by an input ledger in the manner of decision 11, and passes
//! every note it owns unchanged (Humanize at zero amounts behaves so):
//!
//! - **Note-on.** With room in its 256 entries, it becomes an entry (channel, key, voice id and a
//!   sequence number) and passes. Without room it is rejected (not passed). A note-on of velocity
//!   0 is read as a note-off (2.2).
//! - **Note-off.** One with a voice id closes the entry of its channel with that id; one without
//!   closes the oldest anonymous entry of its channel and key. A note-off that closes an entry
//!   passes; one that closes nothing is consumed and counted (2.4). A rejected note keeps no
//!   record (decision 11's saturation rule): its anonymous note-off closes the oldest anonymous
//!   entry of its key, if any.
//! - **Everything else** (controllers, pedals, bend, pressure) passes at its offset.
//! - **Between blocks** the same, at offset 0. It has no clock and schedules nothing, so its
//!   schedule of 512 reserved entries (zero delays) is never used and is not modeled.
//! - **Reset** forgets every entry (the host has ended the notes); later releases find nothing.

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{Message, PluginEvent};
use crate::{Category, PluginInfo};

pub static OWNING_INFO: PluginInfo = PluginInfo {
    id: "midi_test_owning",
    name: "Test Owning Pass-Through",
    short_name: "Own",
    category: Category::Other,
    params: &[],
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// Entries of the ledger.
pub const OWNING_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    used: bool,
    channel: u8,
    key: u8,
    voice: i32,
    seq: u64,
}

#[derive(Clone, Debug)]
pub struct Owning {
    entries: [Entry; OWNING_ENTRIES],
    seq: u64,
    /// Note-offs that closed nothing (consumed).
    pub unmatched_releases: u64,
    /// Note-ons a full ledger rejected.
    pub rejected: u64,
}

impl Default for Owning {
    fn default() -> Self {
        Owning { entries: [Entry::default(); OWNING_ENTRIES], seq: 0, unmatched_releases: 0, rejected: 0 }
    }
}

impl Owning {
    /// Entries in use.
    #[must_use]
    pub fn open(&self) -> usize {
        self.entries.iter().filter(|e| e.used).count()
    }

    fn input(&mut self, offset: u32, ev: PluginEvent, out: &mut MidiOut<'_>) {
        let channel = ev.channel & 0x0F;
        let message = match ev.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        match message {
            Message::NoteOn { key, .. } => {
                let Some(e) = self.entries.iter_mut().find(|e| !e.used) else {
                    self.rejected = self.rejected.saturating_add(1);
                    return;
                };
                self.seq += 1;
                *e = Entry { used: true, channel, key, voice: ev.note_id, seq: self.seq };
            }
            Message::NoteOff { key, .. } => {
                let found = if ev.note_id == PluginEvent::NO_ID {
                    self.entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| e.used && e.voice == PluginEvent::NO_ID && e.channel == channel && e.key == key)
                        .min_by_key(|(_, e)| e.seq)
                        .map(|(i, _)| i)
                } else {
                    self.entries.iter().position(|e| e.used && e.channel == channel && e.voice == ev.note_id)
                };
                let Some(e) = found.and_then(|i| self.entries.get_mut(i)) else {
                    self.unmatched_releases = self.unmatched_releases.saturating_add(1);
                    return;
                };
                *e = Entry::default();
            }
            _ => {}
        }
        out.push(TimedEvent { offset, event: PluginEvent { message, ..ev } });
    }
}

impl MidiEffect for Owning {
    fn info(&self) -> &'static PluginInfo {
        &OWNING_INFO
    }

    fn reset(&mut self) {
        self.entries = [Entry::default(); OWNING_ENTRIES];
    }

    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }

    fn param(&self, _id: &str) -> Option<f32> {
        None
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, _frames: u32) {
        for e in input {
            self.input(e.offset, e.event, out);
        }
    }

    fn process_between(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>) {
        for e in input {
            self.input(0, e.event, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on, run};
    use super::*;

    #[test]
    fn it_passes_what_it_owns_and_consumes_a_release_it_does_not_own() {
        let mut o = Owning::default();
        let pedal = TimedEvent {
            offset: 3,
            event: PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: 64, value: 127 } },
        };
        let input = [on(1, 42, 60, 100), on(2, -1, 60, 90), pedal, off(4, -1, 61), off(5, 42, 60), off(6, -1, 60), off(7, -1, 60)];
        assert_eq!(run(&mut o, &input), [on(1, 42, 60, 100), on(2, -1, 60, 90), pedal, off(5, 42, 60), off(6, -1, 60)]);
        assert_eq!(o.unmatched_releases, 2, "the stray 61 and the second anonymous 60");
        assert_eq!(o.open(), 0);
        let _ = run(&mut o, &[on(0, 7, 50, 100)]);
        o.reset();
        assert!(run(&mut o, &[off(0, 7, 50)]).is_empty(), "after a reset its release finds nothing");
    }
}
