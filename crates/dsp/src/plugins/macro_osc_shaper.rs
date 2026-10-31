//! The Macro Oscillator's per-voice shaper (#243): transit's addition, not part of the original
//! module, and off by default (`shaper`). Each voice has, after its firmware low-pass gate:
//!
//! - an **amp ADSR** with velocity (`amp_velocity` of the way from 1 to velocity/127), which takes
//!   over from the low-pass gate as the voice's envelope: while the shaper is on the gate is held
//!   open (the level input at 1) and the voice ends when the amp envelope's release does;
//! - a **multimode state-variable filter** (low, band or high pass; 12 or 24 dB), the Filter
//!   insert's own sections ([`crate::plugins::filter::svf`]: the drive's soft clip, the trapezoidal
//!   SVF, the 24 dB slope's Butterworth second section), with cutoff, resonance, drive and key
//!   tracking (100%: one octave per octave from middle C);
//! - a **filter ADSR** whose amount (`filter_env_amount`, bipolar) moves the cutoff up to six
//!   octaves, as the Filter insert's envelope does;
//! - **two LFOs** (sine, triangle, saw, square, sample and hold, smooth random; a free rate or a
//!   tempo-synced division; retriggered at each note or free-running; a fade-in; an amount).
//!
//! The envelopes and LFOs run whether or not the shaper is on: they are sources of the modulation
//! matrix ([`super::matrix`]), and with the shaper off they reach the sound only through rows that
//! read them. The ADSR and the filter sections are the ones the Filter insert and the built-in
//! instruments use; nothing here is a new envelope or filter.
//!
//! **Timing.** The filter's coefficients and every modulation are worked out once per 12-frame
//! block (the firmware's block); the envelopes, the filter and the amp gain run per frame. Times
//! are in host-rate frames: the voices' frames play one for one at the host rate.
//!
//! **LFO phase.** A retriggered LFO starts at phase 0 at each new note. A free-running one starts a
//! note at the instrument's own running phase (one clock per LFO that never stops), so held notes
//! sway together. A tempo-synced, free-running LFO follows the song position while the transport
//! plays: its phase is the transport's quarter-note position over the division's length at each
//! block (the rule #245 asks of the Filter insert), so playback from bar 5 and an export from bar 1
//! place it the same; with the transport stopped it runs at the tempo's rate.

use crate::plugins::filter::svf::{Coefs, K_BUTTERWORTH, MIN_CUTOFF, Mode, Section, max_cutoff, q_of, soft_clip};
use crate::plugins::instrument_queue::{Adsr, AdsrRates, Stage};
use crate::plugins::lfo::{Lfo, Shape, division_quarters};

/// The LFO shapes, in the order of the `lfo<n>_shape` parameters' choices (the shared LFO's order).
pub const LFO_SHAPES: [&str; 6] = ["Sine", "Triangle", "Saw", "Square", "Sample & hold", "Smooth random"];
/// The filter modes, in the order of the `filter_mode` parameter's choices.
pub const FILTER_MODES: [&str; 3] = ["Low pass", "Band pass", "High pass"];
/// Octaves the filter envelope and the matrix's Cutoff move the cutoff at 100% and full swing (the
/// Filter insert's figure).
pub const MOD_OCTAVES: f32 = crate::plugins::filter::MOD_OCTAVES;

/// The filter mode of a `filter_mode` choice.
#[must_use]
pub fn filter_mode(i: usize) -> Mode {
    match i {
        1 => Mode::Band,
        2 => Mode::High,
        _ => Mode::Low,
    }
}

/// One LFO's settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LfoSettings {
    pub(crate) shape: Shape,
    /// Hz (free rate).
    pub(crate) rate: f32,
    pub(crate) sync: bool,
    /// One cycle in quarter notes (synced).
    pub(crate) cycle_quarters: f64,
    pub(crate) retrigger: bool,
    /// Blocks to fade in over after a note (0: none).
    pub(crate) fade_blocks: f32,
    /// The output level, 0..1.
    pub(crate) amount: f32,
}

impl Default for LfoSettings {
    fn default() -> Self {
        LfoSettings { shape: Shape::Sine, rate: 2.0, sync: false, cycle_quarters: 1.0, retrigger: true, fade_blocks: 0.0, amount: 1.0 }
    }
}

