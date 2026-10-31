//! What every grain shares (`beads/dsp/grain.h`, `grain_envelope.h`, `voice_stealing_envelope.h`)
//! and the grain scheduler both granular players build on (`beads/dsp/granular_player.h`): the
//! 32-voice pool, its priority list, the voice stealing and the grains' amplitude and pan.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::randomized::{Parameters, bounded_random_float};
use crate::stmlib::random::Random;
use crate::stmlib::{semitones_to_ratio, smooth_step};
use crate::trigger::TriggerOutput;

/// The grains a player holds (`num_grains`).
pub const GRAINS: usize = 32;

const ATTACK: usize = 0;
const SUSTAIN: usize = 1;
const DONE: usize = 3;

/// `GrainEnvelope`: attack, sustain and release, shaped by SHAPE from a square through a decay and
/// a bell to a reversed decay.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrainEnvelope {
    stage: usize,
    phase: F,
    phase_increment: [F; 4],
    smoothness: F,
    min_attack: F,
    min_release: F,
}

impl GrainEnvelope {
    /// `Init(sample_rate)`.
    pub fn init(&mut self, sample_rate: F) {
        self.stage = DONE;
        self.phase = F::ZERO;
        self.phase_increment = [F(0.5), F(0.5), F(0.5), F::ZERO];
        self.smoothness = F::ZERO;
        self.min_attack = 0.0005 * sample_rate;
        self.min_release = 0.010 * sample_rate;
    }

    /// `Start(duration, shape, fractional_sample)`.
    pub fn start(&mut self, duration: F, shape: F, fractional_sample: F) {
        self.stage = ATTACK;
        self.phase = F::ZERO;
        let sustain = (1.0 - 3.0 * shape).max(F::ZERO);
        let mut shape = (shape - 0.333_333_33) * 1.5;
        if shape < 0.0 {
            shape = F::ZERO;
        } else if shape < 0.5 {
            shape = shape * shape * shape * 4.0;
        }
        let attack = shape;
        let release = (1.0 - attack) * (1.0 - sustain);
        let attack_duration = (duration * attack).max(self.min_attack * (1.0 + 3.0 * sustain));
        let sustain_duration = (duration * sustain).max(F(2.0));
        let release_duration = (duration * release).max(self.min_release);
        self.phase_increment[ATTACK] = 1.0 / attack_duration;
        self.phase_increment[SUSTAIN] = 1.0 / sustain_duration;
        self.phase_increment[2] = 1.0 / release_duration;
        self.smoothness = if attack < 0.5 { 2.0 * attack } else { 2.0 - 2.0 * attack };
        self.phase += self.phase_increment[self.stage] * fractional_sample;
        self.wrap_phase();
    }

    /// `Stop()`.
    pub fn stop(&mut self) {
        self.stage = DONE;
    }

    #[inline(always)]
    fn wrap_phase(&mut self) {
        if self.phase >= 1.0 {
            let t = (self.phase - 1.0) / self.phase_increment[self.stage];
            self.stage += 1;
            self.phase = t * self.phase_increment[self.stage.min(DONE)];
        }
    }

    /// `Render()`.
    #[inline(always)]
    pub fn render(&mut self) -> F {
        if self.stage >= DONE {
            return F::ZERO;
        }
        let value = if self.stage == SUSTAIN {
            F(1.0)
        } else {
            let attack = self.stage == ATTACK;
            let t = if attack { self.phase } else { 1.0 - self.phase };
            let base = if attack { t } else { (t * t) * (t * t) };
            base + (smooth_step(t) - base) * self.smoothness
        };
        self.phase += self.phase_increment[self.stage];
        self.wrap_phase();
        value
    }

    /// `done()`.
    #[must_use]
    pub fn done(&self) -> bool {
        self.stage >= DONE
    }
}

/// `VoiceStealingEnvelope`: a linear fade that cuts a grain short to make room for a new one.
#[derive(Clone, Copy, Debug, Default)]
pub struct VoiceStealingEnvelope {
    value: F,
    decrement: F,
}

impl VoiceStealingEnvelope {
    /// `ResetAndPause()` (also `Init()`).
    pub fn reset_and_pause(&mut self) {
        self.value = F(1.0);
        self.decrement = F::ZERO;
    }

    /// `Start(duration)`: returns the samples left until silence.
    pub fn start(&mut self, duration: F) -> i32 {
        self.decrement = 1.0 / duration;
        (self.value * duration).trunc_i32()
    }

    /// `Render()`.
    #[inline(always)]
    pub fn render(&mut self) -> F {
        self.value -= self.decrement;
        self.value.max(F::ZERO)
    }

    /// `done()`.
    #[must_use]
    pub fn done(&self) -> bool {
        self.value <= 0.0
    }
}

