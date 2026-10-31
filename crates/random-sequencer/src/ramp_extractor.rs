//! Recovers a ramp from a clock input by predicting when the next edge comes:
//! `marbles/ramp/ramp_extractor.{h,cc}`. Several predictors (moving averages, a trigram model of
//! quantized intervals, periodic patterns of 1 to 10 pulses) run at once and the most accurate one
//! wins; a steady pulse width corrects the phase at each falling edge.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::gate;
use crate::ramp::{MAX_RAMP_VALUE, Ratio};
use crate::stmlib::{one_pole, slope};

/// `kLogOneFourth`, as written (the fourth root of 2).
#[allow(clippy::excessive_precision)]
const LOG_ONE_FOURTH: f32 = 1.189_207_115;
const PULSE_WIDTH_TOLERANCE: f32 = 0.05;
const NUM_CONSISTENT_PULSES: i32 = 3;
const HISTORY_SIZE: usize = 16;
const HASH_TABLE_SIZE: usize = 256;
/// `PREDICTOR_LAST`: two moving averages, the hash, and periods 1 to 10.
const PREDICTORS: usize = 13;
const PREDICTOR_FAST_MOVING_AVERAGE: usize = 1;
const PREDICTOR_HASH: usize = 2;
const PREDICTOR_PERIOD_1: usize = 3;

fn within_tolerance(x: F, y: F, error: f32) -> bool {
    x >= y * (1.0 - error) && x <= y * (1.0 + error)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Pulse {
    on_duration: u32,
    total_duration: u32,
    /// `4 x log2(total_duration)`.
    bucket: u32,
    pulse_width: F,
}

/// `RampExtractor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampExtractor {
    current_pulse: usize,
    history: [Pulse; HISTORY_SIZE],
    next_bucket: F,
    prediction_hash_table: [F; HASH_TABLE_SIZE],
    predicted_period: [F; PREDICTORS],
    prediction_accuracy: [F; PREDICTORS],
    average_pulse_width: F,
    train_phase: F,
    frequency: F,
    max_train_phase: F,
    reset_frequency: F,
    target_frequency: F,
    lp_coefficient: F,
    f_ratio: F,
    next_f_ratio: F,
    next_max_train_phase: F,
    reset_counter: i32,
    reset_interval: u32,
    audio_rate: bool,
    num_consistent_audio_rate_pulses: i32,
    max_frequency: F,
    audio_rate_period: F,
    audio_rate_period_hysteresis: F,
    reset_at_next_pulse: bool,
}

impl Default for RampExtractor {
    fn default() -> Self {
        Self {
            current_pulse: 0,
            history: [Pulse::default(); HISTORY_SIZE],
            next_bucket: F::ZERO,
            prediction_hash_table: [F::ZERO; HASH_TABLE_SIZE],
            predicted_period: [F::ZERO; PREDICTORS],
            prediction_accuracy: [F::ZERO; PREDICTORS],
            average_pulse_width: F::ZERO,
            train_phase: F::ZERO,
            frequency: F::ZERO,
            max_train_phase: F::ZERO,
            reset_frequency: F::ZERO,
            target_frequency: F::ZERO,
            lp_coefficient: F::ZERO,
            f_ratio: F::ZERO,
            next_f_ratio: F::ZERO,
            next_max_train_phase: F::ZERO,
            reset_counter: 0,
            reset_interval: 0,
            audio_rate: false,
            num_consistent_audio_rate_pulses: 0,
            max_frequency: F::ZERO,
            audio_rate_period: F::ZERO,
            audio_rate_period_hysteresis: F::ZERO,
            reset_at_next_pulse: false,
        }
    }
}

impl RampExtractor {
    /// `Init(max_frequency)`.
    pub fn init(&mut self, max_frequency: F) {
        self.max_frequency = max_frequency;
        self.audio_rate_period = 1.0 / (F(100.0) / 32_000.0);
        self.audio_rate_period_hysteresis = self.audio_rate_period;
        self.reset();
    }

    /// `Reset()`.
    pub fn reset(&mut self) {
        self.audio_rate = false;
        self.num_consistent_audio_rate_pulses = 0;
        self.train_phase = F::ZERO;
        self.frequency = F(0.0001);
        self.target_frequency = self.frequency;
        self.lp_coefficient = F(0.5);
        self.max_train_phase = F(0.999);
        self.next_max_train_phase = self.max_train_phase;
        self.f_ratio = F::ONE;
        self.next_f_ratio = self.f_ratio;
        self.reset_counter = 1;
        self.reset_frequency = F::ZERO;
        self.reset_interval = 32_000 * 3;
        self.reset_at_next_pulse = false;
        let p = Pulse { bucket: 1, on_duration: 2000, total_duration: 4000, pulse_width: F(0.5) };
        self.history = [p; HISTORY_SIZE];
        self.current_pulse = 0;
        self.next_bucket = F(48.0);
        self.average_pulse_width = F::ZERO;
        self.predicted_period = [F(4000.0); PREDICTORS];
        self.prediction_accuracy = [F::ZERO; PREDICTORS];
        self.prediction_hash_table = [F::ZERO; HASH_TABLE_SIZE];
    }

