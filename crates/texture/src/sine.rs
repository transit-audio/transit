//! `beads/dsp/sine_oscillator.h`: the slow sine (0 to 1) behind the internal modulations.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::LUT_SINE;
use crate::stmlib::interpolate;

/// `SineOscillator`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SineOscillator {
    phase: F,
    frequency: F,
}

impl SineOscillator {
    /// `Init(frequency)` (`Init()` is `Init(0)`).
    pub fn init(&mut self, frequency: F) {
        self.phase = F::ZERO;
        self.frequency = frequency;
    }

    /// `phase()`.
    #[must_use]
    pub fn phase(&self) -> F {
        self.phase
    }

    /// `Render()`.
    #[inline(always)]
    pub fn render(&mut self) -> F {
        self.phase += self.frequency;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        0.5 + 0.5 * interpolate(&LUT_SINE, self.phase, 1024.0)
    }

    /// `Render(frequency)`.
    #[inline(always)]
    pub fn render_at(&mut self, frequency: F) -> F {
        self.frequency = frequency;
        self.render()
    }
}

// Continuation checkpoint, version 1. Configuration is reconstructed before reading.
crate::state_fields!(SineOscillator, s, [phase, frequency], (0.0..=1.0).contains(&s.phase.0) && (0.0..=1.0).contains(&s.frequency.0));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::State;

    #[test]
    fn checkpoint_rejects_phase_and_frequency_that_cannot_wrap_in_one_step() {
        for (phase, frequency) in [(f32::MAX, 0.0), (0.0, f32::MAX), (-1.0, 0.0)] {
            let source = SineOscillator { phase: F(phase), frequency: F(frequency) };
            let mut bytes = Vec::new();
            source.write(&mut bytes);
            assert!(SineOscillator::default().read(&mut bytes.as_slice()).is_none());
        }
    }
}
