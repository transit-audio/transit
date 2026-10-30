//! MIDI effects: devices that transform a track's instrument events before the instrument
//! (#201, ADR 0036). Ableton's MIDI effects sit before the instrument in the device chain; Logic's
//! MIDI FX slots sit above the instrument slot. Both run them in order, each bypassable, and so
//! does transit.
//!
//! A MIDI effect gets the events of one block, sorted by sample offset, and writes what the
//! instrument (or the next effect) should get into a fixed-capacity [`MidiOut`]. It may move, add,
//! remove or rewrite events and keep state across blocks; an event it writes at an offset at or
//! past the block's length is due in a later block, and the host carries it there. It never
//! allocates, locks or blocks: its state is sized when it is made.
//!
//! The contract an effect keeps:
//!
//! - **Pairing.** Every note-on it lets through gets its note-off: an effect that changes a key
//!   remembers what it did to each sounding note (by voice id, else by channel and key), so a
//!   parameter change while the note sounds cannot leave it hanging.
//! - **Unknown releases pass.** A note-off for a note it never saw (one that started before the
//!   effect was added) goes through unchanged.
//! - **Velocity 1..=127.** A note-on it writes has a velocity of at least 1 (0 would be an off).
//!
//! The built-ins are values ([`Builtin`]), so the audio thread can make one in place without
//! allocating. A hosted MIDI effect (a CLAP note effect) can occupy a slot later through the same
//! [`MidiEffect`] trait, made off the audio thread like any hosted plug-in.

/// A test-only effect that delays every event (feature `testing`): it proves the carry.
#[cfg(feature = "testing")]
pub mod delay;
pub mod transpose;
pub mod velocity;

use crate::event::PluginEvent;
use crate::{PluginInfo, Transport};

/// One event of a block: its sample offset from the block start and the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedEvent {
    pub offset: u32,
    pub event: PluginEvent,
}

impl TimedEvent {
    /// A placeholder for fixed storage.
    pub const EMPTY: TimedEvent = TimedEvent {
        offset: 0,
        event: PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: crate::event::Message::Program { program: 0 } },
    };
}

/// Where a MIDI effect writes its events: a fixed-capacity buffer the host owns. A full buffer
/// refuses the event ([`MidiOut::push`] returns false) and counts it.
pub struct MidiOut<'a> {
    buf: &'a mut [TimedEvent],
    len: usize,
    refused: u32,
}

impl<'a> MidiOut<'a> {
    /// An empty output over `buf` (its capacity is `buf.len()`).
    pub fn new(buf: &'a mut [TimedEvent]) -> Self {
        MidiOut { buf, len: 0, refused: 0 }
    }

    /// Appends `event`; false (counted) when the buffer is full.
    pub fn push(&mut self, event: TimedEvent) -> bool {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = event;
                self.len += 1;
                true
            }
            None => {
                self.refused = self.refused.saturating_add(1);
                false
            }
        }
    }

    /// The events written so far, in write order.
    #[must_use]
    pub fn events(&self) -> &[TimedEvent] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Events refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u32 {
        self.refused
    }
}

/// A MIDI effect (see the module documentation for the contract).
pub trait MidiEffect: Send {
    /// Its description: id, name and parameters (`is_instrument` false, never in the insert menu).
    fn info(&self) -> &'static PluginInfo;
    /// Forgets every sounding note and any state (Stop, or the chain changed).
    fn reset(&mut self);
    /// Sets a parameter (clamped into range; NaN becomes the default). `false` if `id` is unknown.
    fn set_param(&mut self, id: &str, value: f32) -> bool;
    fn param(&self, id: &str) -> Option<f32>;
    /// The transport for the next block, given before `process` (generators follow the tempo).
    fn set_transport(&mut self, _transport: &Transport) {}
    /// Transforms one block: `input` is sorted by offset, each below `frames`. Writes to `out`;
    /// an offset at or past `frames` is due in a later block.
    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, frames: u32);
}

