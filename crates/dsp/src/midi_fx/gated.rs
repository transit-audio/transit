//! Test-only (feature `testing`): the gated generator, a fixture of the host slices' tests (ADR
//! 0057, decision 13, "Host slices"; slice P3a). Not in the registry and never in the app. It
//! holds the keys it is given and writes gated notes of them, and consumes a sustain pedal-down,
//! as the Arpeggiator does (5.9):
//!
//! - **Held keys.** An input note-on joins the held set and is not passed; its note-off is
//!   consumed, and the key leaves the set, or stays as pedal-held while that channel's CC 64 is
//!   at 64 or above. A note-off that closes nothing is consumed too. At most 16 keys are held.
//! - **Sustain.** A CC 64 at 64 or above is consumed (the instrument never sees it); a CC 64 below
//!   64 removes the channel's pedal-held keys and passes. Every other event passes.
//! - **Steps.** On its render clock (frames processed since its reset), at every multiple of
//!   `period` while it holds a key, it ends the note it is sounding, if any, then writes a
//!   note-on (velocity 100, no voice id) of the most recently struck held key; that note's
//!   note-off follows `gate` frames later, from its own schedule (never the host's carry). A
//!   note sounding when the set empties still ends at its gate.
//! - **Order at one frame** (2.11): the scheduled note-off due there, then the input, then the
//!   step.
//! - **Between blocks** no time passes: each input event's consequences only, at offset 0; no
//!   step, and the clock does not move.
//! - **Reset** forgets the held set, the pedals, the clock and the sounding note (the host has
//!   ended it).

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{CC_SUSTAIN, Message, NoteId, PluginEvent};
use crate::params::lin;
use crate::{Category, ParamInfo, PluginInfo, Unit};

static GATED_PARAMS: [ParamInfo; 2] =
    [lin("period", "Period", 1.0, 48_000.0, 1000.0, Unit::None), lin("gate", "Gate", 1.0, 48_000.0, 500.0, Unit::None)];

