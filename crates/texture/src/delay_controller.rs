//! `beads/dsp/delay_clock.cc` and `beads/dsp/delay_controller.cc`: the delay mode's clock (from
//! SEED taps or pulses, or from DENSITY), and the map from the controls to eight delay taps, as
//! repeating echoes or, while frozen, as slices of the buffer.

#![forbid(unsafe_code)]

use crate::SeedButton;
use crate::float::F;
use crate::randomized::{Parameter, RandomWalkModulation};
use crate::sine::SineOscillator;
use crate::stmlib::hysteresis::{HysteresisFilter, HysteresisQuantizer};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{GATE_FLAG_RISING, constrain, integral_fractional, interpolate, one_pole, semitones_to_ratio};

/// The shortest delay, samples (`kMinDelay`): the multitap's latency and the Hermite reads.
const MIN_DELAY: i32 = 64 + 4;

/// The heads: two clock intervals (the current one and the one fading in) times two ratios.
pub const HEADS: usize = 4;
/// The taps: a head and its slave (or two multiples) per head.
pub const TAPS: usize = 8;

/// One tap (`DelayTap`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DelayTap {
    /// The delay, samples in 22.10 fixed point; 0 is silent.
    pub delay: i32,
    /// The gain.
    pub gain: F,
}

/// One sample of the tap map (`DelayController::Output`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DelayOutput {
    /// The taps.
    pub tap: [DelayTap; TAPS],
    /// The gate output.
    pub gate: bool,
}

/// `DelayClock`: the interval between taps, crossfading from the old one to the new one.
#[derive(Clone, Copy, Debug, Default)]
pub struct DelayClock {
    interval: [F; 2],
    average_interval: F,
    fade: F,
    fade_increment: F,
    max_fade_increment: F,
    min_fade_increment: F,
    acceptable_jitter: F,
    jitter_sensitivity: F,
    schedule_change: bool,
    next_interval: i32,
    max_interval: i32,
    rotate: i32,
}

impl DelayClock {
    /// `Init(sample_rate)`.
    pub fn init(&mut self, sample_rate: F) {
        self.average_interval = sample_rate * 0.25;
        self.interval = [self.average_interval; 2];
        self.fade = F(1.0);
        self.min_fade_increment = 1.0 / (sample_rate * 0.25);
        self.max_fade_increment = 1.0 / (sample_rate * 0.02);
        self.fade_increment = self.max_fade_increment;
        self.acceptable_jitter = sample_rate * 0.001;
        self.jitter_sensitivity = F(1.0);
        self.schedule_change = false;
        self.next_interval = 0;
        self.max_interval = (sample_rate * 4.0).trunc_i32();
        self.rotate = 0;
    }

    /// `Process(tick, tap)`: one sample of the clock input and the SEED button's tap.
    pub fn process(&mut self, tick: bool, tap: bool) {
        self.next_interval += 1;
        self.fade += self.fade_increment;
        if self.fade >= 1.0 {
            self.fade = F(1.0);
        }
        if tick || tap {
            if self.next_interval < self.max_interval {
                let interval = F::from_i32(self.next_interval);
                let big_change = (interval - self.interval[1]).abs() > self.acceptable_jitter;
                let small_change = (self.average_interval - self.interval[1]).abs() > self.jitter_sensitivity * self.acceptable_jitter;
                if big_change {
                    self.jitter_sensitivity = F(1.0);
                    self.average_interval = interval;
                } else {
                    one_pole(&mut self.average_interval, interval, F(0.1));
                    self.jitter_sensitivity = F(0.05).max(self.jitter_sensitivity * 0.95);
                }
                self.schedule_change = self.schedule_change || tap || big_change || small_change;
            }
            self.next_interval = 0;
        }
        let can_change = tap || self.fade >= 1.0;
        if self.schedule_change && can_change {
            self.rotate = self.rotate.wrapping_add(1);
            self.interval[0] = self.interval[1];
            self.interval[1] = self.average_interval;
            self.fade = F::ZERO;
            self.fade_increment = constrain(2.0 / self.average_interval, self.min_fade_increment, self.max_fade_increment);
            self.schedule_change = false;
        }
    }

