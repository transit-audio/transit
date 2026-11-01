//! The firmware's hidden alternative voice (`elements/dsp/ominous_voice.{h,cc}`, its "easter egg"):
//! two phase-modulation oscillator pairs at eight times the rate, each through a decimating FIR and
//! IIR, a multimode filter swept by the gate envelope, a VCA, and a rotating spatializer. It replaces
//! the exciters and the resonator when selected; the panel controls keep their places with other
//! roles.

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::filter::FrequencyApproximation;
use transit_resonator::stmlib::{clip16, interpolate};

use crate::MAX_BLOCK_SIZE;
use crate::dsp::{NaiveSvf, Svf, interpolate_wrap};
use crate::envelope::{self, Envelope};
use crate::part::FPatch as Patch;
use crate::resources::{LUT_DETUNE_QUANTIZER, LUT_FM_FREQUENCY_QUANTIZER, LUT_MIDI_TO_F_HIGH, LUT_MIDI_TO_F_LOW, LUT_MIDI_TO_INCREMENT_HIGH};
use crate::resources_sine::LUT_SINE;

/// `kOversamplingDownMidi`: the oscillators run three octaves down at eight times the rate.
const OVERSAMPLING_DOWN_MIDI: f32 = -36.0;
/// `kOversamplingUp`.
const OVERSAMPLING_UP: usize = 8;
/// `kNumOscillators`.
const NUM_OSCILLATORS: usize = 2;
/// One block at the oversampled rate.
const OVERSAMPLED: usize = OVERSAMPLING_UP * MAX_BLOCK_SIZE;

/// `kDownsamplingFilter`: `scipy.signal.remez(101, [0, 0.3 / 8, 0.495 / 8, 0.5], [1, 0])`.
#[rustfmt::skip]
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
const DOWNSAMPLING_FILTER: [f32; 101] = [
    -0.001859272945,  0.001184937535,  0.001212413444,  0.001369688661,
     0.001555406705,  0.001685761819,  0.001692922383,  0.001526182555,
     0.001157229282,  0.000582212588, -0.000172916131, -0.001054896973,
    -0.001981709311, -0.002852566234, -0.003555372122, -0.003977161477,
    -0.004020648315, -0.003616652394, -0.002735861951, -0.001399720214,
     0.000314317050,  0.002272758644,  0.004295073578,  0.006166173855,
     0.007654982354,  0.008534833653,  0.008614353501,  0.007761247708,
     0.005919024408,  0.003133736224, -0.000445934810, -0.004563290490,
    -0.008867277113, -0.012932060625, -0.016287075604, -0.018456326393,
    -0.018994127671, -0.017542469537, -0.013838153737, -0.007789574712,
     0.000541523640,  0.010920252821,  0.022936167260,  0.036030557729,
     0.049530599001,  0.062696232995,  0.074769247024,  0.085030908865,
     0.092854033134,  0.097753623479,  0.099422113893,  0.097753623479,
     0.092854033134,  0.085030908865,  0.074769247024,  0.062696232995,
     0.049530599001,  0.036030557729,  0.022936167260,  0.010920252821,
     0.000541523640, -0.007789574712, -0.013838153737, -0.017542469537,
    -0.018994127671, -0.018456326393, -0.016287075604, -0.012932060625,
    -0.008867277113, -0.004563290490, -0.000445934810,  0.003133736224,
     0.005919024408,  0.007761247708,  0.008614353501,  0.008534833653,
     0.007654982354,  0.006166173855,  0.004295073578,  0.002272758644,
     0.000314317050, -0.001399720214, -0.002735861951, -0.003616652394,
    -0.004020648315, -0.003977161477, -0.003555372122, -0.002852566234,
    -0.001981709311, -0.001054896973, -0.000172916131,  0.000582212588,
     0.001157229282,  0.001526182555,  0.001692922383,  0.001685761819,
     0.001555406705,  0.001369688661,  0.001212413444,  0.001184937535,
    -0.001859272945,
];

/// The decimator's ring length (`FIRDownsampler<101, 128, 8>`).
const FIR_BUFFER: usize = 128;

/// `FIRDownsampler<101, 128, 8>`.
#[derive(Clone, Copy, Debug)]
struct FirDownsampler {
    ptr: usize,
    buffer: [F; FIR_BUFFER * 2],
}

