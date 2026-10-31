//! `Modulator` (`warps/dsp/modulator.{h,cc}`): the input amplifiers, the internal oscillator, the
//! cross-modulation algorithms behind the 6x converters, the vocoder, the crossfade between them,
//! and the codec's conversions.

#![forbid(unsafe_code)]

use crate::amplifier::Amplifier;
use crate::converter::{Down, Up};
use crate::float::F;
use crate::oscillator::{Oscillator, Shape};
use crate::resources::{SRC_DOWN_6_48, SRC_UP_6_48};
use crate::stmlib::random::Random;
use crate::stmlib::{clip16, constrain, integral_fractional};
use crate::vocoder::Vocoder;
use crate::xmod::{self, PAIRS};
use crate::{BLOCK_SIZE, CarrierShape, OVERSAMPLING, Parameters, SAMPLE_RATE};

/// `Parameters::skewed_modulation_parameter()`: a non-linear response of the timbre for the
/// algorithms between the crossfade and the comparator.
fn skewed_timbre(p: &Parameters) -> F {
    let algorithm = F(p.algorithm);
    let skew = if algorithm <= 1.0 {
        algorithm
    } else if algorithm >= 5.0 {
        F::ONE
    } else if algorithm >= 4.0 {
        5.0 - algorithm
    } else {
        F::ZERO
    };
    let m = F(p.timbre);
    m * (1.0 + skew * (m - 1.0))
}

/// The firmware's meta-modulator at 96 kHz. See [`Modulator::process`] for the render contract.
#[derive(Clone, Debug)]
pub struct Modulator {
    parameters: Parameters,
    previous: Parameters,
    amplifier: [Amplifier; 2],
    xmod_oscillator: Oscillator,
    vocoder_oscillator: Oscillator,
    src_up: [Up<OVERSAMPLING, 8>; 2],
    src_down: Down<OVERSAMPLING, 48>,
    vocoder: Box<Vocoder>,
    internal_modulation: [F; BLOCK_SIZE],
    /// `buffer_[0..3]`: the carrier (later the main output), the modulator, the aux output.
    buffer: [[F; BLOCK_SIZE]; 3],
    /// `src_buffer_[0..2]`: the oversampled carrier (later the oversampled output) and modulator.
    src_buffer: [[F; BLOCK_SIZE * OVERSAMPLING]; 2],
    random: Random,
}

impl Default for Modulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Modulator {
    /// A modulator as the firmware's `Init(96000)` leaves it, its random generator (the noise
    /// carrier's) at the firmware's power-on state. Allocates; nothing else does.
    #[must_use]
    pub fn new() -> Self {
        Self::with_seed(Random::DEFAULT_SEED)
    }

    /// As [`Modulator::new`], with the random generator at `seed` (`Random::Seed`).
    #[must_use]
    pub fn with_seed(seed: u32) -> Self {
        let mut m = Self {
            parameters: Parameters::default(),
            previous: Parameters::default(),
            amplifier: [Amplifier::default(); 2],
            xmod_oscillator: Oscillator::default(),
            vocoder_oscillator: Oscillator::default(),
            src_up: [Up::new(&SRC_UP_6_48), Up::new(&SRC_UP_6_48)],
            src_down: Down::new(&SRC_DOWN_6_48),
            vocoder: Box::new(Vocoder::new()),
            internal_modulation: [F::ZERO; BLOCK_SIZE],
            buffer: [[F::ZERO; BLOCK_SIZE]; 3],
            src_buffer: [[F::ZERO; BLOCK_SIZE * OVERSAMPLING]; 2],
            random: Random::new(seed),
        };
        m.init();
        m
    }

    /// `Init(96000)`. The amplifiers' level followers and gains, which `Init` leaves alone, are
    /// zeroed as well, so `reset` is a fresh modulator (with the random state it has).
    fn init(&mut self) {
        for a in &mut self.amplifier {
            *a = Amplifier::default();
            a.init();
        }
        for s in &mut self.src_up {
            s.init();
        }
        self.src_down.init();
        self.xmod_oscillator.init(SAMPLE_RATE);
        self.vocoder_oscillator.init(SAMPLE_RATE);
        self.vocoder.init(SAMPLE_RATE);
        self.previous = Parameters { drive: [0.0, 0.0], algorithm: 0.0, timbre: 0.0, note: 48.0, carrier: CarrierShape::External };
        self.parameters = Parameters::default();
    }

