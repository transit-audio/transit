//! transit's macro oscillator (ADR 0033, #195): 24 synthesis engines in one mono voice.
//!
//! The DSP is Émilie Gillet's MIT firmware, as ported to Rust by Oliver Rockstedt (MIT) and
//! vendored here (see `VENDOR.md` and `LICENSE-THIRD-PARTY`), with transit's corrections against
//! the C++ oracle (`transit-macro-osc-oracle`). Every engine is nulled against the oracle sample
//! by sample (`tests/null_vs_oracle.rs`), and so is the voice, int16 frame by frame
//! (`tests/null_vs_oracle_voice.rs`, `tests/voice_scenarios_vs_oracle.rs`).
//!
//! The firmware runs at 48 kHz in 12-frame blocks. A voice here also runs at 44.1, 88.2 and 96 kHz
//! ([`SampleRate`]), still in 12-frame blocks; only 48 kHz is nulled against the firmware.

#![forbid(unsafe_code)]

pub mod chords;
pub mod downsampler;
pub mod drums;
pub mod engine;
pub mod engine2;
pub mod envelope;
pub mod fm;
pub mod fx;
pub mod noise;
pub mod oscillator;
pub mod physical_modelling;
pub mod resources;
pub mod speech;
pub mod utils;
pub mod voice;

/// The sample rate the firmware's tables and coefficients assume.
pub const REF_SAMPLE_RATE: f32 = 48000.0;

/// The firmware's corrected sample rate (`kCorrectedSampleRate` in `dsp/dsp.h`): the module's
/// audio clock runs at 47872.34 Hz, and pitch is computed against it so notes are in tune.
pub const CORRECTED_SAMPLE_RATE: f32 = 47872.34;

/// The normalized frequency of A0 that every pitch is computed from (`a0` in `dsp/dsp.h`).
pub const A0: f32 = (440.0 / 8.0) / CORRECTED_SAMPLE_RATE;

/// The rates a voice runs at. 48 kHz is the firmware's rate and the only one the oracle renders;
/// the others keep the firmware's pitch and envelope times (see the crate README, "Sample rate").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SampleRate {
    /// 44.1 kHz.
    Hz44100,
    /// 48 kHz, the firmware's rate.
    #[default]
    Hz48000,
    /// 88.2 kHz.
    Hz88200,
    /// 96 kHz.
    Hz96000,
}

impl SampleRate {
    /// Every supported rate, ascending.
    pub const ALL: [Self; 4] = [Self::Hz44100, Self::Hz48000, Self::Hz88200, Self::Hz96000];

    /// The rate in hertz.
    #[must_use]
    pub const fn hz(self) -> f32 {
        match self {
            Self::Hz44100 => 44_100.0,
            Self::Hz48000 => 48_000.0,
            Self::Hz88200 => 88_200.0,
            Self::Hz96000 => 96_000.0,
        }
    }

    /// The supported rate equal to `hz`, if any.
    #[must_use]
    pub fn from_hz(hz: f32) -> Option<Self> {
        Self::ALL.into_iter().find(|rate| rate.hz().to_bits() == hz.to_bits())
    }
}