impl Default for FirDownsampler {
    fn default() -> Self {
        Self { ptr: 0, buffer: [F::ZERO; FIR_BUFFER * 2] }
    }
}

impl FirDownsampler {
    fn init(&mut self) {
        self.buffer.fill(F::ZERO);
        self.ptr = 0;
    }

    /// `Process(in, out, size)`: one output per eight inputs.
    fn process(&mut self, input: &[F], out: &mut [F]) {
        for (chunk, o) in input.as_chunks::<OVERSAMPLING_UP>().0.iter().zip(out.iter_mut()) {
            for x in chunk {
                self.buffer[self.ptr + FIR_BUFFER] = *x;
                self.buffer[self.ptr] = *x;
                self.ptr = (self.ptr + (FIR_BUFFER - 1)) & (FIR_BUFFER - 1);
            }
            let mut s = F::ZERO;
            for (i, c) in DOWNSAMPLING_FILTER.iter().enumerate() {
                s += self.buffer[self.ptr + i + 1] * *c;
            }
            *o = s;
        }
    }
}

/// `Spatializer`: places a source on a circle around the listener, the far side low-passed.
#[derive(Clone, Copy, Debug, Default)]
struct Spatializer {
    behind: [F; MAX_BLOCK_SIZE],
    left: F,
    right: F,
    angle: F,
    distance: F,
    fixed_position: F,
    behind_filter: NaiveSvf,
}

impl Spatializer {
    fn init(&mut self, fixed_position: F) {
        self.angle = F::ZERO;
        self.fixed_position = fixed_position;
        self.left = F::ZERO;
        self.right = F::ZERO;
        self.distance = F::ZERO;
        self.behind_filter.init();
        self.behind_filter.set_f_q_exact(F(0.05), F(1.0));
    }

    fn rotate(&mut self, rotation_speed: F) {
        self.angle += rotation_speed;
        if self.angle >= 1.0 {
            self.angle -= 1.0;
        }
        if self.angle < 0.0 {
            self.angle += 1.0;
        }
    }

    fn set_distance(&mut self, distance: F) {
        self.distance = distance;
    }

    fn process(&mut self, source: &[F], center: &mut [F], sides: &mut [F]) {
        let size = source.len();
        self.behind_filter.process_low_pass(source, &mut self.behind[..size]);

        let angle = self.angle;
        let mut x = self.distance * interpolate_wrap(&LUT_SINE, angle, 4096.0);
        let y = self.distance * interpolate_wrap(&LUT_SINE, angle + 0.25, 4096.0);
        let backfront = (1.0 + y) * 0.5 * self.distance;
        x += self.fixed_position * (1.0 - self.distance);

        let target_left = interpolate_wrap(&LUT_SINE, (1.0 + x) * 0.125, 4096.0);
        let target_right = interpolate_wrap(&LUT_SINE, (3.0 + x) * 0.125, 4096.0);

        // Prevent zipper noise during rendering.
        let step = 1.0 / F::from_usize(size);
        let left_increment = (target_left - self.left) * step;
        let right_increment = (target_right - self.right) * step;

        for i in 0..size {
            self.left += left_increment;
            self.right += right_increment;
            let y = source[i] + backfront * (self.behind[i] - source[i]);
            let l = self.left * y;
            let r = self.right * y;
            center[i] += (l + r) * 0.5;
            sides[i] += (l - r) * 0.5 / 0.7;
        }
    }
}

/// The pitch tables' index for a MIDI pitch: `32768 + Clip16(int(midi * 256) - 20480)`.
#[inline(always)]
fn pitch_index(midi_pitch: F) -> (usize, usize) {
    let pitch = 32768 + clip16((midi_pitch * 256.0).trunc_i32().wrapping_sub(20480));
    let pitch = usize::try_from(pitch).unwrap_or(0);
    (pitch >> 8, pitch & 0xff)
}

/// `FmOscillator`: a carrier phase-modulated by a modulator with feedback, plus an external
/// modulation input.
#[derive(Clone, Copy, Debug, Default)]
struct FmOscillator {
    fm_amount: F,
    previous_sample: F,
    /// The phases. `Init` never writes them: they start at zero (the module's static storage).
    phase_carrier: u32,
    phase_mod: u32,
}

