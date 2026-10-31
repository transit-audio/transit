//! `stmlib/dsp/cosine_oscillator.h`, approximate mode: a cosine between 0 and 1 from a two-pole
//! recursion.

#![forbid(unsafe_code)]

use crate::float::F;

/// `stmlib::CosineOscillator` in `COSINE_OSCILLATOR_APPROXIMATE` mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct CosineOscillator {
    y1: F,
    y0: F,
    iir_coefficient: F,
    initial_amplitude: F,
}

impl CosineOscillator {
    /// `Init<COSINE_OSCILLATOR_APPROXIMATE>(frequency)`, then `Start`.
    #[inline(always)]
    pub fn init_approximate(&mut self, frequency: F) {
        let mut sign = F(16.0);
        let mut frequency = frequency - 0.25;
        if frequency < 0.0 {
            frequency = -frequency;
        } else if frequency > 0.5 {
            frequency -= 0.5;
        } else {
            sign = F(-16.0);
        }
        self.iir_coefficient = sign * frequency * (1.0 - 2.0 * frequency);
        self.initial_amplitude = self.iir_coefficient * 0.25;
        self.start();
    }

    /// `Start`.
    #[inline(always)]
    pub fn start(&mut self) {
        self.y1 = self.initial_amplitude;
        self.y0 = F(0.5);
    }

    /// `value()`.
    #[inline(always)]
    #[must_use]
    pub fn value(&self) -> F {
        self.y1 + 0.5
    }

    /// `Next()`.
    #[inline(always)]
    #[allow(clippy::should_implement_trait)] // The firmware's name; not an iterator.
    pub fn next(&mut self) -> F {
        let temp = self.y0;
        self.y0 = self.iir_coefficient * self.y0 - self.y1;
        self.y1 = temp;
        temp + 0.5
    }
}

// Continuation checkpoint, version 1. Configuration is reconstructed before reading.
crate::state_fields!(
    CosineOscillator,
    s,
    [y1, y0, iir_coefficient, initial_amplitude],
    // The normal recursion is within +/-0.5; allow drift, but never a delay offset
    // large enough to overflow the reverb's integer address arithmetic.
    s.y1.abs() <= 4.0 && s.y0.abs() <= 4.0 && s.iir_coefficient.abs() < 2.0 && s.initial_amplitude.abs() <= 0.5
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::State;

    #[test]
    fn checkpoint_rejects_huge_finite_modulation_state() {
        for field in 0..4 {
            let mut source = CosineOscillator::default();
            source.init_approximate(F(0.001));
            let mut bytes = Vec::new();
            source.write(&mut bytes);
            bytes[field * 4..field * 4 + 4].copy_from_slice(&f32::MAX.to_le_bytes());
            assert!(CosineOscillator::default().read(&mut bytes.as_slice()).is_none());
        }
    }
}
