//! Macro Oscillator: transit's macro oscillator (`transit-macro-osc`, ADR 0033, #195), a Rust
//! port of Émilie Gillet's MIT-licensed eurorack firmware (24 synthesis models), played from MIDI
//! as a polyphonic instrument of up to 16 voices (#196), or as a mono one with legato.
//!
//! - Voices (`voices`, 1 to 16, default 8): each is one firmware voice ([`voice::Slot`]) with its
//!   own random generator, seeded from the instance's seed plus its index, so an export plays the
//!   same bits every time. A note-on takes, in order: the voice already playing that key (held,
//!   held by the pedal, or in its release tail), a free voice (the lowest-numbered idle one), the
//!   quietest voice in its release tail (the oldest of equally quiet ones), else the oldest held
//!   voice. The newest note always sounds. A note-off releases its voice, whose low-pass gate tail
//!   plays on until the voice is silent; only then is it free (idle and not rendered). Voices above
//!   a lowered `voices` count release and play out their tails. No unison here (racks, ADR 0034).
//! - Mono (`voices` 1): the instrument of #195. The held note is the voice's V/OCT input, a note-on
//!   raises its trigger input and sets its level input from the velocity, a note-off lowers both
//!   and the low-pass gate's decay is the release. Last-note priority and legato: a note played
//!   while another is held changes pitch without a new trigger (the module's trigger input stays
//!   high, so its envelopes are not restarted); releasing the newest note returns to the most
//!   recent one still held.
//! - Every voice: velocity is the level input. Sustain (CC 64) holds released notes; pitch bend
//!   spans `bend_range` semitones each way; the mod wheel (CC 1) adds up to 100% to the
//!   destination `mod_wheel` chooses. The note-to-macro mapping (`velocity_*`, `key_*`, see
//!   [`voice::Mapping`]) adds the note's velocity and key to Harmonics, Timbre, Morph and Decay with
//!   a depth and a curve; all depths are 0 by default.
//! - Blocks: the voices render 12 frames at a time (the firmware's block, which its envelopes are
//!   timed for), whatever the host's block size; frames rendered past the end of a host block wait
//!   in a 12-frame buffer for the next one, so any host block size gives the same output bit for
//!   bit as 12-frame host blocks. Events take effect at the first 12-frame boundary at or after
//!   their offset (at most 0.25 ms late at 48 kHz), the resolution of the module's own inputs.
//! - Output: each voice's int16 frames are read as -1..1 floats ([`to_f32`]: polarity restored,
//!   silence exactly 0, full scale one LSB under 0 dBFS), crossfaded between the main and
//!   auxiliary outputs by `aux_mix`, scaled by `level`, summed in voice order and written to every
//!   channel. One voice at `level` 0 dB peaks within 0.0003 dB of 0 dBFS; voices add, so chords
//!   want a lower level (the -6 dB default leaves 6 dB of headroom for one voice).
//! - Tuning: concert pitch (A4 = 440 Hz, `transit_macro_osc::Tuning::Concert`), as the module plays:
//!   its audio clock runs at the firmware's corrected rate. The firmware's arithmetic, 4.6 cents sharp
//!   at a true 48 kHz, stays bit-exact in the crate for the null tests only.
//! - Rates: the voices run at 44.1, 48, 88.2 or 96 kHz. At any other host rate they run at the
//!   nearest of those and their pitch is corrected to the host rate; their envelopes and engine
//!   timbres then run faster or slower by the rate ratio.
//!
//! Parameters only: the voices' state is not saved, as for every built-in.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, CC_SUSTAIN, Message, PluginEvent};
use crate::params::{Params, choice, lin, param_plumbing};
use crate::plugins::synth::{MAX_EVENTS, RELEASE_RESERVE};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr, sort_events};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use transit_macro_osc::SampleRate;
use transit_macro_osc::utils::random::Random;

#[path = "macro_osc_voice.rs"]
pub mod voice;

use voice::{CURVES, Mapping, Shared, Slot, TARGETS};

/// The voice's block: its envelopes advance once per render call, timed for 12 frames.
pub const BLOCK: usize = 12;

