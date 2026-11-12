//! Texture Synth: the synthesizer of transit's texture processor (`transit-texture`, ADR 0033,
//! #241 slice 3), a Rust port of Émilie Gillet's MIT-licensed eurorack firmware, played from MIDI as
//! an instrument. It is the instrument form of the Texture insert: the module's synth, what it
//! plays with nothing patched into its audio inputs.
//!
//! - **What sounds.** The firmware's synth: a wavetable oscillator (Time scans the waves, Bank picks
//!   one of 12 banks of 16 waves, the module's FEEDBACK knob) and grains of the same wavetables
//!   seeded by the trigger generator, mixed by Dry/Wet (dry is the oscillator, wet the grains), then
//!   the limiters and the reverb. It has no recording buffer: on the module the synth replaces the
//!   effect chain when no audio is patched, and its grains read wavetables, not recorded audio. An
//!   audio input for the instrument is out of scope here (#274 covers instrument audio inputs).
//! - **Notes.** The module is one voice with grain polyphony inside (up to 32 grains), so the
//!   instrument is monophonic in pitch, with no voice allocator. A note-on sets the module's PITCH
//!   CV to the key (`key − 48` semitones over the oscillator's root, MIDI 48 = 130.81 Hz, so MIDI 69
//!   plays 440 Hz) and raises the SEED input's gate; the gate stays high while any key is held. The
//!   SEED gate is the firmware's own gate semantic: below Density 48% a regular train of grains
//!   runs while the gate is high (the rising edge fires the first at once), between 48% and 52% each
//!   rising edge fires one grain, above 52% a random train runs while it is high; with Seed Latch on
//!   the notes clock the module's divider instead. A note-off lowers the gate when no key is left
//!   (grains already started play out); while other keys are held the pitch returns to the newest
//!   of them without a retrigger (Ableton's and Logic's mono legato). A new note while one is held
//!   retriggers at the new pitch, as the module's SEED input would see a new edge: the gate drops
//!   for the block's first frame and rises on the second. The oscillator (Dry) has no gate on the
//!   module: it plays at the last note's pitch for as long as Dry/Wet lets it through; the shaper's
//!   amp envelope gates it when on. The sustain pedal holds the shaper's envelopes only.
//! - **Velocity.** The module has no level or accent input on the synth path (the grains' amplitude
//!   is the firmware's equal-power share of the grains sounding), so velocity sets the output level
//!   from the note on: `level × (1 − Velocity × (1 − velocity))`, the step ramped over the note's
//!   first 12-frame block (0.25 ms); at full velocity, or Velocity 0, the level is the knob's.
//! - **Freeze and Seed.** Freeze is the module's: new grains stop, the sounding grains hold their
//!   envelope level (a frozen cloud), the oscillator's wave position stops wandering. Seed (held)
//!   is the module's SEED button held: the gate is high whatever the keys do; Seed Latch is the
//!   button latched. Both are parameters, so they automate and take a MIDI mapping. A frozen cloud
//!   is saved with the session ([`state`]).
//! - **Quality.** The module's quality setting on the synth picks the resolution of the oscillator
//!   and of the grains (Dirty Tape 4-bit, Clean Tape a 16-bit oscillator with 6-bit grains, Vintage
//!   Digital 7-bit, Digital 16-bit); the synth's rate does not change with it.
//! - **Rate and blocks.** The synth runs at its firmware rate, 48 kHz, in the module's 12-frame
//!   blocks (its codec block at 48 kHz), whatever the host's block size. Events take effect at the
//!   first 12-frame block that starts at or after their offset (at 48 kHz at most 11 frames, 0.23 ms,
//!   late), the Macro Oscillator's rule on its grid. At any other session rate the output goes
//!   through the shared edge converter ([`super::resample`], the Resonator's) from 48 kHz to the
//!   session rate, and the instrument reports its delay (`latency`); at 48 kHz nothing is converted
//!   and the latency is 0. Blocks are rendered as the converter needs them, counted in output
//!   frames, so any host block size renders the same bits.
//! - transit's addition (#243, shared with the Macro Oscillator and the Resonator), not part of the
//!   original module: the shaper (amp and filter envelopes, a multimode filter, two LFOs) on the
//!   output, one per channel driven alike, and the sixteen-row modulation matrix, with the Macro
//!   Oscillator's sources and the module's destinations. Turning the shaper on or off while a key
//!   is held takes effect at once with a 12-frame crossfade (#262); with no key held the switch
//!   waits for the next note. With the shaper off, no active row, level 0 dB and full velocity the
//!   output at 48 kHz is the firmware's, bit for bit.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, CC_SUSTAIN, Message, PluginEvent};
use crate::params::{Params, param_plumbing};
use crate::plugins::instrument_queue::{AdsrRates, MAX_EVENTS, RELEASE_RESERVE};
use crate::plugins::macro_osc::matrix::{self, Matrix, NDST, NSRC, PITCH_SEMITONES, RATE_OCTAVES, Row, dst, src};
use crate::plugins::macro_osc::shaper::{self, Fade, FilterMods, LfoSettings, Settings, Shaper};
use crate::plugins::macro_osc::voice::{key_source, voice_gains};
use crate::plugins::resample::{Converter, Kernel};
use crate::plugins::resonator::edge;
use crate::util::{Smoother, frames_in, sane_channels, sane_sr, sort_events};
use crate::{Category, Plugin, PluginInfo, Transport, db_to_gain};
use transit_instrument_core::pairing::NotePairing;
use transit_texture::stmlib::extract_gate_flags;
use transit_texture::{Controls, Parameter, SYNTH_SAMPLE_RATE, SeedButton, Synth, SynthResolution};

