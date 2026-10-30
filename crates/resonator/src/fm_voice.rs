//! The FM voice (`rings/dsp/fm_voice.{h,cc}` and `follower.h`): two sine operators with feedback,
//! whose modulation index and level follow the input's envelope and spectral centroid, or an
//! internal envelope when strummed.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::{LUT_FM_FREQUENCY_QUANTIZER, LUT_SINE};
use crate::stmlib::filter::NaiveSvf;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::{interpolate, one_pole, semitones_to_ratio, slew, slope};

/// `rings::Follower`: a three-band envelope and centroid follower.
#[derive(Clone, Copy, Debug, Default)]
struct Follower {
    low_mid_filter: NaiveSvf,
    mid_high_filter: NaiveSvf,
    attack: [F; 3],
    decay: [F; 3],
    detector: [F; 3],
    centroid: F,
}

impl Follower {
    fn init(&mut self, low: F, low_mid: F, mid_high: F) {
        self.low_mid_filter.init();
        self.mid_high_filter.init();
        self.low_mid_filter.set_f_q_dirty(low_mid, F(0.5));
        self.mid_high_filter.set_f_q_dirty(mid_high, F(0.5));
        self.attack[0] = low_mid;
        self.decay[0] = (low_mid * low).sqrt();
        self.attack[1] = (low_mid * mid_high).sqrt();
        self.decay[1] = low_mid;
        self.attack[2] = (mid_high * 0.5).sqrt();
        self.decay[2] = (mid_high * low_mid).sqrt();
        self.detector = [F::ZERO; 3];
        self.centroid = F::ZERO;
    }

    /// `Process(sample, &envelope, &centroid)`.
    #[inline(always)]
    fn process(&mut self, sample: F) -> (F, F) {
        let mut bands = [F::ZERO; 3];
        bands[2] = self.mid_high_filter.process_high_pass(sample);
        bands[1] = self.low_mid_filter.process_high_pass(self.mid_high_filter.lp());
        bands[0] = self.low_mid_filter.lp();

        let mut weighted = F::ZERO;
        let mut total = F::ZERO;
        let mut frequency = F::ZERO;
        for (((detector, band), attack), decay) in self.detector.iter_mut().zip(bands).zip(self.attack).zip(self.decay) {
            slope(detector, band.abs(), attack, decay);
            weighted += *detector * frequency;
            total += *detector;
            frequency += 0.5;
        }

        let error = weighted / (total + 0.001) - self.centroid;
        let coefficient = if error > 0.0 { F(0.05) } else { F(0.001) };
        self.centroid += error * coefficient;
        (total, self.centroid)
    }
}

/// `rings::FMVoice`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FmVoice {
    carrier_frequency: F,
    ratio: F,
    brightness: F,
    damping: F,
    position: F,
    feedback_amount: F,
    previous_carrier_frequency: F,
    previous_modulator_frequency: F,
    previous_brightness: F,
    previous_damping: F,
    previous_feedback_amount: F,
    amplitude_envelope: F,
    brightness_envelope: F,
    gain: F,
    fm_amount: F,
    carrier_phase: u32,
    modulator_phase: u32,
    /// `Init` never writes it: zero from the module's static storage, and it survives
    /// re-initialization, as on the module.
    previous_sample: F,
    follower: Follower,
}

/// `SineFm(phase, fm)`: the sine table read at `phase` plus a phase offset of `fm` turns.
#[inline(always)]
fn sine_fm(phase: u32, fm: F) -> F {
    let phase = phase.wrapping_add(((fm + 4.0) * 536_870_912.0).trunc_u32() << 3);
    let integral = (phase >> 20) as usize;
    let fractional = F::from_u32(phase << 12) / 4_294_967_296.0;
    let a = F(LUT_SINE[integral]);
    let b = F(LUT_SINE[integral + 1]);
    a + (b - a) * fractional
}

impl FmVoice {
    /// `Init`.
    pub fn init(&mut self) {
        self.carrier_frequency = F(220.0) / crate::SAMPLE_RATE;
        self.ratio = F(0.5);
        self.brightness = F(0.5);
        self.damping = F(0.5);
        self.position = F(0.5);
        self.feedback_amount = F::ZERO;

        self.previous_carrier_frequency = self.carrier_frequency;
        self.previous_modulator_frequency = self.carrier_frequency;
        self.previous_brightness = self.brightness;
        self.previous_damping = self.damping;
        self.previous_feedback_amount = self.feedback_amount;

        self.amplitude_envelope = F::ZERO;
        self.brightness_envelope = F::ZERO;

        self.carrier_phase = 0;
        self.modulator_phase = 0;
        self.gain = F::ZERO;
        self.fm_amount = F::ZERO;

        self.follower.init(F(8.0) / crate::SAMPLE_RATE, F(160.0) / crate::SAMPLE_RATE, F(1600.0) / crate::SAMPLE_RATE);
    }

    /// `set_frequency` (the carrier, normalized to the sample rate).
    pub fn set_frequency(&mut self, frequency: F) {
        self.carrier_frequency = frequency;
    }

    /// `set_ratio`.
    pub fn set_ratio(&mut self, ratio: F) {
        self.ratio = ratio;
    }

    /// `set_brightness`.
    pub fn set_brightness(&mut self, brightness: F) {
        self.brightness = brightness;
    }

