//! Modal Voice: transit's modal voice (`transit-modal-voice`, ADR 0033, #198 slice 2), a Rust port
//! of Émilie Gillet's MIT-licensed eurorack firmware (a gated exciter-and-resonator voice), played
//! from MIDI as an instrument.
//!
//! - The gate: the module is one voice with a real gate, and the instrument plays it mono. A key
//!   down raises the gate: the strike fires on its rising edge, the bow and the blow sound while it
//!   is high, under the module's envelope (Contour); the last key up lowers it and the voice
//!   releases (the strike's palm mute, the envelope's release). A new note while a key is held
//!   retriggers as a gate input struck again: the gate drops for one 16-frame block (0.5 ms) and
//!   rises at the new pitch. Releasing the sounding key while others are held returns to the newest
//!   of them, legato (the pitch moves, the gate stays up), as Ableton's and Logic's mono modes do.
//!   The sustain pedal holds the gate as a held key does (the module has a gate, so the pedal holds
//!   notes): keys released under it stay down until it lifts. CC 123 (All Notes Off) lowers the
//!   gate; CC 120 (All Sound Off) resets the part at once.
//! - Velocity is the module's strength input (its pressure CV, `1 - CV`, 0 to 1), which the
//!   firmware turns into a gain on the exciters through its 3 dB/V accent tables: strength =
//!   `1 - velocity_amount · (1 - velocity/127)`, so at Velocity 100% (the default) a note's strength
//!   is its velocity over 127 and at 0% every note plays at full strength (1.0). It is set at the
//!   note-on and held until the next one; the firmware ramps it over a block. One deviation: the
//!   module's FM voice also opens its output from the strength, so with nothing patched it drones;
//!   the instrument closes it when no key is held ([`gated_strength`]).
//!   The same goes for its dry path: at low Space the part mixes the FM pair's two oscillators
//!   (ungated) into the left channel, so the dry share is faded to 0 with the gate down
//!   ([`gated_raw`]).
//! - Pitch: the sounding key is the module's note (MIDI 69 plays 440 Hz, the firmware's own
//!   tuning at 32 kHz); Tune, pitch bend (`bend_range`) and the matrix's Pitch move it through the
//!   module's FM input (held to its ±60 semitones).
//! - transit's addition (#243, shared with the Macro Oscillator), not part of the original module:
//!   the shaper (amp and filter envelopes, a multimode filter, two LFOs) on the part's output, and
//!   the sixteen-row modulation matrix, with the Macro Oscillator's sources and the modal voice's
//!   destinations (the four resonator controls, the strength, the bow and the blow, pitch, level and
//!   the shaper's own). The shaper's envelopes follow the module's gate. Turning the shaper on or
//!   off while the gate is up takes effect at once with a 12-frame crossfade (the held-note rule of
//!   #262); with the gate down the switch waits for the next note. With the shaper off and no active
//!   row, the output is the part's, bit for bit.
//! - Rate and blocks: the part runs at its firmware rate, 32 kHz, in 16-frame blocks (its control
//!   block), whatever the host's block size. Events take effect at the first 16-frame block that
//!   starts at or after their offset (at most 15 engine frames, 0.47 ms, late); the matrix sets the
//!   part's controls once per block; the shaper and its matrix rows run on their own 12-frame grid at
//!   32 kHz, which the 16-frame blocks cross (three part blocks per four shaper blocks). At every
//!   session rate the output goes through the shared edge converter ([`super::resample`]) from
//!   32 kHz to the session rate, and the instrument reports its delay (`latency`); at a 32 kHz
//!   session nothing is converted and the latency is 0. Blocks are rendered as the converter needs
//!   them, counted in output frames, so any host block size renders the same bits.
//! - Output: the module's two jacks, its left output (aux: the center plus the side, with the dry
//!   exciter at low space) on the left channel and its right output (main) on the right; Space sets
//!   the dry level, the stereo spread and the reverb (from 175% the reverb freezes). Then `level`; a
//!   mono track hears the mean of the two.
//!
//! Parameters only: the part's state is not saved, as for every built-in. The two audio inputs that
//! would excite the part with a track's sound are not available to an instrument yet (#274); the
//! internal exciters play every note.

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
use transit_modal_voice::{MAX_BLOCK_SIZE, ModalVoice, Patch, PerformanceState, ResonatorModel};

