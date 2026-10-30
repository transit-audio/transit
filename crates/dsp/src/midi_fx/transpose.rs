//! Transpose: shifts note keys by whole semitones (Ableton's Pitch, Logic's Transposer).
//!
//! The shift a note-on got is remembered for that note (by voice id, else by channel and key),
//! so its note-off and poly pressure get the same shift even when the parameter moved while the
//! note sounded. A note shifted past 0 or 127 is dropped, its off too. Every other message passes.

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{Message, PluginEvent};
use crate::params::lin;
use crate::{Category, ParamInfo, PluginInfo, Unit};

static TRANSPOSE_PARAMS: [ParamInfo; 1] = [lin("semitones", "Semitones", -48.0, 48.0, 0.0, Unit::Semitones)];

pub static TRANSPOSE_INFO: PluginInfo = PluginInfo {
    id: "midi_transpose",
    name: "Transpose",
    short_name: "Transpo",
    category: Category::Other,
    params: &TRANSPOSE_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

/// Notes Transpose remembers at once: more than the mix holds open per instrument (224).
pub const TRANSPOSE_NOTES: usize = 256;

#[derive(Clone, Copy, Debug)]
struct Held {
    used: bool,
    voice: i32,
    channel: u8,
    key: u8,
    /// The shift its note-on got; `None`: the note-on was dropped (out of range).
    shift: Option<i8>,
}

const FREE: Held = Held { used: false, voice: PluginEvent::NO_ID, channel: 0, key: 0, shift: None };

#[derive(Clone, Debug)]
pub struct Transpose {
    semitones: f32,
    held: [Held; TRANSPOSE_NOTES],
    /// Note-ons dropped because the table was full (cannot happen with the mix's own limits).
    pub full: u32,
}

impl Default for Transpose {
    fn default() -> Self {
        Self::new()
    }
}

impl Transpose {
    #[must_use]
    pub fn new() -> Self {
        Transpose { semitones: 0.0, held: [FREE; TRANSPOSE_NOTES], full: 0 }
    }

    fn find(&self, ev: &PluginEvent, key: u8) -> Option<usize> {
        let ch = ev.channel & 0x0F;
        self.held.iter().position(|h| {
            h.used
                && h.channel == ch
                && if ev.note_id != PluginEvent::NO_ID { h.voice == ev.note_id } else { h.voice == PluginEvent::NO_ID && h.key == key }
        })
    }

    fn shifted(key: u8, shift: i8) -> Option<u8> {
        u8::try_from(i16::from(key) + i16::from(shift)).ok().filter(|k| *k <= 127)
    }

    fn shift(&self) -> i8 {
        // `semitones` is clamped to ±48 by `set_param`.
        self.semitones as i8
    }
}

impl MidiEffect for Transpose {
    fn info(&self) -> &'static PluginInfo {
        &TRANSPOSE_INFO
    }

    fn reset(&mut self) {
        self.held = [FREE; TRANSPOSE_NOTES];
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match TRANSPOSE_INFO.param(id) {
            Some(p) => {
                self.semitones = p.clamp(value).round();
                true
            }
            None => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        (id == "semitones").then_some(self.semitones)
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, _frames: u32) {
        for e in input {
            let mut ev = e.event;
            match ev.message {
                Message::NoteOn { key, velocity, id } => {
                    let shift = self.shift();
                    // A re-strike of a remembered note replaces its entry.
                    let slot = self.find(&ev, key).or_else(|| self.held.iter().position(|h| !h.used));
                    let Some(slot) = slot else {
                        self.full = self.full.saturating_add(1);
                        continue;
                    };
                    let to = Self::shifted(key, shift);
                    if let Some(h) = self.held.get_mut(slot) {
                        *h = Held { used: true, voice: ev.note_id, channel: ev.channel & 0x0F, key, shift: to.map(|_| shift) };
                    }
                    let Some(to) = to else { continue };
                    ev.message = Message::NoteOn { key: to, velocity, id };
                }
                Message::NoteOff { key, velocity, id } => {
                    let to = match self.find(&ev, key) {
                        Some(i) => {
                            let shift = self.held.get(i).and_then(|h| h.shift);
                            if let Some(h) = self.held.get_mut(i) {
                                *h = FREE;
                            }
                            match shift {
                                Some(s) => Self::shifted(key, s),
                                None => None,
                            }
                        }
                        // Not one of ours: it started before Transpose was there.
                        None => Some(key),
                    };
                    let Some(to) = to else { continue };
                    ev.message = Message::NoteOff { key: to, velocity, id };
                }
                Message::PolyPressure { key, value, id } => {
                    let to = match self.find(&ev, key) {
                        Some(i) => self.held.get(i).and_then(|h| h.shift).and_then(|s| Self::shifted(key, s)),
                        None => Some(key),
                    };
                    let Some(to) = to else { continue };
                    ev.message = Message::PolyPressure { key: to, value, id };
                }
                _ => {}
            }
            out.push(TimedEvent { offset: e.offset, event: ev });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on, run};
    use super::*;

    fn keys(events: &[TimedEvent]) -> Vec<(u32, &'static str, u8)> {
        events
            .iter()
            .map(|e| match e.event.message {
                Message::NoteOn { key, .. } => (e.offset, "on", key),
                Message::NoteOff { key, .. } => (e.offset, "off", key),
                Message::PolyPressure { key, .. } => (e.offset, "pp", key),
                _ => (e.offset, "other", 0),
            })
            .collect()
    }

    #[test]
    fn shifts_ons_and_their_offs_at_their_offsets() {
        let mut t = Transpose::new();
        t.set_param("semitones", 7.0);
        let out = run(&mut t, &[on(3, 1, 60, 100), off(40, 1, 60)]);
        assert_eq!(keys(&out), vec![(3, "on", 67), (40, "off", 67)]);
    }

    #[test]
    fn an_off_keeps_the_shift_its_on_got() {
        let mut t = Transpose::new();
        t.set_param("semitones", 12.0);
        let first = run(&mut t, &[on(0, 5, 60, 100)]);
        t.set_param("semitones", -12.0);
        let second = run(
            &mut t,
            &[
                TimedEvent {
                    offset: 2,
                    event: PluginEvent {
                        channel: 0,
                        note_id: 5,
                        message: Message::PolyPressure { key: 60, value: 9, id: crate::event::NoteId::NONE },
                    },
                },
                off(9, 5, 60),
                on(10, 6, 60, 90),
            ],
        );
        assert_eq!(keys(&first), vec![(0, "on", 72)]);
        assert_eq!(keys(&second), vec![(2, "pp", 72), (9, "off", 72), (10, "on", 48)]);
    }

    #[test]
    fn notes_out_of_range_are_dropped_with_their_offs() {
        let mut t = Transpose::new();
        t.set_param("semitones", 48.0);
        let out = run(&mut t, &[on(0, 1, 100, 100), on(0, 2, 79, 100), off(5, 1, 100), off(5, 2, 79)]);
        assert_eq!(keys(&out), vec![(0, "on", 127), (5, "off", 127)]);
        t.set_param("semitones", -48.0);
        let out = run(&mut t, &[on(0, 3, 47, 100), on(1, 4, 48, 100), off(2, 3, 47), off(3, 4, 48)]);
        assert_eq!(keys(&out), vec![(1, "on", 0), (3, "off", 0)]);
        assert!(t.held.iter().all(|h| !h.used), "every note was released");
    }

    #[test]
    fn an_unknown_off_passes_unchanged_and_other_messages_pass() {
        let mut t = Transpose::new();
        t.set_param("semitones", 5.0);
        let bend = TimedEvent { offset: 4, event: PluginEvent { channel: 3, note_id: -1, message: Message::PitchBend { value: 100 } } };
        let out = run(&mut t, &[off(0, 9, 60), bend]);
        assert_eq!(keys(&out), vec![(0, "off", 60), (4, "other", 0)]);
        assert_eq!(out[1], bend);
    }

    #[test]
    fn notes_without_voice_ids_match_by_channel_and_key() {
        let mut t = Transpose::new();
        t.set_param("semitones", 2.0);
        let mut b = on(0, -1, 60, 100);
        b.event.channel = 1;
        let out = run(&mut t, &[on(0, -1, 60, 100), b]);
        t.set_param("semitones", 0.0);
        let mut b_off = off(1, -1, 60);
        b_off.event.channel = 1;
        let out2 = run(&mut t, &[b_off, off(2, -1, 60)]);
        assert_eq!(keys(&out), vec![(0, "on", 62), (0, "on", 62)]);
        assert_eq!(keys(&out2), vec![(1, "off", 62), (2, "off", 62)]);
        assert_eq!(out2[0].event.channel, 1);
    }

    #[test]
    fn semitones_round_and_clamp() {
        let mut t = Transpose::new();
        t.set_param("semitones", 3.6);
        assert_eq!(t.param("semitones"), Some(4.0));
        t.set_param("semitones", -1000.0);
        assert_eq!(t.param("semitones"), Some(-48.0));
        let out = run(&mut t, &[on(0, 1, 0, 1)]);
        assert!(out.is_empty(), "key 0 down 48 is out of range");
    }
}
