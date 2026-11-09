//! Random Sequencer: random gates and pitches on the transport clock, ahead of the instrument
//! (#202). A port of Émilie Gillet's open-source random sampler (MIT; `transit-random-sequencer`,
//! nulled against her C++, ADR 0033).
//!
//! Convention: generators sit before the instrument and run on the transport clock. Ableton's MIDI
//! effects (Arpeggiator, Random) sit before the instrument in the device chain and sync to the
//! song tempo; Logic's MIDI FX (Arpeggiator, Randomizer) sit above the instrument slot and sync to
//! the project tempo. Here the clock is the beat, divided as the firmware divides a patched clock
//! (`Rate`), and the notes join the track's note stream at sample-accurate offsets.
//!
//! What it plays (see `transit_random_sequencer::Sequencer` for the full rules):
//!
//! - **Rhythm.** At each tick the model decides which of two gates fire (`Model`, `Gate Bias`),
//!   `Jitter` bends the timing, and each gate stays high for its pulse width (`Gate Length`,
//!   randomized by `Length Random`). The clock is a third gate, high for half a tick. A note lasts
//!   as long as its gate is high: the note length is the firmware's gate width, not a fixed percent.
//! - **Pitch.** Gate 1 plays the first X channel, the clock the second, gate 2 the third (`Play`
//!   picks which). Each voltage is drawn by `Spread` and `Bias` and quantized to `Scale` by `Steps`;
//!   it becomes a key at a semitone per twelfth of a volt above `Root` in `Octave`, over `Range`
//!   (2, 5 or 10 octaves up from it, or around it for 10). Microtonal scale degrees round to the
//!   nearest semitone.
//! - **Velocity.** The Y channel's voltage, drawn every tick: `Velocity` its center, `Dynamics` how
//!   far it strays.
//! - **Loop.** `Deja Vu` (the firmware's knob: random at 0, the loop locked across its middle,
//!   shuffled toward 100) over `Loop Length` decisions, on the gates, the pitches or both
//!   (`Deja Vu On`); `Freeze` locks the loop whatever the knob.
//! - **Seed.** `Seed` picks the random sequence; with `Latch` on (the default) every play start,
//!   locate and loop restarts it from the seed, so a passage plays the same notes each time, as
//!   the Texture's latch holds its seed. With `Latch` off the sequence runs on across passes, so a new `Seed` is heard only before the first play.
//! - **Input.** Incoming note-ons are swallowed unless `Thru` is on; releases and every other
//!   message pass (the contract's unknown releases).
//!
//! It allocates nothing: the sequencer is a value made with the effect.

use super::{MidiEffect, MidiOut, TimedEvent};
use crate::event::{Message, NoteId, PluginEvent};
use crate::params::{choice, lin, note_choice, toggle};
use crate::{Category, ParamInfo, PluginInfo, Transport, Unit};
use transit_random_sequencer::{Block, ControlMode, EventKind, Model, RATES, Sequencer, Settings, Voices, VoltageRange};

/// The registry id.
pub const ID: &str = "random_sequencer";

const fn grouped(group: &'static str, p: ParamInfo) -> ParamInfo {
    ParamInfo { group, ..p }
}