#[path = "texture_synth_params.rs"]
mod params;
#[path = "texture_synth_state.rs"]
mod state;

use params::*;

/// The firmware's rate, Hz: the synth, the shaper and the matrix run at it.
pub const ENGINE_RATE: u32 = SYNTH_SAMPLE_RATE;

/// The module's block at 48 kHz (its codec rate / 4000), frames; also the shaper's block.
pub const BLOCK: usize = crate::plugins::macro_osc::BLOCK;

/// The qualities, in the firmware's order (the `quality` parameter's index is the synth's
/// resolution's index), named as the Texture insert names them.
pub const QUALITIES: [&str; 4] = super::texture::QUALITIES;

/// The key whose pitch is the oscillator's root (MIDI 48, 130.81 Hz): a note's PITCH CV is its key
/// minus this, in semitones.
pub const ROOT_KEY: f32 = 48.0;

/// The PITCH CV's range on the module, semitones either way.
const MAX_CV: f32 = 60.0;

/// The matrix's destinations, in the order of the `mod<n>_dest` choices: the Macro Oscillator's
/// list with the module's controls in place of its four macros, and three more past Pan.
pub const DESTINATIONS: [&str; NDST] = [
    "Off",
    "Pitch",
    "Time",
    "Size",
    "Shape",
    "Density",
    "Level",
    "Cutoff",
    "Resonance",
    "Filter env amount",
    "LFO 1 rate",
    "LFO 2 rate",
    "LFO 1 amount",
    "LFO 2 amount",
    "Pan",
    "Bank",
    "Dry/Wet",
    "Reverb",
];

/// The destinations of the module's controls.
const D_TIME: usize = dst::HARMONICS;
const D_SIZE: usize = dst::TIMBRE;
const D_SHAPE: usize = dst::MORPH;
const D_DENSITY: usize = dst::DECAY;
const D_BANK: usize = dst::EXTRA1;
const D_DRY_WET: usize = dst::EXTRA2;
const D_REVERB: usize = dst::EXTRA3;

/// How long the output can sound after the last note, seconds: the longest grain (4 s) and the
/// reverb's tail. A frozen cloud sounds for as long as it stays frozen, which no tail covers.
const TAIL_SECONDS: f32 = 12.0;

pub static TEXTURE_SYNTH_INFO: PluginInfo = PluginInfo {
    id: "texture_synth",
    name: "Texture Synth",
    short_name: "TexSyn",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    audiosuite: false,
    pages: &[],
};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    On(u8, u8),
    /// Key and note-off velocity.
    Off(u8, u8),
    /// All Notes Off (CC 123): release the keys, let the grains play out.
    AllOff,
    /// All Sound Off (CC 120): silence at once.
    SoundOff,
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
        matches!(self, Kind::Off(..) | Kind::AllOff | Kind::SoundOff | Kind::Sustain(false) | Kind::ResetControllers)
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
            CC_ALL_SOUND_OFF => Kind::SoundOff,
            CC_ALL_NOTES_OFF => Kind::AllOff,
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

