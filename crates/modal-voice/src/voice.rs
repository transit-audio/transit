//! The voice (`elements/dsp/voice.{h,cc}`): the gate envelope, the three exciters (bow, blow with
//! its tube and diffuser, strike) summed under the strength's accent, and one of three resonators
//! (the modal bank, one string, or five strings tuned to a chord).

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::filter::DcBlocker;
use transit_resonator::stmlib::random::Random;
use transit_resonator::stmlib::{integral_fractional, semitones_to_ratio};

use crate::MAX_BLOCK_SIZE;
use crate::dsp::index;
use crate::envelope::Envelope;
use crate::exciter::{self, Exciter, Model as ExciterModel};
use crate::fx::Diffuser;
use crate::modal::ModalResonator;
use crate::part::{FPatch as Patch, ResonatorModel};
use crate::resources::{LUT_ACCENT_GAIN_COARSE, LUT_ACCENT_GAIN_FINE};
use crate::string::KarplusString;
use crate::tube::Tube;

/// `kNumStrings`.
const NUM_STRINGS: usize = 5;

/// `chords`: the five strings' transpositions per chord, picked by the geometry.
const CHORDS: [[f32; NUM_STRINGS]; 11] = [
    [0.0, -12.0, 0.0, 0.01, 12.0],
    [0.0, -12.0, 3.0, 7.0, 10.0],
    [0.0, -12.0, 3.0, 7.0, 12.0],
    [0.0, -12.0, 3.0, 7.0, 14.0],
    [0.0, -12.0, 3.0, 7.0, 17.0],
    [0.0, -12.0, 7.0, 12.0, 19.0],
    [0.0, -12.0, 4.0, 7.0, 17.0],
    [0.0, -12.0, 4.0, 7.0, 14.0],
    [0.0, -12.0, 4.0, 7.0, 12.0],
    [0.0, -12.0, 4.0, 7.0, 11.0],
    [0.0, -12.0, 5.0, 7.0, 12.0],
];

/// The scratch buffers the voice renders a block into (one block of [`MAX_BLOCK_SIZE`] frames).
#[derive(Clone, Copy, Debug, Default)]
struct Buffers {
    bow: [F; MAX_BLOCK_SIZE],
    bow_strength: [F; MAX_BLOCK_SIZE],
    blow: [F; MAX_BLOCK_SIZE],
    strike: [F; MAX_BLOCK_SIZE],
}

/// `elements::Voice`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Voice {
    envelope: Envelope,
    tube: Tube,
    bow: Exciter,
    blow: Exciter,
    strike: Exciter,
    diffuser: Diffuser,
    resonator: ModalResonator,
    string: [KarplusString; NUM_STRINGS],
    dc_blocker: DcBlocker,
    strength: F,
    envelope_value: F,
    exciter_level: F,
    buffers: Buffers,
    previous_gate: bool,
    resonator_model: ResonatorModel,
    chord_index: F,
}

impl Voice {
    /// This voice as [`Default`] builds it, moving its delay lines out without allocating: `self`
    /// is left only fit to be dropped or replaced.
    #[must_use]
    pub(crate) fn take_zeroed(&mut self) -> Self {
        Self {
            envelope: Envelope::default(),
            tube: self.tube.take_zeroed(),
            bow: Exciter::default(),
            blow: Exciter::default(),
            strike: Exciter::default(),
            diffuser: self.diffuser.take_zeroed(),
            resonator: self.resonator.take_zeroed(),
            string: std::array::from_fn(|i| self.string[i].take_zeroed()),
            dc_blocker: DcBlocker::default(),
            strength: F::ZERO,
            envelope_value: F::ZERO,
            exciter_level: F::ZERO,
            buffers: Buffers::default(),
            previous_gate: false,
            resonator_model: ResonatorModel::Modal,
            chord_index: F::ZERO,
        }
    }

    /// `Init()`.
    pub(crate) fn init(&mut self) {
        self.envelope.init();
        self.bow.init();
        self.blow.init();
        self.strike.init();
        self.diffuser.init();

        self.reset_resonator();

        self.bow.set_model(ExciterModel::Flow);
        self.bow.set_parameter(F(0.7));
        self.bow.set_timbre(F(0.5));

        self.blow.set_model(ExciterModel::GranularSamplePlayer);

        self.envelope.set_adsr(F(0.5), F(0.5), F(0.5), F(0.5));

        self.previous_gate = false;
        self.strength = F::ZERO;
        self.exciter_level = F::ZERO;
        self.envelope_value = F::ZERO;
        self.chord_index = F::ZERO;

        self.resonator_model = ResonatorModel::Modal;
    }