impl FmOscillator {
    fn init(&mut self) {
        self.fm_amount = F::ZERO;
        self.previous_sample = F::ZERO;
    }

    fn midi_to_increment(midi_pitch: F) -> u32 {
        let (high, low) = pitch_index(midi_pitch);
        let increment = F(LUT_MIDI_TO_INCREMENT_HIGH[high.min(255)]) * F(LUT_MIDI_TO_F_LOW[low]);
        increment.trunc_u32()
    }

    /// `SineFm(phase, fm)`. The offset `fm * 2^31` is often negative or above 2^32; its conversion
    /// wraps modulo one turn, as x86-64 compilers emit the firmware's `static_cast<uint32_t>` and as
    /// the oracle's patch 0012 makes every host compute it (AArch64 and the module's Cortex-M4
    /// saturate instead: the conversion is undefined in C++).
    #[inline(always)]
    fn sine_fm(phase: u32, fm: F) -> F {
        let phase = phase.wrapping_add((fm * 2_147_483_648.0).trunc_u32());
        let integral = (phase >> 20) as usize;
        let fractional = F::from_u32(phase << 12) / 4_294_967_296.0;
        let a = F(LUT_SINE[integral]);
        let b = F(LUT_SINE[integral + 1]);
        a + (b - a) * fractional
    }

    #[allow(clippy::too_many_arguments)]
    fn process(&mut self, frequency: F, ratio: F, feedback_amount: F, target_fm_amount: F, external_fm: &[F], destination: &mut [F]) {
        let size = destination.len();
        let ratio = interpolate(&LUT_FM_FREQUENCY_QUANTIZER, ratio, 128.0);
        let inc_carrier = Self::midi_to_increment(frequency);
        let inc_mod = Self::midi_to_increment(frequency + ratio);
        let mut phase_carrier = self.phase_carrier;
        let mut phase_mod = self.phase_mod;

        // Linear interpolation on the FM amount.
        let step = 1.0 / F::from_usize(size);
        let mut fm_amount = self.fm_amount;
        let fm_amount_increment = (target_fm_amount - fm_amount) * step;
        let mut previous_sample = self.previous_sample;

        // To prevent aliasing, reduce the FM amount when the frequency or the feedback is high.
        let brightness = frequency + ratio * 0.75 - 60.0 + feedback_amount * 24.0;
        let mut amount_attenuation = if brightness <= 0.0 { F(1.0) } else { 1.0 - brightness * brightness * 0.0015 };
        if amount_attenuation < 0.0 {
            amount_attenuation = F::ZERO;
        }

        for (d, x) in destination.iter_mut().zip(external_fm) {
            fm_amount += fm_amount_increment;
            phase_carrier = phase_carrier.wrapping_add(inc_carrier);
            phase_mod = phase_mod.wrapping_add(inc_mod);
            let modulator = Self::sine_fm(phase_mod, feedback_amount * previous_sample);
            previous_sample = Self::sine_fm(phase_carrier, amount_attenuation * (modulator * fm_amount + *x));
            *d = previous_sample;
        }

        self.phase_carrier = phase_carrier;
        self.phase_mod = phase_mod;
        self.fm_amount = fm_amount;
        self.previous_sample = previous_sample;
    }
}

/// The scratch buffers of one block.
#[derive(Clone, Copy, Debug)]
struct Buffers {
    external_fm_oversampled: [F; OVERSAMPLED],
    osc_oversampled: [F; OVERSAMPLED],
    osc: [F; MAX_BLOCK_SIZE],
}

impl Default for Buffers {
    fn default() -> Self {
        Self { external_fm_oversampled: [F::ZERO; OVERSAMPLED], osc_oversampled: [F::ZERO; OVERSAMPLED], osc: [F::ZERO; MAX_BLOCK_SIZE] }
    }
}

