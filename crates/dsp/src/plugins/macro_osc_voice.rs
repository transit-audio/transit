//! One voice of the Macro Oscillator (#196): a `transit_macro_osc::Voice` with the per-voice MIDI
//! state the mono instrument of #195 kept for its one voice (the keys it holds, its note, its
//! velocity, its trigger), the note-to-macro mapping, and the bookkeeping the allocator reads
//! (age, loudness, idle).
//!
//! - A voice is **idle** once it holds no key, its trigger is low and both its outputs have stayed
//!   within one int16 LSB of silence (-90 dBFS) for [`QUIET_SECONDS`]: some models settle on a
//!   +-1 LSB residue rather than exact zero (six-op FM 2 does). An idle voice is not rendered,
//!   costs nothing and is the first one a new note takes. Until then its low-pass gate tail plays
//!   out after the note-off.
//! - The chiptune model envelopes itself (the voice bypasses its low-pass gate) and, with no
//!   envelope set, holds its note for ever. A held key keeps it held, as on the module; after the
//!   note-off the voice gives it the decay envelope the module's TIMBRE attenuverter would
//!   ([`chiptune_release`]), so the note ends as on every other model.
//! - The mapping adds to the knobs, before the mod wheel, clamped to 0..1. Velocity is a 0..1
//!   source (velocity / 127); the key is a -1..1 source centered on middle C (key 60: 0, key 0:
//!   -1, key 120 and up: +1). As in the MIDI Learn range model, the source's travel spans a part of
//!   the target's travel, here from no change to `depth` (velocity) or from `-depth` to `depth`
//!   (key); a negative depth inverts. The curve shapes the source's travel first.

use super::{BLOCK, to_f32};
use crate::offline::FadeShape;
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

/// The shapes a mapping source's travel takes (`velocity_curve`, `key_curve`).
pub const CURVES: [&str; 3] = ["Linear", "Exponential", "S-curve"];

/// A source position 0..1 through curve `curve` ([`CURVES`]): the clip fades' curves themselves
/// (`FadeShape::gain`: Linear, Exponential, S-curve), so 0 and 1 are fixed points of every curve
/// and out-of-range or NaN positions clamp.
#[must_use]
pub fn shape(curve: usize, t: f32) -> f32 {
    let fade = match curve {
        1 => FadeShape::Exponential,
        2 => FadeShape::SCurve,
        _ => FadeShape::Linear,
    };
    fade.gain(t)
}

/// The key as a mapping source: -1 at key 0, 0 at middle C (60), +1 from key 120 up.
#[must_use]
pub fn key_source(note: f32) -> f32 {
    ((note - 60.0) / 60.0).clamp(-1.0, 1.0)
}

/// The macros a mapping drives, in the order of their depth parameters.
pub const TARGETS: usize = 4;

/// The note-to-macro mapping of one block: curves and depths (-1..1) per target (Harmonics,
/// Timbre, Morph, Decay).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Mapping {
    pub velocity_curve: usize,
    pub velocity: [f32; TARGETS],
    pub key_curve: usize,
    pub key: [f32; TARGETS],
}

impl Mapping {
    /// What the mapping adds to each target for a note at `note` played at `velocity` (0..1).
    #[must_use]
    pub fn offsets(&self, note: f32, velocity: f32) -> [f32; TARGETS] {
        let mut out = [0.0; TARGETS];
        if self.velocity.iter().any(|d| *d != 0.0) {
            let v = shape(self.velocity_curve, velocity);
            for (o, d) in out.iter_mut().zip(self.velocity) {
                *o += d * v;
            }
        }
        if self.key.iter().any(|d| *d != 0.0) {
            let k = key_source(note);
            let kv = shape(self.key_curve, k.abs()).copysign(k);
            for (o, d) in out.iter_mut().zip(self.key) {
                *o += d * kv;
            }
        }
        out
    }
}

