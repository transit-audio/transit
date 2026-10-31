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
//!   destination `mod_wheel` chooses.
//! - transit's addition (#243), not part of the original module: a per-voice shaper after each
//!   voice's low-pass gate (amp and filter envelopes, a multimode filter, two LFOs; [`shaper`],
//!   off by default) and a sixteen-row modulation matrix ([`matrix`], empty by default) whose
//!   sources include the note, the controllers (aftertouch, poly aftertouch, the wheel, the bend),
//!   the envelopes, the LFOs and, on a rack member, the rack's eight macros. The note-to-macro
//!   mapping of #196 is now the matrix's first rows. With the shaper off and no row, the output is
//!   the same bits as without them.
//! - Blocks: the voices render 12 frames at a time (the firmware's block, which its envelopes are
//!   timed for), whatever the host's block size; frames rendered past the end of a host block wait
//!   in a 12-frame buffer for the next one, so any host block size gives the same output bit for
//!   bit as 12-frame host blocks. Events take effect at the first 12-frame boundary at or after
//!   their offset (at most 0.25 ms late at 48 kHz), the resolution of the module's own inputs.
//! - Output: each voice's int16 frames are read as -1..1 floats ([`to_f32`]: polarity restored,
//!   silence exactly 0, full scale one LSB under 0 dBFS), crossfaded between the main and
//!   auxiliary outputs by `aux_mix`, scaled by `level`, summed in voice order and written to every
//!   channel (with a matrix row on Level or Pan, each voice's gains into the left and right sums;
//!   the first channel is the left, the second the right, any other their mean). One voice at `level` 0 dB peaks within 0.0003 dB of 0 dBFS; voices add, so chords
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
use crate::params::{Params, param_plumbing};
use crate::plugins::instrument_queue::{AdsrRates, MAX_EVENTS, RELEASE_RESERVE};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr, sort_events};
use crate::{Category, Plugin, PluginInfo, Transport, db_to_gain};
use transit_macro_osc::SampleRate;
use transit_macro_osc::utils::random::Random;

#[path = "macro_osc_matrix.rs"]
pub mod matrix;
#[path = "macro_osc_params.rs"]
mod params;
#[path = "macro_osc_shaper.rs"]
pub mod shaper;
#[path = "macro_osc_voice.rs"]
pub mod voice;

use matrix::{Matrix, Row};
pub use params::row_param;
use params::*;
use shaper::{LfoSettings, Settings};
use voice::{Controls, Shared, Slot};

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

/// The most voices an instance plays (`voices`), all built with the instance.
pub const MAX_VOICES: usize = 16;

/// `voices` choices: the voice count is the index plus one.
pub const VOICE_COUNTS: [&str; MAX_VOICES] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

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
    /// Key and note-off velocity.
    Off(u8, u8),
    AllOff,
    Sustain(bool),
    /// -1..1.
    Bend(f32),
    /// 0..1.
    Wheel(f32),
    /// Channel pressure, 0..127.
    Pressure(u8),
    /// Key and pressure, 0..127.
    PolyPressure(u8, u8),
    ResetControllers,
}

impl Kind {
    fn is_release(self) -> bool {
        matches!(self, Kind::Off(..) | Kind::AllOff | Kind::Sustain(false) | Kind::ResetControllers)
    }

