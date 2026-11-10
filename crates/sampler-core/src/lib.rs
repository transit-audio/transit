//! transit's portable sampler core (ADR 0041): the playback of the Sampler instrument without the
//! desktop host around it. It holds the zones (a sample region with its key and velocity ranges,
//! root and playback settings), the 64 pads in four banks that bind one zone each to a trigger
//! note, and 64 voices; it reads audio through [`reader::RegionReader`]s the caller lends it,
//! interpolates with the four-point cubic of the original sampler, loops forward or ping-pong,
//! runs each voice's amp envelope, its per-voice shaper and modulation matrix (the shared
//! primitives of `transit-instrument-core`, through the shaper's external amp-gain seam so the amp
//! envelope applies once), and ends voices by release, region end, choke or steal.
//!
//! - **Inputs are explicit.** [`SamplerCore::prepare`] takes the session rate and a seed;
//!   [`SamplerCore::process`] takes a [`event::Context`] (absolute frame, tempo, song position), the
//!   borrowed sources ([`reader::SourceBank`]), a borrowed span of time-stamped
//!   [`event::Event`]s and caller-prepared output channels. No clock, thread, logger or file.
//! - **Ownership stays with the caller.** The core receives borrowed readers and never owns, keeps
//!   or drops a sample; the desktop adapter keeps its `Arc`s, a disk reader its windows.
//! - **Bounded.** Fixed capacities ([`PADS`], [`ZONES`], [`VOICES`]); everything is allocated by
//!   [`SamplerCore::prepare`]; processing, events, control changes and reset neither allocate nor
//!   free. A missing tap reads zero and is counted ([`Diagnostics`]), never waited for.
//!
//! ADR 0037 Tier B: the default `std` feature keeps the desktop arithmetic (the inherent float
//! methods); with default features off the crate is `no_std` + `alloc` and its float functions come
//! from `libm`. CI checks that configuration for a bare-metal ARM target.

#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

extern crate alloc;

pub mod envelope;
pub mod event;
pub mod fades;
mod math;
pub mod patch;
pub mod playhead;
pub mod reader;
mod sampler;
mod selection;
mod voice;

pub use sampler::{Diagnostics, SamplerCore};

/// Pads, in four banks of sixteen.
pub const PADS: usize = 64;
/// Banks of pads.
pub const BANKS: usize = 4;
/// Pads in a bank (the visible 4 x 4 grid).
pub const PADS_PER_BANK: usize = 16;
/// Sample-bearing zones (leaves): pad zones and every round-robin alternative count.
pub const ZONES: usize = 256;
/// Voices across pads and unbound zones, release and choke tails included; no hidden extra voice.
pub const VOICES: usize = 64;
/// Choke groups (1..=16; 0 is Off).
pub const CHOKE_GROUPS: u8 = 16;
/// Frames of the choke fade (and of the fade that ends one-shots at All Notes Off and Stop).
pub const CHOKE_FRAMES: u32 = 32;
/// Most voices one note starts (a pad triggered by the note counts as one).
pub const LAYERS_PER_NOTE: usize = 8;
/// Key-map group ids (a group id is a `u8`).
pub const GROUPS: usize = 256;
/// Velocity layers in a key-map group.
pub const LAYERS: usize = 8;
/// Round-robin alternatives in a velocity layer.
pub const ALTERNATIVES: usize = 8;
/// Fastest playback step in source frames per output frame.
pub const MAX_STEP: f64 = 256.0;
/// The modulation grid in output frames (the shaper's block).
pub const MOD_BLOCK: usize = transit_instrument_core::shaper::BLOCK;

#[cfg(test)]
mod tests;
