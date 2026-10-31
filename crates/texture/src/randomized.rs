//! `beads/dsp/randomized_parameter.h` and `beads/dsp/modulated_parameter.h`: the controls that carry
//! internal random modulation or a CV.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, semitones_to_ratio, smooth_step};

/// `CenteredUniform()`: -1 to 1.
#[inline(always)]
pub fn centered_uniform(rng: &mut Random) -> F {
    2.0 * rng.get_float() - 1.0
}

/// `PseudoGaussian()`: -1 to 1, denser near 0.
#[inline(always)]
pub fn pseudo_gaussian(rng: &mut Random) -> F {
    let u = rng.get_float();
    u * (u * (4.0 * u - 6.0) + 4.0) - 1.0
}

/// `BoundedRandomFloat(spread)`: 0.5 plus a random deviation of at most half the spread.
#[inline(always)]
pub fn bounded_random_float(rng: &mut Random, spread: F) -> F {
    let u = rng.get_float();
    let v = rng.get_float();
    0.5 + (u - 0.5) * (spread * v).min(F(1.0))
}

/// `RandomWalkModulation`: a smooth random walk between uniform targets.
#[derive(Clone, Copy, Debug, Default)]
pub struct RandomWalkModulation {
    phase: F,
    frequency: F,
    from: F,
    to: F,
}

impl RandomWalkModulation {
    /// `Init(frequency)`.
    pub fn init(&mut self, frequency: F) {
        self.frequency = frequency;
        self.phase = F::ZERO;
        self.from = F::ZERO;
        self.to = F(1.0);
    }

    /// `value(attenuverter, step_size)`: a negative or zero attenuverter walks 16 times slower.
    #[inline(always)]
    pub fn value(&mut self, rng: &mut Random, attenuverter: F, step_size: usize) -> F {
        let frequency = (if attenuverter <= 0.0 { F(0.25) } else { F(4.0) }) * self.frequency * F::from_usize(step_size);
        self.phase += frequency;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.from = self.to;
            self.to = centered_uniform(rng);
        }
        self.from + (self.to - self.from) * smooth_step(self.phase)
    }
}

/// A control with its attenuverter and CV (`RandomizedParameter`): without CV the attenuverter sets
/// the amount and kind of internal random modulation (positive: smooth, or toward a random value
/// per grain; negative: occasional jumps); with CV it scales the CV.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameter {
    /// The knob.
    pub base: f32,
    /// The attenuverter, -1 to 1.
    pub attenuverter: f32,
    /// The CV, in the knob's units.
    pub cv: f32,
    /// Whether a CV is patched.
    pub has_cv: bool,
}

impl Parameter {
    /// A knob alone: no randomness, no CV.
    #[must_use]
    pub const fn knob(base: f32) -> Self {
        Self { base, attenuverter: 0.0, cv: 0.0, has_cv: false }
    }

    /// `randomness_amount()`.
    #[inline(always)]
    #[must_use]
    pub(crate) fn randomness_amount(&self) -> F {
        if self.has_cv { (-F(self.attenuverter)).max(F::ZERO) } else { F(self.attenuverter).abs() }
    }

    /// `value(modulation, step_size, range, clamp)`.
    #[inline(always)]
    pub(crate) fn value_with(&self, rng: &mut Random, modulation: Option<&mut RandomWalkModulation>, step_size: usize, range: F, clamp: bool) -> F {
        let attenuverter = F(self.attenuverter);
        let mut result = F(self.base);
        if self.has_cv {
            if attenuverter < 0.0 {
                let extra = match modulation {
                    Some(m) => m.value(rng, F(1.0), step_size),
                    None => pseudo_gaussian(rng),
                };
                result += F(self.cv).max(F::ZERO) * -attenuverter * extra;
            } else {
                result += attenuverter * F(self.cv);
            }
        } else if let Some(m) = modulation {
            let walk = m.value(rng, attenuverter, step_size);
            result += attenuverter * range * walk;
        } else {
            let blend = if attenuverter > 0.0 { attenuverter } else { F(1.0).min(attenuverter * rng.get_float() * -3.0) };
            let random_value = if clamp { rng.get_float() * range - result } else { centered_uniform(rng) * range };
            result += random_value * blend;
        }
        if clamp { constrain(result, F::ZERO, F(1.0)) } else { result }
    }

    /// `value()`: per grain, 0 to 1.
    #[inline(always)]
    pub(crate) fn value(&self, rng: &mut Random) -> F {
        self.value_with(rng, None, 0, F(1.0), true)
    }

    /// `pitch_value()`: per grain, as a playback-speed ratio (24 semitones of randomness).
    #[inline(always)]
    pub(crate) fn pitch_value(&self, rng: &mut Random) -> F {
        semitones_to_ratio(self.value_with(rng, None, 0, F(24.0), false))
    }
}

/// The four randomized controls and the monophonic mode (`Parameters` in
/// `beads/dsp/granular_player.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameters {
    /// TIME: where the grains start (0 newest to 1 oldest); in the delay mode the delay multiplier
    /// or the frozen slice.
    pub time: Parameter,
    /// SIZE: grain length, reversed below a third; above 0.99 the delay mode. Its attenuverter is
    /// also the delay's bandwidth.
    pub size: Parameter,
    /// SHAPE: the grain or tap envelope.
    pub shape: Parameter,
    /// PITCH, semitones.
    pub pitch: Parameter,
    /// The monophonic grain mode: each grain cuts the previous ones short.
    pub monophonic: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_knob_draws_once_per_value_and_keeps_its_place() {
        let mut rng = Random::new(1);
        let p = Parameter::knob(0.3);
        // attenuverter 0: blend = min(1, 0 * u * -3) = 0, so the knob comes back unchanged, after
        // two draws (the blend's and the random value's).
        assert_eq!(p.value(&mut rng), F(0.3));
        let mut fresh = Random::new(1);
        let _ = fresh.get_word();
        let _ = fresh.get_word();
        assert_eq!(rng, fresh);
    }

    #[test]
    fn full_randomness_spreads_over_the_range() {
        let mut rng = Random::new(7);
        let p = Parameter { attenuverter: 1.0, ..Parameter::knob(0.5) };
        let values: Vec<f32> = (0..200).map(|_| p.value(&mut rng).0).collect();
        assert!(values.iter().any(|&v| v < 0.1) && values.iter().any(|&v| v > 0.9));
    }
}