#[path = "modal_voice_params.rs"]
mod params;

use params::*;

/// The firmware's rate, Hz: the part, the shaper and the matrix run at it.
pub const ENGINE_RATE: u32 = 32_000;

/// The part's control block, frames.
pub const BLOCK: usize = MAX_BLOCK_SIZE;

/// The shaper's block (the Macro Oscillator's), frames.
const SHAPER_BLOCK: usize = crate::plugins::macro_osc::BLOCK;

/// The models in the firmware's order, named by what they do; the last is the firmware's hidden
/// voice (two FM oscillator pairs instead of the exciters and the resonator).
pub const MODELS: [&str; 4] = ["Modal", "String", "Chord Strings", "FM Pair"];

/// The matrix's destinations, in the order of the `mod<n>_dest` choices: the Macro Oscillator's
/// list with the resonator's four controls in place of its four macros, then the strength, the bow
/// and the blow in the three extra slots.
pub const DESTINATIONS: [&str; NDST] = [
    "Off",
    "Pitch",
    "Geometry",
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
    "Strength",
    "Bow",
    "Blow",
];

/// The destinations of the modal voice's controls (the Macro Oscillator's Harmonics, Timbre, Morph
/// and Decay slots, then the extra ones).
const D_GEOMETRY: usize = dst::HARMONICS;
const D_BRIGHTNESS: usize = dst::TIMBRE;
const D_DAMPING: usize = dst::MORPH;
const D_POSITION: usize = dst::DECAY;
const D_STRENGTH: usize = dst::EXTRA1;
const D_BOW: usize = dst::EXTRA2;
const D_BLOW: usize = dst::EXTRA3;

/// The module's CV scaler ceiling for its controls (0.9995; the strike timbre's is 0.995).
pub const CONTROL_MAX: f32 = 0.9995;
const STRIKE_TIMBRE_MAX: f32 = 0.995;

/// Semitones the FM input (tune, bend and the matrix's Pitch) moves at most, either way: the
/// module's CV scaler range.
const MAX_FM: f32 = 60.0;

/// How long the output can ring after the gate falls, seconds (the strings at full damping, the
/// reverb's tail).
const TAIL_SECONDS: f32 = 30.0;