    /// `Update(interval, lp_coefficient)`: the interval from DENSITY, smoothed.
    pub fn update(&mut self, interval: F, lp_coefficient: F) {
        one_pole(&mut self.interval[0], interval, lp_coefficient);
        self.interval[1] = self.interval[0];
        self.fade = F(1.0);
        self.rotate = 0;
    }

    fn slot(&self, i: usize) -> usize {
        usize::try_from((i32::try_from(i).unwrap_or(0) + self.rotate).rem_euclid(2)).unwrap_or(0)
    }

    /// `interval(i)`.
    #[must_use]
    pub fn interval(&self, i: usize) -> F {
        self.interval[self.slot(i)]
    }

    /// `gain(i)`.
    #[must_use]
    pub fn gain(&self, i: usize) -> F {
        if self.slot(i) == 1 { self.fade } else { 1.0 - self.fade }
    }

    /// `retrig()`: a new interval starts fading in on this sample.
    #[must_use]
    pub fn retrig(&self) -> bool {
        self.fade == 0.0 && self.next_interval == 0
    }

    /// `new_interval_index()`.
    #[must_use]
    pub fn new_interval_index(&self) -> usize {
        usize::try_from((self.rotate.wrapping_add(1)).rem_euclid(2)).unwrap_or(0)
    }
}

/// `PlayHead::Triangle(phase, pw)`.
#[inline(always)]
fn triangle(phase: F, pw: F) -> F {
    let slope_up = 1.0 / pw;
    let slope_down = 1.0 / (1.0 - pw);
    if phase < pw { phase * slope_up } else { 1.0 - (phase - pw) * slope_down }
}

/// The envelopes' pulse widths, picked by SHAPE.
const PULSE_WIDTHS: [f32; 6] = [0.0, 0.001, 0.01, 0.5, 0.75, 0.999];

/// `PlayHead`: one frozen slice played through once.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayHead {
    start: i32,
    remaining: i32,
    mid_point: i32,
    phase: F,
    phase_increment: F,
    gain: F,
}

impl PlayHead {
    /// `Init()`.
    pub fn init(&mut self) {
        *self = Self::default();
    }

    fn done(&self) -> bool {
        self.remaining == 0
    }

    fn half_way(&self) -> bool {
        self.remaining != 0 && self.remaining == self.mid_point
    }

    fn start(&mut self, start: i32, duration: i32, gain: F) {
        self.start = start;
        self.remaining = duration;
        self.mid_point = duration >> 1;
        self.phase = F::ZERO;
        self.phase_increment = 1.0 / F::from_i32(duration);
        self.gain = gain;
    }

    fn render(&mut self, tap: &mut DelayTap, shape: F, slave: bool) {
        if self.done() {
            *tap = DelayTap::default();
            return;
        }
        let envelope = if slave {
            let envelopes = [0.0, 0.0, 0.0, triangle(self.phase, F(0.5)).0, 0.0, 0.0];
            interpolate(&envelopes, shape, 5.0)
        } else if shape < 0.2 {
            let square = (100.0 * triangle(self.phase, F(0.5))).min(F(1.0));
            let tri = triangle(self.phase, F(PULSE_WIDTHS[1]));
            square + (tri - square) * shape * 5.0
        } else {
            triangle(self.phase, interpolate(&PULSE_WIDTHS, shape, 5.0))
        };
        tap.delay = (self.start + self.remaining) << 10;
        tap.gain = self.gain * envelope;
        self.remaining -= 1;
        self.phase += self.phase_increment;
    }
}

/// `TapEnvelope`: the repeating envelope of one echo tap.
#[derive(Clone, Copy, Debug, Default)]
pub struct TapEnvelope {
    phase: F,
    phase_increment: F,
}

