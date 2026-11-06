//! The core path's event queue: the host's events converted to the core's
//! [`Event`]s into a fixed array of [`MAX_EVENTS`], with the built-in instruments' release reserve
//! (the last [`RELEASE_RESERVE`] entries take only releases, so a note-off is never refused while
//! notes fill the queue, and a refusal returns `false` for the mix to count, as before). At the
//! block the array is sorted by frame in place (stable: events at one offset keep their order) and
//! lent to the core as one span. Nothing allocates after construction.

use transit_sampler_core::event::{Control, Event, EventKind};

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, CC_SUSTAIN, Message, PluginEvent};
use crate::plugins::instrument_queue::{MAX_EVENTS, RELEASE_RESERVE};
use crate::util::sort_events;

/// What a host event asks of the core path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Ingress {
    /// One core event.
    Core(EventKind),
    /// Reset All Controllers (CC 121): the pedal lifts, the wheel, bend and pressure go to rest, at
    /// the next block's start.
    ResetControllers,
}

/// Whether the core event is in the release class (never refused while the reserve lasts).
fn is_release(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::NoteOff { .. }
            | EventKind::NoteOn { velocity: 0, .. }
            | EventKind::AllNotesOff
            | EventKind::AllSoundOff
            | EventKind::Stop
            | EventKind::Reset
            | EventKind::Control(Control::Sustain(false))
    )
}

fn unit7(v: u8) -> f32 {
    f32::from(v.min(127)) / 127.0
}

fn bend(value: u16) -> f32 {
    (f32::from(value.min(16383)) - 8192.0) / 8192.0
}

fn from_cc(cc: u8, value: u8) -> Option<Ingress> {
    Some(match cc {
        1 => Ingress::Core(EventKind::Control(Control::ModWheel(unit7(value)))),
        CC_SUSTAIN => Ingress::Core(EventKind::Control(Control::Sustain(value >= 64))),
        CC_ALL_SOUND_OFF => Ingress::Core(EventKind::AllSoundOff),
        CC_ALL_NOTES_OFF => Ingress::Core(EventKind::AllNotesOff),
        CC_RESET_CONTROLLERS => Ingress::ResetControllers,
        _ => return None,
    })
}

/// The core path's reading of a plug-in event: notes with their channel, voice id and release
/// velocity, the sustain pedal, the mod wheel (7- or 14-bit), pitch bend, channel and poly
/// pressure, All Sound Off, All Notes Off and Reset All Controllers. Anything else is `None`.
pub(super) fn from_event(ev: &PluginEvent) -> Option<Ingress> {
    let channel = ev.channel & 0x0F;
    let id = ev.note_id;
    Some(match ev.message {
        Message::NoteOn { key, velocity, .. } => Ingress::Core(EventKind::NoteOn { channel, key: key.min(127), velocity: velocity.min(127), id }),
        Message::NoteOff { key, velocity, .. } => Ingress::Core(EventKind::NoteOff { channel, key: key.min(127), velocity: velocity.min(127), id }),
        Message::PolyPressure { key, value, .. } => {
            Ingress::Core(EventKind::Control(Control::PolyAftertouch { key: key.min(127), value: unit7(value) }))
        }
        Message::ChannelPressure { value } => Ingress::Core(EventKind::Control(Control::Aftertouch(unit7(value)))),
        Message::Control { cc, value } => from_cc(cc, value)?,
        Message::Control14 { msb: 1, value } => Ingress::Core(EventKind::Control(Control::ModWheel(f32::from(value.min(16383)) / 16383.0))),
        Message::PitchBend { value } => Ingress::Core(EventKind::Control(Control::PitchBend(bend(value)))),
        _ => return None,
    })
}

/// The same reading for raw MIDI 1.0 bytes ([`crate::Plugin::midi`]).
pub(super) fn from_midi(msg: [u8; 3]) -> Option<Ingress> {
    let [status, d1, d2] = msg;
    let channel = status & 0x0F;
    let (key, value) = (d1 & 0x7F, d2 & 0x7F);
    Some(match status & 0xF0 {
        0x80 => Ingress::Core(EventKind::NoteOff { channel, key, velocity: value, id: -1 }),
        0x90 => Ingress::Core(EventKind::NoteOn { channel, key, velocity: value, id: -1 }),
        0xA0 => Ingress::Core(EventKind::Control(Control::PolyAftertouch { key, value: unit7(value) })),
        0xB0 => from_cc(key, value)?,
        0xD0 => Ingress::Core(EventKind::Control(Control::Aftertouch(unit7(key)))),
        0xE0 => Ingress::Core(EventKind::Control(Control::PitchBend(bend(u16::from(value) << 7 | u16::from(key))))),
        _ => return None,
    })
}

/// The controller rest state Reset All Controllers applies.
pub(super) const RESET_CONTROLLERS: [Event; 4] = [
    Event { frame: 0, kind: EventKind::Control(Control::Sustain(false)) },
    Event { frame: 0, kind: EventKind::Control(Control::ModWheel(0.0)) },
    Event { frame: 0, kind: EventKind::Control(Control::PitchBend(0.0)) },
    Event { frame: 0, kind: EventKind::Control(Control::Aftertouch(0.0)) },
];

/// The fixed queue of core events.
pub(super) struct CoreQueue {
    ev: Box<[Event; MAX_EVENTS]>,
    len: usize,
}

impl CoreQueue {
    /// Allocates the array (off the audio thread).
    pub(super) fn new() -> CoreQueue {
        CoreQueue { ev: Box::new([Event::new(0, EventKind::Reset); MAX_EVENTS]), len: 0 }
    }

    /// Queues `kind` at `offset`; `false` when refused (the reserve keeps the last entries for
    /// releases). Never logs or allocates.
    pub(super) fn push(&mut self, offset: usize, kind: EventKind) -> bool {
        let room = if is_release(&kind) { MAX_EVENTS } else { MAX_EVENTS - RELEASE_RESERVE };
        if self.len >= room {
            return false;
        }
        let frame = u32::try_from(offset).unwrap_or(u32::MAX);
        match self.ev.get_mut(self.len) {
            Some(slot) => {
                *slot = Event::new(frame, kind);
                self.len += 1;
                true
            }
            None => false,
        }
    }

    /// Sorts the queued events by frame (stable) and returns them as one span.
    pub(super) fn sorted(&mut self) -> &[Event] {
        let span = self.ev.get_mut(..self.len).unwrap_or_default();
        sort_events(span, |e| e.frame as usize);
        span
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.len
    }
}