impl LfoSettings {
    /// The settings from the parameters (`shape` index, `rate` Hz, `division` index, fade in ms),
    /// for a meter of `bar` quarter notes, at `sr` frames per second.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub(crate) fn new(shape: usize, rate: f32, sync: bool, division: usize, retrigger: bool, fade_ms: f32, amount: f32, bar: f64, sr: f32) -> Self {
        let blocks = fade_ms * 0.001 * sr / super::BLOCK as f32;
        LfoSettings {
            shape: Shape::from_index(shape),
            rate,
            sync,
            cycle_quarters: division_quarters(division, bar),
            retrigger,
            fade_blocks: if blocks.is_finite() { blocks.max(0.0) } else { 0.0 },
            amount: amount.clamp(0.0, 1.0),
        }
    }

    /// Cycles per frame: from the tempo when synced, else from the rate scaled by `octaves`.
    #[must_use]
    pub(crate) fn increment(&self, tempo: f64, octaves: f32, sr: f32) -> f64 {
        if self.sync {
            tempo / 60.0 / self.cycle_quarters.max(1e-6) / f64::from(sr)
        } else {
            let hz = (self.rate * octaves.exp2()).clamp(0.001, 1000.0);
            f64::from(hz) / f64::from(sr)
        }
    }
}

/// What every voice's shaper reads in a block: the parameters, worked out off the voices.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settings {
    /// The shaper's audio section (filter and amp envelope) is on.
    pub(crate) on: bool,
    pub(crate) amp: AdsrRates,
    /// 0..1.
    pub(crate) amp_velocity: f32,
    pub(crate) filter_env: AdsrRates,
    pub(crate) mode: Mode,
    pub(crate) slope24: bool,
    /// log2 of the cutoff knob (Hz).
    pub(crate) cutoff_log2: f32,
    /// The resonance knob, 0..1.
    pub(crate) resonance: f32,
    /// Linear gain before the filter.
    pub(crate) drive: f32,
    /// -1..1.
    pub(crate) key_track: f32,
    /// -1..1.
    pub(crate) env_amount: f32,
    pub(crate) lfo: [LfoSettings; 2],
    /// Host frames per second (the voices' frames play one for one at it).
    pub(crate) sr: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            on: false,
            amp: AdsrRates::default(),
            amp_velocity: 0.5,
            filter_env: AdsrRates::default(),
            mode: Mode::Low,
            slope24: false,
            cutoff_log2: 20_000f32.log2(),
            resonance: 0.0,
            drive: 1.0,
            key_track: 0.0,
            env_amount: 0.0,
            lfo: [LfoSettings::default(); 2],
            sr: 48_000.0,
        }
    }
}

/// The modulations one block of the shaper's filter takes from the matrix (destination sums).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FilterMods {
    pub(crate) cutoff: f32,
    pub(crate) resonance: f32,
    pub(crate) env_amount: f32,
}

/// A voice's shaper state.
#[derive(Debug, Clone)]
pub(crate) struct Shaper {
    pub(crate) amp: Adsr,
    pub(crate) filter_env: Adsr,
    sections: [Section; 2],
    coefs: [Coefs; 2],
    pub(crate) lfo: [Lfo; 2],
    /// Blocks since the note started (for the LFOs' fade-in).
    since_note: u32,
    /// The velocity scale of the amp envelope for the note.
    amp_scale: f32,
    /// The matrix's LFO amount sums of the last block (they act one block later).
    pub(crate) lfo_amount_mod: [f32; 2],
    /// The note's random value, -1..1, drawn at each new note.
    pub(crate) random: f32,
    rng: u32,
    seed: u32,
    /// The envelopes are gated (a key holds the voice).
    gate: bool,
}

/// A deterministic non-zero seed from a voice seed and a salt.
fn mix_seed(seed: u32, salt: u32) -> u32 {
    let x = seed.wrapping_mul(0x9E37_79B9) ^ salt;
    if x == 0 { 0x1234_5678 } else { x }
}

impl Shaper {
    /// A voice's shaper, its random draws seeded from the voice's seed.
    #[must_use]
    pub(crate) fn new(seed: u32) -> Shaper {
        let mut s = Shaper {
            amp: Adsr::new(),
            filter_env: Adsr::new(),
            sections: [Section::default(); 2],
            coefs: [Coefs::default(); 2],
            lfo: [Lfo::new(); 2],
            since_note: 0,
            amp_scale: 1.0,
            lfo_amount_mod: [0.0; 2],
            random: 0.0,
            rng: 1,
            seed,
            gate: false,
        };
        s.reset();
        s
    }

