//! The exciters (`elements/dsp/exciter.{h,cc}`): seven models of excitation signal, a low-pass
//! filter on the synthetic ones, and the damping some of them apply to the resonator on release.
//! The voice runs three: the bow (flow noise), the blow (the granular noise sample) and the strike
//! (the sample player, mallet, plectrum or particles, picked by its meta control).

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::random::Random;
use transit_resonator::stmlib::{integral_fractional, semitones_to_ratio};

use crate::dsp::{Mode, Svf, index};
use crate::resources::{LUT_APPROX_SVF_G, LUT_APPROX_SVF_GAIN, LUT_APPROX_SVF_H, LUT_APPROX_SVF_R, SMP_BOUNDARIES};
use crate::samples::{SMP_NOISE_SAMPLE, SMP_SAMPLE_DATA};

/// `EXCITER_FLAG_RISING_EDGE`.
pub(crate) const RISING_EDGE: u8 = 1;
/// `EXCITER_FLAG_FALLING_EDGE`.
pub(crate) const FALLING_EDGE: u8 = 2;
/// `EXCITER_FLAG_GATE`.
pub(crate) const GATE: u8 = 4;

/// `ExciterModel`, in the firmware's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Model {
    /// Zero: the model of an exciter `Init` has not reached (the module's static storage).
    #[default]
    GranularSamplePlayer,
    SamplePlayer,
    Mallet,
    Plectrum,
    Particles,
    Flow,
    Noise,
}

impl Model {
    const ALL: [Self; 7] = [Self::GranularSamplePlayer, Self::SamplePlayer, Self::Mallet, Self::Plectrum, Self::Particles, Self::Flow, Self::Noise];
}

/// `elements::Exciter`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Exciter {
    model: Model,
    parameter: F,
    timbre: F,
    lp: Svf,
    damp_state: F,
    particle_state: F,
    /// The particles' range. `Init` never writes it; it is set on the first rising edge before it
    /// is read.
    particle_range: F,
    damping: F,
    signature: F,
    /// The sample players' phase. `Init` never writes it: it starts at zero (the module's static
    /// storage), and the granular player never resets it.
    phase: u32,
    delay: u32,
    plectrum_delay: u32,
}

/// `RandomSample()`: a word over 2^32.
#[inline(always)]
fn random_sample(random: &mut Random) -> F {
    F::from_u32(random.get_word()) / 4_294_967_296.0
}

/// `uint32_t(x * 4294967296.0f)` for a probability below 1 (a constant the compiler folds).
#[inline(always)]
fn probability(x: f32) -> u32 {
    (F(x) * 4_294_967_296.0).trunc_u32()
}

impl Exciter {
    /// `Init()`.
    pub(crate) fn init(&mut self) {
        self.model = Model::Mallet;
        self.parameter = F::ZERO;
        self.timbre = F(0.99);
        self.lp.init();
        self.damp_state = F::ZERO;
        self.delay = 0;
        self.plectrum_delay = 0;
        self.particle_state = F(0.5);
        self.damping = F::ZERO;
        self.signature = F::ZERO;
    }

    pub(crate) fn set_signature(&mut self, signature: F) {
        self.signature = signature;
    }

    pub(crate) fn set_model(&mut self, model: Model) {
        self.model = model;
    }

    pub(crate) fn set_parameter(&mut self, parameter: F) {
        self.parameter = parameter;
    }

    pub(crate) fn set_timbre(&mut self, timbre: F) {
        self.timbre = timbre;
    }

    /// `set_meta(meta, first, last)`: the meta control's integer part picks the model, its
    /// fractional part is the parameter.
    pub(crate) fn set_meta(&mut self, meta: F, first: Model, last: Model) {
        let span = F::from_i32(last as i32 - first as i32 + 1);
        let (integral, fractional) = integral_fractional(meta * span);
        let m = (first as i32 + integral).clamp(0, Model::Noise as i32);
        self.model = Model::ALL[usize::try_from(m).unwrap_or(0)];
        self.parameter = fractional;
    }

    /// `damping()`: how much the exciter damps the resonator this block.
    pub(crate) fn damping(&self) -> F {
        self.damping
    }

    /// `GetPulseAmplitude(cutoff)`.
    fn pulse_amplitude(cutoff: F) -> F {
        let i = (cutoff * 256.0).trunc_u32();
        F(LUT_APPROX_SVF_GAIN[index(i32::try_from(i).unwrap_or(i32::MAX), LUT_APPROX_SVF_GAIN.len())])
    }