impl TapEnvelope {
    fn init(&mut self) {
        *self = Self::default();
    }

    fn render(&mut self, shape: F) -> F {
        let envelope = if shape < 0.2 {
            let square = F(1.0);
            let tri = triangle(self.phase, F(PULSE_WIDTHS[1]));
            square + (tri - square) * shape * 5.0
        } else {
            triangle(self.phase, interpolate(&PULSE_WIDTHS, shape, 5.0))
        };
        self.phase += self.phase_increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        envelope
    }
}

const NUM_DELAY_RATIOS: usize = 20;

/// `kRatio`: powers of two, then dividers, selected by DENSITY when the delay is clocked.
const RATIO: [f32; NUM_DELAY_RATIOS + 2] = [
    1.0 / 16.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0 / 2.0,
    1.0,
    1.0,
    1.0,
    1.0,
    1.0 / 2.0,
    1.0 / 3.0,
    1.0 / 4.0,
    1.0 / 6.0,
    1.0 / 8.0,
    1.0 / 10.0,
    1.0 / 12.0,
    1.0 / 16.0,
    1.0 / 16.0,
    1.0 / 16.0,
];

/// `DelayController`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DelayController {
    max_delay: i32,
    clock: DelayClock,
    play_head: [PlayHead; HEADS],
    slave_play_head: [PlayHead; HEADS],
    numerator_quantizer: [HysteresisQuantizer; HEADS],
    tap_envelope: [TapEnvelope; TAPS],
    position_modulation: RandomWalkModulation,
    shape_modulation: RandomWalkModulation,
    tap_modulation: SineOscillator,
    density_filter: HysteresisFilter,
    fm_filter: HysteresisFilter,
    slice_index_filter: HysteresisFilter,
    multiplier_lp: F,
    shape: F,
    linear_fm: F,
    fm: F,
    density: F,
    freeze: bool,
    seed_button: SeedButton,
}

impl DelayController {
    /// `Init(sample_rate, max_delay)`.
    pub fn init(&mut self, sample_rate: F, max_delay: i32) {
        self.max_delay = max_delay;
        self.clock.init(sample_rate);
        for q in &mut self.numerator_quantizer {
            q.init(2, F(0.05), false);
        }
        for h in self.play_head.iter_mut().chain(self.slave_play_head.iter_mut()) {
            h.init();
        }
        for e in &mut self.tap_envelope {
            e.init();
        }
        self.position_modulation.init(0.1 / sample_rate);
        self.shape_modulation.init(0.1 / sample_rate);
        self.tap_modulation.init(0.17 / sample_rate);
        self.density_filter.init(F(0.0025));
        self.slice_index_filter.init(F(0.0025));
        self.fm_filter.init(F(0.0025));
        self.multiplier_lp = F::ZERO;
        self.shape = F::ZERO;
        self.linear_fm = F::ZERO;
        self.fm = F(1.0);
        self.density = F::ZERO;
        self.freeze = false;
        self.seed_button = SeedButton::Off;
    }

