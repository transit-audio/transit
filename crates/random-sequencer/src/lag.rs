//! The steps control's smoothing below its center: `marbles/random/lag_processor.{h,cc}`. A glide
//! whose low-pass frequency follows the clock, crossfaded with a shaped interpolation toward the
//! new value.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::LUT_RAISED_COSINE;
use crate::stmlib::{constrain, crossfade, interpolate_clamped, one_pole, semitones_to_ratio};

/// `LagProcessor`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LagProcessor {
    ramp_start: F,
    ramp_value: F,
    lp_state: F,
    previous_phase: F,
}

impl LagProcessor {
    /// `Init()`.
    pub fn init(&mut self) {
        *self = Self::default();
    }

    /// `ResetRamp()`.
    pub fn reset_ramp(&mut self) {
        self.ramp_start = self.ramp_value;
    }

    /// `Process(value, smoothness, phase)`.
    pub fn process(&mut self, value: F, smoothness: F, phase: F) -> F {
        let mut frequency = phase - self.previous_phase;
        if frequency < 0.0 {
            frequency += 1.0;
        }
        self.previous_phase = phase;
        // The glide's frequency: from half the clock (a roughly linear glide) up seven octaves;
        // near zero smoothness the response turns immediate.
        frequency *= 0.25;
        frequency *= semitones_to_ratio(84.0 * (1.0 - smoothness));
        if frequency >= 1.0 {
            frequency = F::ONE;
        }
        if smoothness <= 0.05 {
            frequency += 20.0 * (0.05 - smoothness) * (1.0 - frequency);
        }
        one_pole(&mut self.lp_state, value, frequency);
        // A crossfade between a shaped interpolation and the glide.
        let interp_amount = constrain((smoothness - 0.6) * 5.0, 0.0, 1.0);
        let interp_linearity = constrain((1.0 - smoothness) * 5.0, 0.0, 1.0);
        // The oracle's `patches/all/0011`: a phase of exactly 1.0 reads past the table upstream.
        let warped_phase = interpolate_clamped(&LUT_RAISED_COSINE, 0, phase, 256.0);
        let interp_phase = crossfade(warped_phase, phase, interp_linearity);
        let interp = crossfade(self.ramp_start, value, interp_phase);
        self.ramp_value = interp;
        crossfade(self.lp_state, interp, interp_amount)
    }
}