    pub(crate) fn set_seed(&mut self, seed: u32) {
        self.seed = seed;
        self.reset();
    }

    /// Back to the state of a new voice with the same seed.
    pub(crate) fn reset(&mut self) {
        self.amp = Adsr::new();
        self.filter_env = Adsr::new();
        self.sections = [Section::default(); 2];
        self.coefs = [Coefs::default(); 2];
        self.lfo = [Lfo::seeded(mix_seed(self.seed, 0x4C46_4F31)), Lfo::seeded(mix_seed(self.seed, 0x4C46_4F32))];
        self.since_note = 0;
        self.amp_scale = 1.0;
        self.lfo_amount_mod = [0.0; 2];
        self.random = 0.0;
        self.rng = mix_seed(self.seed, 0x524E_444D);
        self.gate = false;
    }

    /// The voice went idle: its envelopes rest at 0 and the filter forgets its memories.
    pub(crate) fn rest(&mut self) {
        self.amp = Adsr::new();
        self.filter_env = Adsr::new();
        self.sections = [Section::default(); 2];
        self.gate = false;
    }

    /// xorshift32, -1..1.
    fn draw(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }

    /// A new note on the voice (not a legato change): the envelopes start from where they are, a
    /// new random value, the LFOs restart (retriggered) or take the instrument's running phase
    /// (`free`, cycles), and their fade-in starts.
    pub(crate) fn note(&mut self, s: &Settings, velocity: f32, free: [f64; 2]) {
        self.amp.gate_on();
        self.filter_env.gate_on();
        self.gate = true;
        self.amp_scale = 1.0 - s.amp_velocity + s.amp_velocity * velocity;
        self.random = self.draw();
        self.since_note = 0;
        for ((l, set), phase) in self.lfo.iter_mut().zip(&s.lfo).zip(free) {
            if set.retrigger {
                l.retrigger();
            } else {
                l.set_phase(phase);
            }
        }
    }

    /// The last key let go of the voice: the envelopes release.
    pub(crate) fn release(&mut self) {
        if self.gate {
            self.amp.gate_off();
            self.filter_env.gate_off();
            self.gate = false;
        }
    }

    /// The amp envelope has finished: the shaped voice is silent.
    #[must_use]
    pub(crate) fn amp_idle(&self) -> bool {
        self.amp.stage == Stage::Idle
    }

    /// LFO `i`'s output now: its shape, faded in, at its amount (plus the matrix's amount sum of
    /// the last block, clamped to 0..1).
    #[must_use]
    pub(crate) fn lfo_value(&self, s: &Settings, i: usize) -> f32 {
        let (Some(l), Some(set)) = (self.lfo.get(i), s.lfo.get(i)) else { return 0.0 };
        let fade = if set.fade_blocks > 0.0 { (self.since_note as f32 / set.fade_blocks).min(1.0) } else { 1.0 };
        let amount = (set.amount + self.lfo_amount_mod.get(i).copied().unwrap_or(0.0)).clamp(0.0, 1.0);
        l.value(set.shape) * fade * amount
    }

    /// Anchors the tempo-synced, free-running LFOs to the song position `quarters` (the transport
    /// plays): the phase is the position over the cycle's length.
    pub(crate) fn anchor(&mut self, s: &Settings, quarters: f64) {
        for (l, set) in self.lfo.iter_mut().zip(&s.lfo) {
            if set.sync && !set.retrigger {
                l.set_phase(quarters / set.cycle_quarters.max(1e-6));
            }
        }
    }

    /// Advances the LFOs over one block (those not anchored this block) at the rate the matrix's
    /// rate sums (`octaves`, already scaled) give, and the fade-in clock.
    pub(crate) fn advance_lfos(&mut self, s: &Settings, tempo: f64, octaves: [f32; 2], anchored: bool) {
        for ((l, set), oct) in self.lfo.iter_mut().zip(&s.lfo).zip(octaves) {
            if anchored && set.sync && !set.retrigger {
                continue;
            }
            l.advance(set.increment(tempo, oct, s.sr) * super::BLOCK as f64);
        }
        self.since_note = self.since_note.saturating_add(1);
    }

