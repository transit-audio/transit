//! Resonator: transit's resonator (`transit-resonator`, ADR 0033, #198), a Rust port of Émilie
//! Gillet's MIT-licensed eurorack firmware (six physical-modeling models), played from MIDI as an
//! instrument.
//!
//! - Voices: the firmware's own rotation is the allocator. Each MIDI note-on is a strum: the part's
//!   next voice takes the note and its internal exciter strikes it (polyphony 1, 2 or 4, as the
//!   module's panel offers; the voices ring on together and a new strum takes the next one in
//!   turn). The firmware takes one strum per 24-frame block, so notes that arrive in the same block
//!   (a chord) strum on consecutive blocks, 0.5 ms apart, in arrival order, as a fast strum.
//! - The resonator has no gate: a note-off does not stop a ringing voice and the sustain pedal
//!   does nothing to it (the damping decides how long a strum rings). Velocity is the accent: it
//!   darkens Brightness by `accent · (1 − velocity)` of its value from that strum on, the way a
//!   velocity CV patched into the module's brightness input with the strum would (at full velocity,
//!   or Accent 0, Brightness is the knob). The chord of the Chord Strings model follows Structure,
//!   as the module's CV scaler picks it (without its hysteresis).
//! - Pitch: a note-on's key is the voice's pitch (`note = key − tonic` over the module's tonic, so
//!   MIDI 69 plays 440 Hz, the firmware's own tuning at 48 kHz). Tune, pitch bend (`bend_range`) and
//!   the matrix's Pitch move every voice at once, through the module's FM input. The tonic is where
//!   the sympathetic strings drone at low Structure; it does not move the played pitch.
//! - transit's addition (#243, shared with the Macro Oscillator), not part of the original module:
//!   the shaper (amp and filter envelopes, a multimode filter, two LFOs) on the part's output, and
//!   the sixteen-row modulation matrix, with the Macro Oscillator's sources and the resonator's
//!   destinations. The part is one sound source, so there is one shaper: its envelopes gate on at
//!   each strum and release when no key is held (the pedal holds them, as a held key does). Turning
//!   the shaper on or off while a key is held takes effect at once with a 12-frame crossfade (the
//!   held-note rule of #262); with no key held the switch waits for the next strum. With the shaper
//!   off and no active row, the output is the part's, bit for bit.
//! - Rate and blocks: the part runs at its firmware rate, 48 kHz, in 24-frame blocks (its control
//!   block), whatever the host's block size. Events take effect at the first 24-frame block that
//!   starts at or after their offset (at 48 kHz at most 0.5 ms late), the Macro Oscillator's rule
//!   on its 12-frame grid applied to the resonator's block; the shaper and the matrix run per
//!   12-frame half block (the shaper's own block), the part's macros once per 24 frames. At any
//!   other session rate the output goes through the shared edge converter
//!   ([`super::resample`]) from 48 kHz to the session rate, and the instrument reports its delay
//!   (`latency`); at 48 kHz nothing is converted and the latency is 0. Blocks are rendered as the
//!   converter needs them, counted in output frames, so any host block size renders the same bits.
//! - Output: the part's two outputs (at polyphony 2 and 4 the odd and the even voices, so strums
//!   alternate between them; at polyphony 1 the two sets of modes or pickups) on the left and right
//!   channels. `spread` at 100% keeps them apart as the module's two jacks do; lower, each side takes
//!   some of the other, down to the module's mono mix (the two summed, what its first jack carries
//!   alone) on both at 0%; 50% by default. Then `level`; a mono track hears the mean of the two.
//!
//! Parameters only: the part's state is not saved, as for every built-in. The audio input that
//! would excite the part with a track's sound is not available to an instrument yet (see the
//! ADR 0033 note); the internal exciter plays every strum.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, CC_SUSTAIN, Message, PluginEvent};
use crate::params::{Params, param_plumbing};
use crate::plugins::instrument_queue::{AdsrRates, MAX_EVENTS, RELEASE_RESERVE};
use crate::plugins::macro_osc::matrix::{self, Matrix, NDST, NSRC, PITCH_SEMITONES, RATE_OCTAVES, Row, dst, src};
use crate::plugins::macro_osc::shaper::{self, Fade, FilterMods, LfoSettings, Settings, Shaper};
use crate::plugins::macro_osc::voice::{key_source, voice_gains};
use crate::plugins::resample::{Converter, Kernel, half_taps, ticks};
use crate::util::{Smoother, frames_in, sane_channels, sane_sr, sort_events};
use crate::{Category, Plugin, PluginInfo, Transport, db_to_gain};
use transit_instrument_core::pairing::NotePairing;
use transit_resonator::{CHORD_COUNT, MAX_BLOCK_SIZE, Patch, PerformanceState, Resonator, ResonatorModel};