    /// Back to the state [`Modulator::with_seed`] builds with `seed` (allocation-free).
    pub fn reset(&mut self, seed: u32) {
        self.init();
        self.random = Random::new(seed);
    }

    /// The random generator's state (the noise carrier's).
    #[must_use]
    pub fn random_state(&self) -> u32 {
        self.random.state()
    }

    /// One codec block of frames as the module's codec delivers them: `input[i] = [input 1 (the
    /// carrier), input 2 (the modulator)]` as int16, `output[i] = [main, aux]` as int16, the
    /// firmware's `Process` bit for bit. The block is a whole number of 12-frame groups up to
    /// [`BLOCK_SIZE`] (the module's codec delivers 60); any other length renders silence and
    /// leaves the state untouched.
    pub fn process(&mut self, parameters: &Parameters, input: &[[i16; 2]], output: &mut [[i16; 2]]) {
        let n = input.len().min(output.len());
        if !block_ok(n) {
            output.iter_mut().for_each(|o| *o = [0, 0]);
            return;
        }
        let mut in_1 = [0.0_f32; BLOCK_SIZE];
        let mut in_2 = [0.0_f32; BLOCK_SIZE];
        for ((a, b), x) in in_1.iter_mut().zip(in_2.iter_mut()).zip(&input[..n]) {
            *a = (F::from_i32(i32::from(x[0])) / 32768.0).0;
            *b = (F::from_i32(i32::from(x[1])) / 32768.0).0;
        }
        let mut main = [0.0_f32; BLOCK_SIZE];
        let mut aux = [0.0_f32; BLOCK_SIZE];
        self.process_float(parameters, &in_1[..n], &in_2[..n], &mut main[..n], &mut aux[..n]);
        for ((o, m), a) in output.iter_mut().zip(&main[..n]).zip(&aux[..n]) {
            #[allow(clippy::cast_possible_truncation)] // Clip16 keeps it in range.
            let l = clip16((F(*m) * 32768.0).trunc_i32()) as i16;
            #[allow(clippy::cast_possible_truncation)]
            let r = clip16((F(*a) * 16384.0).trunc_i32()) as i16;
            *o = [l, r];
        }
    }