/// `elements::OminousVoice`.
#[derive(Clone, Debug, Default)]
pub(crate) struct FmPairVoice {
    buffers: Buffers,
    previous_gate: bool,
    envelope: Envelope,
    level_state: F,
    /// The smoothed damping and feedback. `Init` never writes them: they start at zero (the
    /// module's static storage) and keep their values across re-initializations.
    damping: F,
    feedback: F,
    osc_level: [F; NUM_OSCILLATORS],
    external_fm_state: [F; NUM_OSCILLATORS],
    oscillator: [FmOscillator; NUM_OSCILLATORS],
    iir_downsampler: [NaiveSvf; NUM_OSCILLATORS],
    fir_downsampler: [FirDownsampler; NUM_OSCILLATORS],
    filter: [Svf; NUM_OSCILLATORS],
    spatializer: [Spatializer; NUM_OSCILLATORS],
}

/// `Upsample<8>(state, source, destination, size)`: linear interpolation to eight times the rate.
fn upsample(state: &mut F, source: &[F], destination: &mut [F]) {
    let down = 1.0 / F::from_usize(OVERSAMPLING_UP);
    let mut s = *state;
    for (x, chunk) in source.iter().zip(destination.as_chunks_mut::<OVERSAMPLING_UP>().0.iter_mut()) {
        let increment = (*x - s) * down;
        for d in chunk {
            *d = s;
            s += increment;
        }
    }
    *state = s;
}

/// `midi_to_frequency(midi_pitch)`, at least MIDI -12.
fn midi_to_frequency(midi_pitch: F) -> F {
    let midi_pitch = if midi_pitch < -12.0 { F(-12.0) } else { midi_pitch };
    let (high, low) = pitch_index(midi_pitch);
    F(LUT_MIDI_TO_F_HIGH[high.min(255)]) * F(LUT_MIDI_TO_F_LOW[low])
}

impl FmPairVoice {
    /// `Init()`.
    pub(crate) fn init(&mut self) {
        self.envelope.init();
        self.envelope.set_adsr(F(0.5), F(0.5), F(0.5), F(0.5));
        self.previous_gate = false;
        self.level_state = F::ZERO;
        for i in 0..NUM_OSCILLATORS {
            self.external_fm_state[i] = F::ZERO;
            self.oscillator[i].init();
            // Downsampling is done mostly by the FIR; the IIR adds a few dB of stopband attenuation
            // at the highest frequencies.
            self.fir_downsampler[i].init();
            self.iir_downsampler[i].init();
            self.iir_downsampler[i].set_f_q_exact(1.0 / F::from_usize(OVERSAMPLING_UP) * 0.8, F(0.5));
            self.osc_level[i] = F::ZERO;
            self.filter[i].init();
            self.spatializer[i].init(if i == 0 { F(-0.7) } else { F(0.7) });
        }
    }

    /// `GetGateFlags(gate_in)`.
    fn gate_flags(&mut self, gate_in: bool) -> u8 {
        let mut flags = 0;
        if gate_in {
            if !self.previous_gate {
                flags |= envelope::RISING_EDGE;
            }
            flags |= envelope::GATE;
        } else if self.previous_gate {
            flags = envelope::FALLING_EDGE;
        }
        self.previous_gate = gate_in;
        flags
    }

    fn configure_envelope(&mut self, patch: &Patch) {
        let shape = patch.exciter_envelope_shape;
        if shape < 0.4 {
            let dr = (shape * 0.625 + 0.2) * 1.8;
            self.envelope.set_adsr(F::ZERO, dr, F::ZERO, dr);
        } else if shape < 0.6 {
            let s = (shape - 0.4) * 5.0;
            self.envelope.set_adsr(F::ZERO, F(0.80), s, F(0.80));
        } else {
            let dr = ((1.0 - shape) * 0.75 + 0.15) * 1.8;
            self.envelope.set_adsr(F::ZERO, dr, F(1.0), dr);
        }
    }

