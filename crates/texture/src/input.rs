//! `beads/gain_control.cc`: the module's input stage. The codec's raw 32-bit frames become floats
//! with an automatic gain (it learns the input's level for 5 seconds after a 1-second wait, then
//! only turns down for peaks and slowly recovers) or a manual one; the tape qualities soft-clip a
//! manually driven input; an unpatched input copies the other one.
//!
//! On the module this stage sits before every quality, so it shapes what the effect chain hears;
//! it is ported and nulled with the chain, and the device decides whether to use it (a host track
//! has its own gain staging).

#![forbid(unsafe_code)]

use crate::MAX_BLOCK_SIZE;
use crate::float::F;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::{constrain, crossfade, one_pole, slope, soft_clip};

const IGNORE_TIME: f32 = 1.0;
const ACTIVE_TIME: f32 = 5.0;
const MIN_GAIN: f32 = 1.0;
const MAX_GAIN: f32 = 40.0;

/// The soft clipping a manual gain drives into (`GainControl::SoftClippingMode`): the module
/// picks it by quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SoftClipping {
    /// Hard clipping at full scale (the digital qualities).
    #[default]
    Off,
    /// Soft clipping (dirty tape).
    On,
    /// Soft clipping as the manual gain exceeds the automatic one (clean tape).
    Dynamic,
}

impl SoftClipping {
    /// The firmware's index.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Off => 0,
            Self::On => 1,
            Self::Dynamic => 2,
        }
    }
}

/// The input stage (`beads::GainControl`).
#[derive(Clone, Copy, Debug, Default)]
pub struct InputStage {
    peak: F,
    agc_peak: F,
    stable_agc_peak: F,
    gain: F,
    soft_clipping_amount: F,
    soft_clipping: SoftClipping,
    auto_gain: bool,
    manual_gain: F,
    one_hertz: F,
    time: usize,
    ignore_time: usize,
    active_time: usize,
}

impl InputStage {
    /// A stage at `sample_rate` (the quality's codec rate), as the module leaves it at power-up:
    /// zeroed, then `Init(sample_rate, soft_clipping, true)`.
    #[must_use]
    pub fn new(sample_rate: f32, soft_clipping: SoftClipping) -> Self {
        let mut stage = Self::default();
        stage.init(sample_rate, soft_clipping, true);
        stage
    }

    /// `Init(sample_rate, soft_clipping_mode, reset)`: as the module calls it on every codec
    /// restart. Without `reset`, a learned gain survives (a completed learning phase is kept).
    pub fn init(&mut self, sample_rate: f32, soft_clipping: SoftClipping, reset: bool) {
        let sample_rate = F(sample_rate);
        if reset {
            self.peak = F(0.5);
            self.agc_peak = F(0.5);
            self.stable_agc_peak = F(0.5);
            self.gain = F(1.0);
            self.soft_clipping_amount = F::ZERO;
        }
        self.soft_clipping = soft_clipping;
        self.ignore_time = (sample_rate * IGNORE_TIME).trunc_usize();
        let no_readjust = !reset && self.time != 0 && self.time >= self.active_time;
        self.active_time = (sample_rate * (IGNORE_TIME + ACTIVE_TIME)).trunc_usize();
        self.time = if no_readjust { self.active_time } else { 0 };
        self.one_hertz = 1.0 / sample_rate;
    }

    /// Converts `input.len()` raw frames into `output` (`Process`). `active` is false while the
    /// codec restarts; `gain` 0 selects the automatic gain, 1 to 255 a manual gain of 1 to 40;
    /// `normalized` says which inputs are unpatched (an unpatched side copies the other; both
    /// unpatched give silence). Longer buffers are converted in blocks of [`MAX_BLOCK_SIZE`]; the
    /// lengths must match (checked in debug builds; a release build converts the shortest).
    pub fn process(&mut self, active: bool, gain: u8, normalized: [bool; 2], input: &[[i32; 2]], output: &mut [[f32; 2]]) {
        debug_assert!(input.len() == output.len(), "lengths differ");
        let n = input.len().min(output.len());
        for (i, o) in input[..n].chunks(MAX_BLOCK_SIZE).zip(output[..n].chunks_mut(MAX_BLOCK_SIZE)) {
            self.process_block(active, gain, normalized, i, o);
        }
    }