    /// Omni: every channel plays the instrument.
    fn from_event(ev: &PluginEvent) -> Option<Kind> {
        Some(match ev.message {
            Message::NoteOn { key, velocity, .. } => Kind::On(key.min(127), velocity.min(127)),
            Message::NoteOff { key, velocity, .. } => Kind::Off(key.min(127), velocity.min(127)),
            Message::ChannelPressure { value } => Kind::Pressure(value.min(127)),
            Message::PolyPressure { key, value, .. } => Kind::PolyPressure(key.min(127), value.min(127)),
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
            0xA0 => Some(Kind::PolyPressure(d1 & 0x7F, d2 & 0x7F)),
            0xB0 => Kind::from_cc(d1 & 0x7F, d2 & 0x7F),
            0xD0 => Some(Kind::Pressure(d1 & 0x7F)),
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
    /// Channel pressure, 0..127.
    pressure: u8,
    /// Poly pressure per key, 0..127.
    poly: [u8; 128],
    /// The rack's macros (0..1) while the track is a rack member.
    macros: [f32; 8],
    /// Phrases started (the voices' ages).
    counter: u64,
    /// The left and right sums of the current block (`block` is the left one).
    block_r: [f32; BLOCK],
    /// The matrix and the shaper's settings, from the parameters.
    matrix: Matrix,
    shaper: Settings,
    /// The instrument's running LFO phases (cycles), where a free-running LFO starts a note.
    free_phase: [f64; 2],
    tempo: f64,
    bar_quarters: f64,
    /// While the transport plays: the sample and song position (quarter notes) the synced LFOs
    /// count from, set when playback starts, jumps or changes tempo (see [`MacroOscillator::position`]).
    anchor: Option<(i64, f64)>,
    /// The timeline sample of the current host block's first frame, and the one the next block
    /// should start at if playback runs on.
    host_pos: i64,
    next_pos: i64,
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
            pressure: 0,
            poly: [0; 128],
            macros: [0.0; 8],
            counter: 0,
            block_r: [0.0; BLOCK],
            matrix: Matrix::default(),
            shaper: Settings::default(),
            free_phase: [0.0; 2],
            tempo: 120.0,
            bar_quarters: 4.0,
            anchor: None,
            host_pos: 0,
            next_pos: 0,
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
        self.matrix = Matrix::new((0..matrix::ROWS).map(|r| self.row(r)));
        self.shaper = self.settings();
    }

    /// Matrix row `r` (0-based) as its parameters set it.
    fn row(&self, r: usize) -> Row {
        let at = ROW1 + r * ROW_PARAMS;
        Row {
            source: self.p.choice(at + ROW_SOURCE),
            via: self.p.choice(at + ROW_VIA),
            dest: self.p.choice(at + ROW_DEST),
            curve: self.p.choice(at + ROW_CURVE),
            amount: self.p.v(at + ROW_AMOUNT) / 100.0,
        }
    }

    /// The shaper's settings from the parameters, at the host rate.
    fn settings(&self) -> Settings {
        let p = &self.p;
        let pct = |i: usize| p.v(i) / 100.0;
        let sr = self.host_sr;
        let lfo = |first: usize| {
            LfoSettings::new(
                p.choice(first + LFO_SHAPE),
                p.v(first + LFO_RATE),
                p.on(first + LFO_SYNC),
                p.choice(first + LFO_DIVISION),
                p.on(first + LFO_RETRIGGER),
                p.v(first + LFO_FADE),
                pct(first + LFO_AMOUNT),
                self.bar_quarters,
                sr,
            )
        };
        Settings {
            on: p.on(SHAPER),
            amp: AdsrRates::new(p.v(AMP_ATTACK), p.v(AMP_DECAY), pct(AMP_SUSTAIN), p.v(AMP_RELEASE), sr),
            amp_velocity: pct(AMP_VELOCITY),
            filter_env: AdsrRates::new(p.v(FILTER_ATTACK), p.v(FILTER_DECAY), pct(FILTER_SUSTAIN), p.v(FILTER_RELEASE), sr),
            mode: shaper::filter_mode(p.choice(FILTER_MODE)),
            slope24: p.choice(FILTER_SLOPE) == 1,
            cutoff_log2: p.v(CUTOFF).max(1.0).log2(),
            resonance: pct(RESONANCE),
            drive: db_to_gain(p.v(DRIVE)),
            key_track: pct(KEY_TRACK),
            env_amount: pct(FILTER_ENV_AMOUNT),
            lfo: [lfo(LFO1), lfo(LFO2)],
            sr,
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
            Kind::On(key, 0) => self.apply(Kind::Off(key, 0)),
            Kind::On(key, vel) => {
                if let Some(p) = self.poly.get_mut(usize::from(key)) {
                    *p = 0;
                }
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
            Kind::Off(key, velocity) => {
                for s in &mut self.slots {
                    s.off(key, velocity, self.sustain);
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
            Kind::Pressure(v) => self.pressure = v,
            Kind::PolyPressure(key, v) => {
                if let Some(p) = self.poly.get_mut(usize::from(key)) {
                    *p = v;
                }
            }
            Kind::ResetControllers => {
                self.apply(Kind::Sustain(false));
                self.bend = 0.0;
                self.wheel = 0.0;
                self.pressure = 0;
                self.poly = [0; 128];
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

    /// Renders the next 12 frames of every voice not idle and sums them into `block` (left) and
    /// `block_r` (right); the block starts `at` frames into the host block.
    fn render_block(&mut self, at: usize) {
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
            matrix: &self.matrix,
            shaper: &self.shaper,
            controls: Controls { aftertouch: f32::from(self.pressure) / 127.0, wheel: self.wheel, bend: self.bend, macros: self.macros },
            poly: &self.poly,
            free_phase: self.free_phase,
            quarters: self.position(at),
            tempo: self.tempo,
            quiet_blocks: (self.rate.hz() * voice::QUIET_SECONDS / BLOCK as f32) as u32,
        };
        for s in &mut self.slots {
            s.render(&shared);
        }
        for (k, (l, r)) in self.block.iter_mut().zip(self.block_r.iter_mut()).enumerate() {
            let g = self.level.next();
            // In voice order from 0: the sum of the voices' outputs as each plays alone (an idle
            // voice's output is exactly 0, which adds nothing). A voice's gains are exactly 1 when
            // no matrix row moves its Level or Pan, so the product is its output exactly.
            let (mut acc_l, mut acc_r) = (0.0f32, 0.0f32);
            for s in self.slots.iter().filter(|s| s.rendered) {
                let y = s.mixed.get(k).copied().unwrap_or(0.0) * g;
                acc_l += y * s.gain.0;
                acc_r += y * s.gain.1;
            }
            (*l, *r) = (acc_l, acc_r);
        }
        // The running phases advance at the LFOs' own rates (a synced one at the tempo's).
        for (phase, set) in self.free_phase.iter_mut().zip(&self.shaper.lfo) {
            let p = *phase + set.increment(self.tempo, 0.0, self.host_sr) * BLOCK as f64;
            *phase = p - p.floor();
        }
        self.pos = 0;
    }

    /// The song position (quarter notes) of frame `at` of this host block while the transport
    /// plays: the anchor's position plus the frames since the anchor at the tempo. Counted in whole
    /// frames from one anchor, so every host block size gives the same position for the same frame
    /// (at a constant tempo; a tempo change re-anchors at the next host block start, the caveat of
    /// #245).
    fn position(&self, at: usize) -> Option<f64> {
        let (sample, quarters) = self.anchor?;
        let frames = self.host_pos.saturating_add(at as i64).saturating_sub(sample);
        Some(quarters + frames as f64 * self.tempo / 60.0 / f64::from(self.host_sr))
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
        self.shaper = self.settings();
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
        self.pressure = 0;
        self.poly = [0; 128];
        self.counter = 0;
        self.block_r = [0.0; BLOCK];
        self.free_phase = [0.0; 2];
        self.anchor = None;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        sort_events(&mut self.events, |e| e.offset);
        for n in 0..frames {
            if self.pos >= BLOCK {
                self.apply_due(n);
                self.render_block(n);
            }
            let l = self.block.get(self.pos).copied().unwrap_or(0.0);
            let r = self.block_r.get(self.pos).copied().unwrap_or(0.0);
            self.pos += 1;
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x = match (c, ch) {
                        // One channel: the mean, which is the left exactly when the two are equal.
                        (_, 1) => (l + r) * 0.5,
                        (0, _) => l,
                        (1, _) => r,
                        _ => (l + r) * 0.5,
                    };
                }
            }
        }
        self.next_pos = self.host_pos.saturating_add(frames as i64);
        // Events not reached wait for the next block boundary, in the next host block.
        for e in &mut self.events {
            e.offset = e.offset.saturating_sub(frames);
        }
    }

    fn tail_samples(&self) -> usize {
        // The low-pass gate's slowest release (decay and color at 100%) is silent after 16 s
        // (measured: `the_slowest_release_ends_within_the_tail`). With the shaper on, the amp
        // envelope's release (its time is to -60 dB; it ends at -100 dB) plus a block.
        let shaped = if self.shaper.on { self.p.v(AMP_RELEASE) * 0.001 * 100.0 / 60.0 + 0.1 } else { 0.0 };
        (self.host_sr * shaped.max(20.0)) as usize
    }

    fn set_transport(&mut self, t: &Transport) {
        let tempo = if t.tempo.is_finite() && t.tempo > 0.0 { t.tempo.clamp(1.0, 999.0) } else { 120.0 };
        let bar = if t.numerator > 0 && t.denominator > 0 { f64::from(t.numerator) * 4.0 / f64::from(t.denominator) } else { 4.0 };
        let bar = bar.clamp(0.25, 64.0);
        let jumped = t.samples != self.next_pos || tempo != self.tempo;
        self.anchor = match self.anchor {
            _ if !t.playing || !t.quarters.is_finite() => None,
            Some(a) if !jumped => Some(a),
            _ => Some((t.samples, t.quarters)),
        };
        self.host_pos = t.samples;
        self.next_pos = t.samples;
        self.tempo = tempo;
        if bar != self.bar_quarters {
            self.bar_quarters = bar;
            self.shaper = self.settings();
        }
    }

    fn reads_rack_macros(&self) -> bool {
        true
    }

    fn set_rack_macros(&mut self, values: &[f32; 8]) {
        for (m, v) in self.macros.iter_mut().zip(values) {
            *m = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        }
    }

    fn voice_activity(&self) -> u32 {
        self.activity()
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.push(offset, Kind::On(note.min(127), velocity.min(127)));
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, Kind::Off(note.min(127), 0));
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

#[cfg(test)]
#[path = "macro_osc_shaper_tests.rs"]
mod shaper_tests;
