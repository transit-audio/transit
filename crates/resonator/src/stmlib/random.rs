//! `stmlib/utils/random.h`: the firmware's linear congruential generator, one per part.
//!
//! The firmware keeps one generator in a static shared by everything that draws from it; the port
//! gives each part its own (the part owns one and hands it to the pluckers and the strings in the
//! firmware's order), so two parts never disturb each other's noise.

#![forbid(unsafe_code)]

use crate::float::F;

/// `stmlib::Random`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Random {
    state: u32,
}

impl Default for Random {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SEED)
    }
}

impl Random {
    /// The firmware's power-on state (`Random::rng_state_ = 0x21`).
    pub const DEFAULT_SEED: u32 = 0x21;

    /// A generator at `seed` (`Random::Seed`).
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> u32 {
        self.state
    }

    /// `GetWord()`.
    #[inline(always)]
    pub fn get_word(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.state
    }

    /// `GetFloat()`: 0 to 1.
    #[inline(always)]
    pub fn get_float(&mut self) -> F {
        F::from_u32(self.get_word()) / 4_294_967_296.0
    }
}
