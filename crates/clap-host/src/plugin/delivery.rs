//! The CLAP adapter's events under ADR 0059: each queued event of the delivery queue in its native
//! form for one `process` call, and raw MIDI 1.0 bytes decoded for admission.

use super::{ClapPlugin, NoteMode, parameter_queue};
use crate::ffi::Event;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::plugins::instrument_queue::delivery::Refusals;

impl ClapPlugin {
    /// Item `j` of the current batch in the port's dialect, inserted after the parameter events at
    /// its time: CLAP note events carry the entry's native id; a MIDI-dialect port gets the bytes,
    /// one note-off for each note-on (ADR 0059: the count is kept; MIDI bytes cannot say which
    /// voice of a key an off ends). Translation changes no ownership: the call's outcome does
    /// (`Delivery::finish`). Every buffer is sized for all admission permits, so nothing is dropped
    /// here; a drop would be counted in `untranslated`.
    pub(super) fn translate(&mut self, j: usize) {
        let Some(it) = self.delivery.items().get(j).copied() else { return };
        let ch = it.event.channel & 0x0F;
        let time = it.offset;
        let unit = |v: u8| f64::from(v.min(127)) / 127.0;
        let message = match it.event.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        let mut put = |queue: &mut Vec<Event>, e: Event| {
            if !parameter_queue::insert(queue, e) {
                self.untranslated = self.untranslated.saturating_add(1);
            }
        };
        match (message, self.notes) {
            (Message::NoteOn { key, velocity, .. }, NoteMode::Clap) => {
                put(&mut self.queue, Event::NoteOn { time, key, velocity: unit(velocity), channel: ch, id: it.native });
            }
            (Message::NoteOff { key, velocity, .. }, NoteMode::Clap) => {
                put(&mut self.queue, Event::NoteOff { time, key, velocity: unit(velocity), channel: ch, id: it.native });
            }
            (Message::PolyPressure { key, value, .. }, NoteMode::Clap) => {
                put(&mut self.queue, Event::Pressure { time, key, value: unit(value), channel: ch, id: it.native });
            }
            (m, _) if self.midi_in => {
                for data in m.midi1(ch).into_iter().flatten() {
                    put(&mut self.queue, Event::Midi { time, data });
                }
            }
            _ => {}
        }
    }

    /// Native events translation could not place so far (an invariant: always zero).
    pub fn untranslated(&self) -> u64 {
        self.untranslated
    }

    /// Refusals at admission so far (ADR 0059: starts at a full ledger, ordinary events at a full
    /// allowance, safety controllers forwarded only in part).
    pub fn refusals(&self) -> Refusals {
        self.delivery.refusals
    }
}

/// A raw MIDI 1.0 channel message as an event on its channel (`None` for anything else).
pub(super) fn from_midi1(msg: [u8; 3]) -> Option<PluginEvent> {
    let [status, d1, d2] = msg;
    let (key, value) = (d1 & 0x7F, d2 & 0x7F);
    let id = NoteId::NONE;
    let message = match status & 0xF0 {
        0x80 => Message::NoteOff { key, velocity: value, id },
        0x90 if value == 0 => Message::NoteOff { key, velocity: 0, id },
        0x90 => Message::NoteOn { key, velocity: value, id },
        0xA0 => Message::PolyPressure { key, value, id },
        0xB0 => Message::Control { cc: key, value },
        0xC0 => Message::Program { program: key },
        0xD0 => Message::ChannelPressure { value: key },
        0xE0 => Message::PitchBend { value: u16::from(value) << 7 | u16::from(key) },
        _ => return None,
    };
    Some(PluginEvent { channel: status & 0x0F, note_id: PluginEvent::NO_ID, message })
}

/// Native events an ordinary host event becomes at a CLAP port: a 14-bit controller is two MIDI
/// messages (MSB, then LSB); everything else one.
pub(super) fn expansion(m: &Message) -> u8 {
    if matches!(m, Message::Control14 { .. }) { 2 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_midi_decodes_onto_its_channel() {
        let ev = from_midi1([0x93, 60, 0]).unwrap();
        assert_eq!((ev.channel, ev.message), (3, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE }));
        assert_eq!(from_midi1([0xB5, 64, 127]).unwrap().message, Message::Control { cc: 64, value: 127 });
        assert_eq!(from_midi1([0xE0, 0, 64]).unwrap().message, Message::PitchBend { value: 8192 });
        assert!(from_midi1([0xF0, 0, 0]).is_none());
    }
}
