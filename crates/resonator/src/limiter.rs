//! The output limiter (`rings/dsp/limiter.h`): a peak follower over left, right and their
//! difference, a gain that holds the peak at full scale, and a soft clipper.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::{slope, soft_limit};

/// `rings::Limiter`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Limiter {
    peak: F,
}

impl Limiter {
    /// `Init`.
    pub fn init(&mut self) {
        self.peak = F(0.5);
    }

    /// `Process(l, r, size, pre_gain)`, in place.
    pub fn process(&mut self, l: &mut [F], r: &mut [F], pre_gain: F) {
        for (l, r) in l.iter_mut().zip(r.iter_mut()) {
            let l_pre = *l * pre_gain;
            let r_pre = *r * pre_gain;
            let l_peak = l_pre.abs();
            let r_peak = r_pre.abs();
            let s_peak = (r_pre - l_pre).abs();
            let peak = l_peak.max(r_peak).max(s_peak);
            slope(&mut self.peak, peak, F(0.05), F(0.00002));
            // Clamp to 8 Vpp, clipping softly toward 10 Vpp.
            let gain = if self.peak <= 1.0 { F(1.0) } else { 1.0 / self.peak };
            *l = soft_limit(l_pre * gain * 0.8);
            *r = soft_limit(r_pre * gain * 0.8);
        }
    }
}