/// The models in the firmware's order (its engine index), named by what they do.
pub const MODELS: [&str; 24] = [
    "Filtered analog",
    "Phase distortion",
    "Six-op FM 1",
    "Six-op FM 2",
    "Six-op FM 3",
    "Wave terrain",
    "String machine",
    "Chiptune",
    "Virtual analog",
    "Waveshaper",
    "FM",
    "Grain",
    "Additive",
    "Wavetable",
    "Chord",
    "Speech",
    "Swarm",
    "Filtered noise",
    "Particle noise",
    "Inharmonic string",
    "Modal",
    "Bass drum",
    "Snare",
    "Hi-hat",
];

/// Where the mod wheel goes (`mod_wheel`).
pub const WHEEL_DESTINATIONS: [&str; 4] = ["Timbre", "Harmonics", "Morph", "Off"];

const MODEL: usize = 0;
const HARMONICS: usize = 1;
const TIMBRE: usize = 2;
const MORPH: usize = 3;
const DECAY: usize = 4;
const LPG_COLOR: usize = 5;
const AUX_MIX: usize = 6;
const LEVEL: usize = 7;
const TUNE: usize = 8;
const BEND_RANGE: usize = 9;
const MOD_WHEEL: usize = 10;
const VOICES: usize = 11;
const VELOCITY_CURVE: usize = 12;
/// The first of the four velocity depths (Harmonics, Timbre, Morph, Decay).
const VELOCITY_DEPTH: usize = 13;
const KEY_CURVE: usize = 17;
/// The first of the four key depths.
const KEY_DEPTH: usize = 18;

/// The most voices an instance plays (`voices`), all built with the instance.
pub const MAX_VOICES: usize = 16;

/// `voices` choices: the voice count is the index plus one.
pub const VOICE_COUNTS: [&str; MAX_VOICES] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