    /// `ResetResonator()` (also `Panic()`): the resonators re-initialized.
    pub(crate) fn reset_resonator(&mut self) {
        self.resonator.init();
        for s in &mut self.string {
            s.init(true);
        }
        self.dc_blocker.init(1.0 - F(10.0) / crate::SAMPLE_RATE);
        // Runs with 56 extremely tightly.
        self.resonator.set_resolution(52);
    }

    /// `exciter_level()`: the exciter meter.
    pub(crate) fn exciter_level(&self) -> F {
        self.exciter_level
    }

    pub(crate) fn set_resonator_model(&mut self, model: ResonatorModel) {
        self.resonator_model = model;
    }

    /// `GetGateFlags(gate_in)`.
    fn gate_flags(&mut self, gate_in: bool) -> u8 {
        let mut flags = 0;
        if gate_in {
            if !self.previous_gate {
                flags |= exciter::RISING_EDGE;
            }
            flags |= exciter::GATE;
        } else if self.previous_gate {
            flags = exciter::FALLING_EDGE;
        }
        self.previous_gate = gate_in;
        flags
    }

    /// `Process(patch, frequency, strength, gate_in, blow_in, strike_in, raw, center, sides, size)`.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) fn process(
        &mut self,
        patch: &Patch,
        frequency: F,
        strength: F,
        gate_in: bool,
        blow_in: &[F],
        strike_in: &[F],
        raw: &mut [F],
        center: &mut [F],
        sides: &mut [F],
        random: &mut Random,
    ) {
        let size = raw.len();
        let flags = self.gate_flags(gate_in);

        // The envelope.
        let shape = patch.exciter_envelope_shape;
        let mut envelope_gain = F(1.0);
        if shape < 0.4 {
            let a = shape * 0.75 + 0.15;
            let dr = a * 1.8;
            self.envelope.set_adsr(a, dr, F::ZERO, dr);
            envelope_gain = 5.0 - shape * 10.0;
        } else if shape < 0.6 {
            let s = (shape - 0.4) * 5.0;
            self.envelope.set_adsr(F(0.45), F(0.81), s, F(0.81));
        } else {
            let a = (1.0 - shape) * 0.75 + 0.15;
            let dr = a * 1.8;
            self.envelope.set_adsr(a, dr, F(1.0), dr);
        }
        let envelope_value = self.envelope.process(flags) * envelope_gain;
        let envelope_increment = (envelope_value - self.envelope_value) / F::from_usize(size);

        // The exciters.
        let brightness_factor = 0.4 + 0.6 * patch.resonator_brightness;
        self.bow.set_timbre(patch.exciter_bow_timbre * brightness_factor);

        self.blow.set_parameter(patch.exciter_blow_meta);
        self.blow.set_timbre(patch.exciter_blow_timbre);
        self.blow.set_signature(patch.exciter_signature);

        let strike_meta = patch.exciter_strike_meta;
        self.strike.set_meta(
            if strike_meta <= 0.4 { strike_meta * 0.625 } else { strike_meta * 1.25 - 0.25 },
            ExciterModel::SamplePlayer,
            ExciterModel::Particles,
        );
        self.strike.set_timbre(patch.exciter_strike_timbre);
        self.strike.set_signature(patch.exciter_signature);

        let b = &mut self.buffers;
        self.bow.process(flags, &mut b.bow[..size], random);

        let mut blow_level = patch.exciter_blow_level * 1.5;
        let tube_level = if blow_level > 1.0 { (blow_level - 1.0) * 2.0 } else { F::ZERO };
        blow_level = if blow_level < 1.0 { blow_level * 0.4 } else { F(0.4) };
        self.blow.process(flags, &mut b.blow[..size], random);
        self.tube.process(frequency, envelope_value, patch.resonator_damping, tube_level, &mut b.blow[..size], tube_level * 0.5);

        for (x, i) in b.blow[..size].iter_mut().zip(blow_in) {
            *x = *x * blow_level + *i;
        }
        self.diffuser.process(&mut b.blow[..size]);
        self.strike.process(flags, &mut b.strike[..size], random);

        // Past a certain level the strike stops getting louder and mixes its raw signal into the
        // resonator's output instead.
        let mut strike_level = patch.exciter_strike_level * 1.25;
        let strike_bleed = if strike_level > 1.0 { (strike_level - 1.0) * 2.0 } else { F::ZERO };
        strike_level = if strike_level < 1.0 { strike_level } else { F(1.0) };
        strike_level *= 1.5;

        // The strength is very sensitive to zipper noise.
        let strength = strength * 256.0;
        let strength_increment = (strength - self.strength) / F::from_usize(size);

        // Sum the excitation.
        for i in 0..size {
            self.strength += strength_increment;
            self.envelope_value += envelope_increment;
            let mut input_sample = F::ZERO;
            let mut e = self.envelope_value;
            let (strength_integral, strength_fractional) = integral_fractional(self.strength);
            let coarse = F(LUT_ACCENT_GAIN_COARSE[index(strength_integral, LUT_ACCENT_GAIN_COARSE.len())]);
            let fine = F(LUT_ACCENT_GAIN_FINE[index((256.0 * strength_fractional).trunc_i32(), LUT_ACCENT_GAIN_FINE.len())]);
            let accent = coarse * fine;
            b.bow_strength[i] = e * patch.exciter_bow_level;

            b.strike[i] *= accent;
            e *= accent;

            input_sample += b.bow[i] * b.bow_strength[i] * 0.125 * accent;
            input_sample += b.blow[i] * e;
            input_sample += b.strike[i] * strike_level;
            input_sample += strike_in[i];
            raw[i] = input_sample * 0.5;
        }

        // The exciter meter.
        for r in raw.iter() {
            let error = *r * *r - self.exciter_level;
            self.exciter_level += error * if error > 0.0 { F(0.5) } else { F(0.001) };
        }

        // Some exciters mute the resonator on release.
        let mut damping = patch.resonator_damping;
        damping -= self.strike.damping() * strike_level * 0.125;
        damping -= (1.0 - b.bow_strength[0]) * patch.exciter_bow_level * 0.0625;
        if damping <= 0.0 {
            damping = F::ZERO;
        }

        // The resonator.
        if self.resonator_model == ResonatorModel::Modal {
            let r = &mut self.resonator;
            r.set_frequency(frequency);
            r.set_geometry(patch.resonator_geometry);
            r.set_brightness(patch.resonator_brightness);
            r.set_position(patch.resonator_position);
            r.set_damping(damping);
            r.set_modulation_frequency(patch.resonator_modulation_frequency);
            r.set_modulation_offset(patch.resonator_modulation_offset);
            r.process(&b.bow_strength[..size], raw, center, sides);
        } else {
            let num_notes = if self.resonator_model == ResonatorModel::String { 1 } else { NUM_STRINGS };
            let normalization = 1.0 / F::from_usize(num_notes);
            self.dc_blocker.process(raw);
            for r in raw.iter_mut() {
                *r *= normalization;
            }

            let chord = patch.resonator_geometry * 10.0;
            let hysteresis = if chord > self.chord_index { F(-0.1) } else { F(0.1) };
            let chord_index = (chord + hysteresis + 0.5).trunc_i32().clamp(0, 10);
            self.chord_index = F::from_i32(chord_index);

            center.fill(F::ZERO);
            sides.fill(F::ZERO);
            let transpositions = &CHORDS[usize::try_from(chord_index).unwrap_or(0)];
            for (s, transpose) in self.string[..num_notes].iter_mut().zip(transpositions) {
                s.set_frequency(frequency * semitones_to_ratio(F(*transpose)));
                s.set_brightness(patch.resonator_brightness);
                s.set_position(patch.resonator_position);
                s.set_damping(damping);
                if num_notes == 1 {
                    s.set_dispersion(patch.resonator_geometry);
                } else {
                    let b = patch.resonator_brightness;
                    s.set_dispersion(if b < 0.5 { F::ZERO } else { (b - 0.5) * -0.4 });
                }
                s.process(raw, center, sides, random);
            }
            for (c, s) in center.iter_mut().zip(sides.iter_mut()) {
                let left = *c;
                let right = *s;
                *c = left - right;
                *s = left + right;
            }
        }

        // The raw mallet signal bleeds into the output.
        for (c, s) in center.iter_mut().zip(&b.strike[..size]) {
            *c += strike_bleed * *s;
        }
    }
}
