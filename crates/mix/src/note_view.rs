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
//! A block holds at most [`NOTE_VIEW_CAP`] events, and the last [`RELEASE_RESERVE`] slots are kept
//! for releases (note-offs, the pedal coming up, the all-notes-off and all-sound-off controllers),
//! as the Filter's own queue keeps a quarter of its room for them (#246). When the rest is full,
//! the next entry becomes an all-notes-off and later note-ons and pedal-downs are dropped (counted
//! in [`MidiFxCounts::note_view_dropped`](crate::midi_fx::MidiFxCounts)): an insert that lost
//! events releases everything rather than holding a note forever, and the releases that follow
//! still reach it until the reserve is spent too. The final slot is always kept for that
//! all-notes-off: a release that would take it becomes the stand-in instead, so whenever anything
//! was dropped, the view holds one.

use transit_dsp::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_SUSTAIN, Message, PluginEvent};
use transit_dsp::midi_fx::TimedEvent;

/// Events one block's view holds at most (the most a block takes into the MIDI effect chain).
pub const NOTE_VIEW_CAP: usize = 512;

/// Slots only releases may fill: a dense block of note-ons never crowds out the note-offs after it.
pub const RELEASE_RESERVE: usize = 64;

/// The all-notes-off that stands in for events a full view dropped.
const ALL_OFF: PluginEvent = PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: CC_ALL_NOTES_OFF, value: 0 } };

pub(crate) struct NoteView {
    events: [TimedEvent; NOTE_VIEW_CAP],
    len: usize,
    /// The non-release slots filled this block: an all-notes-off stands in for the events dropped
    /// from there on.
    full: bool,
}

/// Whether an insert reading notes cares about `m`.
fn is_note_stream(m: &Message) -> bool {
    matches!(m, Message::NoteOn { .. } | Message::NoteOff { .. } | Message::Control { cc: CC_SUSTAIN | CC_ALL_NOTES_OFF | CC_ALL_SOUND_OFF, .. })
}

/// Whether `m` lets go of something, so it may use the reserved slots.
fn is_release(m: &Message) -> bool {
    match m {
        Message::NoteOff { .. } => true,
        Message::Control { cc: CC_ALL_NOTES_OFF | CC_ALL_SOUND_OFF, .. } => true,
        Message::Control { cc: CC_SUSTAIN, value } => *value < 64,
        _ => false,
    }
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
        let release = is_release(&ev.message);
        if self.full && !release {
            return 1;
        }
        if release && !self.full && self.len + 1 >= NOTE_VIEW_CAP {
            // The final slot is always the stand-in's: a release that would take it is lost, so
            // the insert lets go of everything instead of holding the note it names.
            return self.stand_in(offset);
        }
        if !release && self.len + 1 >= NOTE_VIEW_CAP - RELEASE_RESERVE {
            // The last open slot: note-ons from here on are lost, so the insert releases all it holds.
            return self.stand_in(offset);
        }
        let Some(slot) = self.events.get_mut(self.len) else { return 1 };
        *slot = TimedEvent { offset, event: *ev };
        self.len += 1;
        0
    }

    /// Writes the all-notes-off that stands in for the event being dropped; counts that event.
    fn stand_in(&mut self, offset: u32) -> u64 {
        if let Some(slot) = self.events.get_mut(self.len) {
            *slot = TimedEvent { offset, event: ALL_OFF };
            self.len += 1;
        }
        self.full = true;
        1
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

    fn off(key: u8) -> PluginEvent {
        PluginEvent { channel: 0, note_id: i32::from(key), message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE } }
    }

    #[test]
    fn a_full_view_stands_in_an_all_notes_off_and_counts_the_note_ons_after_it() {
        let open = NOTE_VIEW_CAP - RELEASE_RESERVE;
        let mut v = NoteView::new();
        let mut dropped = 0;
        for i in 0..NOTE_VIEW_CAP {
            dropped += v.record(u32::try_from(i % 256).unwrap(), &on((i % 128) as u8));
        }
        assert_eq!(v.events().len(), open, "the open slots, the last of them the stand-in");
        assert_eq!(dropped, (NOTE_VIEW_CAP - open + 1) as u64, "the stand-in's own event and every note-on after it");
        let last = v.events().last().unwrap();
        assert!(matches!(last.event.message, Message::Control { cc: CC_ALL_NOTES_OFF, .. }));
        v.clear();
        assert_eq!(v.record(0, &on(1)), 0, "a cleared view takes events again");
    }

    #[test]
    fn a_note_off_after_a_dense_block_of_note_ons_is_kept() {
        let mut v = NoteView::new();
        let mut dropped = 0;
        for i in 0..600 {
            dropped += v.record(u32::try_from(i % 256).unwrap(), &on((i % 128) as u8));
        }
        let before = v.events().len();
        assert_eq!(v.record(255, &off(60)), 0, "a release takes a reserved slot");
        assert_eq!(v.events().len(), before + 1);
        let last = v.events().last().unwrap();
        assert!(matches!(last.event.message, Message::NoteOff { key: 60, .. }));
        assert_eq!(dropped, 600 - (NOTE_VIEW_CAP - RELEASE_RESERVE) as u64 + 1);
    }

    #[test]
    fn the_reserve_holds_exactly_the_releases_then_drops_the_rest() {
        let mut v = NoteView::new();
        for i in 0..NOTE_VIEW_CAP {
            v.record(0, &on((i % 128) as u8));
        }
        let mut dropped = 0;
        for i in 0..RELEASE_RESERVE + 5 {
            dropped += v.record(1, &off((i % 128) as u8));
        }
        assert_eq!(v.events().len(), NOTE_VIEW_CAP, "capacity is unchanged");
        assert_eq!(dropped, 5, "releases past the reserve are the only ones lost");
        let pedal_up = PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: CC_SUSTAIN, value: 0 } };
        assert_eq!(v.record(2, &pedal_up), 1, "and the view is full");
    }

    #[test]
    fn a_lost_release_leaves_the_all_notes_off_stand_in() {
        let mut v = NoteView::new();
        let mut dropped = 0;
        for i in 0..400 {
            dropped += v.record(0, &on((i % 128) as u8));
        }
        for i in 0..112 {
            dropped += v.record(1, &off((i % 100) as u8));
        }
        dropped += v.record(2, &off(120));
        assert_eq!(v.events().len(), NOTE_VIEW_CAP);
        assert_eq!(dropped, 2, "the release that took the final slot and the one after it");
        let last = v.events().last().unwrap();
        assert!(matches!(last.event.message, Message::Control { cc: CC_ALL_NOTES_OFF, .. }), "the insert is told to let go of everything");
    }
}