/// The output gain a note's velocity (0..1) sets: `1 − amount · (1 − velocity)`, exactly 1 at full
/// velocity or amount 0.
#[must_use]
pub fn velocity_gain(amount: f32, velocity: f32) -> f32 {
    if velocity >= 1.0 || amount <= 0.0 { 1.0 } else { 1.0 - amount.min(1.0) * (1.0 - velocity.max(0.0)) }
}

/// A control (0..1) from its knob and its matrix sum: the knob exactly when no row moves it, else
/// the sum added and clamped to `0..=1`.
fn control(knob: f32, sum: f32) -> f32 {
    if sum == 0.0 { knob } else { (knob + sum).clamp(0.0, 1.0) }
}

/// The PITCH CV of a key with bend and the matrix's Pitch (semitones), clamped to the module's
/// range: exactly `key − 48` when neither moves it.
#[must_use]
pub fn pitch_cv(key: u8, fm: f32) -> f32 {
    let cv = f32::from(key) - ROOT_KEY;
    let cv = if fm == 0.0 || !fm.is_finite() { cv } else { cv + fm };
    cv.clamp(-MAX_CV, MAX_CV)
}

/// The shaper's path for one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Dry,
    Shaped,
    /// Crossfading into the shaper over the block.
    In,
    /// Crossfading out of it.
    Out,
}

/// The texture synth as a MIDI instrument.
pub struct TextureSynth {
    p: Params,
    synth: Box<Synth>,
    kernel: Kernel,
    up: Converter,
    latency: usize,
    host_sr: f32,
    ch: usize,
    events: Vec<Event>,
    /// Keys held down, when each was pressed (a counter), and keys released while the pedal is down.
    held: [bool; 128],
    /// The open note-ons per key, so a note-off closes the oldest (#292).
    pairing: NotePairing,
    pressed: [u32; 128],
    presses: u32,
    pedalled: [bool; 128],
    sustain: bool,
    /// A note-on since the last block: its key and velocity (0..1).
    strike: Option<(u8, f32)>,
    /// The SEED gate's last frame was high, and the gate flags the firmware last saw.
    gate_high: bool,
    gate_flags: u8,
    /// The sounding key (its PITCH CV) and the newest note's velocity (0..1); the last note-off's.
    key: u8,
    velocity: f32,
    release_velocity: f32,
    bend: f32,
    wheel: f32,
    pressure: u8,
    poly: [u8; 128],
    macros: [f32; 8],
    level: Smoother,
    /// The velocity's gain as of the last block ([`velocity_gain`]).
    vel_gain: f32,
    /// One shaper per channel, driven alike (the same seed and calls).
    shapers: [Shaper; 2],
    matrix: Matrix,
    settings: Settings,
    /// The shaper shapes the output (its switch as of the last note or held-note switch).
    shaped: bool,
    /// The envelopes are gated.
    gate: bool,
    /// Nothing played since the last reset: the shaper's switch applies without a crossfade.
    fresh: bool,
    /// All Sound Off (CC 120) holds the output at zero until the next note: the dry oscillator has
    /// no gate, so the synth's reset alone would restart it at once.
    muted: bool,
    free_phase: [f64; 2],
    tempo: f64,
    bar_quarters: f64,
    bar_origin: (i64, f64),
    anchor: Option<(i64, f64)>,
    host_pos: i64,
    next_pos: i64,
    /// A frozen state restored before the first block: kept through `prepare` and `reset`.
    restored: bool,
    capture: Option<state::Capture>,
}

impl Default for TextureSynth {
    fn default() -> Self {
        Self::new()
    }
}

/// The shapers' random seed: the firmware's power-on seed.
const SHAPER_SEED: u32 = transit_texture::stmlib::random::Random::DEFAULT_SEED;