    fn pulse(&self, i: usize) -> Pulse {
        self.history[i % HISTORY_SIZE]
    }

    /// `ComputeAveragePulseWidth(tolerance)`.
    fn average_pulse_width(&self, tolerance: f32) -> F {
        let mut sum = F::ZERO;
        let current = self.pulse(self.current_pulse).pulse_width;
        for p in &self.history {
            if !within_tolerance(p.pulse_width, current, tolerance) {
                return F::ZERO;
            }
            sum += p.pulse_width;
        }
        sum / F::from_usize(HISTORY_SIZE)
    }

    /// `PredictNextPeriod()`: (period, accuracy).
    fn predict_next_period(&mut self) -> (F, F) {
        let last_period = F::from_u32(self.pulse(self.current_pulse).total_duration);
        let mut best = PREDICTOR_FAST_MOVING_AVERAGE;
        for i in PREDICTOR_FAST_MOVING_AVERAGE..PREDICTORS {
            let error = (self.predicted_period[i] - last_period) / (last_period + 0.01);
            // 10% error is half as good as 0% error.
            let accuracy = 1.0 / (1.0 + 100.0 * error * error);
            // Slowly trust good predictors, quickly demote those that err.
            slope(&mut self.prediction_accuracy[i], accuracy, 0.1, 0.5);
            match i {
                PREDICTOR_FAST_MOVING_AVERAGE => one_pole(&mut self.predicted_period[i], last_period, F(0.5)),
                PREDICTOR_HASH => {
                    let t_2 = (self.current_pulse + HISTORY_SIZE - 2) % HISTORY_SIZE;
                    let t_1 = (self.current_pulse + HISTORY_SIZE - 1) % HISTORY_SIZE;
                    let t_0 = self.current_pulse;
                    let hash = self.history[t_1].bucket as usize + 17 * self.history[t_2].bucket as usize;
                    one_pole(&mut self.prediction_hash_table[hash % HASH_TABLE_SIZE], last_period, F(0.5));
                    let hash = self.history[t_0].bucket as usize + 17 * self.history[t_1].bucket as usize;
                    self.predicted_period[i] = self.prediction_hash_table[hash % HASH_TABLE_SIZE];
                    if self.predicted_period[i] == 0.0 {
                        self.predicted_period[i] = last_period;
                    }
                }
                _ => {
                    // Periodicity detector.
                    let candidate_period = i - PREDICTOR_PERIOD_1 + 1;
                    let t = self.current_pulse + 1 + HISTORY_SIZE - candidate_period;
                    self.predicted_period[i] = F::from_u32(self.pulse(t).total_duration);
                }
            }
            if self.prediction_accuracy[i] >= self.prediction_accuracy[best] {
                best = i;
            }
        }
        (self.predicted_period[best], self.prediction_accuracy[best])
    }