pub static MODAL_VOICE_INFO: PluginInfo = PluginInfo {
    id: "modal_voice",
    name: "Modal Voice",
    short_name: "Modal",
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
    /// All Notes Off (CC 123): the keys and the pedal let go, the gate falls.
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

/// The strength a note plays at: `1 - amount · (1 - velocity)`, so velocity itself at 100% and
/// full strength (1.0) at 0%.
#[must_use]
pub fn strength_of(velocity: f32, amount: f32) -> f32 {
    (1.0 - amount.clamp(0.0, 1.0) * (1.0 - velocity.clamp(0.0, 1.0))).clamp(0.0, 1.0)
}

/// The strength just under which the FM pair's output stays shut: its voice adds `2 · strength - 1`
/// to its level at 0.5 and above (the firmware's `ominous_voice.cc`, `level += strength >= 0.5f ? ...`).
pub const FM_SHUT: f32 = 0.5;

/// How far the FM pair's strength falls per part block with the gate down: 0.5 over 200 blocks
/// (100 ms at the engine rate), so closing the voice is a fade and not a step.
pub const FM_CLOSE_STEP: f32 = 0.5 / 200.0;

/// The strength the part is sent. With the gate up it is `strength`. The module's FM voice opens
/// its output from the strength input as well as its envelope, so with nothing patched it drones;
/// the instrument closes it when no key is held: with the gate down (and the FM pair selected) the
/// strength in `sent` falls from where it was to [`FM_SHUT`] and never above `strength`. The other
/// three models pass `strength` through.
pub fn gated_strength(sent: &mut f32, gate: bool, fm_pair: bool, strength: f32) -> f32 {
    if !fm_pair {
        *sent = FM_SHUT;
        return strength;
    }
    if gate {
        *sent = strength;
        return strength;
    }
    *sent = (*sent - FM_CLOSE_STEP).max(FM_SHUT);
    sent.min(strength)
}

/// How far the FM pair's dry share falls per part block with the gate down: the whole share over
/// 200 blocks (100 ms), the same fade as [`FM_CLOSE_STEP`].
pub const RAW_CLOSE_STEP: f32 = 1.0 / 200.0;

/// The share of the part's dry (raw) left-channel path, 0..=1. At Space of 10% or less the part
/// mixes the voice's raw signal into the left output, and the FM pair's raw signal is its two
/// oscillators, which no envelope gates. With a key held the share is 1 (the part's own Space rule,
/// untouched); with the gate down and the FM pair selected it falls to 0 in `sent` over 100 ms and
/// stays shut until the next gate-up. The other three models' raw signal is their exciter, which
/// is silent without a key, so their share is always 1.
pub fn gated_raw(sent: &mut f32, gate: bool, fm_pair: bool) -> f32 {
    if !fm_pair || gate {
        *sent = 1.0;
        return 1.0;
    }
    // The 200 steps do not sum to exactly 1 in f32: what is left under half a step is the end.
    *sent -= RAW_CLOSE_STEP;
    if *sent < RAW_CLOSE_STEP * 0.5 {
        *sent = 0.0;
    }
    *sent
}

/// A control (0..max) from its knob and its matrix sum: the knob exactly when no row moves it, else
/// the sum added and clamped to `0..=max`.
fn control(knob: f32, sum: f32, max: f32) -> f32 {
    if sum == 0.0 { knob.clamp(0.0, max) } else { (knob + sum).clamp(0.0, max) }
}

/// The edge from the part's 32 kHz to the session rate and its delay, session samples (0 at
/// 32 kHz: an exact copy). The delay is the converter's half length lined up on a whole session
/// sample.
fn edge(kernel: &Kernel, host: u32) -> (Converter, usize) {
    if host == ENGINE_RATE {
        return (Converter::new(kernel, ENGINE_RATE, host, 0), 0);
    }
    let (lp, mp) = ticks(ENGINE_RATE, host);
    let j = half_taps(ENGINE_RATE, host);
    let pad = (-(j * lp)).rem_euclid(mp);
    let latency = usize::try_from((j * lp + pad) / mp).unwrap_or(0);
    (Converter::new(kernel, ENGINE_RATE, host, pad), latency)
}

/// The shaper's path for one shaper block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    Dry,
    Shaped,
    /// Crossfading into the shaper over the shaper block.
    In,
    /// Crossfading out of it.
    Out,
}

/// The keys down (held, or kept by the pedal), oldest first: the newest is the sounding note.
#[derive(Debug, Clone, Copy)]
struct Keys {
    order: [u8; 128],
    len: usize,
    held: [bool; 128],
    pedalled: [bool; 128],
}

impl Keys {
    const fn new() -> Self {
        Keys { order: [0; 128], len: 0, held: [false; 128], pedalled: [false; 128] }
    }

    fn top(&self) -> Option<u8> {
        self.len.checked_sub(1).and_then(|i| self.order.get(i).copied())
    }

    fn remove(&mut self, key: u8) {
        if let Some(i) = self.order[..self.len].iter().position(|k| *k == key) {
            self.order.copy_within(i + 1..self.len, i);
            self.len -= 1;
        }
    }

    fn press(&mut self, key: u8) {
        self.remove(key);
        if let Some(slot) = self.order.get_mut(self.len) {
            *slot = key;
            self.len += 1;
        }
        let k = usize::from(key.min(127));
        self.held[k] = true;
        self.pedalled[k] = false;
    }

    /// A key let go: kept by the pedal while it is down, otherwise off the stack.
    fn release(&mut self, key: u8, pedal: bool) -> bool {
        let k = usize::from(key.min(127));
        if !self.held[k] {
            return false;
        }
        self.held[k] = false;
        if pedal {
            self.pedalled[k] = true;
        } else {
            self.remove(key);
        }
        true
    }

