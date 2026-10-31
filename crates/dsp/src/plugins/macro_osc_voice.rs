//! One voice of the Macro Oscillator (#196): a `transit_macro_osc::Voice` with the per-voice MIDI
//! state the mono instrument of #195 kept for its one voice (the keys it holds, its note, its
//! velocity, its trigger), its shaper and matrix sources (#243), and the bookkeeping the allocator
//! reads (age, loudness, idle).
//!
//! - A voice is **idle** once it holds no key, its trigger is low and both its outputs have stayed
//!   within one int16 LSB of silence (-90 dBFS) for [`QUIET_SECONDS`]: some models settle on a
//!   +-1 LSB residue rather than exact zero (six-op FM 2 does). A voice whose phrase started with
//!   the shaper on is idle as soon as it holds no key and its amp envelope has finished. An idle voice is not rendered,
//!   costs nothing and is the first one a new note takes. Until then its tail plays out after the
//!   note-off.
//! - Whether the shaper shapes a voice is taken when its phrase starts and kept until the voice
//!   is idle: turning the shaper on does not cut a voice tailing in its low-pass gate (it keeps
//!   the gate and its idle rule until it ends), and turning it off does not cut a shaped voice's
//!   release. Phrases started after the switch follow the new setting.
//! - The chiptune model envelopes itself (the voice bypasses its low-pass gate) and, with no
//!   envelope set, holds its note for ever. A held key keeps it held, as on the module; after the
//!   note-off the voice gives it the decay envelope the module's TIMBRE attenuverter would
//!   ([`chiptune_release`]), so the note ends as on every other model.
//! - Each block the voice reads its matrix sources at the block's start (velocity 0..1; the key
//!   -1..1 centered on middle C: key 60 is 0, key 0 is -1, key 120 and up +1; the envelopes and
//!   LFOs; the controllers), evaluates the matrix ([`super::matrix`]) and applies the sums: Pitch
//!   to the note, Harmonics, Timbre, Morph and Decay to the knobs ([`targets`]), Level and Pan to
//!   its gains ([`voice_gains`]), the filter's to the shaper. With no row, every one of those is
//!   exactly what it was before the matrix.

use super::matrix::{Matrix, NDST, NSRC, PITCH_SEMITONES, RATE_OCTAVES, dst, src};
use super::shaper::{FilterMods, Settings, Shaper};
use super::{BLOCK, to_f32};
use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc::{SampleRate, Tuning};

/// How long a released voice must render exact silence before it is idle.
pub const QUIET_SECONDS: f32 = 0.05;

/// The loudness the allocator compares falls by this factor per 12-frame block (about 50 ms to
/// -8.7 dB at 48 kHz), so a voice crossing zero does not read as quiet.
const LOUDNESS_FALL: f32 = 0.995;

/// The chiptune model's index: the one model that holds its note with the low-pass gate bypassed.
pub const CHIPTUNE: usize = 7;

/// The chiptune engine's envelope shape after a note-off for a Decay of `decay` (0..1): 0.25 at
/// 100% (silent in about 9 s) to 1 at 0% (about 0.15 s); the engine decays by
/// `1 - 2 / rate * 2^(5 shape) * shape` per sample.
#[must_use]
pub fn chiptune_release(decay: f32) -> f32 {
    1.0 - 0.75 * decay.clamp(0.0, 1.0)
}

/// One int16 step as a float: a block whose samples all stay within it counts as silent.
const LSB: f32 = 1.0 / 32768.0;

/// The key as a matrix source: -1 at key 0, 0 at middle C (60), +1 from key 120 up.
#[must_use]
pub fn key_source(note: f32) -> f32 {
    ((note - 60.0) / 60.0).clamp(-1.0, 1.0)
}

/// The macros the matrix's Harmonics, Timbre, Morph and Decay destinations move, in that order.
pub const TARGETS: usize = 4;

/// The controllers every voice reads as matrix sources.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    /// Channel pressure, 0..1.
    pub aftertouch: f32,
    /// 0..1.
    pub wheel: f32,
    /// -1..1.
    pub bend: f32,
    /// The rack's eight macros (0..1) when the track is a rack member, else 0.
    pub macros: [f32; 8],
}

/// What every voice renders a block with: the patch without the per-voice parts, the matrix, the
/// shaper's settings and the controllers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shared<'a> {
    /// The knobs, 0..1: Harmonics, Timbre, Morph, Decay.
    pub knobs: [f32; TARGETS],
    /// Which target the mod wheel adds to (an index into `knobs`; none for Off) and how much.
    pub wheel: Option<(usize, f32)>,
    /// Semitones added to the note: tune, bend and the rate correction, clamped.
    pub offset: f32,
    pub engine: usize,
    pub lpg_color: f32,
    pub aux_mix: f32,
    pub matrix: &'a Matrix,
    pub(crate) shaper: &'a Settings,
    pub controls: Controls,
    /// Poly pressure per key, 0..127.
    pub poly: &'a [u8; 128],
    /// The instrument's running LFO phases (cycles): where a free-running LFO starts a note.
    pub free_phase: [f64; 2],
    /// The song position at the block's first frame, in quarter notes, while the transport plays.
    pub quarters: Option<f64>,
    pub tempo: f64,
    /// Blocks of silence after which a released voice is idle.
    pub quiet_blocks: u32,
}

