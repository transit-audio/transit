//! The note stream an instrument was given in a block, kept for the track's inserts that read it
//! (#242, ADR 0038).
//!
//! Every event the mix gives an instrument leaves through one of two calls: the note table's
//! `send` when the track has no MIDI effects, or the MIDI effect host's `deliver` (and its
//! end-of-chain releases) when it has (ADR 0036). Both record what the plug-in *accepted* here:
//! note-ons, note-offs, the sustain pedal and the all-notes-off and all-sound-off controllers, at
//! the offsets the instrument got them. After the instrument processes, the strip hands the view to
//! every insert whose [`Plugin::reads_track_notes`](transit_dsp::Plugin::reads_track_notes) is true,
//! then clears it. An event given between blocks (a Stop's or a seek's releases, at offset 0) waits
//! here for the next block, at offset 0.
//!
//! Fixed storage, made with the instrument's event state: nothing allocates on the audio thread.
//! A block holds at most [`NOTE_VIEW_CAP`] events. When it is full, the last entry becomes an
//! all-notes-off and later events are dropped (counted in
//! [`MidiFxCounts::note_view_dropped`](crate::midi_fx::MidiFxCounts)): an insert that lost events
//! releases everything rather than holding a note forever.

use transit_dsp::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_SUSTAIN, Message, PluginEvent};
use transit_dsp::midi_fx::TimedEvent;

/// Events one block's view holds at most (the most a block takes into the MIDI effect chain).
pub const NOTE_VIEW_CAP: usize = 512;

/// The all-notes-off that stands in for events a full view dropped.
const ALL_OFF: PluginEvent = PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: CC_ALL_NOTES_OFF, value: 0 } };

pub(crate) struct NoteView {
    events: [TimedEvent; NOTE_VIEW_CAP],
    len: usize,
    /// The view overflowed this block: its last entry is the stand-in all-notes-off.
    full: bool,
}

/// Whether an insert reading notes cares about `m`.
fn is_note_stream(m: &Message) -> bool {
    matches!(m, Message::NoteOn { .. } | Message::NoteOff { .. } | Message::Control { cc: CC_SUSTAIN | CC_ALL_NOTES_OFF | CC_ALL_SOUND_OFF, .. })
}

impl NoteView {
    pub(crate) fn new() -> Self {
        NoteView { events: [TimedEvent::EMPTY; NOTE_VIEW_CAP], len: 0, full: false }
    }

    /// Records an event the instrument accepted at `offset`. Returns the number of events dropped
    /// (0 or 1) for the caller's counter.
    pub(crate) fn record(&mut self, offset: u32, ev: &PluginEvent) -> u64 {
        if !is_note_stream(&ev.message) {
            return 0;
        }
        if self.full {
            return 1;
        }
        if self.len + 1 >= NOTE_VIEW_CAP {
            // The last slot: everything from here on is lost, so the insert releases all it holds.
            if let Some(slot) = self.events.get_mut(self.len) {
                *slot = TimedEvent { offset, event: ALL_OFF };
                self.len += 1;
            }
            self.full = true;
            return 1;
        }
        if let Some(slot) = self.events.get_mut(self.len) {
            *slot = TimedEvent { offset, event: *ev };
            self.len += 1;
        }
        0
    }

    /// The block's events in delivery order (offsets never decrease within a block's delivery,
    /// except that the chain's own carried events and retries come first at their offsets).
    pub(crate) fn events(&self) -> &[TimedEvent] {
        self.events.get(..self.len).unwrap_or_default()
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.full = false;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use transit_dsp::event::NoteId;

    fn on(key: u8) -> PluginEvent {
        PluginEvent { channel: 0, note_id: i32::from(key), message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE } }
    }

    #[test]
    fn only_the_note_stream_is_kept_in_order() {
        let mut v = NoteView::new();
        assert_eq!(v.record(3, &on(60)), 0);
        let bend = PluginEvent { channel: 0, note_id: -1, message: Message::PitchBend { value: 9000 } };
        assert_eq!(v.record(4, &bend), 0);
        let pedal = PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: CC_SUSTAIN, value: 127 } };
        assert_eq!(v.record(5, &pedal), 0);
        let got: Vec<u32> = v.events().iter().map(|e| e.offset).collect();
        assert_eq!(got, vec![3, 5], "the bend is not part of the note stream");
        v.clear();
        assert!(v.events().is_empty());
    }

    #[test]
    fn a_full_view_ends_with_an_all_notes_off_and_counts_the_rest() {
        let mut v = NoteView::new();
        let mut dropped = 0;
        for i in 0..NOTE_VIEW_CAP + 10 {
            dropped += v.record(u32::try_from(i % 256).unwrap(), &on((i % 128) as u8));
        }
        assert_eq!(v.events().len(), NOTE_VIEW_CAP);
        assert_eq!(dropped, 11, "the stand-in's own event and the ten after it");
        let last = v.events().last().unwrap();
        assert!(matches!(last.event.message, Message::Control { cc: CC_ALL_NOTES_OFF, .. }));
        v.clear();
        assert_eq!(v.record(0, &on(1)), 0, "a cleared view takes events again");
    }
}