#[path = "resonator_params.rs"]
mod params;

use params::*;

/// The firmware's rate, Hz: the part, the shaper and the matrix run at it.
pub const ENGINE_RATE: u32 = 48_000;

/// The part's control block, frames.
pub const BLOCK: usize = MAX_BLOCK_SIZE;

/// The shaper's block (the Macro Oscillator's), frames: two per part block.
const HALF: usize = crate::plugins::macro_osc::BLOCK;

/// The models in the firmware's order, named by what they do.
pub const MODELS: [&str; 6] = ["Modal", "Sympathetic Strings", "Inharmonic String", "FM Voice", "Chord Strings", "String and Reverb"];

/// `polyphony` choices: the module's panel offers 1, 2 and 4 voices.
pub const POLYPHONIES: [&str; 3] = ["1", "2", "4"];

/// The matrix's destinations, in the order of the `mod<n>_dest` choices: the Macro Oscillator's
/// list with the resonator's four macros in place of its four.
pub const DESTINATIONS: [&str; matrix::DESTINATIONS.len()] = [
    "Off",
    "Pitch",
    "Structure",
    "Brightness",
    "Damping",
    "Position",
    "Level",
    "Cutoff",
    "Resonance",
    "Filter env amount",
    "LFO 1 rate",
    "LFO 2 rate",
    "LFO 1 amount",
    "LFO 2 amount",
    "Pan",
];

/// The destinations of the resonator's macros (the Macro Oscillator's Harmonics, Timbre, Morph and
/// Decay slots).
const D_STRUCTURE: usize = dst::HARMONICS;
const D_BRIGHTNESS: usize = dst::TIMBRE;
const D_DAMPING: usize = dst::MORPH;
const D_POSITION: usize = dst::DECAY;

/// The lowest tonic (`tonic` choice 0), MIDI: the module's `12 + transpose`.
const TONIC_LOW: f32 = 12.0;

/// `tonic` choices: MIDI 12 to 72, the fixed names with middle C (MIDI 60) as C3 (what the
/// control channel lists; the UI names them in its Middle C setting through [`TONIC_LABELS`]).
pub const TONICS: [&str; 61] = [
    "C-1", "C#-1", "D-1", "D#-1", "E-1", "F-1", "F#-1", "G-1", "G#-1", "A-1", "A#-1", "B-1", "C0", "C#0", "D0", "D#0", "E0", "F0", "F#0", "G0",
    "G#0", "A0", "A#0", "B0", "C1", "C#1", "D1", "D#1", "E1", "F1", "F#1", "G1", "G#1", "A1", "A#1", "B1", "C2", "C#2", "D2", "D#2", "E2", "F2",
    "F#2", "G2", "G#2", "A2", "A#2", "B2", "C3", "C#3", "D3", "D#3", "E3", "F3", "F#3", "G3", "G#3", "A3", "A#3", "B3", "C4",
];

/// The Tonic's note labels (ADR 0051): entry `i` is MIDI `12 + i`.
pub const TONIC_LABELS: crate::NoteLabels = crate::NoteLabels { first_note: 12, step: 1, octave_only: false };

