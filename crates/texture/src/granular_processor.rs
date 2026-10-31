//! `beads/dsp/granular_processor.h`: one buffer quality's processor. The input and the feedback
//! (soft-clipped at the lower resolutions) are recorded, then read back as grains or as delay
//! taps; the output feeds the next input through a DC blocker and a low-pass. The clean-tape
//! quality runs it at half the codec rate behind 2:1 converters.

#![forbid(unsafe_code)]
// The channel loops index the frames as the firmware's `.l` and `.r` lines do.
#![allow(clippy::needless_range_loop)]

use crate::MAX_BLOCK_SIZE;
use crate::delay_controller::DelayOutput;
use crate::delay_grain::{DelayGranularPlayer, MultitapDelay};
use crate::delay_line::{DelayLine, DelayMemory, Frame, Resolution};
use crate::float::F;
use crate::fx::Limiter;
use crate::randomized::{Parameters, RandomWalkModulation};
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::sample_rate_converter::{Downsampler, Upsampler};
use crate::stmlib::{constrain, one_pole, semitones_to_ratio, soft_clip};
use crate::trigger::TriggerOutput;

/// What drives the grains or the taps for a block: trigger samples in the granular mode, tap maps
/// in the delay mode.
#[derive(Clone, Copy, Debug)]
pub enum Drive<'a> {
    /// The granular mode.
    Grains(&'a [TriggerOutput]),
    /// The delay mode.
    Taps(&'a [DelayOutput]),
}

/// `GranularProcessor<resolution, max_delay, ratio>`.
#[derive(Clone, Copy, Debug)]
pub struct GranularProcessor {
    resolution: Resolution,
    ratio: usize,
    granular_player: DelayGranularPlayer,
    multitap_delay: MultitapDelay,
    stereo_line: DelayLine,
    mono_line: DelayLine,
    src_down: [Downsampler; 2],
    src_up: [Upsampler; 2],
    feedback: F,
    speed: F,
    bandwidth: F,
    previous_sample: Frame,
    dc_blocker: [Svf; 2],
    pitch_modulation: RandomWalkModulation,
    input_limiter: Limiter,
}

impl GranularProcessor {
    /// A processor at `resolution` on a stereo line of `max_delay` frames (the mono line holds
    /// twice as many), running at the codec rate divided by `ratio` (1 or 2).
    #[must_use]
    pub fn new(resolution: Resolution, max_delay: i32, ratio: usize) -> Self {
        Self {
            resolution,
            ratio,
            granular_player: DelayGranularPlayer::default(),
            multitap_delay: MultitapDelay::default(),
            stereo_line: DelayLine::new(resolution, true, max_delay),
            mono_line: DelayLine::new(resolution, false, max_delay * 2),
            src_down: [Downsampler::default(); 2],
            src_up: [Upsampler::default(); 2],
            feedback: F::ZERO,
            speed: F::ZERO,
            bandwidth: F::ZERO,
            previous_sample: [F::ZERO; 2],
            dc_blocker: [Svf::default(); 2],
            pitch_modulation: RandomWalkModulation::default(),
            input_limiter: Limiter::default(),
        }
    }

    /// The stored samples its lines need.
    #[must_use]
    pub fn items_needed(&self) -> usize {
        self.stereo_line.items_needed().max(self.mono_line.items_needed())
    }

    /// `max_delay_times_ratio()`.
    #[must_use]
    pub fn max_delay_times_ratio(&self) -> i32 {
        self.stereo_line.max_delay() * i32::try_from(self.ratio).unwrap_or(1)
    }

    /// `ratio`.
    #[must_use]
    pub fn ratio(&self) -> usize {
        self.ratio
    }

    /// `Init(sample_rate, buffers)`.
    pub fn init(&mut self, sample_rate: F, memory: &mut DelayMemory) {
        self.mono_line.reset(memory);
        self.stereo_line.reset(memory);
        self.granular_player.scheduler.init(sample_rate);
        self.multitap_delay.init();
        self.feedback = F::ZERO;
        self.speed = F::ZERO;
        self.bandwidth = F::ZERO;
        self.previous_sample = [F::ZERO; 2];
        for i in 0..2 {
            self.dc_blocker[i].init();
            self.dc_blocker[i].set_f_q(FrequencyApproximation::Fast, 40.0 / sample_rate, F(0.5));
            self.src_down[i].init();
            self.src_up[i].init();
        }
        self.pitch_modulation.init(0.2 / sample_rate);
        self.input_limiter.init(1.0 / sample_rate, F(0.95));
    }

    /// `Reset()`: the lines (and the memory) cleared, the grains and the taps re-initialized.
    pub fn reset(&mut self, memory: &mut DelayMemory) {
        self.mono_line.reset(memory);
        self.stereo_line.reset(memory);
        self.granular_player.scheduler.reset();
        self.multitap_delay.reset();
    }

    /// `Trim(mono)`.
    pub fn trim(&mut self, mono: bool) {
        if mono { self.mono_line.trim() } else { self.stereo_line.trim() }
    }

    /// `remaining(mono)`.
    #[must_use]
    pub fn remaining(&self, mono: bool) -> i32 {
        if mono { self.mono_line.remaining() } else { self.stereo_line.remaining() }
    }

    /// `Process(triggers, taps, freeze, parameters, feedback, mono, in, out, size)`.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        rng: &mut Random,
        memory: &mut DelayMemory,
        drive: Drive<'_>,
        freeze: &[bool],
        parameters: &Parameters,
        feedback: F,
        mono: bool,
        input: &[Frame],
        out: &mut [Frame],
    ) {
        if self.ratio == 1 {
            self.process_internal(rng, memory, drive, freeze, parameters, feedback, mono, input, out);
            return;
        }
        let size = input.len();
        let ratio = self.ratio;
        let d_size = size / ratio;
        let mut d_freeze = [false; MAX_BLOCK_SIZE];
        for (i, f) in d_freeze[..d_size].iter_mut().enumerate() {
            *f = freeze[i * ratio..(i + 1) * ratio].iter().any(|&x| x);
        }
        let mut d_triggers = [TriggerOutput::default(); MAX_BLOCK_SIZE];
        let mut d_taps = [DelayOutput::default(); MAX_BLOCK_SIZE];
        let d_drive = match drive {
            Drive::Grains(triggers) => {
                let inv_ratio = 1.0 / F::from_usize(ratio);
                for (i, o) in d_triggers[..d_size].iter_mut().enumerate() {
                    let group = &triggers[i * ratio..(i + 1) * ratio];
                    o.regularity = group[0].regularity;
                    let mut has_trigger = false;
                    let mut fractional_sample = F::ZERO;
                    let mut gate = false;
                    for (j, t) in group.iter().enumerate() {
                        if t.trigger && !has_trigger {
                            has_trigger = true;
                            fractional_sample = (t.fractional_sample + F::from_usize(j)) * inv_ratio;
                        }
                        gate |= t.gate;
                    }
                    o.trigger = has_trigger;
                    o.gate = gate;
                    o.fractional_sample = fractional_sample;
                }
                Drive::Grains(&d_triggers[..d_size])
            }
            Drive::Taps(taps) => {
                let r = i32::try_from(ratio).unwrap_or(1);
                for (i, o) in d_taps[..d_size].iter_mut().enumerate() {
                    for (dst, src) in o.tap.iter_mut().zip(taps[i * ratio].tap.iter()) {
                        dst.delay = src.delay / r;
                        dst.gain = src.gain;
                    }
                }
                Drive::Taps(&d_taps[..d_size])
            }
        };
        let mut d_in = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
        let mut d_out = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
        let mut upsampled = [F::ZERO; MAX_BLOCK_SIZE];
        let mut downsampled = [F::ZERO; MAX_BLOCK_SIZE];
        for channel in 0..2 {
            for (u, f) in upsampled.iter_mut().zip(input) {
                *u = f[channel];
            }
            self.src_down[channel].process(&upsampled[..size], &mut downsampled[..d_size]);
            for (d, &s) in d_in.iter_mut().zip(&downsampled[..d_size]) {
                d[channel] = s;
            }
        }
        self.process_internal(rng, memory, d_drive, &d_freeze[..d_size], parameters, feedback, mono, &d_in[..d_size], &mut d_out[..d_size]);
        for channel in 0..2 {
            for (d, f) in downsampled.iter_mut().zip(&d_out[..d_size]) {
                *d = f[channel];
            }
            self.src_up[channel].process(&downsampled[..d_size], &mut upsampled[..size]);
            for (o, &s) in out.iter_mut().zip(&upsampled[..size]) {
                o[channel] = s;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn process_internal(
        &mut self,
        rng: &mut Random,
        memory: &mut DelayMemory,
        drive: Drive<'_>,
        freeze: &[bool],
        parameters: &Parameters,
        feedback: F,
        mono: bool,
        input: &[Frame],
        out: &mut [Frame],
    ) {
        let size = input.len();
        let speed = semitones_to_ratio(parameters.pitch.value_with(rng, Some(&mut self.pitch_modulation), size, F(24.0), false));
        let size_cv = if parameters.size.has_cv { F(parameters.size.cv) } else { F::ZERO };
        let bandwidth = constrain(0.5 * (1.0 + F(parameters.size.attenuverter)) + size_cv, F::ZERO, F(1.0));
        let mut bandwidth_modulation = ParameterInterpolator::new(self.bandwidth, bandwidth, size);
        let mut feedback_modulation = ParameterInterpolator::new(self.feedback, feedback, size);
        let mut speed_modulation = ParameterInterpolator::new(self.speed, speed, size);
        let max_feedback_gain = if matches!(drive, Drive::Taps(_)) { F(1.5) } else { F(3.0) };
        let lp_coefficient = [F(0.75), F(0.9), F(0.999)][self.resolution.index()];
        let soft_limit_gain = [F(1.75), F(1.3), F::ZERO][self.resolution.index()];
        for n in 0..size {
            let feedback = feedback_modulation.next();
            let input_frame = input[n];
            let mut s = input_frame;
            if soft_limit_gain != 0.0 {
                let gain = max_feedback_gain / soft_limit_gain;
                for c in 0..2 {
                    s[c] += self.previous_sample[c] * feedback * gain;
                }
            } else {
                for c in 0..2 {
                    s[c] += self.previous_sample[c] * feedback * feedback * max_feedback_gain;
                }
            }
            self.input_limiter.process_frame(&mut s);
            if soft_limit_gain != 0.0 {
                for c in 0..2 {
                    s[c] = soft_clip(s[c] * soft_limit_gain);
                    s[c] = input_frame[c] + feedback * (s[c] - input_frame[c]);
                }
            }
            let mut o = [F::ZERO; 2];
            let line = if mono { &mut self.mono_line } else { &mut self.stereo_line };
            line.write(memory, s, freeze[n]);
            let line = *line;
            match drive {
                Drive::Grains(triggers) => self.granular_player.render(rng, &line, !mono, memory, &triggers[n], parameters, &mut o),
                Drive::Taps(taps) => {
                    let speed = speed_modulation.next();
                    let bandwidth = bandwidth_modulation.next();
                    self.multitap_delay.render(&line, memory, &taps[n], speed, bandwidth, &mut o);
                }
            }
            out[n] = o;
            for c in 0..2 {
                let blocked = self.dc_blocker[c].process(o[c], FilterMode::HighPass);
                one_pole(&mut self.previous_sample[c], blocked, lp_coefficient);
            }
        }
        self.bandwidth = bandwidth_modulation.value();
        self.feedback = feedback_modulation.value();
        self.speed = speed_modulation.value();
    }
}
