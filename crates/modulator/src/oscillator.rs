//! `warps/dsp/oscillator.{h,cc}`: the internal oscillator (sine, triangle, sawtooth, pulse and
//! filtered noise), with phase or frequency modulation from input 1.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::{LUT_MIDI_TO_F_HIGH, LUT_MIDI_TO_F_LOW, LUT_SIN};
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{clip16, index};

/// `OscillatorShape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Sine,
    Triangle,
    Saw,
    Pulse,
    Noise,
}

impl Shape {
    /// `static_cast<OscillatorShape>(i)` for the indices the modulator uses (0 to 4).
    pub(crate) fn from_index(i: i32) -> Self {
        match i {
            0 => Self::Sine,
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Pulse,
            _ => Self::Noise,
        }
    }
}

/// `kToFloat`, 2^-32.
const TO_FLOAT: f32 = 1.0 / 4_294_967_296.0;
/// `kToUint32`, 2^32.
const TO_UINT32: f32 = 4_294_967_296.0;

/// `midi_to_increment(midi_pitch)`: the phase increment per sample at 96 kHz.
#[inline]
pub(crate) fn midi_to_increment(midi_pitch: F) -> F {
    let pitch = (midi_pitch * 256.0).trunc_i32();
    let pitch = 32768 + clip16(pitch.wrapping_sub(20480));
    F(LUT_MIDI_TO_F_HIGH[index(pitch >> 8, LUT_MIDI_TO_F_HIGH.len())]) * F(LUT_MIDI_TO_F_LOW[index(pitch & 0xff, LUT_MIDI_TO_F_LOW.len())])
}

/// The polyBLEP residual for this sample.
#[inline(always)]
fn this_blep_sample(t: F) -> F {
    0.5 * t * t
}

/// The polyBLEP residual for the next sample.
#[inline(always)]
fn next_blep_sample(t: F) -> F {
    let t = 1.0 - t;
    -0.5 * t * t
}

/// `Oscillator`. Its state is shared by every shape, as in the firmware (switching shapes keeps
/// the phase).
#[derive(Clone, Debug, Default)]
pub(crate) struct Oscillator {
    high: bool,
    phase: F,
    phase_increment: F,
    next_sample: F,
    lp_state: F,
    hp_state: F,
    external_input_level: F,
    one_hertz: F,
    filter: Svf,
}

impl Oscillator {
    /// `Init(sample_rate)`.
    pub(crate) fn init(&mut self, sample_rate: f32) {
        self.one_hertz = 1.0 / F(sample_rate);
        self.next_sample = F::ZERO;
        self.phase = F::ZERO;
        self.phase_increment = 100.0 * self.one_hertz;
        self.hp_state = F::ZERO;
        self.lp_state = F::ZERO;
        self.high = false;
        self.external_input_level = F::ZERO;
        self.filter.init();
    }

    /// `Render(shape, note, modulation, out, size)`: the gain the vocoder applies to this shape.
    pub(crate) fn render(&mut self, shape: Shape, note: F, modulation: &[F], out: &mut [F], random: &mut Random) -> F {
        match shape {
            Shape::Sine => self.render_sine(note, modulation, out),
            Shape::Triangle | Shape::Saw | Shape::Pulse => self.render_polyblep(shape, note, modulation, out),
            Shape::Noise => self.render_noise(note, modulation, out, random),
        }
    }

    fn render_sine(&mut self, note: F, modulation: &[F], out: &mut [F]) -> F {
        let mut phase = self.phase;
        let mut phase_increment = ParameterInterpolator::new(self.phase_increment, midi_to_increment(note), out.len());
        for (o, m) in out.iter_mut().zip(modulation) {
            phase += phase_increment.next();
            if phase >= 1.0 {
                phase -= 1.0;
            }
            let mut modulated_phase = (phase * TO_UINT32).trunc_u32();
            // A modulation input of an int16 sample over 32768 stays within the conversion's range.
            modulated_phase = modulated_phase.wrapping_add((*m * 0.5 * TO_UINT32).trunc_i32().cast_unsigned());
            let integral = (modulated_phase >> 22) as usize;
            let fractional = F::from_u32(modulated_phase << 10) * TO_FLOAT;
            let a = F(LUT_SIN[integral.min(LUT_SIN.len() - 1)]);
            let b = F(LUT_SIN[(integral + 1).min(LUT_SIN.len() - 1)]);
            *o = a + (b - a) * fractional;
        }
        self.phase = phase;
        self.phase_increment = phase_increment.value();
        F::ONE
    }

