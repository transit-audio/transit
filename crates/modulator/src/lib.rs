//! transit's meta-modulator: ported to Rust from Émilie Gillet's MIT firmware (the eurorack
//! repository's `warps/dsp` at `08460a69`, with the stmlib helpers it uses), nulled bit for bit
//! against the C++ oracle (`transit-macro-osc-oracle`, ADR 0033, #200).
//!
//! [`Modulator`] takes two signals, a carrier (input 1) and a modulator (input 2), each through a
//! saturating amplifier, and combines them by one of six cross-modulation algorithms (crossfade,
//! fold, diode ring modulation, digital ring modulation, XOR, comparator; neighbors morph into
//! each other) at 6x oversampling, or by a 20-band vocoder. An internal oscillator (three shapes,
//! each one waveform for the algorithms and another for the vocoder) can stand in for the
//! carrier, input 1 then modulating its phase or frequency. The aux output is the two inputs
//! summed (or the internal oscillator).
//!
//! It runs at the firmware's rate only, 96 kHz ([`SAMPLE_RATE`]), in the module's codec blocks of
//! [`BLOCK_SIZE`] frames (any whole number of 12-frame groups up to it renders; the module always
//! renders 60). Host-rate handling is the device's (`transit-dsp`). See [`Modulator::process`] for
//! the render contract.
//!
//! Pure Rust, `#![forbid(unsafe_code)]`, no allocation once built, one random generator per
//! instance (the noise carrier's), and the pinned flush-to-zero choice of the family's oracles
//! reproduced in its own arithmetic ([`float::F`]), never by changing the host's floating-point
//! mode.

#![forbid(unsafe_code)]

mod amplifier;
mod converter;
pub mod float;
mod modulator;
mod oscillator;
mod resources;
pub mod stmlib;
mod vocoder;
mod xmod;

pub use modulator::Modulator;

/// The module's codec block, frames (`codec.Start(60, ...)` in `warps/warps.cc`), and the largest
/// block the port renders.
pub const BLOCK_SIZE: usize = 60;

/// The firmware's sample rate, Hz (`modulator.Init(96000.0f)`): its pitch table, its filter bank
/// and its converters assume it.
pub const SAMPLE_RATE: f32 = 96_000.0;

/// The cross-modulation algorithms' oversampling (`kOversampling`).
pub const OVERSAMPLING: usize = 6;

/// The internal oscillator (`carrier_shape`): off (input 1 is the carrier), or one of three shapes.
/// Each shape is one waveform for the cross-modulation algorithms and another for the vocoder; the
/// module crossfades between the two around the vocoder's position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CarrierShape {
    /// The carrier is input 1 (the module's default).
    #[default]
    External,
    /// A sine for the algorithms, a sawtooth for the vocoder.
    Sine,
    /// A triangle for the algorithms, a pulse for the vocoder.
    Triangle,
    /// A sawtooth for the algorithms, filtered noise for the vocoder.
    Saw,
}

impl CarrierShape {
    /// Every shape, in the firmware's order.
    pub const ALL: [Self; 4] = [Self::External, Self::Sine, Self::Triangle, Self::Saw];

    /// The firmware's index (0 is the external carrier).
    #[must_use]
    pub fn index(self) -> i32 {
        match self {
            Self::External => 0,
            Self::Sine => 1,
            Self::Triangle => 2,
            Self::Saw => 3,
        }
    }
}

/// What the module's CV scaler hands the modulator each block (`Parameters` in
/// `warps/dsp/parameters.h`, the fields the processing path reads).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameters {
    /// The input amplifiers' drive, input 1 then input 2, 0 to 1 (the module: the level knob
    /// squared, with nothing patched in the level CV). Input 1's is unused while the internal
    /// oscillator runs.
    pub drive: [f32; 2],
    /// The algorithm, 0 to 1: crossfade at 0, fold at 1/8, diode ring at 2/8, digital ring at 3/8,
    /// XOR at 4/8, comparator at 5/8, morphing between neighbors in between; from 0.675 a
    /// crossfade through the dry modulator into the vocoder (fully at 0.725), its release growing
    /// from 0.75 to a freeze near 1.
    pub algorithm: f32,
    /// The algorithm's parameter (TIMBRE), 0 to 1; the vocoder's formant shift.
    pub timbre: f32,
    /// The internal oscillator's pitch, MIDI note (the module's knob covers 36 to 96).
    pub note: f32,
    /// The internal oscillator, or the external carrier.
    pub carrier: CarrierShape,
}