/// The module's structure ceiling (its CV scaler stops the knob at 0.9995).
pub const STRUCTURE_MAX: f32 = 0.9995;

/// Strums waiting for a block (a chord's notes); more than this in one go drops the oldest.
const MAX_STRUMS: usize = 16;

/// Semitones the FM input (tune, bend and the matrix's Pitch) moves at most, either way.
const MAX_FM: f32 = 48.0;

/// How long the output can ring after the last strum, seconds (the strings at full damping).
const TAIL_SECONDS: f32 = 30.0;

pub static RESONATOR_INFO: PluginInfo = PluginInfo {
    id: "resonator",
    name: "Resonator",
    short_name: "Reson",
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
    /// All Notes Off (CC 123): release the keys, let the part ring down.
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

/// The chord the module's CV scaler picks for a structure (without its hysteresis), as the
/// oracle's sweep does.
#[must_use]
pub fn chord_of(structure: f32) -> i32 {
    ((structure * (CHORD_COUNT - 1) as f32 + 0.5) as i32).clamp(0, CHORD_COUNT - 1)
}

/// Brightness after the accent: the knob darkened by `accent · (1 − velocity)` of itself, so a
/// full-velocity strum (or accent 0) leaves the knob exactly.
#[must_use]
pub fn accented(brightness: f32, accent: f32, velocity: f32) -> f32 {
    if velocity >= 1.0 || accent <= 0.0 { brightness } else { brightness * (1.0 - accent * (1.0 - velocity)) }
}

/// A macro (0..1) from its knob and its matrix sum: the knob exactly when no row moves it, else
/// the sum added and clamped to `0..=max`.
fn macro_value(knob: f32, sum: f32, max: f32) -> f32 {
    if sum == 0.0 { knob.min(max) } else { (knob + sum).clamp(0.0, max) }
}

/// The edge from the part's 48 kHz to the session rate and its delay, session samples (0 at 48 kHz:
/// an exact copy). The delay is the converter's half length lined up on a whole session sample.
pub(crate) fn edge(kernel: &Kernel, host: u32) -> (Converter, usize) {
    if host == ENGINE_RATE {
        return (Converter::new(kernel, ENGINE_RATE, host, 0), 0);
    }
    let (lp, mp) = ticks(ENGINE_RATE, host);
    let j = half_taps(ENGINE_RATE, host);
    let pad = (-(j * lp)).rem_euclid(mp);
    let latency = usize::try_from((j * lp + pad) / mp).unwrap_or(0);
    (Converter::new(kernel, ENGINE_RATE, host, pad), latency)
}

/// The shaper's path for one half block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Dry,
    Shaped,
    /// Crossfading into the shaper over the half block.
    In,
    /// Crossfading out of it.
    Out,
}

/// The resonator as a MIDI instrument.
pub struct ResonatorInstrument {
    p: Params,
    part: Box<Resonator>,
    kernel: Kernel,
    up: Converter,
    latency: usize,
    host_sr: f32,
    ch: usize,
    events: Vec<Event>,
    /// Strums waiting for their block (key, velocity 0..1), oldest first, a ring of `strum_len`
    /// from `strum_at`.
    strums: [(u8, f32); MAX_STRUMS],
    strum_at: usize,
    strum_len: usize,
    /// Keys held down, and keys released while the pedal was down.
    held: [bool; 128],
    /// The open note-ons per key, so a note-off closes the oldest (#292).
    pairing: NotePairing,
    pedalled: [bool; 128],
    sustain: bool,
    /// The newest strum's key and velocity (0..1), and the last note-off's velocity.
    key: u8,
    velocity: f32,
    release_velocity: f32,
    bend: f32,
    wheel: f32,
    pressure: u8,
    poly: [u8; 128],
    macros: [f32; 8],
    model: ResonatorModel,
    polyphony: usize,
    level: Smoother,
    /// One shaper per output (left on `out`, right on `aux`), driven alike: the same seed and the
    /// same calls, so their envelopes and LFOs are equal.
    shapers: [Shaper; 2],
    matrix: Matrix,
    settings: Settings,
    /// The shaper shapes the output (its switch as of the last strum or held-note switch).
    shaped: bool,
    /// The envelopes are gated.
    gate: bool,
    /// Nothing struck since the last reset: the shaper's switch applies without a crossfade.
    fresh: bool,
    free_phase: [f64; 2],
    tempo: f64,
    bar_quarters: f64,
    bar_origin: (i64, f64),
    anchor: Option<(i64, f64)>,
    host_pos: i64,
    next_pos: i64,
}

