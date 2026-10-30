//! Velocity: scales and offsets note-on velocities, then clamps them between a minimum and a
//! maximum (Ableton's Velocity, Logic's Velocity Processor). Note-offs and every other message
//! pass unchanged.
//!
//! `out = clamp(round(in × scale / 100 + offset), min, max)`, and never below 1 (a note-on stays a
//! note-on). A minimum above the maximum is read as the same range the other way round.

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::Message;
use crate::params::lin;
use crate::{Category, ParamInfo, PluginInfo, Unit};

static VELOCITY_PARAMS: [ParamInfo; 4] = [
    lin("scale", "Scale", 0.0, 400.0, 100.0, Unit::Percent),
    lin("offset", "Offset", -127.0, 127.0, 0.0, Unit::None),
    lin("min", "Minimum", 1.0, 127.0, 1.0, Unit::None),
    lin("max", "Maximum", 1.0, 127.0, 127.0, Unit::None),
];

pub static VELOCITY_INFO: PluginInfo = PluginInfo {
    id: "midi_velocity",
    name: "Velocity",
    short_name: "Velocity",
    category: Category::Other,
    params: &VELOCITY_PARAMS,
    is_instrument: false,
    audiosuite: false,
};

#[derive(Clone, Debug)]
pub struct Velocity {
    /// Percent.
    scale: f32,
    offset: f32,
    min: f32,
    max: f32,
}

impl Default for Velocity {
    fn default() -> Self {
        Self::new()
    }
}

impl Velocity {
    #[must_use]
    pub fn new() -> Self {
        Velocity { scale: 100.0, offset: 0.0, min: 1.0, max: 127.0 }
    }

    /// The velocity a note-on with velocity `v` leaves with.
    #[must_use]
    pub fn map(&self, v: u8) -> u8 {
        let (lo, hi) = if self.min <= self.max { (self.min, self.max) } else { (self.max, self.min) };
        let out = (f32::from(v) * self.scale / 100.0 + self.offset).round().clamp(lo, hi).clamp(1.0, 127.0);
        // In 1..=127 after the clamps.
        out as u8
    }
}

impl MidiEffect for Velocity {
    fn info(&self) -> &'static PluginInfo {
        &VELOCITY_INFO
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = VELOCITY_INFO.param(id) else { return false };
        let v = p.clamp(value);
        match id {
            "scale" => self.scale = v,
            "offset" => self.offset = v.round(),
            "min" => self.min = v.round(),
            "max" => self.max = v.round(),
            _ => return false,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "scale" => Some(self.scale),
            "offset" => Some(self.offset),
            "min" => Some(self.min),
            "max" => Some(self.max),
            _ => None,
        }
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, _frames: u32) {
        for e in input {
            let mut ev = *e;
            if let Message::NoteOn { key, velocity, id } = ev.event.message {
                ev.event.message = Message::NoteOn { key, velocity: self.map(velocity), id };
            }
            out.push(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on, run};
    use super::*;

    fn vel(events: &[TimedEvent]) -> Vec<u8> {
        events
            .iter()
            .filter_map(|e| match e.event.message {
                Message::NoteOn { velocity, .. } => Some(velocity),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn defaults_are_transparent() {
        let mut v = Velocity::new();
        let input: Vec<_> = (1..=127u8).map(|k| on(u32::from(k), i32::from(k), 60, k)).chain([off(200, 1, 60)]).collect();
        assert_eq!(run(&mut v, &input), input);
    }

    #[test]
    fn scale_offset_and_clamps() {
        let mut v = Velocity::new();
        v.set_param("scale", 50.0);
        v.set_param("offset", 10.0);
        assert_eq!(vel(&run(&mut v, &[on(0, 1, 60, 100), on(1, 2, 60, 1), on(2, 3, 60, 127)])), vec![60, 11, 74]);
        v.set_param("min", 40.0);
        v.set_param("max", 70.0);
        assert_eq!(vel(&run(&mut v, &[on(0, 1, 60, 100), on(1, 2, 60, 1), on(2, 3, 60, 127)])), vec![60, 40, 70]);
    }

    #[test]
    fn the_extremes_never_make_an_off_or_overflow() {
        let mut v = Velocity::new();
        v.set_param("scale", 0.0);
        v.set_param("offset", -127.0);
        assert_eq!(vel(&run(&mut v, &[on(0, 1, 60, 127)])), vec![1], "silence is still a note-on");
        v.set_param("scale", 400.0);
        v.set_param("offset", 127.0);
        assert_eq!(vel(&run(&mut v, &[on(0, 1, 60, 127), on(0, 2, 60, 1)])), vec![127, 127]);
        // Min above max: the same range the other way round.
        v.set_param("scale", 100.0);
        v.set_param("offset", 0.0);
        v.set_param("min", 90.0);
        v.set_param("max", 30.0);
        assert_eq!(vel(&run(&mut v, &[on(0, 1, 60, 10), on(0, 2, 60, 60), on(0, 3, 60, 120)])), vec![30, 60, 90]);
        // Offs keep their release velocity.
        let mut o = off(3, 1, 60);
        o.event.message = Message::NoteOff { key: 60, velocity: 77, id: crate::event::NoteId::NONE };
        assert_eq!(run(&mut v, &[o]), vec![o]);
    }
}
