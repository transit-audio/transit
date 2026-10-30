//! Top-level module for the engines.

// Based on MIT-licensed code (c) 2016 by Emilie Gillet (emilie.o.gillet@gmail.com)

pub mod additive_engine;
pub mod bass_drum_engine;
pub mod chord_engine;
pub mod fm_engine;
pub mod grain_engine;
pub mod hihat_engine;
pub mod modal_engine;
pub mod noise_engine;
pub mod particle_engine;
pub mod snare_drum_engine;
pub mod speech_engine;
pub mod string_engine;
pub mod swarm_engine;
pub mod virtual_analog_engine;
pub mod waveshaping_engine;
pub mod wavetable_engine;

use crate::utils::units::semitones_to_ratio;

pub trait Engine: Send {
    fn init(&mut self, sample_rate_hz: f32);

    fn reset(&mut self) {}

    fn render(&mut self, parameters: &EngineParameters, out: &mut [f32], aux: &mut [f32], already_enveloped: &mut bool);
}

#[derive(Debug, Default, Clone)]
pub struct EngineParameters {
    /// Trigger signal state
    pub trigger: TriggerState,

    /// Pitch in semitones
    /// Range: -119.0 - 120.0
    pub note: f32,

    /// Sweeps the spectral content from dark/sparse to bright/dense.
    /// Range: 0.0 - 1.0
    pub timbre: f32,

    /// Lateral timbral variations.
    /// Range: 0.0 - 1.0
    pub morph: f32,

    /// Frequency spread or the balance between the various constituents of the tone.
    /// Range: 0.0 - 1.0
    pub harmonics: f32,

    /// Level setting
    /// Range: 0.0 - 1.0
    pub accent: f32,

    /// Normalized frequency of A0 (55 Hz) for pitch calculations
    pub a0_normalized: f32,
}

/// The trigger state of a block, a bit set as in the firmware (`TriggerState` in
/// `engine/engine.h`): a block can carry a rising edge and a high gate at once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TriggerState(pub u8);

impl TriggerState {
    /// Gate low.
    pub const LOW: Self = Self(0);
    /// A rising edge in this block.
    pub const RISING_EDGE: Self = Self(1);
    /// No cable in the trigger input: the engine free-runs.
    pub const UNPATCHED: Self = Self(2);
    /// Gate high.
    pub const HIGH: Self = Self(4);

    /// Any bit of `other` is set (the firmware's `trigger & TRIGGER_*`).
    #[inline]
    pub const fn has(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for TriggerState {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[inline]
pub fn note_to_frequency(mut midi_note: f32, a0_normalized: f32) -> f32 {
    midi_note -= 9.0;
    midi_note = midi_note.clamp(-128.0, 127.0);

    a0_normalized * 0.25 * semitones_to_ratio(midi_note)
}