/// `Rate` choices: the firmware's divisions of a patched clock, the beat as the clock, slowest
/// first (as `RATES`).
pub static RATE_NAMES: [&str; 21] = [
    "4/1", "3/1", "2/1", "1/1.", "1/1", "1/2.", "1/1T", "1/2", "1/4.", "1/2T", "1/4", "1/8.", "1/4T", "1/8", "1/16.", "1/8T", "1/16", "1/16T",
    "1/32", "1/32T", "1/64",
];
/// `Model` choices: the module's six (its hidden seventh is not offered).
pub static MODEL_NAMES: [&str; 6] = ["Coin Toss", "Clusters", "Drums", "Independent", "Divider", "Three States"];
/// `Play` choices.
pub static PLAY_NAMES: [&str; 5] = ["Both Gates", "Gate 1", "Gate 2", "Clock", "All"];
/// `Scale` choices: the firmware's six and Chromatic.
pub static SCALE_NAMES: [&str; 7] = ["Major", "Minor", "Pentatonic", "Pelog", "Bhairav", "Shri", "Chromatic"];
/// `Root` choices.
pub static ROOT_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/// `Octave` choices: the octave of the root at 0 V, index `i` putting a C root at MIDI `12 * i`.
/// These are the fixed C3 = 60 numbers (what the control channel lists); the UI shows the octave
/// in its Middle C setting through [`OCTAVE_LABELS`] (ADR 0051).
pub static OCTAVES_C3: [&str; 9] = ["-2", "-1", "0", "1", "2", "3", "4", "5", "6"];
/// The Octave's labels: the octave number of MIDI `12 * i` alone.
pub const OCTAVE_LABELS: crate::NoteLabels = crate::NoteLabels { first_note: 0, step: 12, octave_only: true };
/// `Range` choices: the firmware's voltage ranges (0 to 2 V, 0 to 5 V, -5 to 5 V).
pub static RANGE_NAMES: [&str; 3] = ["2 Octaves", "5 Octaves", "10 Octaves"];
/// `Pitch Mode` choices: how the three pitch channels share spread, bias and steps.
pub static PITCH_MODE_NAMES: [&str; 3] = ["Same", "Bump", "Tilt"];
/// `Loop Length` choices: the firmware's lengths.
pub static LOOP_NAMES: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "10", "12", "14", "16"];
const LOOP_LENGTHS: [i32; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16];
/// `Deja Vu On` choices.
pub static DEJA_VU_ON_NAMES: [&str; 3] = ["Gates and Pitch", "Gates", "Pitch"];

const P_RATE: usize = 0;
const P_MODEL: usize = 1;
const P_GATE_BIAS: usize = 2;
const P_JITTER: usize = 3;
const P_GATE_LENGTH: usize = 4;
const P_LENGTH_RANDOM: usize = 5;
const P_PLAY: usize = 6;
const P_SPREAD: usize = 7;
const P_BIAS: usize = 8;
const P_STEPS: usize = 9;
const P_SCALE: usize = 10;
const P_ROOT: usize = 11;
const P_OCTAVE: usize = 12;
const P_RANGE: usize = 13;
const P_PITCH_MODE: usize = 14;
const P_DEJA_VU: usize = 15;
const P_LOOP: usize = 16;
const P_FREEZE: usize = 17;
const P_DEJA_VU_ON: usize = 18;
const P_VELOCITY: usize = 19;
const P_DYNAMICS: usize = 20;
const P_SEED: usize = 21;
const P_LATCH: usize = 22;
const P_THRU: usize = 23;
const PARAMS: usize = 24;

/// The parameters, by group (the panel's sections).
pub static RANDOM_SEQUENCER_PARAMS: [ParamInfo; PARAMS] = [
    grouped("Rhythm", choice("rate", "Rate", &RATE_NAMES, 16)),
    grouped("Rhythm", choice("model", "Model", &MODEL_NAMES, 0)),
    grouped("Rhythm", lin("gate_bias", "Gate Bias", 0.0, 100.0, 50.0, Unit::Percent)),
    grouped("Rhythm", lin("jitter", "Jitter", 0.0, 100.0, 0.0, Unit::Percent)),
    grouped("Rhythm", lin("gate_length", "Gate Length", 0.0, 100.0, 50.0, Unit::Percent)),
    grouped("Rhythm", lin("length_random", "Length Random", 0.0, 100.0, 0.0, Unit::Percent)),
    grouped("Rhythm", choice("play", "Play", &PLAY_NAMES, 0)),
    grouped("Pitch", lin("spread", "Spread", 0.0, 100.0, 50.0, Unit::Percent)),
    grouped("Pitch", lin("bias", "Bias", 0.0, 100.0, 50.0, Unit::Percent)),
    grouped("Pitch", lin("steps", "Steps", 0.0, 100.0, 50.0, Unit::Percent)),
    grouped("Pitch", choice("scale", "Scale", &SCALE_NAMES, 0)),
    grouped("Pitch", choice("root", "Root", &ROOT_NAMES, 0)),
    grouped("Pitch", note_choice("octave", "Octave", &OCTAVES_C3, 4, OCTAVE_LABELS)),
    grouped("Pitch", choice("range", "Range", &RANGE_NAMES, 0)),
    grouped("Pitch", choice("pitch_mode", "Pitch Mode", &PITCH_MODE_NAMES, 0)),
    grouped("Loop", lin("deja_vu", "Deja Vu", 0.0, 100.0, 0.0, Unit::Percent)),
    grouped("Loop", choice("loop_length", "Loop Length", &LOOP_NAMES, 7)),
    grouped("Loop", toggle("freeze", "Freeze", false)),
    grouped("Loop", choice("deja_vu_on", "Deja Vu On", &DEJA_VU_ON_NAMES, 0)),
    grouped("Velocity", lin("velocity", "Velocity", 0.0, 100.0, 60.0, Unit::Percent)),
    grouped("Velocity", lin("dynamics", "Dynamics", 0.0, 100.0, 30.0, Unit::Percent)),
    grouped("Seed", lin("seed", "Seed", 0.0, 9999.0, 1.0, Unit::None)),
    grouped("Seed", toggle("seed_latch", "Latch", true)),
    grouped("Seed", toggle("thru", "Thru", false)),
];