    fn lut(table: &[f32], x: F) -> F {
        let i = (x * 256.0).trunc_u32();
        F(table[index(i32::try_from(i).unwrap_or(i32::MAX), table.len())])
    }

    /// `Process(flags, out, size)`.
    pub(crate) fn process(&mut self, flags: u8, out: &mut [F], random: &mut Random) {
        self.damping = F::ZERO;
        match self.model {
            Model::GranularSamplePlayer => self.process_granular_sample_player(out, random),
            Model::SamplePlayer => self.process_sample_player(flags, out),
            Model::Mallet => self.process_mallet(flags, out),
            Model::Plectrum => self.process_plectrum(flags, out),
            Model::Particles => self.process_particles(flags, out, random),
            Model::Flow => self.process_flow(flags, out, random),
            Model::Noise => Self::process_noise(out, random),
        }
        if self.model != Model::GranularSamplePlayer && self.model != Model::SamplePlayer {
            let g = Self::lut(&LUT_APPROX_SVF_G, self.timbre);
            if self.model == Model::Noise {
                self.lp.set_g_r(g, Self::lut(&LUT_APPROX_SVF_R, self.parameter));
            } else {
                self.lp.set_g_r_h(g, F(2.0), Self::lut(&LUT_APPROX_SVF_H, self.timbre));
            }
            self.lp.process_block(out, Mode::LowPass);
        }
    }

    fn process_granular_sample_player(&mut self, out: &mut [F], random: &mut Random) {
        let restart_prob = probability(0.01);
        let restart_point = (self.parameter * 32767.0).trunc_u32() << 17;
        let phase_increment = (131_072.0 * semitones_to_ratio(72.0 * self.timbre - 60.0)).trunc_u32();
        let base = (self.signature * 8192.0).trunc_usize();
        let mut phase = self.phase;
        for o in out.iter_mut() {
            let phase_integral = (phase >> 17) as usize;
            let phase_fractional = F::from_u32(phase & 0x1_ffff) / 131_072.0;
            let at = (base + phase_integral).min(SMP_NOISE_SAMPLE.len() - 2);
            let a = F(f32::from(SMP_NOISE_SAMPLE[at]));
            let b = F(f32::from(SMP_NOISE_SAMPLE[at + 1]));
            *o = (a + (b - a) * phase_fractional) / 32768.0;
            phase = phase.wrapping_add(phase_increment);
            if random.get_word() < restart_prob {
                phase = restart_point;
            }
        }
        self.phase = phase;
        self.damping = F::ZERO;
    }

    fn process_sample_player(&mut self, flags: u8, out: &mut [F]) {
        let (mut index_integral, mut index_fractional) = integral_fractional((1.0 - self.parameter) * 8.0);
        if index_integral == 8 {
            index_integral = 7;
            index_fractional = F(1.0);
        }
        let i = usize::try_from(index_integral).unwrap_or(0).min(7);
        let offset_1 = SMP_BOUNDARIES[i];
        let offset_2 = SMP_BOUNDARIES[i + 1];
        let length_1 = offset_2 - offset_1 - 1;
        let length_2 = SMP_BOUNDARIES[i + 2] - offset_2 - 1;
        let phase_increment = (65536.0 * semitones_to_ratio(72.0 * self.timbre - 36.0 + 7.0)).trunc_u32();

        let mut damp = self.damp_state;
        let mut phase = self.phase;
        if flags & RISING_EDGE != 0 {
            damp = F::ZERO;
            phase = 0;
        }
        if flags & GATE == 0 {
            damp = 1.0 - 0.95 * (1.0 - damp);
        }

        for o in out.iter_mut() {
            let phase_integral = (phase >> 16) as usize;
            let phase_fractional = F::from_u32(phase & 0xffff) / 65536.0;
            let mut sample_1 = F::ZERO;
            let mut sample_2 = F::ZERO;
            let mut step = false;
            if phase_integral < length_1 {
                let a = F(f32::from(SMP_SAMPLE_DATA[offset_1 + phase_integral]));
                let b = F(f32::from(SMP_SAMPLE_DATA[offset_1 + phase_integral + 1]));
                sample_1 = a + (b - a) * phase_fractional;
                step = true;
            }
            if phase_integral < length_2 {
                let a = F(f32::from(SMP_SAMPLE_DATA[offset_2 + phase_integral]));
                let b = F(f32::from(SMP_SAMPLE_DATA[offset_2 + phase_integral + 1]));
                sample_2 = a + (b - a) * phase_fractional;
                step = true;
            }
            if step {
                phase = phase.wrapping_add(phase_increment);
            }
            *o = (sample_1 + (sample_2 - sample_1) * index_fractional) / 65536.0;
        }
        self.phase = phase;
        self.damping = damp * if self.parameter >= 0.8 { self.parameter * 5.0 - 4.0 } else { F::ZERO };
        self.damp_state = damp;
    }

