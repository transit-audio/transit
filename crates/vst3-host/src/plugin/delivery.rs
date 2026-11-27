//! The VST3 adapter's events under ADR 0059: each queued event of the delivery queue in its
//! native form for one `process` call.

use super::{MAP_LEN, Vst3Plugin};
use crate::ffi::{AFTERTOUCH, NoteEvent, PITCH_BEND, ParamPoint};
use transit_dsp::event::Message;
use transit_dsp::plugins::instrument_queue::delivery::{Item, Refusals};

impl Vst3Plugin {
    /// A MIDI controller (`ctrl`: a CC, [`AFTERTOUCH`] or [`PITCH_BEND`]) on `channel` at `time`:
    /// a point on the parameter the plug-in maps it to (an unmapped controller drives nothing).
    pub(super) fn control_point(&mut self, time: u32, channel: u8, ctrl: usize, value: f64) {
        // A table re-read on the main thread (`kMidiCCAssignmentChanged`) applies from this
        // controller on.
        self.refresh_midi_map();
        let Some(&Some(id)) = self.midi_map.get(usize::from(channel & 0x0F) * MAP_LEN + ctrl) else { return };
        if self.points.len() < self.points.capacity() {
            self.seq = self.seq.wrapping_add(1);
            self.points.push(ParamPoint { time, id, class: 1, seq: self.seq, value, release: false });
        } else {
            self.untranslated = self.untranslated.saturating_add(1);
        }
    }

    /// One queued event in the native form: notes and poly pressure to the event list with the
    /// entry's native id, controllers to points on the parameters the plug-in maps them to. A
    /// note-on with velocity 0 is the release the ledger matched it as: a native note-off.
    pub(super) fn translate(&mut self, it: &Item) {
        let ch = it.event.channel & 0x0F;
        let unit = |v: u8| f32::from(v.min(127)) / 127.0;
        let time = it.offset;
        let message = match it.event.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        let note = match message {
            Message::NoteOn { key, velocity, .. } if key <= 127 => {
                Some(NoteEvent::On { time, key, velocity: unit(velocity), channel: ch, id: it.native })
            }
            Message::NoteOff { key, velocity, .. } if key <= 127 => {
                Some(NoteEvent::Off { time, key, velocity: unit(velocity), channel: ch, id: it.native })
            }
            Message::PolyPressure { key, value, .. } if key <= 127 => {
                Some(NoteEvent::Pressure { time, key, pressure: unit(value), channel: ch, id: it.native })
            }
            Message::Control { cc, value } if cc <= 127 => {
                self.control_point(time, ch, usize::from(cc), f64::from(value.min(127)) / 127.0);
                None
            }
            Message::Control14 { msb, value } if msb < 32 => {
                self.control_point(time, ch, usize::from(msb), f64::from(value.min(16383)) / 16383.0);
                None
            }
            Message::ChannelPressure { value } => {
                self.control_point(time, ch, AFTERTOUCH, f64::from(value.min(127)) / 127.0);
                None
            }
            Message::PitchBend { value } => {
                self.control_point(time, ch, PITCH_BEND, f64::from(value.min(16383)) / 16383.0);
                None
            }
            _ => None,
        };
        if let Some(n) = note {
            if self.notes.len() < self.notes.capacity() {
                self.notes.push(n);
            } else {
                self.untranslated = self.untranslated.saturating_add(1);
            }
        }
    }

    /// Native events translation could not place so far (an invariant: always zero; every
    /// buffer is sized for all admission permits).
    pub fn untranslated(&self) -> u64 {
        self.untranslated
    }

    /// Refusals at admission so far (ADR 0059: starts at a full ledger, ordinary events at a full
    /// allowance, safety controllers forwarded only in part).
    pub fn refusals(&self) -> Refusals {
        self.delivery.refusals
    }
}