/// The state every grain has (`Grain`).
#[derive(Clone, Copy, Debug, Default)]
pub struct GrainCore {
    /// `remaining_samples_`.
    pub remaining_samples: i32,
    /// `envelope_`.
    pub envelope: GrainEnvelope,
    /// `voice_stealing_envelope_`.
    pub stealing: VoiceStealingEnvelope,
}

impl GrainCore {
    /// `done()`.
    #[must_use]
    pub fn done(&self) -> bool {
        self.envelope.done() || self.stealing.done()
    }

    /// `FadeOut(duration)`.
    pub fn fade_out(&mut self, duration: i32) {
        if self.remaining_samples > duration {
            self.remaining_samples = self.stealing.start(F::from_i32(duration));
        }
    }
}

/// A grain the scheduler can hold.
pub trait Voice: Copy + Default {
    /// `Init(sample_rate)`.
    fn init(&mut self, sample_rate: F);
    /// The shared state.
    fn core(&self) -> &GrainCore;
    /// The shared state, mutably.
    fn core_mut(&mut self) -> &mut GrainCore;
}

/// `std::partial_sort(first, first + middle, last, comp)` as GCC's libstdc++ (the module's library)
/// performs it: a heap over the first `middle` elements, a sift for every later element that
/// compares below the top, then a heap sort of the first `middle`. The firmware's grain list has
/// many ties (every finished grain has 0 samples left), and which tie is stolen decides the order
/// the grains are summed in, so the port must order ties exactly as this algorithm does; the
/// oracle's build uses the same algorithm on every compiler (`patches/all/0004`).
pub fn partial_sort(list: &mut [usize], middle: usize, less: impl Fn(usize, usize) -> bool) {
    fn adjust(list: &mut [usize], mut hole: usize, len: usize, value: usize, less: &impl Fn(usize, usize) -> bool) {
        let top = hole;
        let mut child = hole;
        while len >= 1 && child < (len - 1) / 2 {
            child = 2 * (child + 1);
            if less(list[child], list[child - 1]) {
                child -= 1;
            }
            list[hole] = list[child];
            hole = child;
        }
        if len.is_multiple_of(2) && len >= 2 && child == (len - 2) / 2 {
            child = 2 * (child + 1);
            list[hole] = list[child - 1];
            hole = child - 1;
        }
        while hole > top {
            let parent = (hole - 1) / 2;
            if !less(list[parent], value) {
                break;
            }
            list[hole] = list[parent];
            hole = parent;
        }
        list[hole] = value;
    }
    if middle > 1 {
        let mut parent = (middle - 2) / 2;
        loop {
            adjust(list, parent, middle, list[parent], &less);
            if parent == 0 {
                break;
            }
            parent -= 1;
        }
    }
    for i in middle..list.len() {
        if less(list[i], list[0]) {
            let value = list[i];
            list[i] = list[0];
            adjust(list, 0, middle, value, &less);
        }
    }
    let mut end = middle;
    while end > 1 {
        end -= 1;
        let value = list[end];
        list[end] = list[0];
        adjust(list, 0, end, value, &less);
    }
}

/// `GranularPlayer<T, 32>`: the grain pool and its scheduling.
#[derive(Clone, Copy, Debug)]
pub struct Scheduler<G: Voice> {
    /// `grain_`, in summing order.
    pub grains: [G; GRAINS],
    priority: [usize; GRAINS],
    preemptive_crossfade_time: i32,
    instant_crossfade_time: i32,
    simple_scheduler_threshold: i32,
    max_grain_duration: F,
    sample_rate: F,
    /// `one_hertz_`.
    pub one_hertz: F,
    grain_interval: i32,
}

impl<G: Voice> Default for Scheduler<G> {
    fn default() -> Self {
        Self {
            grains: [G::default(); GRAINS],
            priority: std::array::from_fn(|i| i),
            preemptive_crossfade_time: 0,
            instant_crossfade_time: 0,
            simple_scheduler_threshold: 0,
            max_grain_duration: F::ZERO,
            sample_rate: F::ZERO,
            one_hertz: F::ZERO,
            grain_interval: 0,
        }
    }
}

impl<G: Voice> Scheduler<G> {
    /// `Init(sample_rate)`.
    pub fn init(&mut self, sample_rate: F) {
        self.instant_crossfade_time = (sample_rate * 0.005).trunc_i32();
        self.preemptive_crossfade_time = (sample_rate * 0.1).trunc_i32();
        self.simple_scheduler_threshold = (sample_rate * 0.025).trunc_i32();
        self.grain_interval = 1;
        self.max_grain_duration = sample_rate * 4.0;
        self.sample_rate = sample_rate;
        self.one_hertz = 1.0 / sample_rate;
        self.reset();
    }

