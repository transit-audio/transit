//! Test-only (feature `testing`): a generator on the render clock and a latch, the fixture the
//! host slices' tests drive (ADR 0057, slice P2). Not in the registry and never in the app.
//!
//! - **Steps.** While the transport plays it takes a step at every render frame, counted since its
//!   reset, that is a multiple of `period` (with `period` 0, one step at the first playing frame):
//!   a step ends the note the last step started and starts `key`, so from the first step on it
//!   always holds a note. While stopped it takes no step and keeps its note.
//! - **Latch.** With `latch` on, an input note-on passes and the note-off that closes it is
//!   consumed: the note is held until the host ends it (a latched chord). Without it, input passes.
//! - **Between blocks** no time passes: each input event gets its own consequence at offset 0; no
//!   step is taken and the render clock does not move.
//! - **Reset** forgets the clock, the step note and the latched notes (the host has ended them).

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{Message, NoteId, PluginEvent};
use crate::params::{lin, toggle};
use crate::{Category, ParamInfo, PluginInfo, Transport, Unit};

static GENERATOR_PARAMS: [ParamInfo; 3] =
    [lin("period", "Period", 0.0, 48_000.0, 0.0, Unit::None), lin("key", "Key", 0.0, 127.0, 60.0, Unit::None), toggle("latch", "Latch", false)];

pub static GENERATOR_INFO: PluginInfo = PluginInfo {
    id: "midi_test_generator",
    name: "Test Generator",
    short_name: "Gen",
    category: Category::Other,
    params: &GENERATOR_PARAMS,
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// Latched notes held at most.
const LATCHED: usize = 16;

/// A latched input note: its channel, key and voice id, and whether its note-off is still to come.
#[derive(Clone, Copy, Debug, Default)]
struct Latched {
    channel: u8,
    key: u8,
    voice: i32,
    open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Generator {
    period: u32,
    key: u8,
    latch: bool,
    playing: bool,
    /// Frames processed since the reset (the render clock).
    clock: u64,
    /// A step was taken since the reset.
    stepped: bool,
    /// The key the last step started.
    sounding: Option<u8>,
    latched: [Latched; LATCHED],
    latched_len: usize,
}

impl Generator {
    fn note(offset: u32, key: u8, on: bool) -> TimedEvent {
        let message =
            if on { Message::NoteOn { key, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key, velocity: 0, id: NoteId::NONE } };
        TimedEvent { offset, event: PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message } }
    }

    /// One input event at `offset`: passed, or (a latched note's note-off) consumed.
    fn input(&mut self, e: &TimedEvent, offset: u32, out: &mut MidiOut<'_>) {
        let ev = e.event;
        match ev.message {
            Message::NoteOn { key, .. } if self.latch && self.latched_len < LATCHED => {
                self.latched[self.latched_len] = Latched { channel: ev.channel, key, voice: ev.note_id, open: true };
                self.latched_len += 1;
            }
            Message::NoteOff { key, .. } if self.latch => {
                let mine = self.latched[..self.latched_len].iter_mut().find(|l| {
                    l.open && l.channel == ev.channel && if ev.note_id == PluginEvent::NO_ID { l.key == key } else { l.voice == ev.note_id }
                });
                if let Some(l) = mine {
                    l.open = false;
                    return;
                }
            }
            _ => {}
        }
        out.push(TimedEvent { offset, event: ev });
    }

    fn step(&mut self, offset: u32, out: &mut MidiOut<'_>) {
        if let Some(key) = self.sounding.take() {
            out.push(Self::note(offset, key, false));
        }
        out.push(Self::note(offset, self.key, true));
        self.sounding = Some(self.key);
        self.stepped = true;
    }
}

impl MidiEffect for Generator {
    fn info(&self) -> &'static PluginInfo {
        &GENERATOR_INFO
    }

    fn reset(&mut self) {
        *self = Generator { period: self.period, key: self.key, latch: self.latch, ..Generator::default() };
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = GENERATOR_INFO.param(id) else { return false };
        let v = p.clamp(value).round();
        match id {
            "period" => self.period = v as u32,
            "key" => self.key = v as u8,
            _ => self.latch = v >= 0.5,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "period" => Some(self.period as f32),
            "key" => Some(f32::from(self.key)),
            "latch" => Some(if self.latch { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.playing = transport.playing;
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, frames: u32) {
        for e in input {
            self.input(e, e.offset, out);
        }
        if self.playing {
            if self.period == 0 {
                if !self.stepped {
                    self.step(0, out);
                }
            } else {
                let period = u64::from(self.period);
                let end = self.clock + u64::from(frames);
                let mut at = self.clock.div_ceil(period) * period;
                while at < end {
                    self.step((at - self.clock) as u32, out);
                    at += period;
                }
            }
        }
        self.clock += u64::from(frames);
    }

    fn process_between(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>) {
        for e in input {
            self.input(e, 0, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on};
    use super::*;

    fn run(g: &mut Generator, input: &[TimedEvent], frames: u32) -> Vec<TimedEvent> {
        let mut buf = [TimedEvent::EMPTY; 64];
        let mut out = MidiOut::new(&mut buf);
        g.process(input, &mut out, frames);
        out.events().to_vec()
    }

    #[test]
    fn it_steps_on_the_render_clock_only_while_playing_and_latches_input() {
        let mut g = Generator::default();
        assert!(g.set_param("period", 100.0) && g.set_param("key", 62.0) && g.set_param("latch", 1.0));
        g.set_transport(&Transport { playing: true, ..Transport::default() });
        let steps = run(&mut g, &[on(5, 1, 40, 90)], 250);
        let n = |o, on| Generator::note(o, 62, on);
        assert_eq!(steps, [on(5, 1, 40, 90), n(0, true), n(100, false), n(100, true), n(200, false), n(200, true)]);
        // The latched note's release is consumed; an unknown one passes; between blocks no step.
        let mut buf = [TimedEvent::EMPTY; 8];
        let mut out = MidiOut::new(&mut buf);
        g.process_between(&[off(0, 1, 40), off(0, 2, 41)], &mut out);
        assert_eq!(out.events(), &[off(0, 2, 41)]);
        assert_eq!(run(&mut g, &[], 60), [n(50, false), n(50, true)], "the clock did not move between blocks");
        g.set_transport(&Transport::default());
        assert!(run(&mut g, &[], 500).is_empty(), "stopped: no step, the note is kept");
    }
}