/// What every voice renders a block with: the patch without the per-voice parts.
#[derive(Debug, Clone, Copy)]
pub struct Shared {
    /// The knobs, 0..1: Harmonics, Timbre, Morph, Decay.
    pub knobs: [f32; TARGETS],
    /// Which target the mod wheel adds to (an index into `knobs`; none for Off) and how much.
    pub wheel: Option<(usize, f32)>,
    /// Semitones added to the note: tune, bend and the rate correction, clamped.
    pub offset: f32,
    pub engine: usize,
    pub lpg_color: f32,
    pub aux_mix: f32,
    pub mapping: Mapping,
    /// Blocks of silence after which a released voice is idle.
    pub quiet_blocks: u32,
}

/// Harmonics, Timbre, Morph and Decay (0..1) for a note at `note` played at `velocity` (0..1):
/// the knobs, plus the mapping (clamped to 0..1), plus the mod wheel (capped at 1). With no
/// mapping a target is its knob exactly.
#[must_use]
pub fn targets(s: &Shared, note: f32, velocity: f32) -> [f32; TARGETS] {
    let map = s.mapping.offsets(note, velocity);
    let mut t = [0.0f32; TARGETS];
    for ((t, k), m) in t.iter_mut().zip(s.knobs).zip(map) {
        *t = if m == 0.0 { k } else { (k + m).clamp(0.0, 1.0) };
    }
    if let Some((i, w)) = s.wheel
        && let Some(x) = t.get_mut(i)
    {
        *x = (*x + w).min(1.0);
    }
    t
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
    /// The last block, out and aux crossfaded, before the level.
    pub mixed: [f32; BLOCK],
    pub notes: NoteStack,
    /// The note the voice plays (kept after the release, so the tail keeps its pitch).
    pub note: f32,
    /// The level input while a key sounds: the velocity (0..1) of the newest key.
    pub velocity: f32,
    /// The trigger input of the last block rendered.
    pub last_trigger: bool,
    /// A note started while the trigger was still high: one low block first, so the voice sees
    /// the new edge.
    pub retrigger: bool,
    /// When the voice's newest phrase started (the allocator's counter).
    pub age: u64,
    /// The peak of its recent output, falling slowly: what "quietest" compares.
    pub loudness: f32,
    quiet: u32,
    /// Silent and holding nothing: not rendered.
    pub idle: bool,
    /// Rendered the last block (its `mixed` counts).
    pub rendered: bool,
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
            last_trigger: false,
            retrigger: false,
            age: 0,
            loudness: 0.0,
            quiet: 0,
            idle: true,
            rendered: false,
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
        self.last_trigger = false;
        self.retrigger = false;
        self.age = 0;
        self.loudness = 0.0;
        self.quiet = 0;
        self.idle = true;
        self.rendered = false;
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
        }
        self.notes.push(key);
        self.note = f32::from(key);
        self.velocity = f32::from(velocity) / 127.0;
        self.idle = false;
        self.quiet = 0;
    }

    pub fn off(&mut self, key: u8, sustain: bool) {
        if self.notes.contains(key) {
            self.notes.release(key, sustain);
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

    /// Renders the next 12 frames into `mixed`; an idle voice renders nothing.
    pub fn render(&mut self, s: &Shared) {
        self.rendered = !self.idle;
        if self.idle {
            return;
        }
        let sounding = self.sounding();
        let [harmonics, timbre, morph, decay] = targets(s, self.note, self.velocity);
        let mut patch = Patch { note: s.offset, harmonics, timbre, morph, engine: s.engine, decay, lpg_colour: s.lpg_color, ..Patch::default() };
        if s.engine == CHIPTUNE && !sounding {
            patch.timbre_modulation_amount = chiptune_release(decay);
        }
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
        let mut peak = 0.0f32;
        let mut silent = true;
        for (y, f) in self.mixed.iter_mut().zip(&self.frames) {
            let (out, aux) = (to_f32(f.out), to_f32(f.aux));
            silent &= out.abs() <= LSB && aux.abs() <= LSB;
            *y = out + (aux - out) * s.aux_mix;
            peak = peak.max(y.abs());
        }
        self.loudness = peak.max(self.loudness * LOUDNESS_FALL);
        if sounding || trigger || !silent {
            self.quiet = 0;
        } else {
            self.quiet += 1;
            self.idle = self.quiet >= s.quiet_blocks;
        }
    }
}