    /// `Reset()`: every grain re-initialized, the list in index order.
    pub fn reset(&mut self) {
        for (i, grain) in self.grains.iter_mut().enumerate() {
            grain.init(self.sample_rate);
            self.priority[i] = i;
        }
    }

    /// `ComputeGrainDuration(duration)`: SIZE to samples, from 4 seconds down nine octaves.
    #[inline(always)]
    pub fn grain_duration(&self, duration: F) -> F {
        let reverse = duration < 0.333_333_3;
        let clamped = if reverse { duration * 3.0 * (2.0 - duration * 3.0) } else { 1.0 - 1.5 * (duration - 0.333_333_3) };
        let shortness = F(1.0).min(1.025 * clamped);
        semitones_to_ratio(shortness * -108.0) * self.max_grain_duration
    }

    /// `ComputeGrainAmplitudeAndPan(trigger, p)`: equal-power scaling by the active grains and a
    /// random pan whose spread grows with the overlap and the controls' randomness.
    pub fn amplitude_and_pan(&self, rng: &mut Random, trigger: &TriggerOutput, p: &Parameters) -> (F, F) {
        let active = self.grains.iter().filter(|g| !g.core().done()).count();
        let n = F::from_usize(active + 1);
        let amplitude = 1.0 / n.sqrt();
        let spread = (F::ZERO
            + p.time.randomness_amount()
            + p.size.randomness_amount()
            + p.shape.randomness_amount()
            + p.pitch.randomness_amount()
            + (1.0 - trigger.regularity))
            .min(F(1.0));
        let pan_spread = 0.5 * n * spread;
        (amplitude, bounded_random_float(rng, pan_spread))
    }

    /// `new_grain()`: the grain at the head of the list, the next one to start.
    pub fn new_grain(&mut self) -> &mut G {
        &mut self.grains[self.priority[0]]
    }

    /// `Tick()`: one sample since the last grain started.
    pub fn tick(&mut self) {
        // Wraps like the firmware's `int` counter (see `delay_controller.rs`).
        self.grain_interval = self.grain_interval.wrapping_add(1);
    }

    /// `SortGrainList(monophonic_mode)`: after a grain starts, the grains at the head of the list
    /// are faded so that the next grains can take their place without a click.
    pub fn sort(&mut self, monophonic: bool) {
        let instant = self.instant_crossfade_time;
        if monophonic {
            self.priority.rotate_left(1);
            let first = self.priority[0];
            self.grains[first].core_mut().fade_out(self.grain_interval.min(instant));
            for i in 1..GRAINS - 1 {
                let g = self.priority[i];
                self.grains[g].core_mut().fade_out(instant);
            }
        } else {
            let mut fade_time;
            if self.grain_interval < self.simple_scheduler_threshold {
                self.priority.rotate_left(1);
                fade_time = [self.grain_interval + 1, instant, instant, self.preemptive_crossfade_time];
                if self.grain_interval > instant {
                    fade_time.swap(0, 2);
                }
            } else {
                let grains = &self.grains;
                partial_sort(&mut self.priority, 4, |a, b| grains[a].core().remaining_samples < grains[b].core().remaining_samples);
                fade_time = [instant, instant, self.grain_interval, self.preemptive_crossfade_time];
                if self.grain_interval > self.preemptive_crossfade_time {
                    fade_time.swap(2, 3);
                }
            }
            for (i, &fade) in fade_time.iter().enumerate() {
                let g = self.priority[i];
                self.grains[g].core_mut().fade_out(fade);
            }
        }
        self.grain_interval = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_partial_sort_selects_the_smallest_in_order() {
        let keys = [5, 3, 9, 0, 0, 7, 1, 0, 2, 8];
        let mut list: Vec<usize> = (0..keys.len()).collect();
        partial_sort(&mut list, 4, |a, b| keys[a] < keys[b]);
        let head: Vec<i32> = list[..4].iter().map(|&i| keys[i]).collect();
        assert_eq!(head, [0, 0, 0, 1]);
        let mut rest: Vec<usize> = list.clone();
        rest.sort_unstable();
        assert_eq!(rest, (0..keys.len()).collect::<Vec<_>>(), "a permutation");
    }

    #[test]
    fn the_envelope_runs_through_its_stages() {
        let mut e = GrainEnvelope::default();
        e.init(F(48_000.0));
        e.start(F(1000.0), F(0.5), F::ZERO);
        let mut peak = F::ZERO;
        let mut n = 0;
        while !e.done() && n < 10_000 {
            peak = peak.max(e.render());
            n += 1;
        }
        assert!(e.done() && (900..1100).contains(&n), "{n}");
        assert!(peak.0 > 0.99);
    }
}