/// Harmonics, Timbre, Morph and Decay (0..1) from the knobs and the matrix's sums `sums` for them
/// (in that order): the knob plus the sum, clamped to 0..1, then the mod wheel (capped at 1). A
/// target no row moves is its knob exactly, as before the matrix.
#[must_use]
pub fn targets(knobs: [f32; TARGETS], wheel: Option<(usize, f32)>, sums: [f32; TARGETS]) -> [f32; TARGETS] {
    let mut t = [0.0f32; TARGETS];
    for ((t, k), m) in t.iter_mut().zip(knobs).zip(sums) {
        *t = if m == 0.0 { k } else { (k + m).clamp(0.0, 1.0) };
    }
    if let Some((i, w)) = wheel
        && let Some(x) = t.get_mut(i)
    {
        *x = (*x + w).min(1.0);
    }
    t
}

/// The voice's gain into the left and right channels for the matrix's Level and Pan sums: Level
/// scales by `1 + level` (0..2), Pan is a balance (-1 left, +1 right) that leaves the near side at
/// unity, so the center is exactly 1 on both sides and a voice no row moves is untouched.
#[must_use]
pub fn voice_gains(level: f32, pan: f32) -> (f32, f32) {
    let g = if level == 0.0 { 1.0 } else { (1.0 + level).clamp(0.0, 2.0) };
    let p = pan.clamp(-1.0, 1.0);
    let (l, r) = if p > 0.0 {
        (1.0 - p, 1.0)
    } else if p < 0.0 {
        (1.0, 1.0 + p)
    } else {
        (1.0, 1.0)
    };
    (g * l, g * r)
}

/// The keys sounding on one voice, oldest first: each held, or released while the sustain pedal
/// is down.
#[derive(Debug, Clone)]
pub struct NoteStack {
    keys: [u8; 128],
    len: usize,
    held: [bool; 128],
}

impl Default for NoteStack {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteStack {
    pub const fn new() -> Self {
        Self { keys: [0; 128], len: 0, held: [false; 128] }
    }

    pub fn top(&self) -> Option<u8> {
        self.len.checked_sub(1).and_then(|i| self.keys.get(i)).copied()
    }

    pub fn contains(&self, key: u8) -> bool {
        self.keys.get(..self.len).is_some_and(|k| k.contains(&key))
    }

    /// Whether a key is held down (not only kept by the pedal).
    pub fn any_held(&self) -> bool {
        self.keys.get(..self.len).is_some_and(|k| k.iter().any(|&k| self.held.get(usize::from(k)).copied().unwrap_or(false)))
    }

    fn remove(&mut self, key: u8) {
        let len = self.len;
        if let Some(i) = self.keys.get(..len).and_then(|k| k.iter().position(|&k| k == key)) {
            self.keys.copy_within(i + 1..len, i);
            self.len -= 1;
        }
    }

    pub fn push(&mut self, key: u8) {
        self.remove(key);
        if let Some(slot) = self.keys.get_mut(self.len) {
            *slot = key;
            self.len += 1;
        }
        if let Some(h) = self.held.get_mut(usize::from(key)) {
            *h = true;
        }
    }