static PARAMS: [ParamInfo; 22] = [
    choice("model", "Model", &MODELS, 8),
    lin("harmonics", "Harmonics", 0.0, 100.0, 50.0, Unit::Percent),
    lin("timbre", "Timbre", 0.0, 100.0, 50.0, Unit::Percent),
    lin("morph", "Morph", 0.0, 100.0, 50.0, Unit::Percent),
    lin("decay", "Decay", 0.0, 100.0, 50.0, Unit::Percent),
    lin("lpg_color", "LPG Color", 0.0, 100.0, 50.0, Unit::Percent),
    lin("aux_mix", "Aux Mix", 0.0, 100.0, 0.0, Unit::Percent),
    lin("level", "Level", -48.0, 6.0, -6.0, Unit::Db),
    lin("tune", "Tune", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("bend_range", "Bend Range", 0.0, 24.0, 2.0, Unit::Semitones),
    choice("mod_wheel", "Mod Wheel", &WHEEL_DESTINATIONS, 0),
    choice("voices", "Voices", &VOICE_COUNTS, 7),
    choice("velocity_curve", "Velocity Curve", &CURVES, 0),
    lin("velocity_harmonics", "Velocity to Harmonics", -100.0, 100.0, 0.0, Unit::Percent),
    lin("velocity_timbre", "Velocity to Timbre", -100.0, 100.0, 0.0, Unit::Percent),
    lin("velocity_morph", "Velocity to Morph", -100.0, 100.0, 0.0, Unit::Percent),
    lin("velocity_decay", "Velocity to Decay", -100.0, 100.0, 0.0, Unit::Percent),
    choice("key_curve", "Key Curve", &CURVES, 0),
    lin("key_harmonics", "Key to Harmonics", -100.0, 100.0, 0.0, Unit::Percent),
    lin("key_timbre", "Key to Timbre", -100.0, 100.0, 0.0, Unit::Percent),
    lin("key_morph", "Key to Morph", -100.0, 100.0, 0.0, Unit::Percent),
    lin("key_decay", "Key to Decay", -100.0, 100.0, 0.0, Unit::Percent),
];

pub static MACRO_OSC_INFO: PluginInfo = PluginInfo {
    id: "macro_oscillator",
    name: "Macro Oscillator",
    short_name: "MacroOsc",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    audiosuite: false,
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    On(u8, u8),
    Off(u8),
    AllOff,
    Sustain(bool),
    /// -1..1.
    Bend(f32),
    /// 0..1.
    Wheel(f32),
    ResetControllers,
}

impl Kind {
    fn is_release(self) -> bool {
        matches!(self, Kind::Off(_) | Kind::AllOff | Kind::Sustain(false) | Kind::ResetControllers)
    }

    /// Omni: every channel plays the instrument.
    fn from_event(ev: &PluginEvent) -> Option<Kind> {
        Some(match ev.message {
            Message::NoteOn { key, velocity, .. } => Kind::On(key.min(127), velocity.min(127)),
            Message::NoteOff { key, .. } => Kind::Off(key.min(127)),
            Message::Control { cc, value } => Kind::from_cc(cc, value)?,
            Message::Control14 { msb: 1, value } => Kind::Wheel(f32::from(value.min(16383)) / 16383.0),
            Message::PitchBend { value } => Kind::bend(value),
            _ => return None,
        })
    }

    fn from_cc(cc: u8, value: u8) -> Option<Kind> {
        Some(match cc {
            1 => Kind::Wheel(f32::from(value.min(127)) / 127.0),
            CC_SUSTAIN => Kind::Sustain(value >= 64),
            CC_ALL_SOUND_OFF | CC_ALL_NOTES_OFF => Kind::AllOff,
            CC_RESET_CONTROLLERS => Kind::ResetControllers,
            _ => return None,
        })
    }

    fn bend(value: u16) -> Kind {
        Kind::Bend((f32::from(value.min(16383)) - 8192.0) / 8192.0)
    }

    fn from_midi(msg: [u8; 3]) -> Option<Kind> {
        let [status, d1, d2] = msg;
        match status & 0xF0 {
            0xB0 => Kind::from_cc(d1 & 0x7F, d2 & 0x7F),
            0xE0 => Some(Kind::bend(u16::from(d2 & 0x7F) << 7 | u16::from(d1 & 0x7F))),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Event {
    offset: usize,
    kind: Kind,
}

/// The voice rate for a host rate: the supported rate nearest on a log scale.
fn voice_rate(host: f32) -> SampleRate {
    let dist = |r: SampleRate| (r.hz() / host).ln().abs();
    SampleRate::ALL.into_iter().min_by(|a, b| dist(*a).total_cmp(&dist(*b))).unwrap_or_default()
}

/// One int16 output sample as a float in -1..1. The firmware writes `clip16(1 + trunc(y * -32767))`
/// (inverted for its DAC, plus 1 against the truncation), so the polarity is restored and the 1
/// taken off: silence (1) is exactly 0. Full scale is one LSB under 0 dBFS: y = +1 reads
/// 32767 / 32768 (-0.0003 dB), y = -1 reads -32766 / 32768 (-0.0005 dB, the clip at +32767 absorbs
/// the offset), and only the -32768 clip, reached by y beyond +1, reads 32769 / 32768 (+0.0003 dB).
#[inline]
pub fn to_f32(sample: i16) -> f32 {
    (1.0 - f32::from(sample)) / 32768.0
}

/// The macro oscillator as a MIDI instrument of 1 to 16 voices. Its random seed is the firmware's
/// power-on seed: voice `i` starts from `Random::DEFAULT_SEED + i`, so every instance plays the
/// same noise and a mono instance plays as the instrument of #195.
pub struct MacroOscillator {
    p: Params,
    /// Every voice the instance can play, built with it; the first `voices` take new notes.
    slots: Vec<Slot>,
    voices: usize,
    rate: SampleRate,
    host_sr: f32,
    /// Semitones that correct the voices' pitch from their rate to the host's (0 at the four rates).
    rate_offset: f32,
    ch: usize,
    events: Vec<Event>,
    /// The current 12-frame block, summed and scaled; `pos` frames of it were written out.
    block: [f32; BLOCK],
    pos: usize,
    level: Smoother,
    sustain: bool,
    bend: f32,
    wheel: f32,
    /// Phrases started (the voices' ages).
    counter: u64,
}

impl Default for MacroOscillator {
    fn default() -> Self {
        Self::new()
    }
}

impl MacroOscillator {
    pub fn new() -> Self {
        let rate = SampleRate::Hz48000;
        let seed = Random::DEFAULT_SEED;
        let mut s = Self {
            p: Params::new(&PARAMS),
            slots: (0..MAX_VOICES).map(|i| Slot::new(rate, voice_seed(seed, i))).collect(),
            voices: 1,
            rate,
            host_sr: rate.hz(),
            rate_offset: 0.0,
            ch: 2,
            events: Vec::with_capacity(MAX_EVENTS),
            block: [0.0; BLOCK],
            pos: BLOCK,
            level: Smoother::new(0.5),
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            counter: 0,
        };
        s.update();
        s.level.snap();
        s
    }

    /// Sets the instance's random seed (voice `i` starts from `seed + i`) and resets: a test
    /// compares one voice of an instance with a mono instance seeded as that voice.
    #[cfg(test)]
    fn set_seed(&mut self, seed: u32) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            s.set_seed(voice_seed(seed, i));
        }
        self.reset();
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(LEVEL)));
        let voices = (self.p.choice(VOICES) + 1).clamp(1, MAX_VOICES);
        if voices != self.voices {
            // Voices above the new count take no new notes: they release and play out.
            for s in self.slots.iter_mut().skip(voices) {
                s.notes.clear();
            }
            self.voices = voices;
        }
    }

    fn push(&mut self, offset: usize, kind: Kind) -> bool {
        let room = if kind.is_release() { MAX_EVENTS } else { MAX_EVENTS - RELEASE_RESERVE };
        if self.events.len() < room {
            self.events.push(Event { offset, kind });
            true
        } else {
            false
        }
    }

    /// The voice a new note on `key` takes (several voices only): the one playing that key, a free
    /// one, the quietest releasing one (the oldest of equals), else the oldest.
    fn allocate(&self, key: u8) -> usize {
        let active = self.slots.get(..self.voices).unwrap_or(&[]);
        let found = active.iter().position(|s| s.plays(key)).or_else(|| active.iter().position(|s| s.idle));
        let releasing = || {
            active
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.sounding())
                .min_by(|(_, a), (_, b)| a.loudness.total_cmp(&b.loudness).then(a.age.cmp(&b.age)))
                .map(|(i, _)| i)
        };
        let oldest = || active.iter().enumerate().min_by_key(|(_, s)| s.age).map(|(i, _)| i);
        found.or_else(releasing).or_else(oldest).unwrap_or(0)
    }

    fn apply(&mut self, kind: Kind) {
        match kind {
            Kind::On(key, 0) => self.apply(Kind::Off(key)),
            Kind::On(key, vel) => {
                self.counter += 1;
                let age = self.counter;
                if self.voices == 1 {
                    if let Some(s) = self.slots.first_mut() {
                        s.on(key, vel, age);
                    }
                } else {
                    let i = self.allocate(key);
                    if let Some(s) = self.slots.get_mut(i) {
                        // A new phrase on this voice: a stolen voice drops its key first.
                        s.notes.clear();
                        s.on(key, vel, age);
                    }
                }
            }
            Kind::Off(key) => {
                for s in &mut self.slots {
                    s.off(key, self.sustain);
                }
            }
            Kind::AllOff => {
                for s in &mut self.slots {
                    s.notes.clear();
                }
                self.sustain = false;
            }
            Kind::Sustain(on) => {
                self.sustain = on;
                if !on {
                    for s in &mut self.slots {
                        s.release_pedalled();
                    }
                }
            }
            Kind::Bend(b) => self.bend = b,
            Kind::Wheel(w) => self.wheel = w,
            Kind::ResetControllers => {
                self.apply(Kind::Sustain(false));
                self.bend = 0.0;
                self.wheel = 0.0;
            }
        }
    }

    /// Applies the events due at frame `n` of this host block, in order.
    fn apply_due(&mut self, n: usize) {
        let due = self.events.iter().take_while(|e| e.offset <= n).count();
        for i in 0..due {
            if let Some(e) = self.events.get(i).copied() {
                self.apply(e.kind);
            }
        }
        self.events.drain(..due);
    }

    /// The note-to-macro mapping as the parameters set it.
    fn mapping(&self) -> Mapping {
        let depths = |first: usize| std::array::from_fn::<f32, TARGETS, _>(|i| self.p.v(first + i) / 100.0);
        Mapping {
            velocity_curve: self.p.choice(VELOCITY_CURVE),
            velocity: depths(VELOCITY_DEPTH),
            key_curve: self.p.choice(KEY_CURVE),
            key: depths(KEY_DEPTH),
        }
    }

    /// Renders the next 12 frames of every voice not idle and sums them into `block`.
    fn render_block(&mut self) {
        let v = |i| self.p.v(i) / 100.0;
        let offset = self.p.v(TUNE) + self.bend * self.p.v(BEND_RANGE) + self.rate_offset;
        let shared = Shared {
            knobs: [v(HARMONICS), v(TIMBRE), v(MORPH), v(DECAY)],
            wheel: match self.p.choice(MOD_WHEEL) {
                0 => Some((1, self.wheel)),
                1 => Some((0, self.wheel)),
                2 => Some((2, self.wheel)),
                _ => None,
            },
            offset: if offset.is_finite() { offset.clamp(-96.0, 96.0) } else { 0.0 },
            engine: self.p.choice(MODEL).min(MODELS.len() - 1),
            lpg_color: v(LPG_COLOR),
            aux_mix: v(AUX_MIX),
            mapping: self.mapping(),
            quiet_blocks: (self.rate.hz() * voice::QUIET_SECONDS / BLOCK as f32) as u32,
        };
        for s in &mut self.slots {
            s.render(&shared);
        }
        for (k, y) in self.block.iter_mut().enumerate() {
            let g = self.level.next();
            // In voice order from 0: the sum of the voices' outputs as each plays alone (an idle
            // voice's output is exactly 0, which adds nothing).
            let mut acc = 0.0f32;
            for s in self.slots.iter().filter(|s| s.rendered) {
                acc += s.mixed.get(k).copied().unwrap_or(0.0) * g;
            }
            *y = acc;
        }
        self.pos = 0;
    }

    /// The voices as the panel's activity lights show them: bit `i`, voice `i` sounds (a key holds
    /// it or its tail plays); bit `16 + i`, a key holds it.
    #[must_use]
    pub fn activity(&self) -> u32 {
        self.slots.iter().enumerate().fold(0, |m, (i, s)| m | u32::from(!s.idle) << i | u32::from(s.notes.any_held()) << (16 + i))
    }
}