    fn render_polyblep(&mut self, shape: Shape, note: F, modulation: &[F], out: &mut [F]) -> F {
        let mut phase = self.phase;
        let mut phase_increment = ParameterInterpolator::new(self.phase_increment, midi_to_increment(note), out.len());
        let mut next_sample = self.next_sample;
        let mut high = self.high;
        let mut lp_state = self.lp_state;
        let mut hp_state = self.hp_state;
        #[allow(clippy::cast_possible_truncation)] // The C++ assigns the double literal to a float.
        let min_increment = F(1.0e-7_f64 as f32);

        for (o, m) in out.iter_mut().zip(modulation) {
            let mut this_sample = next_sample;
            next_sample = F::ZERO;
            let mut modulated_increment = phase_increment.next() * (1.0 + *m);
            if modulated_increment <= 0.0 {
                modulated_increment = min_increment;
            }
            phase += modulated_increment;

            if shape == Shape::Triangle {
                if !high && phase >= 0.5 {
                    let t = (phase - 0.5) / modulated_increment;
                    this_sample += this_blep_sample(t);
                    next_sample += next_blep_sample(t);
                    high = true;
                }
                if phase >= 1.0 {
                    phase -= 1.0;
                    let t = phase / modulated_increment;
                    this_sample -= this_blep_sample(t);
                    next_sample -= next_blep_sample(t);
                    high = false;
                }
                let integrator_coefficient = modulated_increment * 0.0625;
                next_sample += if phase < 0.5 { F::ZERO } else { F::ONE };
                this_sample = 128.0 * (this_sample - 0.5);
                lp_state += integrator_coefficient * (this_sample - lp_state);
                *o = lp_state;
            } else {
                if phase >= 1.0 {
                    phase -= 1.0;
                    let t = phase / modulated_increment;
                    this_sample -= this_blep_sample(t);
                    next_sample -= next_blep_sample(t);
                }
                next_sample += phase;
                if shape == Shape::Saw {
                    this_sample = this_sample * 2.0 - 1.0;
                    lp_state += 0.3 * (this_sample - lp_state);
                    *o = lp_state;
                } else {
                    lp_state += 0.25 * ((hp_state - this_sample) - lp_state);
                    *o = 4.0 * lp_state;
                    hp_state = this_sample;
                }
            }
        }
        self.high = high;
        self.phase = phase;
        self.next_sample = next_sample;
        self.lp_state = lp_state;
        self.hp_state = hp_state;
        // The C++ returns before the interpolator's destructor writes the new increment back, so
        // the pulse's gain uses the increment the previous block ended on.
        let gain = if shape == Shape::Pulse { 0.025 / (0.0002 + self.phase_increment) } else { F::ONE };
        self.phase_increment = phase_increment.value();
        gain
    }

    /// `Duck(internal, external, destination, size)` with `internal == destination`: the noise
    /// gives way to input 1 as its level rises.
    fn duck(&mut self, out: &mut [F], external: &[F]) {
        let mut level = self.external_input_level;
        for (o, e) in out.iter_mut().zip(external) {
            let error = *e * *e - level;
            level += F(if error > 0.0 { 0.01 } else { 0.0001 }) * error;
            let mut internal_gain = 1.0 - 32.0 * level;
            if internal_gain <= 0.0 {
                internal_gain = F::ZERO;
            }
            *o = *e + internal_gain * (*o - *e);
        }
        self.external_input_level = level;
    }

    fn render_noise(&mut self, note: F, modulation: &[F], out: &mut [F], random: &mut Random) -> F {
        for o in out.iter_mut() {
            let noise = F::from_u32(random.get_word()) * TO_FLOAT;
            *o = 2.0 * noise - 1.0;
        }
        self.duck(out, modulation);
        self.filter.set_f_q(FrequencyApproximation::Accurate, midi_to_increment(note) * 4.0, F::ONE);
        self.filter.process_block(out, FilterMode::LowPass);
        F::ONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pitch_table_gives_a_440_hz_a_at_96_khz() {
        let f = midi_to_increment(F(69.0)).0 * 96_000.0;
        assert!((f - 440.0).abs() < 0.01, "{f}");
    }
}
