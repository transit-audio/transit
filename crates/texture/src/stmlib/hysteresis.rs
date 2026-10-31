//! `stmlib/dsp/hysteresis_filter.h` and `stmlib/dsp/hysteresis_quantizer.h`
//! (`HysteresisQuantizer2`).

#![forbid(unsafe_code)]

use crate::float::F;

/// `stmlib::HysteresisFilter`: holds its value until the input moves further than the threshold.
#[derive(Clone, Copy, Debug, Default)]
pub struct HysteresisFilter {
    value: F,
    threshold: F,
}

impl HysteresisFilter {
    /// `Init(threshold)`.
    pub fn init(&mut self, threshold: F) {
        self.value = F::ZERO;
        self.threshold = threshold;
    }

    /// `Process(value)`.
    #[inline(always)]
    pub fn process(&mut self, value: F) -> F {
        if self.threshold == 0.0 {
            self.value = value;
        } else {
            let error = value - self.value;
            if error > self.threshold {
                self.value = value - self.threshold;
            } else if error < -self.threshold {
                self.value = value + self.threshold;
            }
        }
        self.value
    }
}

/// `stmlib::HysteresisQuantizer2`: a value in 0 to 1 to an integer in 0 to `num_steps - 1`, with
/// hysteresis near each boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct HysteresisQuantizer {
    num_steps: i32,
    hysteresis: F,
    scale: F,
    offset: F,
    quantized_value: i32,
}

impl HysteresisQuantizer {
    /// `Init(num_steps, hysteresis, symmetric)`.
    pub fn init(&mut self, num_steps: i32, hysteresis: F, symmetric: bool) {
        self.num_steps = num_steps;
        self.hysteresis = hysteresis;
        self.scale = F::from_i32(if symmetric { num_steps - 1 } else { num_steps });
        self.offset = if symmetric { F::ZERO } else { F(-0.5) };
        self.quantized_value = 0;
    }

    /// `num_steps()`.
    #[must_use]
    pub fn num_steps(&self) -> i32 {
        self.num_steps
    }

    /// `Process(value)`.
    #[inline(always)]
    pub fn process(&mut self, value: F) -> i32 {
        let value = value * self.scale + self.offset + F::ZERO;
        let sign = if value > F::from_i32(self.quantized_value) { F(-1.0) } else { F(1.0) };
        let mut q = (value + sign * self.hysteresis + 0.5).trunc_i32();
        // `CONSTRAIN(q, 0, num_steps - 1)`, in its own order (never a clamp: with no steps the
        // firmware's macro returns -1).
        if q < 0 {
            q = 0;
        } else if q > self.num_steps - 1 {
            q = self.num_steps - 1;
        }
        self.quantized_value = q;
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quantizer_holds_near_a_boundary() {
        let mut q = HysteresisQuantizer::default();
        q.init(8, F(0.075), false);
        assert_eq!(q.process(F(0.0)), 0);
        assert_eq!(q.process(F(1.0)), 7);
        assert_eq!(q.process(F(0.5)), 4);
        // Just below the 3/4 boundary from above: still 4 thanks to the hysteresis.
        assert_eq!(q.process(F(0.495)), 4);
        assert_eq!(q.process(F(0.44)), 3);
    }

    #[test]
    fn the_filter_lags_by_its_threshold() {
        let mut f = HysteresisFilter::default();
        f.init(F(0.1));
        assert_eq!(f.process(F(0.05)), F(0.0));
        assert_eq!(f.process(F(0.5)), F(0.4));
        assert_eq!(f.process(F(0.45)), F(0.4));
    }
}
