//! Shared plumbing for the built-in instruments: the event queue and its reading of plug-in
//! events, the block runner, and the ADSR the Sampler uses.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, Message, PluginEvent};
use crate::util::sort_events;
// The envelope moved to the shared instrument primitives (ADR 0041); its old path stays.
pub use transit_instrument_core::adsr::{Adsr, AdsrRates, Stage};

/// Capacity of a built-in instrument's event queue.
pub(crate) const MAX_EVENTS: usize = 1024;
/// Queue entries kept for releases: one per note and pedal entry the mix can hold open (224 +
/// 32), plus one reset (F10). Other events are refused once only these remain.
pub(crate) const RELEASE_RESERVE: usize = 257;

#[derive(Debug, Clone, Copy)]
pub(crate) enum EventKind {
    /// Channel, key, velocity, voice id (-1: none).
    On(u8, u8, u8, i32),
    /// Channel, key, voice id (-1: match by channel and key).
    Off(u8, u8, i32),
    AllOff,
}

impl EventKind {
    /// What must never be refused: releases (see [`Message::is_release`]).
    fn is_release(&self) -> bool {
        matches!(self, EventKind::Off(..) | EventKind::AllOff)
    }

    /// The built-in instruments' reading of a plug-in event: notes and the all-off channel-mode
    /// messages. Anything else (pedals, bend, other controllers) is `None` (ignored).
    pub(crate) fn from_event(ev: &PluginEvent) -> Option<EventKind> {
        let ch = ev.channel & 0x0F;
        Some(match ev.message {
            Message::NoteOn { key, velocity, .. } => EventKind::On(ch, key.min(127), velocity.min(127), ev.note_id),
            Message::NoteOff { key, .. } => EventKind::Off(ch, key.min(127), ev.note_id),
            Message::Control { cc: CC_ALL_SOUND_OFF | CC_ALL_NOTES_OFF, .. } => EventKind::AllOff,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Event {
    pub(crate) offset: usize,
    pub(crate) kind: EventKind,
}

/// Fixed-capacity event queue (never reallocates on the audio thread). The last
/// [`RELEASE_RESERVE`] entries only take releases, so a note-off is never refused while notes
/// fill the queue.
#[derive(Debug)]
pub(crate) struct EventQueue {
    ev: Vec<Event>,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self { ev: Vec::with_capacity(MAX_EVENTS) }
    }

    /// Queues an event; `false` when it was refused (never logs: this runs on the audio thread).
    pub(crate) fn push(&mut self, offset: usize, kind: EventKind) -> bool {
        let room = if kind.is_release() { MAX_EVENTS } else { MAX_EVENTS - RELEASE_RESERVE };
        if self.ev.len() < room {
            self.ev.push(Event { offset, kind });
            true
        } else {
            false
        }
    }

    /// Takes the queued events, sorted by offset (stable), leaving the queue empty. Give the
    /// vector back with [`EventQueue::restore`] so its capacity is reused (no allocation).
    pub(crate) fn take_sorted(&mut self) -> Vec<Event> {
        let mut ev = std::mem::take(&mut self.ev);
        sort_events(&mut ev, |e| e.offset);
        ev
    }

    /// Hands back the vector [`EventQueue::take_sorted`] took, emptied (`apply` never queues, so
    /// nothing was queued meanwhile).
    pub(crate) fn restore(&mut self, mut ev: Vec<Event>) {
        ev.clear();
        self.ev = ev;
    }

    pub(crate) fn clear(&mut self) {
        self.ev.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ev.len()
    }
}

/// Runs a block's events against `frames` samples: `apply` for each event when its offset is
/// reached, `render` once per sample. Events past the block take effect at its last sample;
/// for an empty block, at once.
pub(crate) fn run_block<P>(
    p: &mut P,
    events: &[Event],
    frames: usize,
    mut apply: impl FnMut(&mut P, EventKind),
    mut render: impl FnMut(&mut P, usize),
) {
    let mut next = 0;
    for n in 0..frames {
        while let Some(e) = events.get(next) {
            if e.offset > n && n + 1 < frames {
                break;
            }
            apply(p, e.kind);
            next += 1;
        }
        render(p, n);
    }
    for e in events.iter().skip(next) {
        apply(p, e.kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(i: usize) -> EventKind {
        EventKind::On((i % 16) as u8, (i % 128) as u8, 100, i as i32)
    }

    fn off(i: usize) -> EventKind {
        EventKind::Off((i % 16) as u8, (i % 128) as u8, i as i32)
    }

    #[test]
    fn every_admitted_note_on_gets_its_off_through_a_full_queue() {
        let mut q = EventQueue::new();
        for _ in 0..700 {
            assert!(q.push(0, EventKind::AllOff));
        }
        let ons: Vec<bool> = (0..300).map(|i| q.push(1, on(i))).collect();
        let offs: Vec<bool> = (0..300).map(|i| q.push(2, off(i))).collect();
        let admitted = ons.iter().filter(|a| **a).count();
        assert_eq!(admitted, MAX_EVENTS - RELEASE_RESERVE - 700, "non-releases stop at the reserve");
        for (i, (a, b)) in ons.iter().zip(&offs).enumerate() {
            assert!(!*a || *b, "note {i} was admitted but its off was refused");
        }
        assert_eq!(q.len(), MAX_EVENTS);
        assert!(!q.push(3, on(999)), "a full queue refuses a note-on");
    }

    #[test]
    fn only_notes_and_all_off_reach_the_queue() {
        let ev = |message| PluginEvent { channel: 0x13, note_id: 7, message };
        assert!(matches!(
            EventKind::from_event(&ev(Message::NoteOn { key: 200, velocity: 100, id: crate::event::NoteId::NONE })),
            Some(EventKind::On(3, 127, 100, 7))
        ));
        assert!(matches!(EventKind::from_event(&ev(Message::Control { cc: CC_ALL_NOTES_OFF, value: 0 })), Some(EventKind::AllOff)));
        assert!(EventKind::from_event(&ev(Message::Control { cc: 64, value: 127 })).is_none(), "pedals are not read");
        assert!(EventKind::from_event(&ev(Message::PitchBend { value: 9000 })).is_none(), "bend is not read");
    }

    #[test]
    fn run_block_applies_events_at_their_offsets() {
        let mut q = EventQueue::new();
        q.push(2, on(0));
        q.push(9, off(0));
        let ev = q.take_sorted();
        // State: (last frame rendered, (that frame, is a note-on) for each event applied).
        let mut state: (usize, Vec<(usize, bool)>) = (0, Vec::new());
        run_block(
            &mut state,
            &ev,
            4,
            |s, k| {
                let at = s.0;
                s.1.push((at, matches!(k, EventKind::On(..))));
            },
            |s, n| s.0 = n,
        );
        // The note-on applies just before frame 2 renders; the one due past the block, just before
        // the block's last frame (3).
        assert_eq!(state.1, vec![(1, true), (2, false)]);
    }
}
