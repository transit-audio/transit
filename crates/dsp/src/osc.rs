//! Band-limited (PolyBLEP) oscillators shared by the signal generator.

use std::f32::consts::TAU;

use crate::util::{PinkNoise, Rng};

/// Test-signal / oscillator waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
    WhiteNoise,
    PinkNoise,
}

impl Waveform {
    pub const ALL: [Waveform; 6] = [Waveform::Sine, Waveform::Square, Waveform::Saw, Waveform::Triangle, Waveform::WhiteNoise, Waveform::PinkNoise];

    /// Waveform for a choice index (out of range → Sine).
    pub fn from_index(i: usize) -> Waveform {
        Self::ALL.get(i).copied().unwrap_or(Waveform::Sine)
    }
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// Phase-accumulator oscillator. Output peaks at ±1 (noise is uniform ±1 / pink ≈ ±1).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Osc {
    pub(crate) phase: f32,
    rng: Rng,
    pink: PinkNoise,
}

impl Osc {
    pub(crate) fn new(seed: u32) -> Self {
        Self { phase: 0.0, rng: Rng::new(seed), pink: PinkNoise::new() }
    }

    pub(crate) fn reset(&mut self) {
        self.phase = 0.0;
        self.pink = PinkNoise::new();
    }

    /// Produces one sample; `inc` is frequency / sample rate.
    #[inline]
    pub(crate) fn next(&mut self, wave: Waveform, inc: f32) -> f32 {
        let dt = if inc.is_finite() { inc.clamp(0.0, 0.49) } else { 0.0 };
        let t = self.phase;
        let out = match wave {
            Waveform::Sine => (t * TAU).sin(),
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                naive + poly_blep(t, dt) - poly_blep(t2, dt)
            }
            Waveform::Triangle => {
                // Starts at 0 rising, like a sine.
                let u = t + 0.25;
                let u = if u >= 1.0 { u - 1.0 } else { u };
                1.0 - 4.0 * (u - 0.5).abs()
            }
            Waveform::WhiteNoise => self.rng.bipolar(),
            Waveform::PinkNoise => {
                let w = self.rng.bipolar();
                self.pink.next(w).clamp(-1.0, 1.0)
            }
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}
