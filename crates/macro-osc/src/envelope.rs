//! Envelope for the internal LPG.

// Based on MIT-licensed code (c) 2016 by Emilie Gillet (emilie.o.gillet@gmail.com)
//
// Both envelopes decay toward zero for as long as a voice is silent, so every operation that can
// reach the subnormal range is flushed to zero, as on the firmware (see `utils::ftz`).

use crate::utils::ftz;

#[derive(Debug, Default, Clone)]
pub struct LpgEnvelope {
    vactrol_state: f32,
    gain: f32,
    frequency: f32,
    hf_bleed: f32,
    ramp_up: bool,
}

impl LpgEnvelope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&mut self) {
        self.vactrol_state = 0.0;
        self.gain = 1.0;
        self.frequency = 0.5;
        self.hf_bleed = 0.0;
        self.ramp_up = false;
    }

    pub fn trigger(&mut self) {
        self.ramp_up = true;
    }

    #[inline]
    pub fn process_ping(&mut self, attack: f32, short_decay: f32, decay_tail: f32, hf: f32) {
        if self.ramp_up {
            self.vactrol_state += attack;
            if self.vactrol_state >= 1.0 {
                self.vactrol_state = 1.0;
                self.ramp_up = false;
            }
        }
        self.process_lp(if self.ramp_up { self.vactrol_state } else { 0.0 }, short_decay, decay_tail, hf);
    }

    #[inline]
    pub fn process_lp(&mut self, level: f32, short_decay: f32, decay_tail: f32, hf: f32) {
        let vactrol_input = level;
        let vactrol_error = ftz(vactrol_input - self.vactrol_state);
        let vactrol_state_2 = ftz(self.vactrol_state * self.vactrol_state);
        let vactrol_state_4 = ftz(vactrol_state_2 * vactrol_state_2);
        let tail = 1.0 - self.vactrol_state;
        let tail_2 = tail * tail;
        let vactrol_coefficient = if vactrol_error > 0.0 { 0.6 } else { short_decay + (1.0 - vactrol_state_4) * decay_tail };
        self.vactrol_state = ftz(self.vactrol_state + ftz(vactrol_coefficient * vactrol_error));

        self.gain = self.vactrol_state;
        self.frequency = 0.003 + ftz(0.3 * vactrol_state_4) + hf * 0.04;
        self.hf_bleed = ftz(ftz(ftz(tail_2 + ftz((1.0 - tail_2) * hf)) * hf) * hf);
    }

    #[inline]
    pub fn gain(&self) -> f32 {
        self.gain
    }

    #[inline]
    pub fn frequency(&self) -> f32 {
        self.frequency
    }

    #[inline]
    pub fn hf_bleed(&self) -> f32 {
        self.hf_bleed
    }
}

#[derive(Debug, Default, Clone)]
pub struct DecayEnvelope {
    value: f32,
}

impl DecayEnvelope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init(&mut self) {
        self.value = 0.0;
    }

    pub fn trigger(&mut self) {
        self.value = 1.0;
    }

    #[inline]
    pub fn process(&mut self, decay: f32) {
        self.value = ftz(self.value * (1.0 - decay));
    }

    #[inline]
    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_voice_s_envelopes_never_go_subnormal() {
        let mut decay = DecayEnvelope::new();
        decay.init();
        decay.trigger();
        let mut lpg = LpgEnvelope::new();
        lpg.init();
        lpg.trigger();
        for _ in 0..20_000 {
            decay.process(0.1);
            lpg.process_ping(0.5, 0.05, 0.001, 0.0);
            assert!(!decay.value().is_subnormal() && !lpg.gain().is_subnormal());
        }
        assert_eq!(decay.value().to_bits(), 0);
        // The vactrol's decrement flushes to zero before its state does, so the state freezes at
        // a tiny normal value, as on the firmware.
        let frozen = lpg.gain();
        assert!(frozen > 0.0 && frozen < 1e-36, "{frozen:e}");
        lpg.process_ping(0.5, 0.05, 0.001, 0.0);
        assert_eq!(lpg.gain().to_bits(), frozen.to_bits());
    }
}
