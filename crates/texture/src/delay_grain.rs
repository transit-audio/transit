//! `beads/dsp/delay_grain.h`, `beads/dsp/delay_granular_player.h` and `beads/dsp/tap.h`,
//! `beads/dsp/multitap_delay.h`: the two ways the processor reads its recording, as grains or as
//! delay taps.

#![forbid(unsafe_code)]

use crate::delay_controller::{DelayOutput, TAPS};
use crate::delay_line::{DelayLine, DelayMemory, Frame};
use crate::float::F;
use crate::grain::{GrainCore, Scheduler, Voice};
use crate::randomized::Parameters;
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, semitones_to_ratio, smooth_step};
use crate::trigger::TriggerOutput;

/// The zone near the recording head where a grain fades (`head_crossing_safety_zone`), samples.
const SAFETY_ZONE: i32 = 256;

/// `DelayGrain`: a grain reading the recording at a speed, 22.10 fixed point, stopped where it
/// would cross the recording head.
#[derive(Clone, Copy, Debug, Default)]
pub struct DelayGrain {
    core: GrainCore,
    gain: [[F; 2]; 2],
    delay: i32,
    speed: i32,
    initial_delay: i32,
}

impl Voice for DelayGrain {
    fn init(&mut self, sample_rate: F) {
        self.initial_delay = 0;
        self.delay = 0;
        self.speed = 1024;
        self.gain = [[F(0.5); 2]; 2];
        self.core.envelope.init(sample_rate);
        self.core.stealing.reset_and_pause();
        self.core.remaining_samples = 0;
    }

    fn core(&self) -> &GrainCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut GrainCore {
        &mut self.core
    }
}

impl DelayGrain {
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, line: &DelayLine, stereo: bool, mut delay: F, speed: F, mut duration: F, shape: F, gain: F, pan: F, fractional_sample: F) {
        let max_delay = F::from_i32(line.max_delay());
        let consumption_speed = speed - if line.frozen() { F::ZERO } else { F(1.0) };
        delay -= fractional_sample * consumption_speed;
        if consumption_speed != 0.0 {
            duration = duration.min(max_delay / consumption_speed.abs());
        }
        if consumption_speed > 0.0 {
            delay = delay.max(duration * consumption_speed);
        } else if consumption_speed < 0.0 {
            delay = delay.min(max_delay + duration * consumption_speed);
        }
        self.delay = (delay * 1024.0).trunc_i32();
        self.initial_delay = self.delay;
        self.speed = (speed * 1024.0).trunc_i32();
        // Roughly equal power, from a first-order approximation of the square root.
        if stereo {
            if pan < 0.5 {
                let pan = pan * 2.0;
                self.gain[0][0] = gain;
                self.gain[1][0] = F::ZERO;
                self.gain[1][1] = pan * (2.0 - pan) * gain;
                self.gain[0][1] = (1.0 - pan) * (1.0 + pan) * gain;
            } else {
                let pan = (1.0 - pan) * 2.0;
                self.gain[1][1] = gain;
                self.gain[0][1] = F::ZERO;
                self.gain[0][0] = pan * (2.0 - pan) * gain;
                self.gain[1][0] = (1.0 - pan) * (1.0 + pan) * gain;
            }
        } else {
            self.gain[1][0] = pan * (2.0 - pan) * gain;
            self.gain[0][0] = (1.0 - pan) * (1.0 + pan) * gain;
            self.gain[0][1] = F::ZERO;
            self.gain[1][1] = F::ZERO;
        }
        self.core.envelope.start(duration, shape, fractional_sample);
        self.core.remaining_samples = duration.trunc_i32();
        self.core.stealing.reset_and_pause();
    }

    /// `Render<HEAD_CROSSING_POLICY_STOP, true>`.
    #[inline(always)]
    fn render(&mut self, line: &DelayLine, memory: &DelayMemory, out: &mut Frame) {
        if self.core.done() {
            return;
        }
        self.delay = self.delay.wrapping_sub(self.speed);
        if !line.frozen() {
            self.delay = self.delay.wrapping_add(1024);
        }
        let tail = line.max_delay() << 10;
        let fade_zone = SAFETY_ZONE << 10;
        if self.delay < 0 || self.delay >= tail {
            self.delay = 0;
            self.core.envelope.stop();
        }
        let d = (if self.delay >= tail / 2 { self.delay - tail } else { self.delay }).abs();
        let fade_ramp = if d < fade_zone { F::from_i32(d) / F::from_i32(fade_zone) } else { F(1.0) };
        let sample = line.read_hermite_fixed(memory, self.delay);
        let fade = smooth_step(fade_ramp);
        let envelope = self.core.envelope.render() * self.core.stealing.render();
        let sample = [sample[0] * fade * envelope, sample[1] * fade * envelope];
        out[0] += sample[0] * self.gain[0][0] + sample[1] * self.gain[0][1];
        out[1] += sample[0] * self.gain[1][0] + sample[1] * self.gain[1][1];
        self.core.remaining_samples -= 1;
        if self.core.done() {
            self.core.remaining_samples = 0;
        }
    }
}

/// `DelayGranularPlayer`: up to 32 grains from the recording.
#[derive(Clone, Copy, Debug, Default)]
pub struct DelayGranularPlayer {
    /// The grains and their scheduling.
    pub scheduler: Scheduler<DelayGrain>,
}

