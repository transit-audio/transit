//! Limiter.

// Based on MIT-licensed code (c) 2015 by Olivier Gillet (ol.gillet@gmail.com)

use super::ftz;

#[derive(Debug, Default, Clone)]
pub struct Limiter {
    peak: f32,
}

impl Limiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&mut self) {
        self.peak = 0.5;
    }

    #[inline]
    pub fn process(&mut self, pre_gain: f32, in_out: &mut [f32]) {
        // Every operation is flushed to zero, as on the firmware: in silence the peak follower
        // decays toward zero and the samples are tiny.
        for sample in in_out.iter_mut() {
            let s = ftz(*sample * pre_gain);
            let error = ftz(s.abs() - self.peak);
            self.peak = ftz(self.peak + ftz((if error > 0.0 { 0.05 } else { 0.00002 }) * error));
            let gain = if self.peak <= 1.0 { 1.0 } else { 1.0 / self.peak };
            *sample = ftz(ftz(s * gain) * 0.8);
        }
    }
}
