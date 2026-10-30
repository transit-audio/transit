//! Fast 16-bit pseudo random number generator.
//!
//! The firmware keeps one generator for the whole module (`stmlib::Random`, a static) that every
//! engine draws from. Here the owner of the engines (a voice, or a test rendering one engine) owns
//! one and passes it to whichever engine renders, which passes it on to whatever draws from it in
//! the firmware's order. Seeded like the firmware's generator, it draws the same sequence, and two
//! voices never share or race over a state.

// Based on MIT-licensed code (c) 2012 by Olivier Gillet (ol.gillet@gmail.com)

/// A linear congruential generator, `stmlib::Random`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Random {
    state: u32,
}

impl Default for Random {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SEED)
    }
}

impl Random {
    /// The firmware's power-on state (`Random::rng_state_` in `stmlib/utils/random.cc`).
    pub const DEFAULT_SEED: u32 = 0x21;

    /// A generator in state `seed`.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// Sets the state (`Random::Seed`).
    #[inline]
    pub fn seed(&mut self, seed: u32) {
        self.state = seed;
    }

    /// The current state.
    #[inline]
    #[must_use]
    pub fn state(&self) -> u32 {
        self.state
    }

    /// The next 32-bit word.
    #[inline]
    pub fn get_word(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(1664525).wrapping_add(1013904223);
        self.state
    }

    /// The next signed 16-bit sample.
    #[inline]
    pub fn get_sample(&mut self) -> i16 {
        (self.get_word() >> 16) as i16
    }

    /// The next float in [0, 1).
    #[inline]
    pub fn get_float(&mut self) -> f32 {
        self.get_word() as f32 / 4294967296.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sequence_is_the_firmware_lcg() {
        let mut r = Random::new(1);
        assert_eq!(r.get_word(), 1664525 + 1013904223);
        let mut a = Random::new(0x0194_5EED);
        let mut b = a.clone();
        assert_eq!(a.get_float(), b.get_float());
        assert_eq!(Random::default().state(), 0x21);
    }
}
