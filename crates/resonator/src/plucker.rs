//! The noise burst that plucks a string (`rings/dsp/plucker.h`): white noise for one period,
//! through a comb filter set by position and a low-pass set by brightness.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::delay_line::DelayLine;
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::random::Random;

/// `rings::Plucker`.
#[derive(Clone, Debug, Default)]
pub struct Plucker {
    svf: Svf,
    comb_filter: DelayLine<256>,
    remaining_samples: usize,
    comb_filter_period: F,
    /// `Init` never writes it: zero from the module's static storage until the first trigger.
    comb_filter_gain: F,
}

impl Plucker {
    /// `Init`.
    pub fn init(&mut self) {
        self.svf.init();
        self.comb_filter.init();
        self.remaining_samples = 0;
        self.comb_filter_period = F::ZERO;
    }

    /// `Trigger(frequency, cutoff, position)`.
    pub fn trigger(&mut self, frequency: F, cutoff: F, position: F) {
        let ratio = position * 0.9 + 0.05;
        let mut comb_period = 1.0 / frequency * ratio;
        self.remaining_samples = comb_period.trunc_usize();
        while comb_period >= 255.0 {
            comb_period *= 0.5;
        }
        self.comb_filter_period = comb_period;
        self.comb_filter_gain = (1.0 - position) * 0.8;
        self.svf.set_f_q(FrequencyApproximation::Dirty, cutoff.min(F(0.499)), F(1.0));
    }

    /// `Process(out, size)`.
    pub fn process(&mut self, out: &mut [F], random: &mut Random) {
        let comb_gain = self.comb_filter_gain;
        let comb_delay = self.comb_filter_period;
        for o in out.iter_mut() {
            let mut x = F::ZERO;
            if self.remaining_samples != 0 {
                x = 2.0 * random.get_float() - 1.0;
                self.remaining_samples -= 1;
            }
            *o = x + comb_gain * self.comb_filter.read(comb_delay);
            self.comb_filter.write(*o);
        }
        self.svf.process_block(out, FilterMode::LowPass);
    }
}