impl DelayGranularPlayer {
    /// `Render(delay_line, trigger, p, out)`: one sample; a trigger starts a grain.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        rng: &mut Random,
        line: &DelayLine,
        stereo: bool,
        memory: &DelayMemory,
        trigger: &TriggerOutput,
        p: &Parameters,
        out: &mut Frame,
    ) {
        if trigger.trigger {
            let duration = p.size.value(rng);
            let reverse = duration < 0.333_333_3;
            let (amplitude, pan) = self.scheduler.amplitude_and_pan(rng, trigger, p);
            let safety = SAFETY_ZONE + 1;
            let max_delay = line.duration() - 2 * safety;
            // The grain's random draws in source order (the module's), as the oracle's build
            // sequences them (`patches/all/0003`).
            let time_value = p.time.value(rng);
            let pitch_value = p.pitch.pitch_value(rng);
            let grain_duration = self.scheduler.grain_duration(duration);
            let shape_value = p.shape.value(rng);
            let speed = (if reverse { F(-1.0) } else { F(1.0) }) * pitch_value;
            let delay = time_value * F::from_i32(max_delay) + F::from_i32(safety);
            self.scheduler.new_grain().start(line, stereo, delay, speed, grain_duration, shape_value, amplitude, pan, trigger.fractional_sample);
            self.scheduler.sort(p.monophonic);
        }
        for grain in &mut self.scheduler.grains {
            grain.render(line, memory, out);
        }
        self.scheduler.tick();
    }
}

/// `Tap<window_size>`: a delay tap; with a window, pitch-shifted by two crossfading heads.
#[derive(Clone, Copy, Debug, Default)]
struct Tap {
    phase: i32,
}

impl Tap {
    #[inline(always)]
    fn render(&mut self, window: i32, line: &DelayLine, memory: &DelayMemory, delay: i32, gain: F, speed: F, out: &mut Frame) {
        if delay == 0 {
            return;
        }
        if window == 1 {
            let s = line.read_hermite_fixed(memory, delay);
            out[0] += gain * s[0];
            out[1] += gain * s[1];
            return;
        }
        let full = 1024 * window;
        let half = full >> 1;
        self.phase += (1024.0 * (speed - 1.0)).trunc_i32();
        if self.phase >= full {
            self.phase -= full;
        } else if self.phase < 0 {
            self.phase += full;
        }
        let delay = (full + 1).max(delay + half);
        let phase = F::from_i32(self.phase) / F::from_i32(full);
        let triangle = if phase < 0.5 { 2.0 * phase } else { 2.0 - 2.0 * phase };
        let head_1 = line.read_hermite_fixed(memory, -self.phase + delay);
        let head_2 = line.read_hermite_fixed(memory, -((self.phase + half) % full) + delay);
        for c in 0..2 {
            out[c] += (head_2[c] + (head_1[c] - head_2[c]) * triangle) * gain;
        }
    }
}

/// `MultitapDelay`: eight taps, each crossfading between a plain read and a pitch-shifted one, then
/// the bandwidth filter.
#[derive(Clone, Copy, Debug, Default)]
pub struct MultitapDelay {
    tap: [Tap; TAPS],
    no_latency_tap: [Tap; TAPS],
    filter: [Svf; 2],
}

impl MultitapDelay {
    /// `Init(sample_rate)`.
    pub fn init(&mut self) {
        self.reset();
        for f in &mut self.filter {
            f.init();
        }
    }

    /// `Reset()`.
    pub fn reset(&mut self) {
        self.tap = [Tap::default(); TAPS];
        self.no_latency_tap = [Tap::default(); TAPS];
    }

    /// `Render(delay_line, taps, speed, bandwidth, out)`: one sample.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, line: &DelayLine, memory: &DelayMemory, taps: &DelayOutput, speed: F, bandwidth: F, out: &mut Frame) {
        let gain = constrain(100.0 * ((speed - 1.0).abs() - 0.01), F::ZERO, F(1.0));
        let mut mix = [F::ZERO; 2];
        for i in 0..TAPS {
            let t = taps.tap[i];
            self.tap[i].render(2048, line, memory, t.delay, t.gain * gain, speed, &mut mix);
            self.no_latency_tap[i].render(1, line, memory, t.delay, t.gain * (1.0 - gain), speed, &mut mix);
        }
        let amount = (2.0 * bandwidth - 1.0).abs();
        // Low-pass from -7 to -2 octaves, high-pass from -8 to -3.
        let octave = if bandwidth < 0.5 { -7.0 + bandwidth * 10.0 } else { -8.0 + (bandwidth - 0.5) * 10.0 };
        let f = 0.49 * semitones_to_ratio(12.0 * octave);
        let q = 1.0 + 16.0 * amount * f;
        let wet_amount_sqrt = amount * (2.0 - amount);
        let wet_amount = wet_amount_sqrt * (2.0 - wet_amount_sqrt);
        self.filter[0].set_f_q(FrequencyApproximation::Fast, f, q);
        let first = self.filter[0];
        self.filter[1].set(&first);
        let mode = if bandwidth < 0.5 { FilterMode::LowPass } else { FilterMode::HighPass };
        let wet = [self.filter[0].process(mix[0], mode), self.filter[1].process(mix[1], mode)];
        for c in 0..2 {
            out[c] += mix[c] + (wet[c] - mix[c]) * wet_amount;
        }
    }
}
