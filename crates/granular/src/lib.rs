//! transit's granular texture processor: ported to Rust from Émilie Gillet's MIT firmware (the
//! eurorack repository's `clouds/dsp` at `08460a69`, with the stmlib helpers it uses), nulled bit
//! for bit against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #199).
//!
//! [`Granular`] is the firmware's processor in its granular playback mode: it records its stereo
//! input (with feedback) into a buffer, plays up to 64 overlapping grains from it (position, size,
//! pitch, density, window shape, random panning, freeze, trigger), then a diffuser, a reverb and a
//! dry/wet crossfade. It runs at 32 kHz ([`SAMPLE_RATE`], the firmware's rate) in blocks of up to
//! 32 stereo int16 frames ([`MAX_BLOCK_SIZE`]), at four buffer qualities ([`Quality`]: 16-bit or
//! 8-bit mu-law, stereo or mono), on memory sized at construction ([`BufferSize`]). See
//! [`Granular::process`] for the render contract.
//!
//! The firmware's other playback modes (pitch shifter and time stretcher, looping delay, spectral)
//! are not ported yet.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once a [`Granular`] is built, one random
//! generator per processor, and the oracle's flush-to-zero (the firmware's desktop test harness;
//! the module itself runs with FZ off) reproduced in its own arithmetic ([`float::F`]), never by
//! changing the host's floating-point mode.

#![forbid(unsafe_code)]

mod audio_buffer;
pub mod float;
mod fx;
mod grain;
mod processor;
mod resources;
mod sample_player;
mod sample_rate_converter;
pub mod stmlib;

pub use processor::{BufferSize, Frame, Granular, Parameters, Quality};

/// The firmware's sample rate: the module's codec runs at 32 kHz (`clouds/clouds.cc`), and its
/// filters, grain sizes, effects and reverb are tuned for it. The low-fidelity qualities run the
/// grains at half of it, behind the firmware's own 2:1 converters.
pub const SAMPLE_RATE: f32 = 32_000.0;

/// The firmware's block size (`kMaxBlockSize` in `clouds/dsp/frame.h`): the module renders 32
/// frames per call.
pub const MAX_BLOCK_SIZE: usize = 32;