    /// `Process(ratio, always_ramp_to_maximum, &reset, gate_flags, ramp, size)`.
    pub fn process(&mut self, ratio: Ratio, always_ramp_to_maximum: bool, reset: &mut bool, gate_flags: &[u8], ramp: &mut [F]) {
        if *reset {
            self.reset_at_next_pulse = true;
        }
        for (flags, out) in gate_flags.iter().copied().zip(ramp.iter_mut()) {
            // The previous pulse is complete.
            if flags & gate::RISING != 0 {
                let cp = self.current_pulse;
                let p = self.history[cp];
                let record_pulse = p.total_duration < self.reset_interval;
                if !record_pulse {
                    // A long pause: the clock was probably stopped and restarted.
                    self.reset_frequency = F::ZERO;
                    self.train_phase = F::ZERO;
                    self.reset_counter = ratio.q;
                    self.reset_interval = p.total_duration.wrapping_mul(4);
                    // Flag a reset so everything downstream resets.
                    *reset = true;
                } else {
                    if self.reset_at_next_pulse {
                        self.reset_counter = 1;
                        self.reset_at_next_pulse = false;
                    }
                    let period = F::from_u32(p.total_duration);
                    if period <= self.audio_rate_period_hysteresis {
                        self.num_consistent_audio_rate_pulses = (self.num_consistent_audio_rate_pulses + 1).min(NUM_CONSISTENT_PULSES);
                        self.audio_rate_period_hysteresis = self.audio_rate_period * 1.1;
                    } else {
                        self.num_consistent_audio_rate_pulses = 0;
                        self.audio_rate_period_hysteresis = self.audio_rate_period;
                    }
                    // Audio rate only after a run of uninterrupted audio-rate pulses.
                    self.audio_rate = self.num_consistent_audio_rate_pulses == NUM_CONSISTENT_PULSES;
                    if self.audio_rate {
                        self.average_pulse_width = F::ZERO;
                        let mut no_glide = self.f_ratio != ratio.to_float();
                        self.f_ratio = ratio.to_float();
                        let frequency = 1.0 / period;
                        self.target_frequency = (self.f_ratio * frequency).min(self.max_frequency);
                        let up_tolerance = (1.02 + 2.0 * frequency) * self.frequency;
                        let down_tolerance = (0.98 - 2.0 * frequency) * self.frequency;
                        no_glide |= self.target_frequency > up_tolerance || self.target_frequency < down_tolerance;
                        self.lp_coefficient = if no_glide { F::ONE } else { (period * 0.00001).min(F(0.1)) };
                    } else {
                        // The previous pulse's width, and whether it held over the past pulses.
                        self.history[cp].pulse_width = F::from_u32(p.on_duration) / period;
                        self.average_pulse_width = self.average_pulse_width(PULSE_WIDTH_TOLERANCE);
                        if p.on_duration < 32 {
                            self.average_pulse_width = F::ZERO;
                        }
                        // Predict the next interval; a reliable predictor, or a steady width,
                        // tells when the next pulse comes.
                        let (prediction, _) = self.predict_next_period();
                        self.frequency = 1.0 / prediction;
                        self.reset_counter -= 1;
                        if self.reset_counter == 0 {
                            self.next_f_ratio = ratio.to_float() * MAX_RAMP_VALUE;
                            self.next_max_train_phase = F::from_i32(ratio.q);
                            if always_ramp_to_maximum && self.train_phase < self.max_train_phase {
                                self.reset_frequency = (0.01 + self.max_train_phase - self.train_phase) * 0.0625;
                            } else {
                                self.reset_frequency = F::ZERO;
                                self.train_phase = F::ZERO;
                                self.f_ratio = self.next_f_ratio;
                                self.max_train_phase = self.next_max_train_phase;
                            }
                            self.reset_counter = ratio.q;
                        } else {
                            let expected = self.max_train_phase - F::from_i32(self.reset_counter);
                            let warp = expected - self.train_phase + 1.0;
                            self.frequency *= warp.max(F(0.01));
                        }
                    }
                    self.reset_interval = (4.0 / self.target_frequency).max(F(32_000.0 * 3.0)).trunc_u32();
                    self.current_pulse = (self.current_pulse + 1) % HISTORY_SIZE;
                }
                let cp = self.current_pulse;
                self.history[cp].on_duration = 0;
                self.history[cp].total_duration = 0;
                self.history[cp].bucket = 0;
                self.next_bucket = F(48.0);
            }

            // The history: total and on durations.
            let cp = self.current_pulse;
            self.history[cp].total_duration = self.history[cp].total_duration.wrapping_add(1);
            if flags & gate::HIGH != 0 {
                self.history[cp].on_duration = self.history[cp].on_duration.wrapping_add(1);
            }
            if F::from_u32(self.history[cp].total_duration) >= self.next_bucket {
                self.history[cp].bucket = self.history[cp].bucket.wrapping_add(1);
                self.next_bucket *= LOG_ONE_FOURTH;
            }

            // A steady pulse width and a falling edge: the period from the on time and the width.
            if flags & gate::FALLING != 0 && self.average_pulse_width > 0.0 {
                let t_on = F::from_u32(self.history[cp].on_duration);
                let next = self.max_train_phase - F::from_i32(self.reset_counter) + 1.0;
                let pw = self.average_pulse_width;
                self.frequency = (next - self.train_phase).max(F::ZERO) * pw / ((1.0 - pw) * t_on);
            }

            if self.audio_rate {
                one_pole(&mut self.frequency, self.target_frequency, self.lp_coefficient);
                self.train_phase += self.frequency;
                if self.train_phase >= 1.0 {
                    self.train_phase -= 1.0;
                }
                *out = self.train_phase;
            } else {
                if self.reset_frequency != 0.0 {
                    self.train_phase += self.reset_frequency;
                    if self.train_phase >= self.max_train_phase {
                        self.train_phase = F::ZERO;
                        self.reset_frequency = F::ZERO;
                        self.f_ratio = self.next_f_ratio;
                        self.max_train_phase = self.next_max_train_phase;
                    }
                } else {
                    self.train_phase += self.frequency;
                    if self.train_phase >= self.max_train_phase {
                        if self.frequency == self.max_frequency {
                            self.train_phase -= self.max_train_phase;
                        } else {
                            self.train_phase = self.max_train_phase;
                        }
                    }
                }
                let mut output_phase = self.train_phase * self.f_ratio;
                output_phase -= F::from_i32(output_phase.trunc_i32());
                *out = output_phase;
            }
        }
    }
}
