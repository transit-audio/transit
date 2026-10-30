//! Approximative low pass gate.

// Based on MIT-licensed code (c) 2014 by Emilie Gillet (emilie.o.gillet@gmail.com)

use crate::utils::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::utils::parameter_interpolator::ParameterInterpolator;

#[derive(Debug, Default, Clone)]
pub struct LowPassGate {
    previous_gain: f32,
    filter: Svf,
}

impl LowPassGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&mut self) {
        self.previous_gain = 0.0;
        self.filter.init();
    }

    #[inline]
    pub fn process_replacing(&mut self, gain: f32, frequency: f32, hf_bleed: f32, in_out: &mut [f32]) {
        let mut gain_modulation = ParameterInterpolator::new(&mut self.previous_gain, gain, in_out.len());
        self.filter.set_f_q(frequency, 0.4, FrequencyApproximation::Dirty);

        for in_out_sample in in_out.iter_mut() {
            let s = *in_out_sample * gain_modulation.next();
            let lp = self.filter.process(s, FilterMode::LowPass);
            *in_out_sample = lp + (s - lp) * hf_bleed;
        }
    }
}