    /// `Render(...)` over `out.len()` samples. The delay is clocked (synchronized) by the SEED input
    /// or the SEED button's taps, unless the button is latched with nothing patched: then DENSITY
    /// sets the interval.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        rng: &mut Random,
        seed_button: SeedButton,
        external: Option<&[u8]>,
        freeze: &[bool],
        density: F,
        linear_fm: F,
        exponential_fm: F,
        position: &Parameter,
        shape: &Parameter,
        out: &mut [DelayOutput],
    ) {
        let synchronized = !(seed_button == SeedButton::Locked && external.is_none());
        let (linear_fm, exponential_fm) = if synchronized { (linear_fm + exponential_fm, F::ZERO) } else { (linear_fm, exponential_fm) };
        let size = out.len();
        one_pole(&mut self.linear_fm, linear_fm, F(0.05));
        let density = constrain(density + self.linear_fm, F::ZERO, F(1.0));
        let slice_modulation = 1.005 * position.value_with(rng, Some(&mut self.position_modulation), size, F(1.0), true);
        let shape_target = shape.value_with(rng, Some(&mut self.shape_modulation), size, F(1.0), true) * 0.995;
        let mut modulated_shape = ParameterInterpolator::new(self.shape, shape_target, size);
        let fm_target = semitones_to_ratio(-self.fm_filter.process(exponential_fm) * 60.0);
        let mut modulated_fm = ParameterInterpolator::new(self.fm, fm_target, size);
        let density_target = self.density_filter.process(density);
        let mut modulated_density = ParameterInterpolator::new(self.density, density_target, size);

        let mut tap = seed_button == SeedButton::On && self.seed_button == SeedButton::Off;
        self.seed_button = seed_button;

        for (n, o) in out.iter_mut().enumerate() {
            let mut tap_ratio = [F::ZERO; 2];
            let mut tap_gain = [F::ZERO; 2];
            let density = modulated_density.next();
            let fm = modulated_fm.next();
            if synchronized {
                let tick = external.is_some_and(|e| e[n] & GATE_FLAG_RISING != 0);
                self.clock.process(tick, tap);
                tap = false;
                let (ratio_integral, ratio_fractional) = integral_fractional(density * F::from_usize(NUM_DELAY_RATIOS));
                let mut ratio_fractional = ratio_fractional - 0.5;
                ratio_fractional *= if density < 0.5 { F(4.0) } else { F(1.5) };
                ratio_fractional += 0.5;
                let ratio_fractional = constrain(ratio_fractional, F::ZERO, F(1.0));
                let ri = usize::try_from(ratio_integral).unwrap_or(0).min(NUM_DELAY_RATIOS);
                let flip = ri & 1;
                tap_ratio[flip] = F(RATIO[ri]);
                tap_ratio[1 - flip] = F(RATIO[ri + 1]);
                tap_gain[flip] = 1.0 - ratio_fractional;
                tap_gain[1 - flip] = ratio_fractional;
                tap_ratio[0] *= fm;
                tap_ratio[1] *= fm;
            } else {
                let new_interval = F::from_i32(MIN_DELAY).max(fm * F::from_i32(self.max_delay) * semitones_to_ratio(-(density - 0.5).abs() * 216.0));
                let lp_coefficient = constrain(10.0 / new_interval, F(0.005), F(0.7));
                self.clock.update(new_interval, lp_coefficient);
                tap_ratio = [F(1.0), F(0.77)];
                let fade_to_odd = constrain((density - 0.5) * 25.0, F::ZERO, F(1.0));
                let fade_to_odd = fade_to_odd * (0.25 + 0.75 * self.tap_modulation.render());
                tap_gain = [1.0 - fade_to_odd * 0.2, fade_to_odd * 0.8];
            }
            let shape = modulated_shape.next();
            if freeze[n] {
                self.render_frozen(o, shape, slice_modulation, tap_ratio, tap_gain);
            } else {
                self.render_echoes(o, shape, slice_modulation, tap_ratio, tap_gain);
            }
            self.freeze = freeze[n];
        }
        self.shape = modulated_shape.value();
        self.fm = modulated_fm.value();
        self.density = modulated_density.value();
    }

    fn render_frozen(&mut self, o: &mut DelayOutput, shape: F, slice_modulation: F, tap_ratio: [F; 2], tap_gain: [F; 2]) {
        if !self.freeze {
            for h in self.play_head.iter_mut().chain(self.slave_play_head.iter_mut()) {
                h.init();
            }
        }
        if self.clock.retrig() {
            let start = self.clock.new_interval_index() * 2;
            for i in start..start + 2 {
                self.play_head[i].init();
                self.slave_play_head[i].init();
            }
        }
        let slice_index_smooth = self.slice_index_filter.process(slice_modulation);
        for i in 0..HEADS {
            let new_head = if self.play_head[i].done() {
                Some(false)
            } else if self.play_head[i].half_way() {
                Some(true)
            } else {
                None
            };
            let (interval, ratio) = (i / 2, i % 2);
            if let Some(slave) = new_head {
                // Two undefined cases of the firmware, defined as the oracle's build defines them
                // (`patches/all/0005`): a slice is at most the whole buffer (a negative density CV can
                // stretch the interval far past it, and the play head would start before the buffer),
                // and a slice shorter than a sample counts zero slices, the module's result for its
                // division by zero (the play head stays silent).
                let slice_duration = (self.clock.interval(interval) * tap_ratio[ratio]).trunc_i32().min(self.max_delay);
                let num_slices = if slice_duration == 0 { 0 } else { self.max_delay / slice_duration };
                let q = &mut self.numerator_quantizer[i];
                if q.num_steps() != num_slices {
                    q.init(num_slices, 0.05_f32 / 5.0 * F::from_i32(num_slices), false);
                }
                let slice_index = q.process(slice_index_smooth);
                let head = if slave { &mut self.slave_play_head[i] } else { &mut self.play_head[i] };
                head.start(slice_duration.wrapping_mul(slice_index) + MIN_DELAY, slice_duration, tap_gain[ratio] * self.clock.gain(interval));
            }
            let (a, b) = o.tap.split_at_mut(2 * i + 1);
            self.play_head[i].render(&mut a[2 * i], shape, false);
            self.slave_play_head[i].render(&mut b[0], shape, true);
        }
        o.gate = self.play_head[self.clock.new_interval_index() * 2].phase < 0.5;
    }

    fn render_echoes(&mut self, o: &mut DelayOutput, shape: F, slice_modulation: F, tap_ratio: [F; 2], tap_gain: [F; 2]) {
        if self.freeze {
            for e in &mut self.tap_envelope {
                e.init();
            }
        }
        if self.clock.retrig() {
            let start = self.clock.new_interval_index() * HEADS;
            for e in &mut self.tap_envelope[start..start + HEADS] {
                e.init();
            }
        }
        one_pole(&mut self.multiplier_lp, slice_modulation, F(0.01));
        let multiplier = constrain(self.multiplier_lp * 15.0 + 1.0, F(1.0), F(16.0));
        let (multiplier_integral, multiplier_fractional) = integral_fractional(multiplier);
        let multiplier_fractional = constrain(0.5 + 8.0 * (multiplier_fractional - 0.5), F::ZERO, F(1.0));
        let mut loudest = F::ZERO;
        let mut loudest_index = 0;
        for t in 0..TAPS {
            let interval = t / HEADS;
            let ratio = (t / 2) % 2;
            let tap = t & 1;
            let i = (2 * ratio + if multiplier_integral & 1 != 0 { tap } else { 1 - tap }) + interval * HEADS;
            let tap_i32 = i32::try_from(tap).unwrap_or(0);
            let delay_time = self.clock.interval(interval) * tap_ratio[ratio] * F::from_i32(multiplier_integral + tap_i32);
            let delay_time = constrain(delay_time, F::from_i32(MIN_DELAY), F::from_i32(self.max_delay));
            self.tap_envelope[i].phase_increment = 1.0 / delay_time;
            o.tap[i].delay = (delay_time * 1024.0).trunc_i32();
            let ratio_fade = if tap == 1 { multiplier_fractional } else { 1.0 - multiplier_fractional };
            let base_gain = tap_gain[ratio] * ratio_fade * self.clock.gain(interval);
            o.tap[i].gain = base_gain * self.tap_envelope[i].render(shape);
            if base_gain > loudest {
                loudest = base_gain;
                loudest_index = i;
            }
        }
        o.gate = self.tap_envelope[loudest_index].phase < 0.5;
    }
}
