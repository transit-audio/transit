//! transit's random sequencer: ported to Rust from Émilie Gillet's MIT firmware (the eurorack
//! repository's `marbles/random` and `marbles/ramp` at `08460a69`, with the stmlib helpers they
//! use), nulled bit for bit against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #202).
//!
//! Two layers:
//!
//! - [`Generators`]: the firmware's generators as its main loop drives them, at 32 kHz
//!   ([`SAMPLE_RATE`]) in blocks of [`BLOCK_SIZE`] frames. A master clock ticks (internally, or
//!   recovered from a clock input); on each tick a random vector decides, by one of seven models,
//!   which of two gate channels fire and how long, the jitter bending the timing; three X channels
//!   and a Y channel draw voltages shaped by spread and bias, quantized to a scale by steps; deja
//!   vu loops, mutates or shuffles every random decision. This layer nulls against the oracle.
//! - [`Sequencer`]: the same generators locked to a host's transport. Ticks fall on a beat grid
//!   (the firmware's clock divisions of a patched clock, with the beat as the clock), every
//!   random decision is the firmware's code, and the gates' timing is computed from the ramps'
//!   slopes in beats, so events land on exact sample offsets whatever the host's block size.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once built, one random source per instance,
//! and the oracle family's pinned flush-to-zero reproduced in its own arithmetic ([`float::F`]),
//! never by changing the host's floating-point mode.

#![forbid(unsafe_code)]

mod distributions;
pub mod float;
mod generators;
mod lag;
mod output_channel;
mod pitch_tables;
mod quantizer;
mod ramp;
mod ramp_extractor;
mod random;
mod resources;
mod scale_recorder;
pub mod scales;
mod sequencer;
mod stmlib;
mod t_generator;
mod xy_generator;

pub use generators::{Controls, DEJA_VU_LOCKED, DEJA_VU_OFF, DEJA_VU_ON, Generators, LOOP_LENGTHS, Y_DIVIDERS, deja_vu_dead_band};
pub use quantizer::{MAX_DEGREES, Scale};
pub use ramp::Ratio;
pub use scale_recorder::ScaleRecorder;
pub use sequencer::{Block, Event, EventKind, RATES, Sequencer, Settings, Voices, division, key_of, velocity_of};
pub use t_generator::Model;
pub use xy_generator::{ControlMode, VoltageRange};

/// The module's block, frames (`kBlockSize` in `marbles/io_buffer.h`).
pub const BLOCK_SIZE: usize = 5;

/// The firmware's sample rate, Hz (`kSampleRate` in `marbles/marbles.cc`).
pub const SAMPLE_RATE: f32 = 32_000.0;

/// Gate flags of a clock input, one byte per frame (`stmlib/utils/gate_flags.h`).
pub mod gate {
    /// The input is high.
    pub const HIGH: u8 = 1;
    /// The input rose at this frame.
    pub const RISING: u8 = 2;
    /// The input fell at this frame.
    pub const FALLING: u8 = 4;
}
