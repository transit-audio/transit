//! Timing as ramps from 0 to [`MAX_RAMP_VALUE`]: `marbles/ramp/ramp.h`, `ramp_divider.h`
//! (`Ratio`, `RampDivider`) and `slave_ramp.h` (`SlaveRamp`).

#![forbid(unsafe_code)]

use crate::float::F;

/// `kMaxRampValue`.
pub const MAX_RAMP_VALUE: f32 = 0.9999;

/// `Ratio`: `p / q`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Ratio {
    pub p: i32,
    pub q: i32,
}

impl Ratio {
    /// `to_float()`.
    #[must_use]
    pub fn to_float(self) -> F {
        F::from_i32(self.p) / F::from_i32(self.q)
    }

    /// `Simplify<2>()`.
    pub fn simplify2(&mut self) {
        while self.p % 2 == 0 && self.q % 2 == 0 && self.q != 0 {
            self.p /= 2;
            self.q /= 2;
        }
    }
}

/// `RampDivider`: a ramp at `p / q` times its input's frequency, phase-locked to it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RampDivider {
    phase: F,
    train_phase: F,
    max_train_phase: F,
    f_ratio: F,
    reset_counter: i32,
    /// Never set by `Init` upstream (an upstream finding): zero, as the module's global.
    reset_at_next_pulse: bool,
}

impl RampDivider {
    /// `Init()`.
    pub fn init(&mut self) {
        self.phase = F::ZERO;
        self.train_phase = F::ZERO;
        self.max_train_phase = F::ONE;
        self.f_ratio = F(0.99999);
        self.reset_counter = 1;
    }

    /// `Reset()`.
    pub fn reset(&mut self) {
        self.reset_at_next_pulse = true;
    }

    /// `Process(ratio, in, out, size)`.
    pub fn process(&mut self, ratio: Ratio, input: &[F], out: &mut [F]) {
        for (new_phase, o) in input.iter().copied().zip(out.iter_mut()) {
            let mut frequency = new_phase - self.phase;
            if frequency < 0.0 {
                if self.reset_at_next_pulse {
                    self.reset_at_next_pulse = false;
                    self.reset_counter = 1;
                }
                frequency += 1.0;
                self.reset_counter -= 1;
                if self.reset_counter == 0 {
                    self.train_phase = new_phase;
                    self.reset_counter = ratio.q;
                    self.f_ratio = ratio.to_float() * MAX_RAMP_VALUE;
                    frequency = F::ZERO;
                    self.max_train_phase = F::from_i32(ratio.q);
                }
            }
            self.train_phase += frequency;
            if self.train_phase >= self.max_train_phase {
                self.train_phase = self.max_train_phase;
            }
            let mut output_phase = self.train_phase * self.f_ratio;
            output_phase -= F::from_i32(output_phase.trunc_i32());
            *o = output_phase;
            self.phase = new_phase;
        }
    }
}

/// `SlaveRamp`: a ramp that follows the master ramp, divided or multiplied (the divider and
/// clusters models), or with an adaptive slope that reaches 1.0 at the next tick only when a pulse
/// comes then (the Bernoulli models).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlaveRamp {
    pub(crate) phase: F,
    pub(crate) max_phase: F,
    pub(crate) ratio: F,
    pub(crate) pulse_width: F,
    target: F,
    pulse_length: i32,
    pub(crate) bernoulli: bool,
    pub(crate) must_complete: bool,
}

impl SlaveRamp {
    /// `Init()`.
    pub fn init(&mut self) {
        self.phase = F::ZERO;
        self.max_phase = F(MAX_RAMP_VALUE);
        self.ratio = F::ONE;
        self.pulse_width = F::ZERO;
        self.target = F::ONE;
        self.pulse_length = 0;
        self.bernoulli = false;
        self.must_complete = false;
    }

    /// `Reset()`.
    pub fn reset(&mut self) {
        self.init();
        self.phase = F::ONE;
    }

    /// `Init(pattern_length, ratio, pulse_width)`: a rate multiplied or divided from the master's.
    pub fn init_pattern(&mut self, pattern_length: i32, ratio: Ratio, pulse_width: F) {
        self.bernoulli = false;
        self.phase = F::ZERO;
        self.max_phase = F::from_i32(pattern_length) * MAX_RAMP_VALUE;
        self.ratio = ratio.to_float();
        self.pulse_width = pulse_width;
        self.target = F::ONE;
        self.pulse_length = 0;
    }

    /// `Init(must_complete, pulse_width, expected_value)`: the adaptive slope. True when the ramp
    /// restarted (the previous tick asked for a pulse now).
    pub fn init_bernoulli(&mut self, must_complete: bool, pulse_width: F, expected_value: f32) -> bool {
        self.bernoulli = true;
        let restarted = self.must_complete;
        if self.must_complete {
            self.phase = F::ZERO;
            self.pulse_width = pulse_width;
            self.ratio = F::ONE;
            self.pulse_length = 0;
        }
        self.ratio = if must_complete { 1.0 - self.phase } else { (1.0 - self.phase) * expected_value };
        self.must_complete = must_complete;
        restarted
    }

    /// `Process(frequency, &phase, &gate)`.
    pub fn process(&mut self, frequency: F) -> (F, bool) {
        let mut output_phase;
        if self.bernoulli {
            self.phase += frequency * self.ratio;
            output_phase = self.phase;
            if output_phase >= 1.0 {
                output_phase = F::ONE;
            }
        } else {
            self.phase += frequency;
            if self.phase >= self.max_phase {
                self.phase = self.max_phase;
            }
            output_phase = self.phase * self.ratio;
            if output_phase > self.target {
                self.pulse_length = 0;
                self.target += 1.0;
            }
            output_phase -= F::from_i32(output_phase.trunc_i32());
        }
        let gate = if self.pulse_width == 0.0 { self.pulse_length < 32 && output_phase <= 0.5 } else { output_phase < self.pulse_width };
        self.pulse_length = self.pulse_length.wrapping_add(1);
        (output_phase, gate)
    }
}