pub static GATED_INFO: PluginInfo = PluginInfo {
    id: "midi_test_gated",
    name: "Test Gated Generator",
    short_name: "Gated",
    category: Category::Other,
    params: &GATED_PARAMS,
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// Keys held at most.
const HELD: usize = 16;

#[derive(Clone, Copy, Debug, Default)]
struct Held {
    channel: u8,
    key: u8,
    voice: i32,
    /// Its note-off has not arrived.
    down: bool,
}

#[derive(Clone, Debug)]
pub struct Gated {
    period: u32,
    gate: u32,
    held: [Held; HELD],
    held_len: usize,
    pedal: [bool; 16],
    clock: u64,
    /// The note it sounds: channel, key and the render-clock frame of its note-off.
    sounding: Option<(u8, u8, u64)>,
}

impl Default for Gated {
    fn default() -> Self {
        Gated { period: 1000, gate: 500, held: [Held::default(); HELD], held_len: 0, pedal: [false; 16], clock: 0, sounding: None }
    }
}

fn note(offset: u32, channel: u8, key: u8, on: bool) -> TimedEvent {
    let message = if on { Message::NoteOn { key, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key, velocity: 0, id: NoteId::NONE } };
    TimedEvent { offset, event: PluginEvent { channel, note_id: PluginEvent::NO_ID, message } }
}

impl Gated {
    fn remove(&mut self, i: usize) {
        if i < self.held_len {
            self.held.copy_within(i + 1..self.held_len, i);
            self.held_len -= 1;
        }
    }

    fn input(&mut self, offset: u32, ev: PluginEvent, out: &mut MidiOut<'_>) {
        let ch = ev.channel & 0x0F;
        let message = match ev.message {
            Message::NoteOn { key, velocity: 0, id } => Message::NoteOff { key, velocity: 0, id },
            m => m,
        };
        match message {
            Message::NoteOn { key, .. } => {
                if let Some(h) = self.held.get_mut(self.held_len) {
                    *h = Held { channel: ch, key, voice: ev.note_id, down: true };
                    self.held_len += 1;
                }
            }
            Message::NoteOff { key, .. } => {
                let i = self.held[..self.held_len]
                    .iter()
                    .position(|h| h.down && h.channel == ch && if ev.note_id == PluginEvent::NO_ID { h.key == key } else { h.voice == ev.note_id });
                if let Some(i) = i {
                    if self.pedal[usize::from(ch)] {
                        self.held[i].down = false;
                    } else {
                        self.remove(i);
                    }
                }
            }
            Message::Control { cc: CC_SUSTAIN, value } if value >= 64 => self.pedal[usize::from(ch)] = true,
            Message::Control { cc: CC_SUSTAIN, .. } => {
                self.pedal[usize::from(ch)] = false;
                let mut i = 0;
                while i < self.held_len {
                    if self.held[i].channel == ch && !self.held[i].down {
                        self.remove(i);
                    } else {
                        i += 1;
                    }
                }
                out.push(TimedEvent { offset, event: ev });
            }
            _ => {
                out.push(TimedEvent { offset, event: ev });
            }
        }
    }
}

impl MidiEffect for Gated {
    fn info(&self) -> &'static PluginInfo {
        &GATED_INFO
    }

    fn reset(&mut self) {
        *self = Gated { period: self.period, gate: self.gate, ..Gated::default() };
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = GATED_INFO.param(id) else { return false };
        let v = p.clamp(value).round() as u32;
        match id {
            "period" => self.period = v,
            _ => self.gate = v,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "period" => Some(self.period as f32),
            "gate" => Some(self.gate as f32),
            _ => None,
        }
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, frames: u32) {
        let mut next = 0;
        for f in 0..frames {
            let at = self.clock + u64::from(f);
            if let Some((ch, key, due)) = self.sounding
                && due == at
            {
                out.push(note(f, ch, key, false));
                self.sounding = None;
            }
            while let Some(e) = input.get(next).filter(|e| e.offset <= f) {
                self.input(f, e.event, out);
                next += 1;
            }
            if at.is_multiple_of(u64::from(self.period.max(1)))
                && let Some(h) = self.held[..self.held_len].last().copied()
            {
                if let Some((ch, key, _)) = self.sounding.take() {
                    out.push(note(f, ch, key, false));
                }
                out.push(note(f, h.channel, h.key, true));
                self.sounding = Some((h.channel, h.key, at + u64::from(self.gate.max(1))));
            }
        }
        while let Some(e) = input.get(next) {
            self.input(e.offset, e.event, out);
            next += 1;
        }
        self.clock += u64::from(frames);
    }

    fn process_between(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>) {
        for e in input {
            self.input(0, e.event, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on};
    use super::*;

    fn run(g: &mut Gated, input: &[TimedEvent], frames: u32) -> Vec<TimedEvent> {
        let mut buf = [TimedEvent::EMPTY; 64];
        let mut out = MidiOut::new(&mut buf);
        g.process(input, &mut out, frames);
        out.events().to_vec()
    }

    fn pedal(offset: u32, value: u8) -> TimedEvent {
        TimedEvent { offset, event: PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message: Message::Control { cc: 64, value } } }
    }

    #[test]
    fn it_writes_gated_notes_of_a_held_key_and_consumes_the_pedal_down() {
        let mut g = Gated::default();
        assert!(g.set_param("period", 100.0) && g.set_param("gate", 40.0));
        let n = |o, on| note(o, 0, 60, on);
        // Struck at 10: steps at 100 and 200; the pedal-down at 150 is consumed; released at 180
        // under the pedal it stays held; the pedal-up at 250 passes and empties the set.
        let out = run(&mut g, &[on(10, 5, 60, 90), pedal(150, 127), off(180, 5, 60), pedal(250, 0)], 400);
        assert_eq!(out, [n(100, true), n(140, false), n(200, true), n(240, false), pedal(250, 0)]);
        // Between blocks: input only, no step and no clock.
        let mut buf = [TimedEvent::EMPTY; 8];
        let mut o = MidiOut::new(&mut buf);
        g.process_between(&[on(0, 6, 62, 90)], &mut o);
        assert!(o.is_empty());
        assert_eq!(run(&mut g, &[], 100), [note(0, 0, 62, true), note(40, 0, 62, false)], "the next step on the clock, 400");
    }
}