    /// `Process(patch, frequency, strength, gate_in, blow_in, strike_in, raw, center, sides, size)`;
    /// `frequency` is the MIDI pitch here.
    #[allow(clippy::too_many_arguments)]
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
    ) {
        let size = raw.len();
        let flags = self.gate_flags(gate_in);

        // The envelope.
        self.configure_envelope(patch);
        let mut level = self.envelope.process(flags);
        level += if strength >= 0.5 { 2.0 * strength - 1.0 } else { F::ZERO };
        let level_increment = (level - self.level_state) / F::from_usize(size);

        self.damping += 0.1 * (patch.resonator_damping - self.damping);
        let filter_env_amount = if self.damping <= 0.9 { 1.1 * self.damping } else { F(0.99) };
        let vca_env_amount = 1.0 + self.damping * self.damping * self.damping * self.damping * 0.5;

        // The filter.
        let mut cutoff_midi = F(12.0);
        cutoff_midi += patch.resonator_brightness * 140.0;
        cutoff_midi += filter_env_amount * level * 120.0;
        cutoff_midi += 0.5 * (frequency - 64.0);

        let cutoff = midi_to_frequency(cutoff_midi);
        let q_bump = patch.resonator_geometry - 0.6;
        let q = 1.72 - q_bump * q_bump * 2.0;
        let cutoff_2 = cutoff * (1.0 + patch.resonator_modulation_offset);

        self.filter[0].set_f_q(FrequencyApproximation::Fast, cutoff, q);
        self.filter[1].set_f_q(FrequencyApproximation::Fast, cutoff_2, q * 1.25);

        // Each oscillator.
        center.fill(F::ZERO);
        sides.fill(F::ZERO);
        raw.fill(F::ZERO);

        const ROTATION_SPEED: [f32; 2] = [1.0, 1.123_456];
        self.feedback += 0.01 * (patch.exciter_bow_timbre - self.feedback);
        let frequency = frequency + OVERSAMPLING_DOWN_MIDI;
        let oversampled = size * OVERSAMPLING_UP;
        #[allow(clippy::needless_range_loop)] // The firmware's index-parallel arrays.
        for i in 0..NUM_OSCILLATORS {
            let b = &mut self.buffers;
            upsample(&mut self.external_fm_state[i], if i == 0 { blow_in } else { strike_in }, &mut b.external_fm_oversampled[..oversampled]);
            let (detune, ratio, amount, level) = if i == 0 {
                (F::ZERO, patch.exciter_blow_meta, patch.exciter_blow_timbre, patch.exciter_blow_level)
            } else {
                (
                    interpolate(&LUT_DETUNE_QUANTIZER, patch.exciter_bow_level, 64.0),
                    patch.exciter_strike_meta,
                    patch.exciter_strike_timbre,
                    patch.exciter_strike_level,
                )
            };

            self.oscillator[i].process(
                frequency + detune,
                ratio,
                self.feedback * (0.25 + 0.15 * patch.exciter_signature),
                (2.0 - patch.exciter_signature * self.feedback) * amount,
                &b.external_fm_oversampled[..oversampled],
                &mut b.osc_oversampled[..oversampled],
            );

            self.iir_downsampler[i].process_low_pass_in_place(&mut b.osc_oversampled[..oversampled]);
            self.fir_downsampler[i].process(&b.osc_oversampled[..oversampled], &mut b.osc[..size]);

            // Copy to the raw buffer.
            let mut level_state = self.osc_level[i];
            for (o, r) in b.osc[..size].iter_mut().zip(raw.iter_mut()) {
                level_state += 0.01 * (level - level_state);
                *o *= level_state;
                *r += *o;
            }
            self.osc_level[i] = level_state;

            // The filter.
            self.filter[i].process_multimode(&mut b.osc[..size], patch.resonator_geometry);

            // The VCA.
            let mut l = self.level_state;
            for o in &mut b.osc[..size] {
                let mut gain = l * vca_env_amount;
                if gain >= 1.0 {
                    gain = F(1.0);
                }
                *o *= gain;
                l += level_increment;
            }

            // Spatialize.
            let f = patch.resonator_position * patch.resonator_position * 0.001;
            let distance = patch.resonator_position;
            self.spatializer[i].rotate(f * ROTATION_SPEED[i]);
            self.spatializer[i].set_distance(distance * (2.0 - distance));
            self.spatializer[i].process(&b.osc[..size], center, sides);
        }

        self.level_state = level;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pitch_index_clips_to_the_tables() {
        assert_eq!(pitch_index(F(80.0)), (128, 0));
        assert_eq!(pitch_index(F(-500.0)), (0, 0));
        assert_eq!(pitch_index(F(500.0)).0, 255);
    }
}