    fn process_mallet(&mut self, flags: u8, out: &mut [F]) {
        out.fill(F::ZERO);
        if flags & RISING_EDGE != 0 {
            self.damp_state = F::ZERO;
            out[0] = Self::pulse_amplitude(self.timbre);
        }
        if flags & GATE == 0 {
            self.damp_state = 1.0 - 0.95 * (1.0 - self.damp_state);
        }
        self.damping = self.damp_state * (1.0 - self.parameter);
    }

    fn process_plectrum(&mut self, flags: u8, out: &mut [F]) {
        let amplitude = Self::pulse_amplitude(self.timbre);
        let mut damp = self.damp_state;
        let mut impulse = F::ZERO;
        if flags & RISING_EDGE != 0 {
            impulse = -amplitude * (0.05 + self.signature * 0.2);
            self.plectrum_delay = (4096.0 * self.parameter * self.parameter).trunc_u32().wrapping_add(64);
        }
        for o in out.iter_mut() {
            if self.plectrum_delay != 0 {
                self.plectrum_delay -= 1;
                if self.plectrum_delay == 0 {
                    impulse = amplitude;
                }
                damp = 1.0 - 0.997 * (1.0 - damp);
            } else {
                damp = 0.9 * damp;
            }
            *o = impulse;
            impulse = F::ZERO;
        }
        self.damping = damp * 0.5;
        self.damp_state = damp;
    }

    fn process_particles(&mut self, flags: u8, out: &mut [F], random: &mut Random) {
        if flags & RISING_EDGE != 0 {
            self.particle_state = random_sample(random);
            self.particle_state = 1.0 - 0.6 * self.particle_state * self.particle_state;
            self.delay = 0;
            self.particle_range = F(1.0);
        }
        out.fill(F::ZERO);
        if flags & GATE != 0 {
            let up_probability = probability(0.7);
            let down_probability = probability(0.3);
            let amplitude = Self::pulse_amplitude(self.timbre);
            for o in out.iter_mut() {
                if self.delay == 0 {
                    let mut amount = random_sample(random);
                    amount = 1.05 + 0.5 * amount * amount;
                    if random.get_word() > up_probability {
                        self.particle_state *= amount;
                        if self.particle_state >= self.particle_range + 0.25 {
                            self.particle_state = self.particle_range + 0.25;
                        }
                    } else if random.get_word() < down_probability {
                        self.particle_state /= amount;
                        if self.particle_state <= 0.02 {
                            self.particle_state = F(0.02);
                        }
                    }
                    self.delay = (self.particle_state * 0.15 * crate::SAMPLE_RATE).trunc_u32();
                    let mut gain = 1.0 - self.particle_range;
                    gain *= gain;
                    *o = self.particle_state * amplitude * (1.0 - gain);

                    let decay_factor = 1.0 - self.parameter;
                    self.particle_range *= 1.0 - decay_factor * decay_factor * 0.5;
                } else {
                    self.delay -= 1;
                }
            }
        }
    }

    fn process_flow(&mut self, flags: u8, out: &mut [F], random: &mut Random) {
        let scale = self.parameter * self.parameter * self.parameter * self.parameter;
        let threshold = 0.0001 + scale * 0.125;
        if flags & RISING_EDGE != 0 {
            self.particle_state = F(0.5);
        }
        for o in out.iter_mut() {
            let sample = random_sample(random);
            if sample < threshold {
                self.particle_state = -self.particle_state;
            }
            *o = self.particle_state + (sample - 0.5 - self.particle_state) * scale;
        }
    }

    fn process_noise(out: &mut [F], random: &mut Random) {
        for o in out.iter_mut() {
            *o = random_sample(random) - 0.5;
        }
    }
}