/// The built-in MIDI effects, in menu order.
pub static REGISTRY: &[&PluginInfo] = &[&transpose::TRANSPOSE_INFO, &velocity::VELOCITY_INFO];

/// The description of built-in MIDI effect `id`.
#[must_use]
pub fn info(id: &str) -> Option<&'static PluginInfo> {
    REGISTRY.iter().copied().find(|i| i.id == id)
}

/// A built-in MIDI effect by value: made in place on the audio thread without allocating.
#[derive(Clone, Debug)]
pub enum Builtin {
    Transpose(transpose::Transpose),
    Velocity(velocity::Velocity),
    #[cfg(feature = "testing")]
    Delay(delay::Delay),
}

impl Builtin {
    /// The built-in with registry id `id`, at its defaults.
    #[must_use]
    pub fn new(id: &str) -> Option<Self> {
        if id == transpose::TRANSPOSE_INFO.id {
            Some(Builtin::Transpose(transpose::Transpose::new()))
        } else if id == velocity::VELOCITY_INFO.id {
            Some(Builtin::Velocity(velocity::Velocity::new()))
        } else {
            #[cfg(feature = "testing")]
            if id == delay::DELAY_INFO.id {
                return Some(Builtin::Delay(delay::Delay::default()));
            }
            None
        }
    }

    pub fn effect(&mut self) -> &mut dyn MidiEffect {
        match self {
            Builtin::Transpose(x) => x,
            Builtin::Velocity(x) => x,
            #[cfg(feature = "testing")]
            Builtin::Delay(x) => x,
        }
    }

    #[must_use]
    pub fn effect_ref(&self) -> &dyn MidiEffect {
        match self {
            Builtin::Transpose(x) => x,
            Builtin::Velocity(x) => x,
            #[cfg(feature = "testing")]
            Builtin::Delay(x) => x,
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::event::{Message, NoteId};

    pub(crate) fn on(offset: u32, voice: i32, key: u8, velocity: u8) -> TimedEvent {
        TimedEvent { offset, event: PluginEvent { channel: 0, note_id: voice, message: Message::NoteOn { key, velocity, id: NoteId::NONE } } }
    }

    pub(crate) fn off(offset: u32, voice: i32, key: u8) -> TimedEvent {
        TimedEvent { offset, event: PluginEvent { channel: 0, note_id: voice, message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE } } }
    }

    pub(crate) fn run(fx: &mut dyn MidiEffect, input: &[TimedEvent]) -> Vec<TimedEvent> {
        let mut buf = [TimedEvent::EMPTY; 256];
        let mut out = MidiOut::new(&mut buf);
        fx.process(input, &mut out, 256);
        out.events().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_makes_every_built_in() {
        for info in REGISTRY {
            let mut b = Builtin::new(info.id).unwrap();
            assert_eq!(b.effect().info().id, info.id);
            assert!(!info.is_instrument);
            for p in info.params {
                assert_eq!(b.effect_ref().param(p.id), Some(p.default), "{} {}", info.id, p.id);
                assert!(b.effect().set_param(p.id, f32::NAN));
                assert_eq!(b.effect_ref().param(p.id), Some(p.default), "NaN is the default");
                assert!(b.effect().set_param(p.id, f32::INFINITY));
                assert_eq!(b.effect_ref().param(p.id), Some(p.max));
            }
            assert!(!b.effect().set_param("nope", 1.0));
        }
        assert!(Builtin::new("eq7").is_none());
        assert!(REGISTRY.iter().all(|i| crate::plugins::REGISTRY.iter().all(|p| p.id != i.id)), "MIDI effect ids are their own");
    }

    #[test]
    fn a_full_output_refuses_and_counts() {
        let mut buf = [TimedEvent::EMPTY; 2];
        let mut out = MidiOut::new(&mut buf);
        let e = test_util::on(0, 1, 60, 100);
        assert!(out.push(e) && out.push(e));
        assert!(!out.push(e));
        assert_eq!((out.len(), out.refused()), (2, 1));
    }
}