impl TextureSynth {
    pub fn new() -> Self {
        let kernel = Kernel::new();
        let (up, latency) = edge(&kernel, ENGINE_RATE);
        let mut s = Self {
            p: Params::new(&PARAMS),
            synth: Box::new(Synth::new()),
            kernel,
            up,
            latency,
            host_sr: ENGINE_RATE as f32,
            ch: 2,
            events: Vec::with_capacity(MAX_EVENTS),
            held: [false; 128],
            pairing: NotePairing::new(),
            pressed: [0; 128],
            presses: 0,
            pedalled: [false; 128],
            sustain: false,
            strike: None,
            gate_high: false,
            gate_flags: 0,
            key: 60,
            velocity: 1.0,
            release_velocity: 0.0,
            bend: 0.0,
            wheel: 0.0,
            pressure: 0,
            poly: [0; 128],
            macros: [0.0; 8],
            level: Smoother::new(0.5),
            vel_gain: 1.0,
            shapers: [Shaper::new(SHAPER_SEED), Shaper::new(SHAPER_SEED)],
            matrix: Matrix::default(),
            settings: Settings::default(),
            shaped: false,
            gate: false,
            fresh: true,
            muted: false,
            free_phase: [0.0; 2],
            tempo: 120.0,
            bar_quarters: 4.0,
            bar_origin: (0, 0.0),
            anchor: None,
            host_pos: 0,
            next_pos: 0,
            restored: false,
            capture: None,
        };
        s.update();
        s.reset();
        s
    }

    fn update(&mut self) {
        self.matrix = Matrix::new((0..matrix::ROWS).map(|r| self.row(r)));
        self.settings = self.settings();
    }

