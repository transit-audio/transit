//! The granular sample player (`clouds/dsp/granular_sample_player.h`): seeds grains from the
//! density (at a regular rate or at random), sizes, pitches and pans each one, overlap-adds them
//! and normalizes the sum by the number playing.

#![forbid(unsafe_code)]

use crate::audio_buffer::AudioBuffer;
use crate::float::F;
use crate::grain::{Grain, GrainQuality, GrainStart};
use crate::resources::{LUT_GRAIN_SIZE, LUT_SIN};
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, crossfade, fast_rsqrt_carmack, interpolate, one_pole, semitones_to_ratio, slope};

/// `kMaxNumGrains`.
pub(crate) const MAX_GRAINS: usize = 64;

/// `kMaxBlockSize`: the envelope scratch buffer's length.
const MAX_BLOCK_SIZE: usize = crate::MAX_BLOCK_SIZE;

/// What the processor derives from its parameters for the player (`Parameters::granular` and the
/// fields `Play` reads).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GrainParameters {
    pub(crate) position: F,
    pub(crate) size: F,
    pub(crate) pitch: F,
    pub(crate) stereo_spread: F,
    pub(crate) trigger: bool,
    pub(crate) overlap: F,
    pub(crate) window_shape: F,
    pub(crate) use_deterministic_seed: bool,
}

/// `GranularSamplePlayer`. Every field starts at zero, as on the module; `Init` leaves the grain
/// rate phasor alone, so it survives a quality change.
#[derive(Clone, Debug)]
pub(crate) struct GranularSamplePlayer {
    max_num_grains: i32,
    num_midfi_grains: i32,
    stereo: bool,
    num_grains: F,
    gain_normalization: F,
    grain_size_hint: F,
    grain_rate_phasor: F,
    grains: [Grain; MAX_GRAINS],
    available_grains: [i32; MAX_GRAINS],
    envelope_buffer: [F; MAX_BLOCK_SIZE],
}

impl Default for GranularSamplePlayer {
    fn default() -> Self {
        Self {
            max_num_grains: 0,
            num_midfi_grains: 0,
            stereo: false,
            num_grains: F::ZERO,
            gain_normalization: F::ZERO,
            grain_size_hint: F::ZERO,
            grain_rate_phasor: F::ZERO,
            grains: [Grain::default(); MAX_GRAINS],
            available_grains: [0; MAX_GRAINS],
            envelope_buffer: [F::ZERO; MAX_BLOCK_SIZE],
        }
    }
}

impl GranularSamplePlayer {
    /// `Init(num_channels, max_num_grains)`.
    pub(crate) fn init(&mut self, stereo: bool, max_num_grains: i32) {
        self.max_num_grains = max_num_grains;
        self.num_midfi_grains = 3 * max_num_grains / 4;
        self.gain_normalization = F(1.0);
        for g in &mut self.grains {
            g.init();
        }
        self.num_grains = F::ZERO;
        self.stereo = stereo;
        self.grain_size_hint = F(1024.0);
    }

    fn grain_count(&self) -> usize {
        usize::try_from(self.max_num_grains).unwrap_or(0).min(MAX_GRAINS)
    }

    /// `FillAvailableGrainsList()`.
    fn fill_available_grains_list(&mut self) -> i32 {
        let mut n = 0;
        for i in 0..self.grain_count() {
            if !self.grains[i].active() {
                self.available_grains[n] = i32::try_from(i).unwrap_or(0);
                n += 1;
            }
        }
        i32::try_from(n).unwrap_or(0)
    }

