//! Test-only (feature `testing`): the owning pass-through, a fixture of the host slices' tests
//! (ADR 0057, decision 13, "Host slices"; slices P3a and P3b). Not in the registry and never in
//! the app.
//!
//! It owns every note it lets through, by an input ledger as decision 11 states (`ledger`), and
//! passes every note it owns unchanged, with a schedule of 512 reserved entries (Humanize at zero
//! amounts behaves so, 2.8):
//!
//! - **Note-on.** With a free ledger entry and a free reservation it becomes an Open entry and
//!   passes, its note-off's reservation held until that note-off is written. Otherwise it is
//!   rejected (decision 11: an anonymous rejection is recorded, so its note-off is consumed in its
//!   oldest-first place). A note-on of velocity 0 is read as a note-off (2.2).
//! - **Its own output refuses the note-on** (`MidiOut::push` returns false, final under slice
//!   P3b): the output note is Cancelled with its release, its reservation freed, the entry
//!   Orphaned (2.8, Humanize's column), counted in `cancelled`.
//! - **Note-off.** It closes an entry by decision 11's rule. An Open entry's note-off passes and
//!   frees the reservation; an Orphaned entry's, a rejection record's and one that closes nothing
//!   are consumed (the last counted, 2.4). A refused note-off is not retried: at the host a
//!   release refused by a full buffer triggers the reset barrier, which resets this fixture.
//! - **Everything else** (controllers, pedals, bend, pressure) passes at its offset.
//! - **Between blocks** the same, at offset 0. Its delays are zero, so the schedule only reserves.
//! - **Reset** forgets every entry, record and reservation (the host has ended the notes).

use super::ledger::{Closed, Ledger};
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
pub const OWNING_ENTRIES: usize = super::ledger::LEDGER_ENTRIES;
/// Reserved entries of the schedule.
pub const OWNING_SCHEDULE: usize = 512;

#[derive(Clone, Debug, Default)]
pub struct Owning {
    ledger: Ledger<()>,
    /// Note-offs reserved in the schedule (one per passed note whose note-off is not written).
    reserved: usize,
    /// Note-offs that closed nothing (consumed).
    pub unmatched_releases: u64,
    /// Note-ons rejected (a full ledger or schedule).
    pub rejected: u64,
    /// Note-ons its own output refused (Cancelled with their releases).
    pub cancelled: u64,
}

impl Owning {
    /// Entries in use (Open or Orphaned).
    #[must_use]
    pub fn open(&self) -> usize {
        self.ledger.open()
    }

    /// The most entries in use at once since it was made.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.ledger.peak
    }

    /// Note-offs held in the schedule's reservations.
    #[must_use]
    pub fn reserved(&self) -> usize {
        self.reserved
    }

    /// Anonymous rejection records in use.
    #[must_use]
    pub fn records(&self) -> usize {
        self.ledger.records()
    }

    fn input(&mut self, offset: u32, ev: PluginEvent, out: &mut MidiOut<'_>) {
        let channel = ev.channel & 0x0F;
        let message = match ev.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        let event = TimedEvent { offset, event: PluginEvent { message, ..ev } };
        match message {
            Message::NoteOn { key, .. } => {
                let Some(i) = self.ledger.admit(channel, key, ev.note_id, (), self.reserved < OWNING_SCHEDULE) else {
                    self.rejected = self.ledger.rejected;
                    return;
                };
                self.reserved += 1;
                if !out.push(event) {
                    self.reserved -= 1;
                    self.ledger.orphan(i);
                    self.cancelled = self.cancelled.saturating_add(1);
                }
            }
            Message::NoteOff { key, .. } => match self.ledger.close(channel, key, ev.note_id) {
                Closed::Open(()) => {
                    self.reserved = self.reserved.saturating_sub(1);
                    out.push(event);
                }
                Closed::Nothing => self.unmatched_releases = self.ledger.unmatched_releases,
                Closed::Orphaned | Closed::Record => {}
            },
            _ => {
                out.push(event);
            }
        }
    }
}

impl MidiEffect for Owning {
    fn info(&self) -> &'static PluginInfo {
        &OWNING_INFO
    }

    fn reset(&mut self) {
        self.ledger.reset();
        self.reserved = 0;
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
        assert_eq!((o.open(), o.reserved()), (0, 0));
        let _ = run(&mut o, &[on(0, 7, 50, 100)]);
        assert_eq!(o.reserved(), 1, "its note-off's reservation is held");
        o.reset();
        assert!(run(&mut o, &[off(0, 7, 50)]).is_empty(), "after a reset its release finds nothing");
        assert_eq!(o.reserved(), 0);
    }

    #[test]
    fn a_note_on_its_output_refuses_is_cancelled_and_its_release_consumed() {
        // 2.8, Humanize's column (the owning pass-through is Humanize at zero amounts): the output
        // note Cancelled with its release, the entry Orphaned.
        let mut o = Owning::default();
        let mut buf = [TimedEvent::EMPTY; 1];
        let mut out = MidiOut::new(&mut buf);
        o.process(&[on(0, -1, 60, 100), on(1, 5, 62, 100)], &mut out, 64);
        assert_eq!(out.events(), [on(0, -1, 60, 100)]);
        assert_eq!((o.cancelled, o.open(), o.reserved()), (1, 2, 1));
        assert_eq!(run(&mut o, &[off(0, 5, 62), off(1, -1, 60)]), [off(1, -1, 60)], "the cancelled note's release is consumed");
        assert_eq!((o.open(), o.reserved(), o.unmatched_releases), (0, 0, 0));
    }

    #[test]
    fn a_full_ledger_records_an_anonymous_rejection() {
        let mut o = Owning::default();
        let ons: Vec<_> = (0..=OWNING_ENTRIES).map(|_| on(0, -1, 60, 100)).collect();
        let mut buf = [TimedEvent::EMPTY; 512];
        let mut out = MidiOut::new(&mut buf);
        o.process(&ons, &mut out, 64);
        assert_eq!((out.len(), o.rejected, o.records(), o.peak()), (OWNING_ENTRIES, 1, 1, OWNING_ENTRIES));
    }
}