    pub fn release(&mut self, key: u8, sustain: bool) {
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
    pub fn release_pedalled(&mut self) {
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

    pub fn clear(&mut self) {
        self.len = 0;
        self.held = [false; 128];
    }
}

/// One voice and its MIDI state.
pub struct Slot {
    pub voice: Box<Voice<'static>>,
    frames: [Frame; BLOCK],
    /// The last block, out and aux crossfaded (and shaped, with the shaper on), before the level.
    pub mixed: [f32; BLOCK],
    pub notes: NoteStack,
    /// The note the voice plays (kept after the release, so the tail keeps its pitch).
    pub note: f32,
    /// The level input while a key sounds: the velocity (0..1) of the newest key.
    pub velocity: f32,
    /// The note-off velocity (0..1) of the key that last let go of the voice.
    pub release_velocity: f32,
    /// The trigger input of the last block rendered.
    pub last_trigger: bool,
    /// A note started while the trigger was still high: one low block first, so the voice sees
    /// the new edge.
    pub retrigger: bool,
    /// A new phrase started since the last block: the shaper's envelopes and LFOs start with it.
    started: bool,
    /// A key held the voice in the last block (its envelopes are gated).
    was_sounding: bool,
    /// The shaper shapes this voice: the shaper's switch when the phrase started, kept until the
    /// voice is idle.
    shaped: bool,
    /// When the voice's newest phrase started (the allocator's counter).
    pub age: u64,
    /// The peak of its recent output, falling slowly: what "quietest" compares.
    pub loudness: f32,
    quiet: u32,
    /// Silent and holding nothing: not rendered.
    pub idle: bool,
    /// Rendered the last block (its `mixed` counts).
    pub rendered: bool,
    /// The last block's gains into the left and right channels (the matrix's Level and Pan).
    pub gain: (f32, f32),
    pub(crate) shaper: Shaper,
    seed: u32,
}

impl Slot {
    /// A voice at `rate` with random state `seed`, idle. Allocates: off the audio thread.
    pub fn new(rate: SampleRate, seed: u32) -> Self {
        let mut s = Self {
            voice: Box::new(Voice::new(rate)),
            frames: [Frame::default(); BLOCK],
            mixed: [0.0; BLOCK],
            notes: NoteStack::new(),
            note: 60.0,
            velocity: 0.0,
            release_velocity: 0.0,
            last_trigger: false,
            retrigger: false,
            started: false,
            was_sounding: false,
            shaped: false,
            age: 0,
            loudness: 0.0,
            quiet: 0,
            idle: true,
            rendered: false,
            gain: (1.0, 1.0),
            shaper: Shaper::new(seed),
            seed,
        };
        s.voice.set_tuning(Tuning::Concert);
        s.reset();
        s
    }

    /// A new voice for another rate (allocates: `prepare` only).
    pub fn rebuild(&mut self, rate: SampleRate) {
        *self.voice = Voice::new(rate);
        self.voice.set_tuning(Tuning::Concert);
    }

    pub fn set_seed(&mut self, seed: u32) {
        self.seed = seed;
        self.voice.seed(seed);
        self.shaper.set_seed(seed);
    }

    /// The voice's power-on state and its seed again (the tuning survives `init`), idle.
    pub fn reset(&mut self) {
        self.voice.init();
        self.voice.seed(self.seed);
        self.frames = [Frame::default(); BLOCK];
        self.mixed = [0.0; BLOCK];
        self.notes.clear();
        self.note = 60.0;
        self.velocity = 0.0;
        self.release_velocity = 0.0;
        self.last_trigger = false;
        self.retrigger = false;
        self.started = false;
        self.was_sounding = false;
        self.shaped = false;
        self.age = 0;
        self.loudness = 0.0;
        self.quiet = 0;
        self.idle = true;
        self.rendered = false;
        self.gain = (1.0, 1.0);
        self.shaper.set_seed(self.seed);
    }

    /// A key sounds on this voice (held, or kept by the pedal).
    pub fn sounding(&self) -> bool {
        self.notes.top().is_some()
    }

    /// Whether this voice plays `key`: holds it, or its tail is that key's.
    pub fn plays(&self, key: u8) -> bool {
        !self.idle && (self.notes.contains(key) || (!self.sounding() && self.note == f32::from(key)))
    }

    /// A note-on, mono style: a new phrase when no key sounds (a new trigger edge, after one low
    /// block if the trigger is still high), else legato.
    pub fn on(&mut self, key: u8, velocity: u8, age: u64) {
        if self.notes.top().is_none() {
            self.retrigger = self.last_trigger;
            self.age = age;
            self.started = true;
        }
        self.notes.push(key);
        self.note = f32::from(key);
        self.velocity = f32::from(velocity) / 127.0;
        self.idle = false;
        self.quiet = 0;
    }

    /// A note-off at `velocity` (0..127).
    pub fn off(&mut self, key: u8, velocity: u8, sustain: bool) {
        if self.notes.contains(key) {
            self.notes.release(key, sustain);
            self.release_velocity = f32::from(velocity.min(127)) / 127.0;
            self.follow_top();
        }
    }

    pub fn release_pedalled(&mut self) {
        self.notes.release_pedalled();
        self.follow_top();
    }

    /// Legato back to the newest key still sounding (the velocity stays).
    pub fn follow_top(&mut self) {
        if let Some(key) = self.notes.top() {
            self.note = f32::from(key);
        }
    }

    /// The matrix's source values for this block (index [`src`]), read at the block's start.
    fn sources(&self, s: &Shared) -> [f32; NSRC] {
        let mut v = [0.0f32; NSRC];
        let mut set = |i: usize, x: f32| {
            if let Some(slot) = v.get_mut(i) {
                *slot = x;
            }
        };
        let m = s.matrix;
        if m.reads(src::LFO1) {
            set(src::LFO1, self.shaper.lfo_value(s.shaper, 0));
        }
        if m.reads(src::LFO2) {
            set(src::LFO2, self.shaper.lfo_value(s.shaper, 1));
        }
        set(src::AMP_ENV, self.shaper.amp.level);
        set(src::FILTER_ENV, self.shaper.filter_env.level);
        set(src::VELOCITY, self.velocity);
        set(src::KEY, key_source(self.note));
        set(src::RELEASE_VELOCITY, self.release_velocity);
        set(src::AFTERTOUCH, s.controls.aftertouch);
        let key = if self.note.is_finite() { self.note.clamp(0.0, 127.0) as usize } else { 0 };
        set(src::POLY_AFTERTOUCH, f32::from(s.poly.get(key).copied().unwrap_or(0)) / 127.0);
        set(src::MOD_WHEEL, s.controls.wheel);
        set(src::PITCH_BEND, s.controls.bend);
        set(src::RANDOM, self.shaper.random);
        for (i, x) in s.controls.macros.iter().enumerate() {
            set(src::MACRO1 + i, *x);
        }
        v
    }

    /// Renders the next 12 frames into `mixed`; an idle voice renders nothing.
    pub(crate) fn render(&mut self, s: &Shared) {
        self.rendered = !self.idle;
        if self.idle {
            return;
        }
        let sounding = self.sounding();
        let sh = s.shaper;
        if self.started {
            self.started = false;
            self.shaped = sh.on;
            self.shaper.note(sh, self.velocity, s.free_phase);
        } else if !sounding && self.was_sounding {
            self.shaper.release();
        }
        self.was_sounding = sounding;
        if let Some(q) = s.quarters {
            self.shaper.anchor(sh, q);
        }
        let sums = if s.matrix.is_empty() { [0.0; NDST] } else { s.matrix.evaluate(&self.sources(s)) };
        let sum = |d: usize| sums.get(d).copied().unwrap_or(0.0);
        let [harmonics, timbre, morph, decay] = targets(s.knobs, s.wheel, [sum(dst::HARMONICS), sum(dst::TIMBRE), sum(dst::MORPH), sum(dst::DECAY)]);
        let pitch = sum(dst::PITCH);
        let note = if pitch == 0.0 { s.offset } else { (s.offset + PITCH_SEMITONES * pitch).clamp(-96.0, 96.0) };
        let mut patch = Patch { note, harmonics, timbre, morph, engine: s.engine, decay, lpg_colour: s.lpg_color, ..Patch::default() };
        if s.engine == CHIPTUNE && !sounding {
            patch.timbre_modulation_amount = chiptune_release(decay);
        }
        let trigger = sounding && !self.retrigger;
        self.retrigger = false;
        self.last_trigger = trigger;
        // A shaped voice's envelope is the amp envelope: the low-pass gate stays open.
        let shaped = self.shaped;
        let level = if shaped {
            1.0
        } else if sounding {
            self.velocity
        } else {
            0.0
        };
        let mods = Modulations {
            note: self.note,
            trigger: if trigger { 1.0 } else { 0.0 },
            level,
            trigger_patched: true,
            level_patched: true,
            ..Modulations::default()
        };
        self.voice.render(&patch, &mods, &mut self.frames);
        let mut silent = true;
        for (y, f) in self.mixed.iter_mut().zip(&self.frames) {
            let (out, aux) = (to_f32(f.out), to_f32(f.aux));
            silent &= out.abs() <= LSB && aux.abs() <= LSB;
            *y = out + (aux - out) * s.aux_mix;
        }
        if shaped {
            let mods = FilterMods { cutoff: sum(dst::CUTOFF), resonance: sum(dst::RESONANCE), env_amount: sum(dst::ENV_AMOUNT) };
            self.shaper.design(sh, self.note, mods);
        }
        self.shaper.process(sh, shaped, &mut self.mixed);
        self.shaper.lfo_amount_mod = [sum(dst::LFO1_AMOUNT), sum(dst::LFO2_AMOUNT)];
        let rate = |d: usize| RATE_OCTAVES * sum(d).clamp(-1.0, 1.0);
        self.shaper.advance_lfos(sh, s.tempo, [rate(dst::LFO1_RATE), rate(dst::LFO2_RATE)], s.quarters.is_some());
        self.gain = voice_gains(sum(dst::LEVEL), sum(dst::PAN));
        let peak = self.mixed.iter().fold(0.0f32, |p, y| p.max(y.abs()));
        self.loudness = peak.max(self.loudness * LOUDNESS_FALL);
        if shaped {
            self.idle = !sounding && self.shaper.amp_idle();
        } else if sounding || trigger || !silent {
            self.quiet = 0;
        } else {
            self.quiet += 1;
            self.idle = self.quiet >= s.quiet_blocks;
        }
        if self.idle {
            self.shaper.rest();
            self.was_sounding = false;
        }
    }
}