pub static RANDOM_SEQUENCER_INFO: PluginInfo = PluginInfo {
    id: ID,
    name: "Random Sequencer",
    short_name: "RandSeq",
    category: Category::Other,
    params: &RANDOM_SEQUENCER_PARAMS,
    is_instrument: false,
    audiosuite: false,
    pages: &[],
};

/// The credit line of the panel and the help (ADR 0033).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source random sampler (MIT)";

/// The Random Sequencer (see the module documentation).
#[derive(Clone, Debug)]
pub struct RandomSequencer {
    values: [f32; PARAMS],
    sequencer: Sequencer,
    transport: Transport,
    sample_rate: f64,
}

impl Default for RandomSequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomSequencer {
    #[must_use]
    pub fn new() -> Self {
        let values = std::array::from_fn(|i| RANDOM_SEQUENCER_PARAMS[i].default);
        let mut s = RandomSequencer { values, sequencer: Sequencer::new(1, true), transport: Transport::default(), sample_rate: 48_000.0 };
        s.sync_seed();
        s
    }

    fn v(&self, i: usize) -> f32 {
        self.values[i]
    }

    fn index(&self, i: usize, len: usize) -> usize {
        // Choices are whole numbers within range after the clamp.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let k = self.v(i).max(0.0) as usize;
        k.min(len.saturating_sub(1))
    }

    fn sync_seed(&mut self) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 to 9999.
        let seed = self.v(P_SEED).round().max(0.0) as u32;
        self.sequencer.set_seed(seed, self.v(P_LATCH) >= 0.5);
    }

    /// The parameters as the sequencer's settings.
    #[must_use]
    pub fn settings(&self) -> Settings {
        let pct = |i: usize| (self.v(i) / 100.0).clamp(0.0, 1.0);
        let octave = i32::try_from(self.index(P_OCTAVE, OCTAVES_C3.len())).unwrap_or(4) - 1;
        let root = i32::try_from(self.index(P_ROOT, ROOT_NAMES.len())).unwrap_or(0);
        let deja_vu_on = self.index(P_DEJA_VU_ON, DEJA_VU_ON_NAMES.len());
        Settings {
            rate: self.index(P_RATE, RATES.len()),
            model: Model::from_index(i32::try_from(self.index(P_MODEL, MODEL_NAMES.len())).unwrap_or(0)),
            t_bias: pct(P_GATE_BIAS),
            jitter: pct(P_JITTER),
            gate_length: pct(P_GATE_LENGTH),
            gate_random: pct(P_LENGTH_RANDOM),
            voices: match self.index(P_PLAY, PLAY_NAMES.len()) {
                1 => Voices::Gate1,
                2 => Voices::Gate2,
                3 => Voices::Clock,
                4 => Voices::All,
                _ => Voices::BothGates,
            },
            spread: pct(P_SPREAD),
            bias: pct(P_BIAS),
            steps: pct(P_STEPS),
            x_mode: match self.index(P_PITCH_MODE, PITCH_MODE_NAMES.len()) {
                1 => ControlMode::Bump,
                2 => ControlMode::Tilt,
                _ => ControlMode::Identical,
            },
            range: match self.index(P_RANGE, RANGE_NAMES.len()) {
                0 => VoltageRange::Narrow,
                1 => VoltageRange::Positive,
                _ => VoltageRange::Full,
            },
            scale: self.index(P_SCALE, SCALE_NAMES.len()),
            base_note: 12 * (octave + 1) + root,
            deja_vu: pct(P_DEJA_VU),
            length: LOOP_LENGTHS[self.index(P_LOOP, LOOP_LENGTHS.len())],
            freeze: self.v(P_FREEZE) >= 0.5,
            deja_vu_rhythm: deja_vu_on != 2,
            deja_vu_pitch: deja_vu_on != 1,
            velocity: pct(P_VELOCITY),
            dynamics: pct(P_DYNAMICS),
        }
    }
}

