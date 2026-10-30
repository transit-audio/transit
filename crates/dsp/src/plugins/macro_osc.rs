//! Macro Oscillator: one voice of transit's macro oscillator (`transit-macro-osc`, ADR 0033, #195),
//! a Rust port of Émilie Gillet's MIT-licensed eurorack firmware (24 synthesis models), played
//! from MIDI as a mono instrument.
//!
//! - MIDI to the voice: the held note is the voice's V/OCT input, a note-on raises its trigger
//!   input and sets its level input from the velocity, a note-off lowers both and the low-pass
//!   gate's decay is the release. Mono with last-note priority and legato: a note played while
//!   another is held changes pitch without a new trigger (the module's trigger input stays high,
//!   so its envelopes are not restarted); releasing the newest note returns to the most recent one
//!   still held. Sustain (CC 64) holds released notes; pitch bend spans `bend_range` semitones each
//!   way; the mod wheel (CC 1) adds up to 100% to the destination `mod_wheel` chooses.
//! - Blocks: the voice renders 12 frames at a time (the firmware's block, which its envelopes are
//!   timed for), whatever the host's block size; frames rendered past the end of a host block wait
//!   in a 12-frame buffer for the next one, so any host block size gives the same output bit for
//!   bit as 12-frame host blocks. Events take effect at the first 12-frame boundary at or after
//!   their offset (at most 0.25 ms late at 48 kHz), the resolution of the module's own inputs.
//! - Output: the voice's int16 frames are read as -1..1 floats ([`to_f32`]: full scale = 0 dBFS,
//!   polarity restored, silence exactly 0), crossfaded between the main and
//!   auxiliary outputs by `aux_mix`, scaled by `level` and written to every channel. The voice's
//!   limiter and int16 clip keep each output within full scale, so at `level` 0 dB the
//!   instrument stays within 0 dBFS (+0.0003 dB at the clip); the -6 dB default leaves 6 dB of
//!   headroom on the strip.
//! - Tuning: concert pitch (A4 = 440 Hz, `transit_macro_osc::Tuning::Concert`), as the module plays:
//!   its audio clock runs at the firmware's corrected rate. The firmware's arithmetic, 4.6 cents sharp
//!   at a true 48 kHz, stays bit-exact in the crate for the null tests only.
//! - Rates: the voice runs at 44.1, 48, 88.2 or 96 kHz. At any other host rate it runs at the
//!   nearest of those and its pitch is corrected to the host rate; its envelopes and engine
//!   timbres then run faster or slower by the rate ratio.
//!
//! Parameters only: the voice's state is not saved, as for every built-in.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, CC_SUSTAIN, Message, PluginEvent};
use crate::params::{Params, choice, lin, param_plumbing};
use crate::plugins::synth::{MAX_EVENTS, RELEASE_RESERVE};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr, sort_events};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc::{SampleRate, Tuning};

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

