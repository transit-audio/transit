//! transit's macro oscillator (ADR 0033, #195): 24 synthesis engines in one mono voice.
//!
//! The DSP is Émilie Gillet's MIT firmware, as ported to Rust by Oliver Rockstedt (MIT) and
//! vendored here (see `VENDOR.md` and `LICENSE-THIRD-PARTY`), with transit's corrections against
//! the C++ oracle (`transit-macro-osc-oracle`). Every engine is nulled against the oracle sample
//! by sample (`tests/null_vs_oracle.rs`).
//!
//! The firmware runs at 48 kHz in 12-frame blocks; so does this crate for now.

#![forbid(unsafe_code)]
#![allow(missing_docs)]

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