impl MidiEffect for RandomSequencer {
    fn info(&self) -> &'static PluginInfo {
        &RANDOM_SEQUENCER_INFO
    }

    fn reset(&mut self) {
        self.sequencer.reset();
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = RANDOM_SEQUENCER_PARAMS.iter().position(|p| p.id == id) else { return false };
        let v = RANDOM_SEQUENCER_PARAMS[i].clamp(value);
        let v = if i == P_SEED { v.round() } else { v };
        if self.values[i].to_bits() != v.to_bits() {
            self.values[i] = v;
            if i == P_SEED || i == P_LATCH {
                self.sync_seed();
            }
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        RANDOM_SEQUENCER_PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = *transport;
    }

    fn set_sample_rate(&mut self, sample_rate: f64) {
        if sample_rate > 0.0 {
            self.sample_rate = sample_rate;
        }
    }

    fn process(&mut self, input: &[TimedEvent], out: &mut MidiOut<'_>, frames: u32) {
        let thru = self.v(P_THRU) >= 0.5;
        for e in input {
            if thru || !matches!(e.event.message, Message::NoteOn { .. }) {
                out.push(*e);
            }
        }
        let t = &self.transport;
        let block = Block { playing: t.playing, samples: t.samples, quarters: t.quarters, tempo: t.tempo, sample_rate: self.sample_rate, frames };
        let settings = self.settings();
        let (dropped_before, ons_before) = (self.sequencer.dropped(), self.sequencer.dropped_ons());
        self.sequencer.process(&settings, &block, &mut |e| {
            let message = match e.kind {
                EventKind::NoteOn { key, velocity } => Message::NoteOn { key, velocity, id: NoteId::NONE },
                EventKind::NoteOff { key } => Message::NoteOff { key, velocity: 0, id: NoteId::NONE },
            };
            out.push(TimedEvent { offset: e.offset, event: PluginEvent { channel: 0, note_id: PluginEvent::NO_ID, message } });
        });
        // A full schedule (never expected) joins the chain's overflow count (`MidiFxCounts::overflow`, read through
        // `MixEngine::event_status().midi_fx.overflow`; no UI reader shows it yet).
        // Only refused note-ons are lost notes; a refused Off is retried by the sequencer, so it counts in the
        // overflow total alone.
        let ons = self.sequencer.dropped_ons() - ons_before;
        let others = (self.sequencer.dropped() - dropped_before).saturating_sub(ons);
        out.add_refused(u32::try_from(others).unwrap_or(u32::MAX));
        out.add_refused_ons(u32::try_from(ons).unwrap_or(u32::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{off, on};
    use super::*;

    fn transport(playing: bool, samples: i64) -> Transport {
        Transport {
            playing,
            tempo: 120.0,
            numerator: 4,
            denominator: 4,
            samples,
            seconds: samples as f64 / 48_000.0,
            quarters: samples as f64 * 2.0 / 48_000.0,
            ..Transport::default()
        }
    }

    fn run_blocks(fx: &mut RandomSequencer, from: i64, blocks: usize, frames: u32) -> Vec<(i64, TimedEvent)> {
        let mut all = Vec::new();
        for b in 0..blocks {
            let at = from + b as i64 * i64::from(frames);
            fx.set_transport(&transport(true, at));
            let mut buf = [TimedEvent::EMPTY; 512];
            let mut out = MidiOut::new(&mut buf);
            fx.process(&[], &mut out, frames);
            all.extend(out.events().iter().map(|e| (at + i64::from(e.offset), *e)));
        }
        all
    }

    #[test]
    fn defaults_map_to_the_sequencers_defaults() {
        let fx = RandomSequencer::new();
        let s = fx.settings();
        assert_eq!(RATE_NAMES[s.rate], "1/16");
        assert_eq!(s.base_note, 48, "an octave below middle C");
        assert_eq!((s.length, s.voices, s.scale), (8, Voices::BothGates, 0));
        assert!(s.deja_vu_rhythm && s.deja_vu_pitch && !s.freeze);
        for (i, name) in RATE_NAMES.iter().enumerate() {
            assert_eq!(*name, RATES[i].0);
        }
    }

    #[test]
    fn it_plays_on_the_transport_and_rests_when_stopped() {
        let mut fx = RandomSequencer::new();
        fx.set_sample_rate(48_000.0);
        fx.set_param("play", 3.0); // the clock: a note every sixteenth
        let ev = run_blocks(&mut fx, 0, 375, 128); // 1 s at 120 BPM: 8 sixteenths
        let ons: Vec<i64> = ev.iter().filter(|(_, e)| matches!(e.event.message, Message::NoteOn { .. })).map(|(t, _)| *t).collect();
        assert_eq!(ons, (0..8).map(|k| k * 6000).collect::<Vec<_>>());
        // Stopped: the sounding note ends, nothing more starts.
        fx.set_transport(&transport(false, 48_000));
        let mut buf = [TimedEvent::EMPTY; 64];
        let mut out = MidiOut::new(&mut buf);
        fx.process(&[], &mut out, 128);
        assert!(out.events().iter().all(|e| matches!(e.event.message, Message::NoteOff { .. })));
    }

    #[test]
    fn incoming_note_ons_are_swallowed_unless_thru() {
        let mut fx = RandomSequencer::new();
        fx.set_transport(&transport(false, 0));
        let input = [on(3, 1, 60, 100), off(9, 1, 60)];
        let mut buf = [TimedEvent::EMPTY; 8];
        let mut out = MidiOut::new(&mut buf);
        fx.process(&input, &mut out, 64);
        assert_eq!(out.events(), &[off(9, 1, 60)], "the release passes (an unknown release)");
        fx.set_param("thru", 1.0);
        let mut buf = [TimedEvent::EMPTY; 8];
        let mut out = MidiOut::new(&mut buf);
        fx.process(&input, &mut out, 64);
        assert_eq!(out.events(), &input);
    }

    /// ADR 0051: the stored index and its sound are unchanged; only the fixed labels moved to the
    /// C3 = 60 octave numbers (index 4, the default, is "2": its root is MIDI 48).
    #[test]
    fn each_octave_index_keeps_its_root_and_the_labels_are_c3_octaves() {
        for (i, label) in OCTAVES_C3.iter().enumerate() {
            let mut fx = RandomSequencer::new();
            assert!(fx.set_param("octave", i as f32));
            assert_eq!(fx.settings().base_note, 12 * i as i32, "index {i}");
            assert_eq!(*label, (i as i32 - 2).to_string());
        }
        let p = RANDOM_SEQUENCER_INFO.param("octave").unwrap();
        assert_eq!((p.default, p.format(4.0).as_str()), (4.0, "2"));
        assert_eq!(p.note_labels, Some(crate::NoteLabels { first_note: 0, step: 12, octave_only: true }));
    }

    #[test]
    fn the_seed_and_latch_replay_and_vary() {
        let play = |seed: f32| {
            let mut fx = RandomSequencer::new();
            fx.set_param("seed", seed);
            run_blocks(&mut fx, 0, 400, 256)
        };
        assert_eq!(play(5.0), play(5.0));
        assert_ne!(play(5.0), play(6.0));
    }
}
