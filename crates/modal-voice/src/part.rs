//! The part (`elements/dsp/part.{h,cc}`): one voice (the exciters and a resonator, or the FM pair),
//! the space control's dry level, stereo spread and reverb, the soft limiter, the meters and the
//! runaway guard.

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::random::Random;
use transit_resonator::stmlib::soft_limit;

use crate::MAX_BLOCK_SIZE;
use crate::fm_pair::FmPairVoice;
use crate::fx::Reverb;
use crate::resources::{LUT_MIDI_TO_F_HIGH, LUT_MIDI_TO_F_LOW};
use crate::voice::Voice;

/// The resonator models, in the firmware's order (`ResonatorModel` in `elements/dsp/voice.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResonatorModel {
    /// A bank of 52 band-pass modes with eight bowed waveguides (`RESONATOR_MODEL_MODAL`).
    #[default]
    Modal,
    /// One string with dispersion (`RESONATOR_MODEL_STRING`).
    String,
    /// Five strings tuned to a chord picked by the geometry (`RESONATOR_MODEL_STRINGS`).
    Strings,
}

impl ResonatorModel {
    /// Every model, in index order.
    pub const ALL: [Self; 3] = [Self::Modal, Self::String, Self::Strings];
}

/// The panel controls (the fields of `Patch` in `elements/dsp/patch.h` the module's CV scaler writes
/// every block), each 0 to 1 but `space`, 0 to 2. The module's CV scaler keeps every control but
/// the levels and the envelope shape at or below 0.9995 (the strike timbre at 0.995); the port
/// clamps a table index the firmware would read past instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    /// The gate envelope's shape: short and percussive at 0, a sustained gate at 0.5, slow attack
    /// and release at 1.
    pub exciter_envelope_shape: f32,
    /// The bow's level.
    pub exciter_bow_level: f32,
    /// The bow's timbre (the cutoff of its noise filter).
    pub exciter_bow_timbre: f32,
    /// The blow's level (from two thirds on, the tube joins in).
    pub exciter_blow_level: f32,
    /// The blow's meta control: where the granular noise sample restarts.
    pub exciter_blow_meta: f32,
    /// The blow's timbre: the noise sample's playback rate.
    pub exciter_blow_timbre: f32,
    /// The strike's level (from 80% on, the raw strike bleeds into the output).
    pub exciter_strike_level: f32,
    /// The strike's meta control: samples, mallet, plectrum, particles.
    pub exciter_strike_meta: f32,
    /// The strike's timbre.
    pub exciter_strike_timbre: f32,
    /// The resonator's geometry.
    pub resonator_geometry: f32,
    /// The resonator's brightness.
    pub resonator_brightness: f32,
    /// The resonator's damping.
    pub resonator_damping: f32,
    /// The excitation or pickup position.
    pub resonator_position: f32,
    /// Space: the dry level, the stereo spread and the reverb; from 1.75 the reverb freezes.
    pub space: f32,
}

impl Default for Patch {
    /// The values `Part::Init` writes, the envelope shape at 0.9995 (Init writes 1.0, which the
    /// module's CV scaler replaces before the first block).
    fn default() -> Self {
        Self {
            exciter_envelope_shape: 0.9995,
            exciter_bow_level: 0.0,
            exciter_bow_timbre: 0.5,
            exciter_blow_level: 0.0,
            exciter_blow_meta: 0.5,
            exciter_blow_timbre: 0.5,
            exciter_strike_level: 0.8,
            exciter_strike_meta: 0.5,
            exciter_strike_timbre: 0.5,
            resonator_geometry: 0.2,
            resonator_brightness: 0.5,
            resonator_damping: 0.25,
            resonator_position: 0.3,
            space: 0.5,
        }
    }
}