    /// The pedal lifted: the keys it kept go.
    fn lift_pedal(&mut self) {
        let mut i = 0;
        while i < self.len {
            let k = usize::from(self.order[i]);
            if self.pedalled[k] && !self.held[k] {
                self.pedalled[k] = false;
                self.remove(self.order[i]);
            } else {
                i += 1;
            }
        }
    }

    fn clear(&mut self) {
        *self = Keys::new();
    }
}

/// The modal voice as a MIDI instrument.
pub struct ModalVoiceInstrument {
    p: Params,
    part: Box<ModalVoice>,
    kernel: Kernel,
    up: Converter,
    latency: usize,
    host_sr: f32,
    ch: usize,
    events: Vec<Event>,
    keys: Keys,
    /// The open note-ons per key, so a note-off closes the oldest (#292).
    pairing: NotePairing,
    sustain: bool,
    /// A note arrived while the gate was up: the next block drops the gate to retrigger.
    retrigger: bool,
    /// The gate as the last block played it.
    gate: bool,
    /// The shapers' envelopes are gated (from a rising gate until no key is down; a retrigger's
    /// dropped block does not release them).
    shaper_gate: bool,
    /// The sounding key (the last one, while the gate is down), the newest note's velocity (0..1)
    /// and the last note-off's.
    key: u8,
    velocity: f32,
    /// The strength last sent to the FM pair ([`gated_strength`]).
    strength_sent: f32,
    /// The dry share last sent to the part ([`gated_raw`]); shut until the first gate-up.
    raw_sent: f32,
    /// A note-on since the last part block: the gate rises for one block even when its note-off
    /// arrived in the same engine block (a note shorter than 0.5 ms still plays).
    latch: bool,
    release_velocity: f32,
    bend: f32,
    wheel: f32,
    pressure: u8,
    poly: [u8; 128],
    macros: [f32; 8],
    model: usize,
    level: Smoother,
    /// One shaper per output (left on aux, right on main), driven alike: the same seed and the same
    /// calls, so their envelopes and LFOs are equal.
    shapers: [Shaper; 2],
    matrix: Matrix,
    settings: Settings,
    /// The frame of the shaper's 12-frame block the next part frame falls on.
    shaper_at: usize,
    /// The matrix sums and the path of the current shaper block.
    shaper_sums: [f32; NDST],
    shaper_path: Path,
    /// The shaper shapes the output (its switch as of the last note or held-gate switch).
    shaped: bool,
    /// Nothing played since the last reset: the shaper's switch applies without a crossfade.
    fresh: bool,
    /// A rising gate this part block, for the next shaper block's path.
    rose: bool,
    free_phase: [f64; 2],
    tempo: f64,
    bar_quarters: f64,
    bar_origin: (i64, f64),
    anchor: Option<(i64, f64)>,
    quarters: Option<f64>,
    host_pos: i64,
    next_pos: i64,
}

impl Default for ModalVoiceInstrument {
    fn default() -> Self {
        Self::new()
    }
}

/// The shapers' random seed: the part's power-on seed (stmlib's).
const SHAPER_SEED: u32 = transit_resonator::stmlib::random::Random::DEFAULT_SEED;

impl ModalVoiceInstrument {
    pub fn new() -> Self {
        let kernel = Kernel::new();
        let (up, latency) = edge(&kernel, ENGINE_RATE);
        let mut s = Self {
            p: Params::new(&PARAMS),
            part: Box::new(ModalVoice::new()),
            kernel,
            up,
            latency,
            host_sr: ENGINE_RATE as f32,
            ch: 2,
            events: Vec::with_capacity(MAX_EVENTS),
            keys: Keys::new(),
            pairing: NotePairing::new(),
            sustain: false,
            retrigger: false,
            gate: false,
            shaper_gate: false,
            key: 60,
            velocity: 1.0,
            strength_sent: FM_SHUT,
            raw_sent: 0.0,
            latch: false,
            release_velocity: 0.0,
            bend: 0.0,
            wheel: 0.0,
            pressure: 0,
            poly: [0; 128],
            macros: [0.0; 8],
            model: 0,
            level: Smoother::new(0.5),
            shapers: [Shaper::new(SHAPER_SEED), Shaper::new(SHAPER_SEED)],
            matrix: Matrix::default(),
            settings: Settings::default(),
            shaper_at: 0,
            shaper_sums: [0.0; NDST],
            shaper_path: Path::Dry,
            shaped: false,
            fresh: true,
            rose: false,
            free_phase: [0.0; 2],
            tempo: 120.0,
            bar_quarters: 4.0,
            bar_origin: (0, 0.0),
            anchor: None,
            quarters: None,
            host_pos: 0,
            next_pos: 0,
        };
        s.update();
        s.reset();
        s
    }

