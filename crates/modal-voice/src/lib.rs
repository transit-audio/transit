//! transit's modal voice: a gated exciter-and-resonator voice ported to Rust from Émilie Gillet's
//! MIT firmware (the eurorack repository's `elements/dsp` at `08460a69`, with the stmlib helpers it
//! uses), nulled bit for bit against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #198).
//!
//! [`ModalVoice`] is the firmware's part: a gate envelope drives three exciters (a bow, a blow
//! through a reed tube and a diffuser, a strike from samples, a mallet, a plectrum or particles),
//! summed under the strength's accent into one of three resonators (a modal bank, one string, or
//! five strings tuned to a chord), then the space control's spread and reverb; or, instead of the
//! exciters and resonator, the firmware's hidden FM pair voice. It runs at 32 kHz (the firmware's
//! only rate; [`SAMPLE_RATE`]) in blocks of up to 16 frames ([`MAX_BLOCK_SIZE`]); see
//! [`ModalVoice::process`] for the render contract.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once a [`ModalVoice`] is built (its reset
//! included), one random generator per voice, and the oracle's flush-to-zero reproduced in its own
//! arithmetic (`transit_resonator::float::F`), never by changing the host's floating-point mode.
//! The firmware's 9 strike samples and its noise sample (128 013 and 40 963 16-bit values) are
//! compiled in, as the module keeps them in flash.

#![forbid(unsafe_code)]

mod dsp;
mod envelope;
mod exciter;
mod fm_pair;
mod fx;
mod modal;
mod noise_1;
mod noise_2;
mod part;
mod resources;
mod resources_sine;
mod samples;
mod samples_1;
mod samples_2;
mod samples_3;
mod samples_4;
mod samples_5;
mod string;
mod tube;
mod voice;

pub use part::{ModalVoice, Patch, PerformanceState, ResonatorModel};

/// The firmware's sample rate (`kSampleRate` in `elements/dsp/dsp.h`), the only rate the voice
/// runs at: its filters, delays, envelope and reverb are tuned for it.
pub const SAMPLE_RATE: f32 = 32_000.0;

/// The firmware's block size (`kMaxBlockSize`): the module renders 16 frames per call.
pub const MAX_BLOCK_SIZE: usize = 16;