    fn process_block(&mut self, active: bool, gain_setting: u8, normalized: [bool; 2], input: &[[i32; 2]], output: &mut [[f32; 2]]) {
        if normalized[0] && normalized[1] {
            output.fill([0.0; 2]);
            self.peak = F::ZERO;
            self.agc_peak = F(0.5);
            self.stable_agc_peak = F(0.5);
            self.gain = F(1.0);
            self.time = 0;
            return;
        }
        let size = input.len();
        let hz = F::from_usize(size) * self.one_hertz;
        if self.time < self.ignore_time {
            self.agc_peak = F(0.5);
        } else if self.time < self.active_time {
            // Turn down within 0.1 s for a peak, up within 0.5 s for a low level.
            slope(&mut self.agc_peak, self.peak, 10.0 * hz, 2.0 * hz);
            self.stable_agc_peak = self.agc_peak;
        } else {
            // Turn down within 0.2 s for a peak; recover within 2 s while the level stays above
            // what was learned, else within 100 s.
            let recover = if self.peak > self.stable_agc_peak { F(0.5) } else { F(0.01) };
            slope(&mut self.agc_peak, self.peak, 5.0 * hz, recover * hz);
        }
        if active {
            if self.time > self.ignore_time || self.peak > 0.001 {
                self.time += size;
            }
            if self.time >= self.active_time {
                self.time = self.active_time;
            }
        }
        self.auto_gain = gain_setting == 0;
        let mut manual_gain = F::from_i32(i32::from(gain_setting)) / 255.0;
        manual_gain *= manual_gain;
        manual_gain *= manual_gain;
        let manual_gain = MIN_GAIN + (MAX_GAIN - MIN_GAIN) * manual_gain;
        one_pole(&mut self.manual_gain, manual_gain, F(0.01));
        let optimal_gain = (0.9 / self.agc_peak.min(F(0.9))).min(F(MAX_GAIN));
        let gain = if self.auto_gain { optimal_gain } else { self.manual_gain };
        let soft_clipping_amount = match self.soft_clipping {
            SoftClipping::Off => F::ZERO,
            SoftClipping::On => F(1.0),
            SoftClipping::Dynamic => constrain(2.0 * (self.manual_gain / optimal_gain - 0.75), F::ZERO, F(1.0)),
        };
        let mut gain_modulation = ParameterInterpolator::new(self.gain, gain, size);
        let mut amount_modulation = ParameterInterpolator::new(self.soft_clipping_amount, soft_clipping_amount, size);
        for (frame, o) in input.iter().zip(output.iter_mut()) {
            let l_int = if normalized[0] { frame[1] } else { frame[0] };
            let r_int = if normalized[1] { frame[0] } else { frame[1] };
            let mut l = F::from_i32(l_int) / 2_147_483_648.0;
            let mut r = F::from_i32(r_int) / 2_147_483_648.0;
            if active {
                let peak = l.abs().max(r.abs());
                slope(&mut self.peak, peak, F(0.1), 2.0 * self.one_hertz);
            }
            let g = gain_modulation.next();
            l *= g;
            r *= g;
            if !self.auto_gain {
                if self.soft_clipping == SoftClipping::Off {
                    l = constrain(l, F(-1.0), F(1.0));
                    r = constrain(r, F(-1.0), F(1.0));
                } else {
                    // Crossfade between the clean (possibly hard-clipped) signal and the soft-clipped
                    // one.
                    let amount = amount_modulation.next();
                    let bias = 0.05;
                    let soft_l = (1.0 - bias) * soft_clip(l + bias) - bias;
                    let soft_r = (1.0 - bias) * soft_clip(r + bias) - bias;
                    l = crossfade(constrain(l, F(-1.0), F(1.0)), soft_l, amount);
                    r = crossfade(constrain(r, F(-1.0), F(1.0)), soft_r, amount);
                }
            }
            *o = [l.0, r.0];
        }
        self.gain = gain_modulation.value();
        self.soft_clipping_amount = amount_modulation.value();
    }
}