    fn update(&mut self) {
        self.level.set(db_to_gain(self.p.v(LEVEL)));
        let model = self.p.choice(MODEL).min(MODELS.len() - 1);
        if model != self.model {
            self.model = model;
            self.configure();
        }
        self.matrix = Matrix::new((0..matrix::ROWS).map(|r| self.row(r)));
        self.settings = self.settings();
    }

    /// The part's model and voice as the `model` choice sets them (they take effect on its next
    /// block, as on the module).
    fn configure(&mut self) {
        match self.model {
            3 => self.part.set_easter_egg(true),
            m => {
                self.part.set_easter_egg(false);
                self.part.set_model(ResonatorModel::ALL.get(m).copied().unwrap_or_default());
            }
        }
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

    /// All Sound Off: the part, the shapers and the edge converter back to power-on at once (the
    /// part's `reset` allocates nothing), the keys and the pedal let go. The queued events and the
    /// controllers stay.
    fn silence(&mut self) {
        self.part.reset();
        self.configure();
        self.up.reset();
        self.keys.clear();
        self.pairing.clear();
        self.sustain = false;
        self.retrigger = false;
        self.gate = false;
        self.shaper_gate = false;
        self.strength_sent = FM_SHUT;
        self.raw_sent = 0.0;
        self.latch = false;
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaped = self.settings.on;
    }

    fn apply(&mut self, kind: Kind) {
        match kind {
            Kind::On(key, 0) => self.apply(Kind::Off(key, 0)),
            Kind::On(key, vel) => {
                if let Some(p) = self.poly.get_mut(usize::from(key)) {
                    *p = 0;
                }
                let held = self.keys.held.get(usize::from(key)).copied().unwrap_or(false);
                self.pairing.on(0, key, held);
                if self.gate {
                    self.retrigger = true;
                }
                self.keys.press(key);
                self.latch = true;
                self.key = key;
                self.velocity = f32::from(vel) / 127.0;
            }
            Kind::Off(key, velocity) => {
                // An off that closes an earlier note-on of a key struck again since leaves the
                // newest note sounding (#292).
                self.pairing.off(0, key);
                if self.pairing.open(0, key) > 0 {
                    return;
                }
                if self.keys.release(key, self.sustain) {
                    self.release_velocity = f32::from(velocity.min(127)) / 127.0;
                    if let Some(top) = self.keys.top() {
                        self.key = top;
                    }
                }
            }
            Kind::AllOff => {
                self.latch = false;
                self.keys.clear();
                self.pairing.clear();
                self.sustain = false;
                self.retrigger = false;
            }
            Kind::SoundOff => self.silence(),
            Kind::Sustain(on) => {
                self.sustain = on;
                if !on {
                    self.keys.lift_pedal();
                    if let Some(top) = self.keys.top() {
                        self.key = top;
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

    /// The gate for the next part block, the shapers following it: up while a key is down, except
    /// for the one block a retrigger drops it.
    fn next_gate(&mut self) -> bool {
        let latched = std::mem::take(&mut self.latch);
        let down = self.keys.top().is_some() || latched;
        let gate = if self.retrigger && self.gate {
            self.retrigger = false;
            false
        } else {
            self.retrigger = false;
            down
        };
        let set = self.settings;
        if gate && !self.gate {
            for sh in &mut self.shapers {
                sh.note(&set, self.velocity, self.free_phase);
            }
            self.shaper_gate = true;
            self.rose = true;
        } else if !down && self.shaper_gate {
            for sh in &mut self.shapers {
                sh.release();
            }
            self.shaper_gate = false;
        }
        self.gate = gate;
        gate
    }

    /// Renders the part's next 16 frames, shaped and leveled, as (left, right) pairs; the block is
    /// rendered while the host block's frame `at` is due.
    fn render_block(&mut self, at: usize) -> [[f32; 2]; BLOCK] {
        let gate = self.next_gate();
        self.quarters = self.position(at);
        if let Some(q) = self.quarters {
            let set = self.settings;
            for sh in &mut self.shapers {
                sh.anchor(&set, q);
            }
        }
        let sums = self.sums();
        let sum = |d: usize| sums.get(d).copied().unwrap_or(0.0);
        let pct = |i: usize| self.p.v(i) / 100.0;
        let knob = |i: usize| control(pct(i), 0.0, CONTROL_MAX);
        {
            let patch: &mut Patch = self.part.patch_mut();
            patch.exciter_envelope_shape = knob(CONTOUR);
            patch.exciter_bow_level = control(pct(BOW_LEVEL), sum(D_BOW), CONTROL_MAX);
            patch.exciter_bow_timbre = knob(BOW_TIMBRE);
            patch.exciter_blow_level = control(pct(BLOW_LEVEL), sum(D_BLOW), CONTROL_MAX);
            patch.exciter_blow_meta = knob(FLOW);
            patch.exciter_blow_timbre = knob(BLOW_TIMBRE);
            patch.exciter_strike_level = knob(STRIKE_LEVEL);
            patch.exciter_strike_meta = knob(MALLET);
            patch.exciter_strike_timbre = control(pct(STRIKE_TIMBRE), 0.0, STRIKE_TIMBRE_MAX);
            patch.resonator_geometry = control(pct(GEOMETRY), sum(D_GEOMETRY), CONTROL_MAX);
            patch.resonator_brightness = control(pct(BRIGHTNESS), sum(D_BRIGHTNESS), CONTROL_MAX);
            patch.resonator_damping = control(pct(DAMPING), sum(D_DAMPING), CONTROL_MAX);
            patch.resonator_position = control(pct(POSITION), sum(D_POSITION), CONTROL_MAX);
            patch.space = control(pct(SPACE), 0.0, 2.0);
        }
        let pitch = sum(dst::PITCH);
        let fm = self.p.v(TUNE) + self.bend * self.p.v(BEND_RANGE) + if pitch == 0.0 { 0.0 } else { PITCH_SEMITONES * pitch };
        let strength = control(strength_of(self.velocity, pct(VELOCITY)), sum(D_STRENGTH), 1.0);
        let strength = gated_strength(&mut self.strength_sent, gate, self.model == 3, strength);
        let raw = gated_raw(&mut self.raw_sent, gate, self.model == 3);
        self.part.set_raw_share(raw);
        let performance =
            PerformanceState { gate, note: f32::from(self.key), modulation: if fm.is_finite() { fm.clamp(-MAX_FM, MAX_FM) } else { 0.0 }, strength };
        let (mut main, mut aux) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        self.part.process(&performance, &[0.0; BLOCK], &[0.0; BLOCK], &mut main, &mut aux);
        self.shape(aux, main)
    }

    /// Runs the part block through the shaper on the shaper's own 12-frame grid (a new shaper block
    /// evaluates the matrix and the filter, an ended one advances the LFOs) and the level.
    fn shape(&mut self, left: [f32; BLOCK], right: [f32; BLOCK]) -> [[f32; 2]; BLOCK] {
        let set = self.settings;
        let mut frames = [[0.0f32; 2]; BLOCK];
        let mut k = 0;
        while k < BLOCK {
            if self.shaper_at == 0 {
                self.shaper_sums = self.sums();
                let rose = std::mem::take(&mut self.rose);
                self.shaper_path = self.path(rose);
                let s = self.shaper_sums;
                let mods = FilterMods { cutoff: s[dst::CUTOFF], resonance: s[dst::RESONANCE], env_amount: s[dst::ENV_AMOUNT] };
                for sh in &mut self.shapers {
                    sh.design(&set, f32::from(self.key), mods);
                }
            }
            let take = (BLOCK - k).min(SHAPER_BLOCK - self.shaper_at);
            let range = k..k + take;
            let s = self.shaper_sums;
            let gains = voice_gains(s[dst::LEVEL], s[dst::PAN]);
            let path = self.shaper_path;
            let mut out = [[0.0f32; SHAPER_BLOCK]; 2];
            for ((o, sh), src) in out.iter_mut().zip(&mut self.shapers).zip([&left, &right]) {
                let dry = src.get(range.clone()).unwrap_or(&[]);
                let o = o.get_mut(..take).unwrap_or_default();
                o.copy_from_slice(dry);
                // The filter always runs (on its own copy), so it is warm whenever the path turns to it.
                let mut wet = [0.0f32; SHAPER_BLOCK];
                let wet = wet.get_mut(..take).unwrap_or_default();
                wet.copy_from_slice(dry);
                sh.process(&set, true, Fade::None, wet);
                for (j, (y, w)) in o.iter_mut().zip(wet.iter()).enumerate() {
                    let t = (self.shaper_at + j + 1) as f32 / SHAPER_BLOCK as f32;
                    *y = match path {
                        Path::Dry => *y,
                        Path::Shaped => *w,
                        Path::In => *y + (*w - *y) * t,
                        Path::Out => *w + (*y - *w) * t,
                    };
                }
            }
            for (j, f) in frames.get_mut(range).unwrap_or_default().iter_mut().enumerate() {
                let g = self.level.next();
                *f = [out[0][j] * g * gains.0, out[1][j] * g * gains.1];
            }
            self.shaper_at += take;
            k += take;
            if self.shaper_at == SHAPER_BLOCK {
                self.shaper_at = 0;
                let rate = |d: usize| RATE_OCTAVES * s[d].clamp(-1.0, 1.0);
                let anchored = self.quarters.is_some();
                for sh in &mut self.shapers {
                    sh.lfo_amount_mod = [s[dst::LFO1_AMOUNT], s[dst::LFO2_AMOUNT]];
                    sh.advance_lfos(&set, self.tempo, [rate(dst::LFO1_RATE), rate(dst::LFO2_RATE)], anchored);
                }
                for (phase, lfo) in self.free_phase.iter_mut().zip(&set.lfo) {
                    let p = *phase + lfo.increment(self.tempo, 0.0, set.sr) * SHAPER_BLOCK as f64;
                    *phase = p - p.floor();
                }
            }
        }
        frames
    }

    /// The shaper's path for a shaper block: a switch while the gate is up (or at a note) crossfades
    /// over it; otherwise the path stays until the next note. Before anything is played the path
    /// follows the switch at once.
    fn path(&mut self, note: bool) -> Path {
        let want = self.settings.on;
        let fade = want != self.shaped && !self.fresh && (note || self.gate);
        if fade || self.fresh {
            self.shaped = want;
        }
        if note {
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

impl Plugin for ModalVoiceInstrument {
    param_plumbing!(MODAL_VOICE_INFO);

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
        // The part back to its power-on state with the parameters' model, so a render after a
        // reset plays as a new instance.
        self.part.reset();
        self.configure();
        self.up.reset();
        self.events.clear();
        self.keys.clear();
        self.pairing.clear();
        self.sustain = false;
        self.retrigger = false;
        self.gate = false;
        self.shaper_gate = false;
        self.key = 60;
        self.velocity = 1.0;
        self.strength_sent = FM_SHUT;
        self.raw_sent = 0.0;
        self.latch = false;
        self.release_velocity = 0.0;
        self.bend = 0.0;
        self.wheel = 0.0;
        self.pressure = 0;
        self.poly = [0; 128];
        self.level.snap();
        for sh in &mut self.shapers {
            sh.reset();
        }
        self.shaper_at = 0;
        self.shaper_sums = [0.0; NDST];
        self.shaper_path = Path::Dry;
        self.shaped = self.settings.on;
        self.fresh = true;
        self.rose = false;
        self.free_phase = [0.0; 2];
        self.anchor = None;
        self.quarters = None;
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
#[path = "modal_voice_tests.rs"]
mod tests;