    /// The firmware's `Process` between the codec's conversions: `input_1` (the carrier, or the
    /// internal oscillator's modulation) and `input_2` (the modulator) in the codec's float scale
    /// (an int16 sample over 32768; any float is taken, a full-scale int16 is 1), `main` and `aux`
    /// before the codec's int16 conversion (`main * 32768` and `aux * 16384` truncated and clipped
    /// are [`Modulator::process`]'s output). The same length rules as [`Modulator::process`];
    /// `main` and `aux` are zeroed on a refused length.
    pub fn process_float(&mut self, parameters: &Parameters, input_1: &[f32], input_2: &[f32], main: &mut [f32], aux: &mut [f32]) {
        let size = input_1.len().min(input_2.len()).min(main.len()).min(aux.len());
        if !block_ok(size) {
            main.iter_mut().for_each(|x| *x = 0.0);
            aux.iter_mut().for_each(|x| *x = 0.0);
            return;
        }
        self.parameters = *parameters;
        let p = self.parameters;
        let shape = p.carrier.index();
        let algorithm = F(p.algorithm);

        // 0: the cross-modulation algorithms; 1: the vocoder.
        let vocoder_amount = constrain((algorithm - 0.7) * 20.0 + 0.5, F::ZERO, F::ONE);

        let [carrier_buf, modulator_buf, aux_buf] = &mut self.buffer;
        let carrier = &mut carrier_buf[..size];
        let modulator = &mut modulator_buf[..size];
        let aux_out = &mut aux_buf[..size];
        if shape == 0 {
            aux_out.fill(F::ZERO);
        }
        let mut converted = [[F::ZERO; BLOCK_SIZE]; 2];
        for (c, x) in converted[0].iter_mut().zip(input_1) {
            *c = F(*x);
        }
        for (c, x) in converted[1].iter_mut().zip(input_2) {
            *c = F(*x);
        }
        let limit = 1.0 - vocoder_amount;
        if shape == 0 {
            self.amplifier[0].process(F(p.drive[0]), limit, &converted[0][..size], carrier, aux_out);
        }
        self.amplifier[1].process(F(p.drive[1]), limit, &converted[1][..size], modulator, aux_out);

        if shape != 0 {
            let modulation = &mut self.internal_modulation[..size];
            modulation.copy_from_slice(&converted[0][..size]);
            let xmod_shape = Shape::from_index(shape - 1);
            let vocoder_shape = Shape::from_index(shape + 1);
            let xmod_carrier_gain = F(0.5);
            let note = F(p.note);
            if vocoder_amount == 0.0 {
                self.xmod_oscillator.render(xmod_shape, note, modulation, aux_out, &mut self.random);
                for (c, a) in carrier.iter_mut().zip(aux_out.iter()) {
                    *c = *a * xmod_carrier_gain;
                }
            } else if vocoder_amount >= 0.5 {
                let gain = self.vocoder_oscillator.render(vocoder_shape, note, modulation, aux_out, &mut self.random);
                for (c, a) in carrier.iter_mut().zip(aux_out.iter()) {
                    *c = *a * gain;
                }
            } else {
                let balance = vocoder_amount * 2.0;
                self.xmod_oscillator.render(xmod_shape, note, modulation, carrier, &mut self.random);
                let gain = self.vocoder_oscillator.render(vocoder_shape, note, modulation, aux_out, &mut self.random);
                for (c, a) in carrier.iter_mut().zip(aux_out.iter_mut()) {
                    let (x, y) = (*c, *a);
                    *a = x + (y - x) * balance;
                    let (x, y) = (x * xmod_carrier_gain, y * gain);
                    *c = x + (y - x) * balance;
                }
            }
        }

        // `main_output` is `buffer_[0]`, the carrier's buffer, in the firmware.
        let mut main_out = [F::ZERO; BLOCK_SIZE];
        let main_out = &mut main_out[..size];
        if vocoder_amount < 0.5 {
            let [over_carrier, over_modulator] = &mut self.src_buffer;
            let over_carrier = &mut over_carrier[..size * OVERSAMPLING];
            let over_modulator = &mut over_modulator[..size * OVERSAMPLING];
            let [up_carrier, up_modulator] = &mut self.src_up;
            up_carrier.process(carrier, over_carrier);
            up_modulator.process(modulator, over_modulator);

            let current = (algorithm * 8.0).min(F(5.999));
            let previous = (F(self.previous.algorithm) * 8.0).min(F(5.999));
            let (integral, fractional) = integral_fractional(current);
            let (previous_integral, mut previous_fractional) = integral_fractional(previous);
            if integral != previous_integral {
                previous_fractional = fractional;
            }
            let pair = PAIRS[usize::try_from(integral).unwrap_or(0).min(PAIRS.len() - 1)];
            // The output overwrites the oversampled carrier as it is read, as in the firmware.
            let mut over_out = [F::ZERO; BLOCK_SIZE * OVERSAMPLING];
            let over_out = &mut over_out[..size * OVERSAMPLING];
            xmod::process(
                pair,
                (previous_fractional, fractional),
                (skewed_timbre(&self.previous), skewed_timbre(&p)),
                over_modulator,
                over_carrier,
                over_out,
            );
            over_carrier.copy_from_slice(over_out);
            self.src_down.process(over_carrier, main_out);
        } else {
            let release_time = constrain(4.0 * (algorithm - 0.75), F::ZERO, F::ONE);
            self.vocoder.set_release_time(release_time * (2.0 - release_time));
            self.vocoder.set_formant_shift(F(p.timbre));
            self.vocoder.process(modulator, carrier, main_out);
        }

        // The crossfade to the raw modulator between the algorithms and the vocoder.
        let transition_gain = 2.0 * if vocoder_amount < 0.5 { vocoder_amount } else { 1.0 - vocoder_amount };
        if transition_gain != 0.0 {
            for (o, m) in main_out.iter_mut().zip(modulator.iter()) {
                *o += transition_gain * (*m - *o);
            }
        }
        carrier.copy_from_slice(main_out);
        for (o, x) in main.iter_mut().zip(main_out.iter()) {
            *o = x.0;
        }
        for (o, x) in aux.iter_mut().zip(aux_out.iter()) {
            *o = x.0;
        }
        self.previous = self.parameters;
    }
}

/// A block the firmware renders completely: whole groups of 12 frames (the vocoder's 3x and 4x
/// decimators) up to [`BLOCK_SIZE`].
fn block_ok(n: usize) -> bool {
    n > 0 && n <= BLOCK_SIZE && n.is_multiple_of(12)
}