    /// Works out the filter's coefficients for this block: the cutoff knob, key tracking for
    /// `note`, the filter envelope at its level now times its amount, and the matrix's sums.
    pub(crate) fn design(&mut self, s: &Settings, note: f32, m: FilterMods) {
        let amount = (s.env_amount + m.env_amount).clamp(-1.0, 1.0);
        let oct = MOD_OCTAVES * (amount * self.filter_env.level + m.cutoff) + s.key_track * (note - 60.0) / 12.0;
        let hz = (s.cutoff_log2 + oct).exp2();
        let hz = if hz.is_finite() { hz.clamp(MIN_CUTOFF, max_cutoff(s.sr)) } else { MIN_CUTOFF };
        let g = Coefs::g_of(hz, s.sr);
        let k = 1.0 / q_of(100.0 * (s.resonance + m.resonance));
        self.coefs = [Coefs::new(g, k), Coefs::new(g, K_BUTTERWORTH)];
    }

    /// Runs one block: the envelopes advance a frame at a time; with the shaper on, each frame of
    /// `block` goes through the drive, the filter and the amp envelope's gain.
    pub(crate) fn process(&mut self, s: &Settings, block: &mut [f32]) {
        let [c0, c1] = self.coefs;
        for y in block.iter_mut() {
            let amp = self.amp.next(&s.amp);
            self.filter_env.next(&s.filter_env);
            if !s.on {
                continue;
            }
            let [s0, s1] = &mut self.sections;
            let mut v = s0.tick(&c0, s.mode, soft_clip(*y * s.drive));
            if s.slope24 {
                v = s1.tick(&c1, s.mode, v);
            }
            *y = v * amp * self.amp_scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(on: bool) -> Settings {
        Settings {
            on,
            amp: AdsrRates::new(1.0, 50.0, 0.5, 20.0, 48_000.0),
            filter_env: AdsrRates::new(1.0, 50.0, 0.0, 20.0, 48_000.0),
            ..Settings::default()
        }
    }

    /// Off, the audio passes untouched while the envelopes still run (they are matrix sources).
    #[test]
    fn off_leaves_the_audio_alone_and_still_runs_the_envelopes() {
        let s = settings(false);
        let mut sh = Shaper::new(3);
        sh.note(&s, 1.0, [0.0; 2]);
        let mut block = [0.3f32; super::super::BLOCK];
        sh.design(&s, 60.0, FilterMods::default());
        sh.process(&s, &mut block);
        assert!(block.iter().all(|y| y.to_bits() == 0.3f32.to_bits()));
        assert!(sh.amp.level > 0.0 && sh.filter_env.level > 0.0);
    }

    /// On, the amp envelope gates the voice: silent before a note and after its release ends.
    #[test]
    fn on_the_amp_envelope_gates_the_voice() {
        let s = settings(true);
        let mut sh = Shaper::new(3);
        let mut block = [0.5f32; super::super::BLOCK];
        sh.design(&s, 60.0, FilterMods::default());
        sh.process(&s, &mut block);
        assert!(block.iter().all(|y| *y == 0.0), "no note, no sound");
        sh.note(&s, 1.0, [0.0; 2]);
        let mut peak = 0.0f32;
        for _ in 0..400 {
            let mut b = [0.5f32; super::super::BLOCK];
            sh.process(&s, &mut b);
            peak = peak.max(b.iter().fold(0.0, |m, y| m.max(y.abs())));
        }
        assert!(peak > 0.2, "{peak}");
        sh.release();
        for _ in 0..2000 {
            let mut b = [0.5f32; super::super::BLOCK];
            sh.process(&s, &mut b);
        }
        assert!(sh.amp_idle());
    }

    /// A tempo-synced, free-running LFO anchored to the song position has the same phase for the
    /// same position whatever happened before.
    #[test]
    fn an_anchored_lfo_follows_the_song_position() {
        let mut s = settings(false);
        s.lfo[0] = LfoSettings { sync: true, retrigger: false, cycle_quarters: 1.0, ..LfoSettings::default() };
        let mut a = Shaper::new(1);
        let mut b = Shaper::new(2);
        for _ in 0..37 {
            b.advance_lfos(&s, 133.0, [0.0; 2], false);
        }
        a.anchor(&s, 5.25);
        b.anchor(&s, 5.25);
        assert_eq!(a.lfo[0].phase(), 0.25);
        assert_eq!(a.lfo[0].phase(), b.lfo[0].phase());
    }
}