/// The whole patch as the DSP reads it: the panel controls and the five values `Init` and `Seed`
/// set (the side channel's LFO rate and offset, the reverb's diffusion and low-pass, the exciter
/// signature).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FPatch {
    pub(crate) exciter_envelope_shape: F,
    pub(crate) exciter_bow_level: F,
    pub(crate) exciter_bow_timbre: F,
    pub(crate) exciter_blow_level: F,
    pub(crate) exciter_blow_meta: F,
    pub(crate) exciter_blow_timbre: F,
    pub(crate) exciter_strike_level: F,
    pub(crate) exciter_strike_meta: F,
    pub(crate) exciter_strike_timbre: F,
    pub(crate) exciter_signature: F,
    pub(crate) resonator_geometry: F,
    pub(crate) resonator_brightness: F,
    pub(crate) resonator_damping: F,
    pub(crate) resonator_position: F,
    pub(crate) resonator_modulation_frequency: F,
    pub(crate) resonator_modulation_offset: F,
    pub(crate) reverb_diffusion: F,
    pub(crate) reverb_lp: F,
    pub(crate) space: F,
}

impl FPatch {
    fn set_controls(&mut self, p: &Patch) {
        self.exciter_envelope_shape = F(p.exciter_envelope_shape);
        self.exciter_bow_level = F(p.exciter_bow_level);
        self.exciter_bow_timbre = F(p.exciter_bow_timbre);
        self.exciter_blow_level = F(p.exciter_blow_level);
        self.exciter_blow_meta = F(p.exciter_blow_meta);
        self.exciter_blow_timbre = F(p.exciter_blow_timbre);
        self.exciter_strike_level = F(p.exciter_strike_level);
        self.exciter_strike_meta = F(p.exciter_strike_meta);
        self.exciter_strike_timbre = F(p.exciter_strike_timbre);
        self.resonator_geometry = F(p.resonator_geometry);
        self.resonator_brightness = F(p.resonator_brightness);
        self.resonator_damping = F(p.resonator_damping);
        self.resonator_position = F(p.resonator_position);
        self.space = F(p.space);
    }
}

/// What the module hands the part each block (`PerformanceState` in `elements/dsp/part.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerformanceState {
    /// The gate: the exciters' envelope follows it, the strike fires on its rising edge.
    pub gate: bool,
    /// The note, MIDI semitones (69 is A4, 440 Hz).
    pub note: f32,
    /// The FM input, semitones (added to the note).
    pub modulation: f32,
    /// The strength (the module's pressure input, `1 - CV`): the accent, 0 to 1.
    pub strength: f32,
}

/// The part's scratch buffers (one block of [`MAX_BLOCK_SIZE`] frames).
#[derive(Clone, Copy, Debug, Default)]
struct Buffers {
    blow: [F; MAX_BLOCK_SIZE],
    strike: [F; MAX_BLOCK_SIZE],
    main: [F; MAX_BLOCK_SIZE],
    aux: [F; MAX_BLOCK_SIZE],
    raw: [F; MAX_BLOCK_SIZE],
    center: [F; MAX_BLOCK_SIZE],
    sides: [F; MAX_BLOCK_SIZE],
}

/// The modal voice: the firmware's part, one voice at 32 kHz ([`crate::SAMPLE_RATE`]) in blocks of
/// up to 16 frames ([`MAX_BLOCK_SIZE`]).
///
/// Build it once ([`ModalVoice::new`] allocates its delay memory); after that nothing allocates,
/// [`ModalVoice::reset`] included. The random generator is the part's own (stmlib's linear
/// congruential generator, one static on the module), starting at stmlib's power-on state.
#[derive(Clone, Debug)]
pub struct ModalVoice {
    patch: FPatch,
    controls: Patch,
    voice: Voice,
    fm_pair: FmPairVoice,
    /// Set by the runaway guard; `Init` never writes it, so it starts clear (static storage).
    panic: bool,
    bypass: bool,
    easter_egg: bool,
    previous_gate: bool,
    note: F,
    buffers: Buffers,
    scaled_exciter_level: F,
    scaled_resonator_level: F,
    resonator_level: F,
    reverb: Reverb,
    resonator_model: ResonatorModel,
    random: Random,
}

impl Default for ModalVoice {
    fn default() -> Self {
        Self::new()
    }
}

impl ModalVoice {
    /// A part as the firmware's `Part::Init` leaves it on zeroed memory: the modal model, the
    /// default patch, unseeded.
    #[must_use]
    pub fn new() -> Self {
        let mut part = Self {
            patch: FPatch::default(),
            controls: Patch::default(),
            voice: Voice::default(),
            fm_pair: FmPairVoice::default(),
            panic: false,
            bypass: false,
            easter_egg: false,
            previous_gate: false,
            note: F::ZERO,
            buffers: Buffers::default(),
            scaled_exciter_level: F::ZERO,
            scaled_resonator_level: F::ZERO,
            resonator_level: F::ZERO,
            reverb: Reverb::default(),
            resonator_model: ResonatorModel::Modal,
            random: Random::default(),
        };
        part.init();
        part
    }