impl Default for ResonatorInstrument {
    fn default() -> Self {
        Self::new()
    }
}

/// The shapers' random seed: the part's power-on seed.
const SHAPER_SEED: u32 = transit_resonator::stmlib::random::Random::DEFAULT_SEED;

/// The part as the instrument plays it: the firmware's, but a chord's notes keep their own pitches
/// (#521). The instrument strums one queued note per 24-frame block, so the notes of a chord strum
/// on consecutive blocks, closer than the module's trigger input ever strums; the part's
/// `set_strum_keeps_voice_notes` keeps each just-plucked voice on its note there and leaves every
/// other strum the firmware's, bit for bit. Kept across the part's `reset`.
fn chord_ready_part() -> Resonator {
    let mut part = Resonator::new();
    part.set_strum_keeps_voice_notes(true);
    part
}

impl ResonatorInstrument {
    pub fn new() -> Self {
        let kernel = Kernel::new();
        let (up, latency) = edge(&kernel, ENGINE_RATE);
        let mut s = Self {
            p: Params::new(&PARAMS),
            part: Box::new(chord_ready_part()),
            kernel,
            up,
            latency,
            host_sr: ENGINE_RATE as f32,
            ch: 2,
            events: Vec::with_capacity(MAX_EVENTS),
            strums: [(0, 0.0); MAX_STRUMS],
            strum_at: 0,
            strum_len: 0,
            held: [false; 128],
            pairing: NotePairing::new(),
            pedalled: [false; 128],
            sustain: false,
            key: 60,
            velocity: 1.0,
            release_velocity: 0.0,
            bend: 0.0,
            wheel: 0.0,
            pressure: 0,
            poly: [0; 128],
            macros: [0.0; 8],
            model: ResonatorModel::Modal,
            polyphony: 1,
            level: Smoother::new(0.5),
            shapers: [Shaper::new(SHAPER_SEED), Shaper::new(SHAPER_SEED)],
            matrix: Matrix::default(),
            settings: Settings::default(),
            shaped: false,
            gate: false,
            fresh: true,
            free_phase: [0.0; 2],
            tempo: 120.0,
            bar_quarters: 4.0,
            bar_origin: (0, 0.0),
            anchor: None,
            host_pos: 0,
            next_pos: 0,
        };
        s.update();
        s.reset();
        s
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(LEVEL)));
        let model = ResonatorModel::ALL.get(self.p.choice(MODEL)).copied().unwrap_or_default();
        if model != self.model {
            self.part.set_model(model);
            self.model = model;
        }
        let polyphony = match self.p.choice(POLYPHONY) {
            0 => 1,
            1 => 2,
            _ => 4,
        };
        if polyphony != self.polyphony {
            self.part.set_polyphony(polyphony);
            self.polyphony = polyphony;
        }
        self.matrix = Matrix::new((0..matrix::ROWS).map(|r| self.row(r)));
        self.settings = self.settings();
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

    /// The shaper's settings from the parameters, at the part's rate.
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

    fn queue_strum(&mut self, key: u8, velocity: f32) {
        if self.strum_len == MAX_STRUMS {
            self.strum_at = (self.strum_at + 1) % MAX_STRUMS;
            self.strum_len -= 1;
        }
        if let Some(s) = self.strums.get_mut((self.strum_at + self.strum_len) % MAX_STRUMS) {
            *s = (key, velocity);
        }
        self.strum_len += 1;
    }

    fn next_strum(&mut self) -> Option<(u8, f32)> {
        if self.strum_len == 0 {
            return None;
        }
        let s = self.strums.get(self.strum_at).copied();
        self.strum_at = (self.strum_at + 1) % MAX_STRUMS;
        self.strum_len -= 1;
        s
    }

    /// A key is held, or kept by the pedal.
    fn holding(&self) -> bool {
        self.held.iter().zip(&self.pedalled).any(|(h, p)| *h || *p)
    }

    /// All Sound Off: the part, the shapers and the edge converter back to power-on at once (the
    /// part's `reset` allocates nothing), the keys, the pedal and the queued strums cleared. The
    /// queued events and the controllers stay.
    fn silence(&mut self) {
        self.part.reset();
        self.part.set_model(self.model);
        if self.polyphony != 1 {
            self.part.set_polyphony(self.polyphony);
        }
        self.up.reset();
        self.strum_at = 0;
        self.strum_len = 0;
        self.held = [false; 128];
        self.pairing.clear();
        self.pedalled = [false; 128];
        self.sustain = false;
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaped = self.settings.on;
        self.gate = false;
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
                self.queue_strum(key, f32::from(vel) / 127.0);
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
                }
            }
            Kind::AllOff => {
                self.held = [false; 128];
                self.pairing.clear();
                self.pedalled = [false; 128];
                self.sustain = false;
                self.strum_len = 0;
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

    /// Renders the part's next 24 frames, shaped and leveled, as (left, right) pairs; the block is
    /// rendered while the host block's frame `at` is due.
    fn render_block(&mut self, at: usize) -> [[f32; 2]; BLOCK] {
        let strum = self.next_strum();
        let set = self.settings;
        if let Some((key, velocity)) = strum {
            self.key = key;
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
        let sum = |s: &[f32; NDST], d: usize| s.get(d).copied().unwrap_or(0.0);
        let pct = |i: usize| self.p.v(i) / 100.0;
        let brightness = accented(pct(BRIGHTNESS), pct(ACCENT), self.velocity);
        let structure = macro_value(pct(STRUCTURE), sum(&sums, D_STRUCTURE), STRUCTURE_MAX);
        let patch = Patch {
            structure,
            brightness: macro_value(brightness, sum(&sums, D_BRIGHTNESS), 1.0),
            damping: macro_value(pct(DAMPING), sum(&sums, D_DAMPING), 1.0),
            position: macro_value(pct(POSITION), sum(&sums, D_POSITION), 1.0),
        };
        let pitch = sum(&sums, dst::PITCH);
        let fm = self.p.v(TUNE) + self.bend * self.p.v(BEND_RANGE) + if pitch == 0.0 { 0.0 } else { PITCH_SEMITONES * pitch };
        let tonic = TONIC_LOW + self.p.choice(TONIC).min(TONICS.len() - 1) as f32;
        let performance = PerformanceState {
            strum: strum.is_some(),
            internal_exciter: true,
            internal_strum: false,
            internal_note: false,
            tonic,
            note: f32::from(self.key) - tonic,
            fm: if fm.is_finite() { fm.clamp(-MAX_FM, MAX_FM) } else { 0.0 },
            chord: chord_of(structure),
        };
        let (mut out, mut aux) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        self.part.process(&performance, &patch, &[0.0; BLOCK], &mut out, &mut aux);
        // Spread: at 100% the two outputs as they are; below, each takes some of the other, down
        // to the module's mono mix (both outputs summed on each side) at 0%.
        let feed = 1.0 - pct(SPREAD).clamp(0.0, 1.0);
        if feed > 0.0 {
            for (o, a) in out.iter_mut().zip(aux.iter_mut()) {
                (*o, *a) = (*o + feed * *a, *a + feed * *o);
            }
        }

        let mut frames = [[0.0f32; 2]; BLOCK];
        for h in 0..BLOCK / HALF {
            let sums = if h == 0 { sums } else { self.sums() };
            let path = self.path(h == 0 && strum.is_some());
            let mods = FilterMods { cutoff: sum(&sums, dst::CUTOFF), resonance: sum(&sums, dst::RESONANCE), env_amount: sum(&sums, dst::ENV_AMOUNT) };
            let range = h * HALF..(h + 1) * HALF;
            let gains = voice_gains(sum(&sums, dst::LEVEL), sum(&sums, dst::PAN));
            let rate = |d: usize| RATE_OCTAVES * sum(&sums, d).clamp(-1.0, 1.0);
            let mut halves = [[0.0f32; HALF]; 2];
            for ((half, sh), src) in halves.iter_mut().zip(&mut self.shapers).zip([&out, &aux]) {
                let dry = src.get(range.clone()).unwrap_or(&[]);
                half.copy_from_slice(dry);
                // The filter always runs (on its own copy), so it is warm whenever the path turns to it.
                sh.design(&set, f32::from(self.key), mods);
                let mut wet = *half;
                sh.process(&set, true, Fade::None, &mut wet);
                for (k, (y, w)) in half.iter_mut().zip(wet).enumerate() {
                    let t = (k + 1) as f32 / HALF as f32;
                    *y = match path {
                        Path::Dry => *y,
                        Path::Shaped => w,
                        Path::In => *y + (w - *y) * t,
                        Path::Out => w + (*y - w) * t,
                    };
                }
                sh.lfo_amount_mod = [sum(&sums, dst::LFO1_AMOUNT), sum(&sums, dst::LFO2_AMOUNT)];
                sh.advance_lfos(&set, self.tempo, [rate(dst::LFO1_RATE), rate(dst::LFO2_RATE)], quarters.is_some());
            }
            for (k, f) in frames.get_mut(range).unwrap_or_default().iter_mut().enumerate() {
                let g = self.level.next();
                let (l, r) = (halves[0][k], halves[1][k]);
                *f = [l * g * gains.0, r * g * gains.1];
            }
            for (phase, lfo) in self.free_phase.iter_mut().zip(&set.lfo) {
                let p = *phase + lfo.increment(self.tempo, 0.0, set.sr) * HALF as f64;
                *phase = p - p.floor();
            }
        }
        frames
    }

    /// The shaper's path for a half block: a switch while a key holds the sound (or at a strum)
    /// crossfades over it; otherwise the path stays until the next strum. Before anything is
    /// struck the path follows the switch at once.
    fn path(&mut self, strum: bool) -> Path {
        let want = self.settings.on;
        let fade = want != self.shaped && !self.fresh && (strum || self.holding());
        if fade || self.fresh {
            self.shaped = want;
        }
        if strum {
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

impl Plugin for ResonatorInstrument {
    param_plumbing!(RESONATOR_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.host_sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        (self.up, self.latency) = edge(&self.kernel, self.host_sr.round() as u32);
        self.level.set_time(20.0, ENGINE_RATE as f32);
        self.events.reserve(MAX_EVENTS.saturating_sub(self.events.len()));
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        // The part back to its power-on state with the parameters' model and polyphony, so a
        // render after a reset plays as a new instance.
        self.part.reset();
        self.part.set_model(self.model);
        if self.polyphony != 1 {
            self.part.set_polyphony(self.polyphony);
        }
        self.up.reset();
        self.events.clear();
        self.strum_at = 0;
        self.strum_len = 0;
        self.held = [false; 128];
        self.pairing.clear();
        self.pedalled = [false; 128];
        self.sustain = false;
        self.key = 60;
        self.velocity = 1.0;
        self.release_velocity = 0.0;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.pressure = 0;
        self.poly = [0; 128];
        self.level.snap();
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaped = self.settings.on;
        self.gate = false;
        self.fresh = true;
        self.free_phase = [0.0; 2];
        self.anchor = None;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
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
#[path = "resonator_tests.rs"]
mod tests;