static PARAMS: [ParamInfo; 11] = [
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

    /// Omni: a mono instrument plays every channel.
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

/// The keys sounding, oldest first: each held, or released while the sustain pedal is down.
#[derive(Debug, Clone)]
struct NoteStack {
    keys: [u8; 128],
    len: usize,
    held: [bool; 128],
}

impl NoteStack {
    const fn new() -> Self {
        Self { keys: [0; 128], len: 0, held: [false; 128] }
    }

    fn top(&self) -> Option<u8> {
        self.len.checked_sub(1).and_then(|i| self.keys.get(i)).copied()
    }

    fn remove(&mut self, key: u8) {
        let len = self.len;
        if let Some(i) = self.keys.get(..len).and_then(|k| k.iter().position(|&k| k == key)) {
            self.keys.copy_within(i + 1..len, i);
            self.len -= 1;
        }
    }

    fn push(&mut self, key: u8) {
        self.remove(key);
        if let Some(slot) = self.keys.get_mut(self.len) {
            *slot = key;
            self.len += 1;
        }
        if let Some(h) = self.held.get_mut(usize::from(key)) {
            *h = true;
        }
    }

    fn release(&mut self, key: u8, sustain: bool) {
        let Some(h) = self.held.get_mut(usize::from(key)) else { return };
        if !*h {
            return;
        }
        *h = false;
        if !sustain {
            self.remove(key);
        }
    }

    /// Drops every key the pedal alone was keeping.
    fn release_pedalled(&mut self) {
        let mut kept = 0;
        for i in 0..self.len {
            let key = self.keys.get(i).copied().unwrap_or(0);
            if self.held.get(usize::from(key)).copied().unwrap_or(false) {
                if let Some(slot) = self.keys.get_mut(kept) {
                    *slot = key;
                }
                kept += 1;
            }
        }
        self.len = kept;
    }

    fn clear(&mut self) {
        self.len = 0;
        self.held = [false; 128];
    }
}

/// The voice rate for a host rate: the supported rate nearest on a log scale.
fn voice_rate(host: f32) -> SampleRate {
    let dist = |r: SampleRate| (r.hz() / host).ln().abs();
    SampleRate::ALL.into_iter().min_by(|a, b| dist(*a).total_cmp(&dist(*b))).unwrap_or_default()
}

/// One int16 output sample as a float at full scale -1..1: the firmware writes inverted samples
/// for its DAC and adds 1 when it truncates to int16, so the polarity is restored and the 1 taken
/// off; silence (1) is exactly 0, and the int16 clip (-32768) is 32769 / 32768 (+0.0003 dB).
#[inline]
pub fn to_f32(sample: i16) -> f32 {
    (1.0 - f32::from(sample)) / 32768.0
}

/// The macro oscillator as a mono MIDI instrument.
pub struct MacroOscillator {
    p: Params,
    voice: Box<Voice<'static>>,
    rate: SampleRate,
    host_sr: f32,
    /// Semitones that correct the voice's pitch from its rate to the host's (0 at the four rates).
    rate_offset: f32,
    ch: usize,
    events: Vec<Event>,
    frames: [Frame; BLOCK],
    /// The current 12-frame block, mixed and scaled; `pos` frames of it were written out.
    block: [f32; BLOCK],
    pos: usize,
    level: Smoother,
    notes: NoteStack,
    /// The note the voice plays (kept after the release, so the tail keeps its pitch).
    note: f32,
    /// The level input while a note sounds: the velocity of the newest note.
    velocity: f32,
    sustain: bool,
    bend: f32,
    wheel: f32,
    /// The trigger input of the last block rendered.
    last_trigger: bool,
    /// A note started while the trigger was still high: one low block first, so the voice sees
    /// the new edge.
    retrigger: bool,
}

impl Default for MacroOscillator {
    fn default() -> Self {
        Self::new()
    }
}

impl MacroOscillator {
    pub fn new() -> Self {
        let rate = SampleRate::Hz48000;
        let mut s = Self {
            p: Params::new(&PARAMS),
            voice: Box::new(Voice::new(rate)),
            rate,
            host_sr: rate.hz(),
            rate_offset: 0.0,
            ch: 2,
            events: Vec::with_capacity(MAX_EVENTS),
            frames: [Frame::default(); BLOCK],
            block: [0.0; BLOCK],
            pos: BLOCK,
            level: Smoother::new(0.5),
            notes: NoteStack::new(),
            note: 60.0,
            velocity: 0.0,
            sustain: false,
            bend: 0.0,
            wheel: 0.0,
            last_trigger: false,
            retrigger: false,
        };
        s.voice.set_tuning(Tuning::Concert);
        s.update();
        s.level.snap();
        s
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(LEVEL)));
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

    fn apply(&mut self, kind: Kind) {
        match kind {
            Kind::On(key, 0) => self.apply(Kind::Off(key)),
            Kind::On(key, vel) => {
                if self.notes.top().is_none() {
                    // A new phrase: a new trigger edge, after one low block if it is still high.
                    self.retrigger = self.last_trigger;
                }
                self.notes.push(key);
                self.note = f32::from(key);
                self.velocity = f32::from(vel) / 127.0;
            }
            Kind::Off(key) => {
                self.notes.release(key, self.sustain);
                self.follow_top();
            }
            Kind::AllOff => {
                self.notes.clear();
                self.sustain = false;
            }
            Kind::Sustain(on) => {
                self.sustain = on;
                if !on {
                    self.notes.release_pedalled();
                    self.follow_top();
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

    /// Legato back to the newest key still sounding (the velocity stays).
    fn follow_top(&mut self) {
        if let Some(key) = self.notes.top() {
            self.note = f32::from(key);
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

    /// Renders the next 12 frames into `block`.
    fn render_block(&mut self) {
        let v = |i| self.p.v(i) / 100.0;
        let (mut harmonics, mut timbre, mut morph) = (v(HARMONICS), v(TIMBRE), v(MORPH));
        match self.p.choice(MOD_WHEEL) {
            0 => timbre = (timbre + self.wheel).min(1.0),
            1 => harmonics = (harmonics + self.wheel).min(1.0),
            2 => morph = (morph + self.wheel).min(1.0),
            _ => {}
        }
        let offset = self.p.v(TUNE) + self.bend * self.p.v(BEND_RANGE) + self.rate_offset;
        let patch = Patch {
            note: if offset.is_finite() { offset.clamp(-96.0, 96.0) } else { 0.0 },
            harmonics,
            timbre,
            morph,
            engine: self.p.choice(MODEL).min(MODELS.len() - 1),
            decay: v(DECAY),
            lpg_colour: v(LPG_COLOR),
            ..Patch::default()
        };
        let sounding = self.notes.top().is_some();
        let trigger = sounding && !self.retrigger;
        self.retrigger = false;
        self.last_trigger = trigger;
        let mods = Modulations {
            note: self.note,
            trigger: if trigger { 1.0 } else { 0.0 },
            level: if sounding { self.velocity } else { 0.0 },
            trigger_patched: true,
            level_patched: true,
            ..Modulations::default()
        };
        self.voice.render(&patch, &mods, &mut self.frames);
        let mix = v(AUX_MIX);
        for (y, f) in self.block.iter_mut().zip(&self.frames) {
            let (out, aux) = (to_f32(f.out), to_f32(f.aux));
            *y = (out + (aux - out) * mix) * self.level.next();
        }
        self.pos = 0;
    }
}

impl Plugin for MacroOscillator {
    param_plumbing!(MACRO_OSC_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        let host = sane_sr(sample_rate);
        let rate = voice_rate(host);
        if rate != self.rate {
            *self.voice = Voice::new(rate);
            self.voice.set_tuning(Tuning::Concert);
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
        // The tuning survives `init`.
        self.voice.init();
        self.events.clear();
        self.frames = [Frame::default(); BLOCK];
        self.block = [0.0; BLOCK];
        self.pos = BLOCK;
        self.level.snap();
        self.notes.clear();
        self.note = 60.0;
        self.velocity = 0.0;
        self.sustain = false;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.last_trigger = false;
        self.retrigger = false;
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
