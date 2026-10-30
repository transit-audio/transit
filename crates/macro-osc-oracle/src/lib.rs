//! The null-test oracle for transit's ported instruments (ADR 0033): the macro oscillator (#194)
//! and the resonator (#198).
//!
//! This crate builds Émilie Gillet's original C++ DSP (MIT; vendored under `vendor/`, see
//! `VENDOR.md` and `LICENSE-THIRD-PARTY`) with the `cc` crate and exposes it to Rust tests. It is
//! **dev-only**: `publish = false`, and `cargo xtask layers` fails if any other workspace crate
//! takes it as a normal or build dependency, so it can never be linked into the app.
//!
//! - [`Session`]: exclusive access to the oracle (the C++ keeps its random state in a static).
//! - [`Voice`] and [`EngineOracle`]: a full voice (int16 out/aux, as the firmware) and one engine
//!   on its own (float out/aux before post-processing), so engines can be nulled one at a time.
//! - [`sweep`]: the fixed parameter sweeps every engine is rendered with, at 48 kHz in the
//!   firmware's 12-frame blocks.
//! - [`residual`]: peak and RMS residual between two renders, in dBFS.
//! - [`floor`]: the measured residual floor per CI runner family.
//! - [`resonator`] and [`resonator_sweep`]: the resonator part (six models, polyphony 1 to 4, float
//!   out/aux) and its fixed sweep, at 48 kHz in the firmware's 24-frame blocks (#198).

mod ffi;
pub mod floor;
mod model;
mod oracle;
pub mod residual;
pub mod resonator;
pub mod resonator_sweep;
pub mod sweep;

pub use ffi::Session;
pub use model::Model;
pub use oracle::{EngineOracle, EngineParameters, Error, Frame, Modulations, Patch, Trigger, Voice};

/// The only sample rate the firmware supports (`kSampleRate` in `plaits/dsp/dsp.h`).
pub const SAMPLE_RATE: f32 = 48_000.0;

/// The firmware's block size, `kBlockSize` (the audio driver renders 12 frames per call).
pub const BLOCK_SIZE: usize = 12;

/// The largest block one render call accepts, `kMaxBlockSize`.
pub const MAX_BLOCK_SIZE: usize = 24;

/// The number of engines (16 in `engine/`, 8 in `engine2/`; three of them are the six-operator FM
/// engine with its three built-in banks).
pub const ENGINE_COUNT: usize = 24;