    /// Back to what [`ModalVoice::new`] builds (the delay memory cleared, the random generator at
    /// its power-on state), without allocating.
    pub fn reset(&mut self) {
        *self = Self {
            patch: FPatch::default(),
            controls: Patch::default(),
            voice: self.voice.take_zeroed(),
            fm_pair: FmPairVoice::default(),
            panic: false,
            bypass: false,
            easter_egg: false,
            previous_gate: false,
            note: F::ZERO,
            buffers: Buffers::default(),
            scaled_exciter_level: F::ZERO,
            scaled_resonator_level: F::ZERO,
            resonator_level: F::ZERO,
            reverb: self.reverb.take_zeroed(),
            resonator_model: ResonatorModel::Modal,
            random: Random::default(),
        };
        self.init();
    }

    /// `Init(reverb_buffer)`.
    fn init(&mut self) {
        let p = &mut self.patch;
        p.exciter_envelope_shape = F(1.0);
        p.exciter_bow_level = F::ZERO;
        p.exciter_bow_timbre = F(0.5);
        p.exciter_blow_level = F::ZERO;
        p.exciter_blow_meta = F(0.5);
        p.exciter_blow_timbre = F(0.5);
        p.exciter_strike_level = F(0.8);
        p.exciter_strike_meta = F(0.5);
        p.exciter_strike_timbre = F(0.5);
        p.exciter_signature = F::ZERO;
        p.resonator_geometry = F(0.2);
        p.resonator_brightness = F(0.5);
        p.resonator_damping = F(0.25);
        p.resonator_position = F(0.3);
        p.resonator_modulation_frequency = F(0.5) / crate::SAMPLE_RATE;
        p.resonator_modulation_offset = F(0.1);
        p.reverb_diffusion = F(0.625);
        p.reverb_lp = F(0.7);
        p.space = F(0.5);
        self.previous_gate = false;
        self.note = F(69.0);
        self.voice.init();
        self.fm_pair.init();
        self.reverb.init();
        self.scaled_exciter_level = F::ZERO;
        self.scaled_resonator_level = F::ZERO;
        self.resonator_level = F::ZERO;
        self.bypass = false;
        self.resonator_model = ResonatorModel::Modal;
    }