/// The random seed of voice `index` of an instance seeded `seed`.
fn voice_seed(seed: u32, index: usize) -> u32 {
    seed.wrapping_add(index as u32)
}

impl Plugin for MacroOscillator {
    param_plumbing!(MACRO_OSC_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        let host = sane_sr(sample_rate);
        let rate = voice_rate(host);
        if rate != self.rate {
            for s in &mut self.slots {
                s.rebuild(rate);
            }
            self.rate = rate;
        }
        self.rate_offset = if rate.hz().to_bits() == host.to_bits() { 0.0 } else { 12.0 * (rate.hz() / host).log2() };
        self.host_sr = host;
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, host);
        self.events.reserve(MAX_EVENTS.saturating_sub(self.events.len()));
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        // Every voice back to its power-on state and its own seed, so the noise-driven models play
        // after a reset as from a new instance.
        for s in &mut self.slots {
            s.reset();
        }
        self.events.clear();
        self.block = [0.0; BLOCK];
        self.pos = BLOCK;
        self.level.snap();
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.counter = 0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        sort_events(&mut self.events, |e| e.offset);
        for n in 0..frames {
            if self.pos >= BLOCK {
                self.apply_due(n);
                self.render_block();
            }
            let y = self.block.get(self.pos).copied().unwrap_or(0.0);
            self.pos += 1;
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
                }
            }
        }
        // Events not reached wait for the next block boundary, in the next host block.
        for e in &mut self.events {
            e.offset = e.offset.saturating_sub(frames);
        }
    }

    fn tail_samples(&self) -> usize {
        // The low-pass gate's slowest release (decay and color at 100%) is silent after 16 s
        // (measured: `the_slowest_release_ends_within_the_tail`).
        (self.host_sr * 20.0) as usize
    }

    fn voice_activity(&self) -> u32 {
        self.activity()
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.push(offset, Kind::On(note.min(127), velocity.min(127)));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, Kind::Off(note.min(127)));
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(kind) = Kind::from_midi(msg) {
            self.push(offset, kind);
        }
    }

    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        Kind::from_event(ev).is_none_or(|kind| self.push(offset as usize, kind))
    }

    fn all_notes_off(&mut self) {
        self.push(0, Kind::AllOff);
    }
}

#[cfg(test)]
#[path = "macro_osc_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "macro_osc_poly_tests.rs"]
mod poly_tests;