    /// `Play(buffer, parameters, out, size)`: `out` (interleaved stereo) is overwritten.
    pub(crate) fn play(&mut self, buffers: &[AudioBuffer; 2], parameters: &GrainParameters, random: &mut Random, out: &mut [[F; 2]]) {
        let size = out.len();
        let overlap = parameters.overlap;
        let overlap = overlap * overlap * overlap;
        let target_num_grains = F::from_i32(self.max_num_grains) * overlap;
        let mut p = target_num_grains / self.grain_size_hint;
        let space_between_grains = self.grain_size_hint / target_num_grains;
        if parameters.use_deterministic_seed {
            p = F(-1.0);
        } else {
            self.grain_rate_phasor = F(-1000.0);
        }

        // Build a list of available grains.
        let mut num_available_grains = self.fill_available_grains_list();

        // Try to schedule new grains.
        let mut seed_trigger = parameters.trigger;
        let size_i32 = i32::try_from(size).unwrap_or(0);
        for t in 0..size_i32 {
            self.grain_rate_phasor += 1.0;
            // The C++ draws before testing the grain count (`&&` evaluates left first).
            let seed_probabilistic = random.get_float() < p.0 && target_num_grains > self.num_grains.0;
            let seed_deterministic = self.grain_rate_phasor >= space_between_grains.0;
            let seed = seed_probabilistic || seed_deterministic || seed_trigger;
            if num_available_grains != 0 && seed {
                num_available_grains -= 1;
                let index = usize::try_from(self.available_grains[usize::try_from(num_available_grains).unwrap_or(0)]).unwrap_or(0);
                let quality = if num_available_grains < self.num_midfi_grains { GrainQuality::Medium } else { GrainQuality::High };
                let buffer_size = buffers[0].size();
                let buffer_head = buffers[0].head() - size_i32 + t;
                self.schedule_grain(index, parameters, random, t, buffer_size, buffer_head, quality);
                self.grain_rate_phasor = F::ZERO;
                seed_trigger = false;
            }
        }

        // Overlap grains.
        out.fill([F::ZERO; 2]);
        let stereo = self.stereo;
        for i in 0..self.grain_count() {
            self.grains[i].overlap_add(buffers, stereo, out, &mut self.envelope_buffer);
        }

        // Compute normalization factor.
        let active_grains = self.max_num_grains - num_available_grains;
        slope(&mut self.num_grains, F::from_i32(active_grains), F(0.9), F(0.2));

        let mut gain_normalization = if self.num_grains > 2.0 { fast_rsqrt_carmack(self.num_grains - 1.0) } else { F(1.0) };
        let window_gain = constrain(1.0 + 2.0 * parameters.window_shape, F(1.0), F(2.0));
        // The crossfade takes the overlap before it was cubed.
        gain_normalization *= crossfade(F(1.0), window_gain, parameters.overlap);

        // Apply gain normalization.
        for frame in out.iter_mut() {
            one_pole(&mut self.gain_normalization, gain_normalization, F(0.01));
            frame[0] *= self.gain_normalization;
            frame[1] *= self.gain_normalization;
        }
    }

    /// `ScheduleGrain(grain, parameters, pre_delay, buffer_size, buffer_head, quality)`.
    #[allow(clippy::too_many_arguments)]
    fn schedule_grain(
        &mut self,
        index: usize,
        parameters: &GrainParameters,
        random: &mut Random,
        pre_delay: i32,
        buffer_size: i32,
        buffer_head: i32,
        quality: GrainQuality,
    ) {
        let position = parameters.position;
        let pitch = parameters.pitch;
        let window_shape = parameters.window_shape;
        let mut grain_size = interpolate(&LUT_GRAIN_SIZE, parameters.size, 256.0);
        let pitch_ratio = semitones_to_ratio(pitch);
        let inv_pitch_ratio = semitones_to_ratio(-pitch);
        let pan = 0.5 + parameters.stereo_spread * (random.get_float() - 0.5);
        let (gain_l, gain_r) = if self.stereo {
            if pan < 0.5 { (F(1.0), 2.0 * pan) } else { (2.0 * (1.0 - pan), F(1.0)) }
        } else {
            (interpolate(&LUT_SIN, pan, 256.0), interpolate(&LUT_SIN[256..], pan, 256.0))
        };

        if pitch_ratio > 1.0 {
            // The grain's play head moves faster than the record head: shorten the grain so it
            // does not consume too much of the buffer.
            grain_size = grain_size.min(F::from_i32(buffer_size) * 0.25 * inv_pitch_ratio);
        }

        let eaten_by_play_head = grain_size * pitch_ratio;
        let eaten_by_recording_head = grain_size;

        let mut available = F::ZERO;
        available += F::from_i32(buffer_size);
        available -= eaten_by_play_head;
        available -= eaten_by_recording_head;

        let width = grain_size.trunc_i32() & !1;
        let start = buffer_head - (position * available + eaten_by_play_head).trunc_i32();
        #[allow(clippy::cast_possible_wrap)]
        let phase_increment = (pitch_ratio * 65536.0).trunc_u32() as i32;
        self.grains[index].start(&GrainStart { pre_delay, buffer_size, start, width, phase_increment, window_shape, gain_l, gain_r, quality });
        one_pole(&mut self.grain_size_hint, grain_size, F(0.1));
    }
}