    /// The velocity's gain for the newest note.
    fn vel_target(&self) -> f32 {
        velocity_gain(self.p.v(VELOCITY) / 100.0, self.velocity)
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

    /// The shaper's settings from the parameters, at the synth's rate.
    fn settings(&self) -> Settings {
        let p = &self.p;
        let pct = |i: usize| p.v(i) / 100.0;
        let sr = ENGINE_RATE as f32;
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
                self.bar_origin,
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

    /// A key is held (the SEED gate's source).
    fn keyed(&self) -> bool {
        self.held.iter().any(|h| *h)
    }

    /// A key is held, or kept by the pedal (the shaper's envelopes).
    fn holding(&self) -> bool {
        self.held.iter().zip(&self.pedalled).any(|(h, p)| *h || *p)
    }

    /// The newest key still held.
    fn newest_held(&self) -> Option<u8> {
        (0u8..128).filter(|k| self.held[usize::from(*k)]).max_by_key(|k| self.pressed[usize::from(*k)])
    }

    /// All Sound Off: the synth, the shapers and the edge converter back to power-on at once (the
    /// synth's `reset` allocates nothing), the keys, the pedal and a pending note cleared. The
    /// queued events and the controllers stay.
    fn silence(&mut self) {
        self.synth.reset();
        self.up.reset();
        self.held = [false; 128];
        self.pairing.clear();
        self.pedalled = [false; 128];
        self.sustain = false;
        self.strike = None;
        self.gate_high = false;
        self.gate_flags = 0;
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaped = self.settings.on;
        self.gate = false;
        self.muted = true;
    }

    fn apply(&mut self, kind: Kind) {
        match kind {
            Kind::On(key, 0) => self.apply(Kind::Off(key, 0)),
            Kind::On(key, vel) => {
                let k = usize::from(key);
                let held = self.held.get(k).copied().unwrap_or(false);
                self.pairing.on(0, key, held);
                if let Some(p) = self.poly.get_mut(k) {
                    *p = 0;
                }
                if let Some(h) = self.held.get_mut(k) {
                    *h = true;
                }
                if let Some(p) = self.pedalled.get_mut(k) {
                    *p = false;
                }
                self.presses = self.presses.wrapping_add(1);
                if let Some(p) = self.pressed.get_mut(k) {
                    *p = self.presses;
                }
                self.key = key;
                self.strike = Some((key, f32::from(vel) / 127.0));
            }
            Kind::Off(key, velocity) => {
                // An off that closes an earlier note-on of a key struck again since leaves the
                // newest note sounding (#292).
                self.pairing.off(0, key);
                if self.pairing.open(0, key) > 0 {
                    return;
                }
                let k = usize::from(key);
                if self.held.get(k).copied().unwrap_or(false) {
                    if let Some(h) = self.held.get_mut(k) {
                        *h = false;
                    }
                    if let Some(p) = self.pedalled.get_mut(k) {
                        *p = self.sustain;
                    }
                    self.release_velocity = f32::from(velocity.min(127)) / 127.0;
                    // Legato back to the newest key still held, without a retrigger.
                    if key == self.key
                        && let Some(back) = self.newest_held()
                    {
                        self.key = back;
                    }
                }
            }
            Kind::AllOff => {
                self.held = [false; 128];
                self.pairing.clear();
                self.pedalled = [false; 128];
                self.sustain = false;
            }
            Kind::SoundOff => self.silence(),
            Kind::Sustain(on) => {
                self.sustain = on;
                if !on {
                    self.pedalled = [false; 128];
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

    /// The matrix's source values now (index [`src`]).
    fn sources(&self) -> [f32; NSRC] {
        let mut v = [0.0f32; NSRC];
        let mut set = |i: usize, x: f32| {
            if let Some(slot) = v.get_mut(i) {
                *slot = x;
            }
        };
        let [sh, _] = &self.shapers;
        if self.matrix.reads(src::LFO1) {
            set(src::LFO1, sh.lfo_value(&self.settings, 0));
        }
        if self.matrix.reads(src::LFO2) {
            set(src::LFO2, sh.lfo_value(&self.settings, 1));
        }
        set(src::AMP_ENV, sh.amp.level);
        set(src::FILTER_ENV, sh.filter_env.level);
        set(src::VELOCITY, self.velocity);
        set(src::KEY, key_source(f32::from(self.key)));
        set(src::RELEASE_VELOCITY, self.release_velocity);
        set(src::AFTERTOUCH, f32::from(self.pressure) / 127.0);
        set(src::POLY_AFTERTOUCH, f32::from(self.poly.get(usize::from(self.key)).copied().unwrap_or(0)) / 127.0);
        set(src::MOD_WHEEL, self.wheel);
        set(src::PITCH_BEND, self.bend);
        set(src::RANDOM, sh.random);
        for (i, x) in self.macros.iter().enumerate() {
            set(src::MACRO1 + i, *x);
        }
        v
    }

    fn sums(&self) -> [f32; NDST] {
        if self.matrix.is_empty() { [0.0; NDST] } else { self.matrix.evaluate(&self.sources()) }
    }

    /// The song position (quarter notes) of frame `at` of this host block while the transport
    /// plays (the Macro Oscillator's rule: whole frames from one anchor).
    fn position(&self, at: usize) -> Option<f64> {
        let (sample, quarters) = self.anchor?;
        let frames = self.host_pos.saturating_add(at as i64).saturating_sub(sample);
        Some(quarters + frames as f64 * self.tempo / 60.0 / f64::from(self.host_sr))
    }

    /// The firmware's controls for a block, from the parameters, the matrix's sums and the key.
    fn controls(&self, sums: &[f32; NDST]) -> Controls {
        let p = &self.p;
        let sum = |d: usize| sums.get(d).copied().unwrap_or(0.0);
        let unit = |i: usize| p.v(i) / 100.0;
        let knob =
            |i: usize, random: usize, d: usize| Parameter { base: control(unit(i), sum(d)), attenuverter: unit(random), cv: 0.0, has_cv: false };
        let pitch = sum(dst::PITCH);
        let fm = self.bend * p.v(BEND_RANGE) + if pitch == 0.0 { 0.0 } else { PITCH_SEMITONES * pitch };
        Controls {
            time: knob(TIME, TIME_RANDOM, D_TIME),
            size: knob(SIZE, SIZE_RANDOM, D_SIZE),
            shape: knob(SHAPE, SHAPE_RANDOM, D_SHAPE),
            pitch: Parameter { base: p.v(PITCH), attenuverter: unit(PITCH_RANDOM), cv: pitch_cv(self.key, fm), has_cv: true },
            density: control(unit(DENSITY), sum(D_DENSITY)),
            density_cv: 0.0,
            feedback: control(unit(BANK), sum(D_BANK)),
            dry_wet: control(unit(DRY_WET), sum(D_DRY_WET)),
            reverb: control(unit(REVERB), sum(D_REVERB)),
            seed_button: if p.on(SEED_LATCH) {
                SeedButton::Locked
            } else if p.on(SEED) {
                SeedButton::On
            } else {
                SeedButton::Off
            },
            monophonic: p.on(MONO_GRAINS),
            output_gate: false,
        }
    }

    /// The SEED input's gate flags for the next block: high while a key is held (at least for the
    /// block of a note shorter than a block); at a note-on while the gate was high, low on the
    /// first frame and rising on the second (a retrigger).
    fn seed_flags(&mut self, strike: bool) -> [u8; BLOCK] {
        let high = self.keyed() || strike;
        let retrigger = strike && self.gate_high;
        let mut flags = [0u8; BLOCK];
        for (k, f) in flags.iter_mut().enumerate() {
            self.gate_flags = extract_gate_flags(self.gate_flags, high && !(retrigger && k == 0));
            *f = self.gate_flags;
        }
        self.gate_high = high;
        flags
    }

    /// Renders the synth's next 12 frames, shaped and leveled, as (left, right) pairs; the block is
    /// rendered while the host block's frame `at` is due.
    fn render_block(&mut self, at: usize) -> [[f32; 2]; BLOCK] {
        let strike = self.strike.take();
        let set = self.settings;
        if let Some((_, velocity)) = strike {
            self.muted = false;
            self.velocity = velocity;
            for sh in &mut self.shapers {
                sh.note(&set, velocity, self.free_phase);
            }
            self.gate = true;
        } else if self.gate && !self.holding() {
            for sh in &mut self.shapers {
                sh.release();
            }
            self.gate = false;
        }
        let quarters = self.position(at);
        if let Some(q) = quarters {
            for sh in &mut self.shapers {
                sh.anchor(&set, q);
            }
        }
        let sums = self.sums();
        let sum = |d: usize| sums.get(d).copied().unwrap_or(0.0);
        let controls = self.controls(&sums);
        let flags = self.seed_flags(strike.is_some());
        let freeze = [self.p.on(FREEZE); BLOCK];
        let resolution = SynthResolution::ALL.get(self.p.choice(QUALITY)).copied().unwrap_or_default();
        let mut synth_out = [[0.0f32; 2]; BLOCK];
        self.synth.render(&controls, resolution, &freeze, Some(&flags), &mut synth_out);

        self.level.set(db_to_gain(self.p.v(LEVEL)));
        let (from, to) = (self.vel_gain, self.vel_target());
        self.vel_gain = to;
        let path = self.path(strike.is_some());
        let mods = FilterMods { cutoff: sum(dst::CUTOFF), resonance: sum(dst::RESONANCE), env_amount: sum(dst::ENV_AMOUNT) };
        let gains = voice_gains(sum(dst::LEVEL), sum(dst::PAN));
        let rate = |d: usize| RATE_OCTAVES * sum(d).clamp(-1.0, 1.0);
        let mut chans = [[0.0f32; BLOCK]; 2];
        for (c, (chan, sh)) in chans.iter_mut().zip(&mut self.shapers).enumerate() {
            for (y, f) in chan.iter_mut().zip(&synth_out) {
                *y = f[c];
            }
            // The filter always runs (on its own copy), so it is warm whenever the path turns to it.
            sh.design(&set, f32::from(self.key), mods);
            let mut wet = *chan;
            sh.process(&set, true, Fade::None, &mut wet);
            for (k, (y, w)) in chan.iter_mut().zip(wet).enumerate() {
                let t = (k + 1) as f32 / BLOCK as f32;
                *y = match path {
                    Path::Dry => *y,
                    Path::Shaped => w,
                    Path::In => *y + (w - *y) * t,
                    Path::Out => w + (*y - w) * t,
                };
            }
            sh.lfo_amount_mod = [sum(dst::LFO1_AMOUNT), sum(dst::LFO2_AMOUNT)];
            sh.advance_lfos(&set, self.tempo, [rate(dst::LFO1_RATE), rate(dst::LFO2_RATE)], quarters.is_some());
        }
        let mut frames = [[0.0f32; 2]; BLOCK];
        for (k, f) in frames.iter_mut().enumerate() {
            // A velocity step ramps over the block; a steady one is exact.
            let v = if from == to { to } else { from + (to - from) * (k + 1) as f32 / BLOCK as f32 };
            let g = self.level.next() * v * if self.muted { 0.0 } else { 1.0 };
            *f = [chans[0][k] * g * gains.0, chans[1][k] * g * gains.1];
        }
        for (phase, lfo) in self.free_phase.iter_mut().zip(&set.lfo) {
            let p = *phase + lfo.increment(self.tempo, 0.0, set.sr) * BLOCK as f64;
            *phase = p - p.floor();
        }
        frames
    }

    /// The shaper's path for a block: a switch while a key holds the sound (or at a note)
    /// crossfades over it; otherwise the path stays until the next note. Before anything is played
    /// the path follows the switch at once.
    fn path(&mut self, strike: bool) -> Path {
        let want = self.settings.on;
        let fade = want != self.shaped && !self.fresh && (strike || self.holding());
        if fade || self.fresh {
            self.shaped = want;
        }
        if strike {
            self.fresh = false;
        }
        match (fade, self.shaped) {
            (true, true) => Path::In,
            (true, false) => Path::Out,
            (false, true) => Path::Shaped,
            (false, false) => Path::Dry,
        }
    }
}

impl Plugin for TextureSynth {
    param_plumbing!(TEXTURE_SYNTH_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        let sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, ENGINE_RATE as f32);
        self.events.reserve(MAX_EVENTS.saturating_sub(self.events.len()));
        self.update();
        if self.restored && sr == self.host_sr {
            // A frozen state restored at this rate: keep it (converter included) for its first block.
            return;
        }
        self.host_sr = sr;
        (self.up, self.latency) = edge(&self.kernel, self.host_sr.round() as u32);
        if self.restored {
            // Restored at another rate: the synth plays on, the converter starts empty.
            return;
        }
        self.reset();
    }

    fn snap_initial_params(&mut self) {
        // A restored cloud and its converter must survive initialization. Level's target is
        // normally computed while rendering, so set it explicitly before snapping.
        self.level.set(db_to_gain(self.p.v(LEVEL)));
        self.level.snap();
    }

    fn reset(&mut self) {
        if self.restored {
            return;
        }
        // The synth back to its power-on state (in place), so a render after a reset plays as a new
        // instance.
        self.synth.reset();
        self.up.reset();
        self.events.clear();
        self.held = [false; 128];
        self.pairing.clear();
        self.pressed = [0; 128];
        self.presses = 0;
        self.pedalled = [false; 128];
        self.sustain = false;
        self.strike = None;
        self.gate_high = false;
        self.gate_flags = 0;
        self.key = 60;
        self.velocity = 1.0;
        self.release_velocity = 0.0;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.pressure = 0;
        self.poly = [0; 128];
        self.level.set(db_to_gain(self.p.v(LEVEL)));
        self.level.snap();
        self.vel_gain = 1.0;
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaped = self.settings.on;
        self.gate = false;
        self.fresh = true;
        self.muted = false;
        self.free_phase = [0.0; 2];
        self.anchor = None;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.answer_capture();
        self.restored = false;
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        sort_events(&mut self.events, |e| e.offset);
        for n in 0..frames {
            while !self.up.ready() {
                self.apply_due(n);
                let block = self.render_block(n);
                for f in block {
                    self.up.push(f);
                }
            }
            let [l, r] = self.up.pull(&self.kernel);
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                if let Some(x) = b.get_mut(n) {
                    *x = match (c, ch) {
                        (_, 1) => (l + r) * 0.5,
                        (0, _) => l,
                        (1, _) => r,
                        _ => (l + r) * 0.5,
                    };
                }
            }
        }
        self.next_pos = self.host_pos.saturating_add(frames as i64);
        // Events not reached wait for the next block, in the next host block.
        for e in &mut self.events {
            e.offset = e.offset.saturating_sub(frames);
        }
    }

    fn service_idle_state(&mut self) {
        self.answer_capture();
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.snapshot()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.restore(data)
    }

    fn state_reader(&mut self) -> Option<Box<dyn crate::PluginStateReader>> {
        let (capture, reader) = state::channel();
        self.capture = Some(capture);
        Some(Box::new(reader))
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn tail_samples(&self) -> usize {
        (self.host_sr * TAIL_SECONDS) as usize + self.latency
    }

    fn set_transport(&mut self, t: &Transport) {
        let tempo = if t.tempo.is_finite() && t.tempo > 0.0 { t.tempo.clamp(1.0, 999.0) } else { 120.0 };
        let bar = if t.numerator > 0 && t.denominator > 0 { f64::from(t.numerator) * 4.0 / f64::from(t.denominator) } else { 4.0 };
        let bar = bar.clamp(0.0625, 256.0);
        let jumped = t.samples != self.next_pos || tempo != self.tempo;
        self.anchor = match self.anchor {
            _ if !t.playing || !t.quarters.is_finite() => None,
            Some(a) if !jumped => Some(a),
            _ => Some((t.samples, t.quarters)),
        };
        self.host_pos = t.samples;
        self.next_pos = t.samples;
        self.tempo = tempo;
        if bar != self.bar_quarters || self.bar_origin != (t.bar, t.bar_start) {
            self.bar_quarters = bar;
            self.bar_origin = (t.bar, t.bar_start);
            self.settings = self.settings();
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
#[path = "texture_synth_tests.rs"]
mod tests;
