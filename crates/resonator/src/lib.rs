//! transit's resonator: a physical-modeling resonator ported to Rust from Émilie Gillet's MIT
//! firmware (the eurorack repository's `rings/dsp` at `08460a69`, with the stmlib helpers it uses),
//! nulled bit for bit against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #198).
//!
//! [`Resonator`] is the firmware's part: up to four voices of one of six models (modal, sympathetic
//! strings, inharmonic string, FM voice, quantized sympathetic strings, string and reverb), excited
//! by an audio input or by its own exciter on a strum, with two outputs. It runs at 48 kHz (the
//! firmware's only rate; [`SAMPLE_RATE`]) in blocks of up to 24 frames ([`MAX_BLOCK_SIZE`]); see
//! [`Resonator::process`] for the render contract.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once a [`Resonator`] is built, one random
//! generator per resonator, and the firmware's flush-to-zero reproduced in its own arithmetic
//! ([`float::F`]), never by changing the host's floating-point mode.

#![forbid(unsafe_code)]

mod chords;
pub mod float;
mod fm_voice;
mod limiter;
mod modal;
mod note_filter;
mod part;
mod plucker;
mod resources;
mod reverb;
pub mod stmlib;
mod string;

pub use part::{Patch, PerformanceState, Resonator, ResonatorModel};

/// The firmware's sample rate (`kSampleRate` in `rings/dsp/dsp.h`), the only rate the resonator
/// runs at: its filters, delays and reverb are tuned for it.
pub const SAMPLE_RATE: f32 = 48_000.0;

/// The firmware's block size (`kMaxBlockSize`): the module renders 24 frames per call.
pub const MAX_BLOCK_SIZE: usize = 24;

/// The largest polyphony (`kMaxPolyphony`).
pub const MAX_POLYPHONY: usize = 4;

/// The number of chords of the quantized sympathetic strings (`kNumChords`).
pub const CHORD_COUNT: i32 = 11;