    /// `Seed(seed, size)`: scrambles the words (the module passes its chip's serial number) into the
    /// side channel's LFO rate and offset, the reverb's diffusion and low-pass and the exciter
    /// signature (which picks the blow's noise segment and shapes the plectrum).
    pub fn seed(&mut self, words: &[u32]) {
        let mut signature = 0xf0ca_cc1a_u32;
        for w in words {
            signature ^= *w;
            signature = signature.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
        let mut next = || {
            let x = F::from_u32(signature & 7) / 8.0;
            signature >>= 3;
            x
        };
        let p = &mut self.patch;
        p.resonator_modulation_frequency = (0.4 + 0.8 * next()) / crate::SAMPLE_RATE;
        p.resonator_modulation_offset = 0.05 + 0.1 * next();
        p.reverb_diffusion = 0.55 + 0.15 * next();
        p.reverb_lp = 0.7 + 0.2 * next();
        p.exciter_signature = next();
    }

    /// The panel controls the next block renders with.
    pub fn patch_mut(&mut self) -> &mut Patch {
        &mut self.controls
    }

    /// The panel controls.
    #[must_use]
    pub fn patch(&self) -> &Patch {
        &self.controls
    }

    /// Selects the resonator model, as the module's calibration menu does (it takes effect on the
    /// next block).
    pub fn set_model(&mut self, model: ResonatorModel) {
        self.resonator_model = model;
    }

    /// The resonator model.
    #[must_use]
    pub fn model(&self) -> ResonatorModel {
        self.resonator_model
    }

    /// Selects the FM pair voice instead of the exciters and resonator (the module's hidden mode).
    pub fn set_easter_egg(&mut self, easter_egg: bool) {
        self.easter_egg = easter_egg;
    }

    /// Whether the FM pair voice is selected.
    #[must_use]
    pub fn easter_egg(&self) -> bool {
        self.easter_egg
    }

    /// Bypass: the strike input is copied to main, the blow input to aux.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    /// The random generator's state.
    #[must_use]
    pub fn random_state(&self) -> u32 {
        self.random.state()
    }

    /// Writes the random generator's state.
    pub fn set_random_state(&mut self, state: u32) {
        self.random = Random::new(state);
    }

    /// The exciter meter, 0 to 1.
    #[must_use]
    pub fn exciter_level(&self) -> f32 {
        self.scaled_exciter_level.0
    }

    /// The resonator meter, 0 to 1.
    #[must_use]
    pub fn resonator_level(&self) -> f32 {
        self.scaled_resonator_level.0
    }

    /// `Process(performance_state, blow_in, strike_in, main, aux, size)`: renders `main.len()`
    /// frames (1 to [`MAX_BLOCK_SIZE`]); one call is one control block (the gate, the envelope and
    /// the exciters' edges advance per call). `blow_in` and `strike_in` are the two audio inputs
    /// (silence when nothing is patched), as long as the outputs.
    ///
    /// # Panics
    ///
    /// In debug builds, when the block is empty or longer than [`MAX_BLOCK_SIZE`] or the lengths
    /// differ.
    #[allow(clippy::too_many_lines)]
    pub fn process(&mut self, performance: &PerformanceState, blow_in: &[f32], strike_in: &[f32], main: &mut [f32], aux: &mut [f32]) {
        let size = main.len();
        debug_assert!((1..=MAX_BLOCK_SIZE).contains(&size), "a block is 1 to {MAX_BLOCK_SIZE} frames");
        debug_assert!(blow_in.len() == size && strike_in.len() == size && aux.len() == size, "the buffers differ in length");
        let size = size.min(MAX_BLOCK_SIZE).min(aux.len()).min(blow_in.len()).min(strike_in.len());
        if size == 0 {
            return;
        }
        let controls = self.controls;
        self.patch.set_controls(&controls);
        let b = &mut self.buffers;
        for i in 0..size {
            b.blow[i] = F(blow_in[i]);
            b.strike[i] = F(strike_in[i]);
        }

        // Copy the inputs to the outputs when bypassed.
        if self.bypass || self.panic {
            if self.panic {
                // If the resonator blows up, its filters are reset so that it does not stay stuck at
                // NaN.
                self.voice.reset_resonator();
                self.resonator_level = F::ZERO;
                self.panic = false;
            }
            aux[..size].copy_from_slice(&blow_in[..size]);
            main[..size].copy_from_slice(&strike_in[..size]);
            return;
        }

        self.previous_gate = performance.gate;
        self.note = F(performance.note);
        let (bm, ba) = (&mut b.main[..size], &mut b.aux[..size]);
        bm.fill(F::ZERO);
        ba.fill(F::ZERO);

        // The raw signal's gain, the stereo spread and the reverb from the space control.
        let p = &self.patch;
        let mut space = if p.space >= 1.0 { F(1.0) } else { p.space };
        let raw_gain = if space <= 0.05 {
            F(1.0)
        } else if space <= 0.1 {
            2.0 - space * 20.0
        } else {
            F::ZERO
        };
        space = if space >= 0.1 { space - 0.1 } else { F::ZERO };
        let spread = if space <= 0.7 { space } else { F(0.7) };
        let reverb_amount = if space >= 0.5 { 1.0 * (space - 0.5) } else { F::ZERO };
        let reverb_time = 0.35 + 1.2 * reverb_amount;

        // The voice.
        let midi_pitch = self.note + performance.modulation;
        let strength = F(performance.strength);
        let (raw, center, sides) = (&mut b.raw[..size], &mut b.center[..size], &mut b.sides[..size]);
        if self.easter_egg {
            self.fm_pair.process(p, midi_pitch, strength, performance.gate, &b.blow[..size], &b.strike[..size], raw, center, sides);
        } else {
            // The MIDI pitch as a frequency.
            let pitch = ((midi_pitch + 48.0) * 256.0).trunc_i32().clamp(0, 65535);
            let pitch = usize::try_from(pitch).unwrap_or(0);
            let frequency = F(LUT_MIDI_TO_F_HIGH[pitch >> 8]) * F(LUT_MIDI_TO_F_LOW[pitch & 0xff]);
            self.voice.set_resonator_model(self.resonator_model);
            self.voice.process(p, frequency, strength, performance.gate, &b.blow[..size], &b.strike[..size], raw, center, sides, &mut self.random);
        }

        // Mixdown.
        for j in 0..size {
            let side = sides[j] * spread;
            let r = center[j] - side;
            let l = center[j] + side;
            bm[j] += r;
            ba[j] += l + (raw[j] - l) * raw_gain;
        }

        // Pre-clipping.
        if !self.easter_egg {
            for x in bm.iter_mut().chain(ba.iter_mut()) {
                *x = soft_limit(*x);
            }
        }

        // The meters.
        let mut exciter_level = self.voice.exciter_level();
        let mut resonator_level = self.resonator_level;
        for x in bm.iter() {
            let error = *x * *x - resonator_level;
            resonator_level += error * if error > 0.0 { F(0.05) } else { F(0.0005) };
        }
        self.resonator_level = resonator_level;
        if resonator_level >= 200.0 {
            self.panic = true;
        }
        if self.easter_egg {
            let l = (p.exciter_blow_level + p.exciter_strike_level) * 0.5;
            self.scaled_exciter_level = l * (2.0 - l);
        } else {
            exciter_level *= 16.0;
            self.scaled_exciter_level = if exciter_level > 0.1 { F(1.0) } else { exciter_level };
        }
        resonator_level *= 16.0;
        self.scaled_resonator_level = if resonator_level < 1.0 { resonator_level } else { F(1.0) };

        // The reverb.
        self.reverb.set_amount(reverb_amount);
        self.reverb.set_diffusion(p.reverb_diffusion);
        let freeze = p.space >= 1.75;
        if freeze {
            self.reverb.set_time(F(1.0));
            self.reverb.set_input_gain(F::ZERO);
            self.reverb.set_lp(F(1.0));
        } else {
            self.reverb.set_time(reverb_time);
            self.reverb.set_input_gain(F(0.2));
            self.reverb.set_lp(p.reverb_lp);
        }
        self.reverb.process(bm, ba);

        for j in 0..size {
            main[j] = bm[j].0;
            aux[j] = ba[j].0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(v: &mut ModalVoice, blocks: usize, note: f32) -> Vec<u32> {
        let silence = [0.0_f32; MAX_BLOCK_SIZE];
        let mut bits = Vec::new();
        for b in 0..blocks {
            let (mut m, mut a) = ([0.0_f32; MAX_BLOCK_SIZE], [0.0_f32; MAX_BLOCK_SIZE]);
            let performance = PerformanceState { gate: b % 300 < 150, note, modulation: 0.0, strength: 0.7 };
            v.process(&performance, &silence, &silence, &mut m, &mut a);
            bits.extend(m.iter().chain(&a).map(|x| x.to_bits()));
        }
        bits.push(v.random_state());
        bits
    }

    #[test]
    fn reset_returns_to_what_new_builds() {
        let mut dirty = ModalVoice::new();
        let busy = Patch { exciter_bow_level: 0.6, exciter_blow_level: 0.9, space: 1.9, ..Patch::default() };
        for (model, egg) in
            [(ResonatorModel::Modal, false), (ResonatorModel::String, false), (ResonatorModel::Strings, false), (ResonatorModel::Modal, true)]
        {
            dirty.set_model(model);
            dirty.set_easter_egg(egg);
            dirty.seed(&[7, 8, 9]);
            *dirty.patch_mut() = busy;
            render(&mut dirty, 400, 50.0);
        }
        for (model, egg) in
            [(ResonatorModel::Modal, false), (ResonatorModel::String, false), (ResonatorModel::Strings, false), (ResonatorModel::Modal, true)]
        {
            dirty.reset();
            let mut fresh = ModalVoice::new();
            for v in [&mut dirty, &mut fresh] {
                v.set_model(model);
                v.set_easter_egg(egg);
                *v.patch_mut() = busy;
            }
            assert_eq!(render(&mut dirty, 600, 62.0), render(&mut fresh, 600, 62.0), "{model:?} easter egg {egg}");
            render(&mut dirty, 200, 40.0);
        }
    }
}