    /// `set_damping`.
    pub fn set_damping(&mut self, damping: F) {
        self.damping = damping;
    }

    /// `set_position` (unused by the voice's processing, as in the firmware).
    pub fn set_position(&mut self, position: F) {
        self.position = position;
    }

    /// `set_feedback_amount`.
    pub fn set_feedback_amount(&mut self, feedback_amount: F) {
        self.feedback_amount = feedback_amount;
    }

    /// `TriggerInternalEnvelope`.
    pub fn trigger_internal_envelope(&mut self) {
        self.amplitude_envelope = F(1.0);
        self.brightness_envelope = F(1.0);
    }

    /// `Process(in, out, aux, size)` (overwrites `out` and `aux`).
    pub fn process(&mut self, input: &[F], out: &mut [F], aux: &mut [F]) {
        let size = input.len();
        // Interpolate between the "oscillator" behavior and the "FM and low-pass gate" behavior.
        let envelope_amount = if self.damping < 0.9 { F(1.0) } else { (1.0 - self.damping) * 10.0 };

        let amplitude_rt60 = 0.1 * semitones_to_ratio(self.damping * 96.0) * crate::SAMPLE_RATE;
        let amplitude_decay = 1.0 - F(0.001).powf(1.0 / amplitude_rt60);

        let brightness_rt60 = 0.1 * semitones_to_ratio(self.damping * 84.0) * crate::SAMPLE_RATE;
        let brightness_decay = 1.0 - F(0.001).powf(1.0 / brightness_rt60);

        let ratio = interpolate(&LUT_FM_FREQUENCY_QUANTIZER, self.ratio, 128.0);
        let mut modulator_frequency = self.carrier_frequency * semitones_to_ratio(ratio);

        if modulator_frequency > 0.5 {
            modulator_frequency = F(0.5);
        }

        let feedback = (self.feedback_amount - 0.5) * 2.0;

        let mut carrier_increment = ParameterInterpolator::new(self.previous_carrier_frequency, self.carrier_frequency, size);
        let mut modulator_increment = ParameterInterpolator::new(self.previous_modulator_frequency, modulator_frequency, size);
        let mut brightness = ParameterInterpolator::new(self.previous_brightness, self.brightness, size);
        let mut feedback_amount = ParameterInterpolator::new(self.previous_feedback_amount, feedback, size);

        let mut carrier_phase = self.carrier_phase;
        let mut modulator_phase = self.modulator_phase;
        let mut previous_sample = self.previous_sample;

        for ((x, o), a) in input.iter().zip(out.iter_mut()).zip(aux.iter_mut()) {
            // Envelope follower and internal envelope.
            let (amplitude_envelope, mut brightness_envelope) = self.follower.process(*x);
            brightness_envelope *= 2.0 * amplitude_envelope * (2.0 - amplitude_envelope);

            slope(&mut self.amplitude_envelope, amplitude_envelope, F(0.05), amplitude_decay);
            slope(&mut self.brightness_envelope, brightness_envelope, F(0.01), brightness_decay);

            // Compute envelopes.
            let mut brightness_value = brightness.next();
            brightness_value *= brightness_value;
            let fm_amount_min = if brightness_value < 0.5 { F::ZERO } else { brightness_value * 2.0 - 1.0 };
            let fm_amount_max = if brightness_value < 0.5 { 2.0 * brightness_value } else { F(1.0) };
            let fm_envelope = 0.5 + envelope_amount * (self.brightness_envelope - 0.5);
            let fm_amount = (fm_amount_min + fm_amount_max * fm_envelope) * 2.0;
            slew(&mut self.fm_amount, fm_amount, 0.005 + fm_amount_max * 0.015);

            // FM synthesis itself.
            let phase_feedback = if feedback < 0.0 { 0.5 * feedback * feedback } else { F::ZERO };
            modulator_phase =
                modulator_phase.wrapping_add((4_294_967_296.0 * modulator_increment.next() * (1.0 + previous_sample * phase_feedback)).trunc_u32());
            carrier_phase = carrier_phase.wrapping_add((4_294_967_296.0 * carrier_increment.next()).trunc_u32());

            let feedback = feedback_amount.next();
            let modulator_fb = if feedback > 0.0 { 0.25 * feedback * feedback } else { F::ZERO };
            let modulator = sine_fm(modulator_phase, modulator_fb * previous_sample);
            let carrier = sine_fm(carrier_phase, self.fm_amount * modulator);
            one_pole(&mut previous_sample, carrier, F(0.1));

            // Compute the amplitude envelope.
            let gain = 1.0 + envelope_amount * (self.amplitude_envelope - 1.0);
            one_pole(&mut self.gain, gain, 0.005 + 0.045 * self.fm_amount);

            *o = (carrier + 0.5 * modulator) * self.gain;
            *a = 0.5 * modulator * self.gain;
        }
        self.carrier_phase = carrier_phase;
        self.modulator_phase = modulator_phase;
        self.previous_sample = previous_sample;

        self.previous_carrier_frequency = carrier_increment.value();
        self.previous_modulator_frequency = modulator_increment.value();
        self.previous_brightness = brightness.value();
        self.previous_feedback_amount = feedback_amount.value();
    }
}
