//! The meta-modulator oracle (#200): the firmware's `warps::Modulator`, rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawModulator`]. One [`ModulatorOracle`] is one modulator: two
//! saturating input amplifiers, the internal oscillator, the six cross-modulation algorithms at 6x
//! oversampling and the 20-band vocoder, at 96 kHz ([`MODULATOR_SAMPLE_RATE`]) in the module's
//! 60-frame codec blocks ([`MODULATOR_BLOCK_SIZE`]), stereo int16 in and out as the codec delivers
//! them. Input 1 (left) is the carrier, input 2 (right) the modulator; the main output is on the
//! left, the aux output on the right. The frequency shifter (the module's hidden mode) is not
//! wired.

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CModulatorParameters, RawModulator, Session};

/// The module's codec block, frames (`codec.Start(60, ...)` in `warps/warps.cc`). The oracle takes
/// whole groups of 12 frames up to this (the vocoder's decimators need groups of 12).
pub const MODULATOR_BLOCK_SIZE: usize = 60;

/// The module's sample rate, Hz (`modulator.Init(96000.0f)` in `warps/warps.cc`).
pub const MODULATOR_SAMPLE_RATE: f32 = 96_000.0;

/// The firmware's oversampling of the cross-modulation algorithms (`kOversampling`).
pub const MODULATOR_OVERSAMPLING: usize = 6;

/// The internal oscillator (`carrier_shape` in `warps/dsp/parameters.h`): off (input 1 is the
/// carrier), or one of three shapes. Each shape is one waveform for the cross-modulation
/// algorithms and another for the vocoder (the module crossfades between the two around the
/// vocoder's position).
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

    /// A stable label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::External => "ext",
            Self::Sine => "sine",
            Self::Triangle => "tri",
            Self::Saw => "saw",
        }
    }
}

/// What the module's CV scaler hands the modulator each block (`Parameters` in
/// `warps/dsp/parameters.h`, its main-path fields).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameters {
    /// The input amplifiers' drive, input 1 then input 2, 0 to 1 (the module: the level knob
    /// squared, with nothing patched in the level CV).
    pub drive: [f32; 2],
    /// The algorithm, 0 to 1: crossfade at 0, fold at 1/8, diode ring at 2/8, digital ring at
    /// 3/8, XOR at 4/8, comparator at 5/8, morphing between neighbors in between, then the
    /// vocoder from 0.7, its release growing to a freeze at 1.
    pub algorithm: f32,
    /// The algorithm's parameter (the TIMBRE knob), 0 to 1.
    pub timbre: f32,
    /// The internal oscillator's pitch, MIDI note (0 to 127; the module's knob covers 36 to 96).
    pub note: f32,
    /// The internal oscillator, or the external carrier.
    pub carrier: CarrierShape,
}

impl Parameters {
    pub(crate) fn to_c(self) -> CModulatorParameters {
        CModulatorParameters {
            channel_drive: self.drive,
            modulation_algorithm: self.algorithm,
            modulation_parameter: self.timbre,
            note: self.note,
            carrier_shape: self.carrier.index(),
        }
    }
}

/// Whether the C++ can take these parameters (invariant 4 of the FFI; its wrapper checks the same):
/// drives, algorithm and timbre within 0 to 1 (the module's range, which the algorithm table
/// index, the crossfade tables and the vocoder's band tables assume), the note within 0 to 127
/// (its pitch table index is clipped anyway; the bound keeps the float-to-int conversion
/// defined), every value finite.
#[must_use]
pub fn parameters_in_range(parameters: &Parameters) -> bool {
    let unit = |x: f32| (0.0..=1.0).contains(&x);
    parameters.drive.iter().all(|d| unit(*d)) && unit(parameters.algorithm) && unit(parameters.timbre) && (0.0..=127.0).contains(&parameters.note)
}

/// One modulator of the oracle at 96 kHz.
pub struct ModulatorOracle<'s> {
    raw: RawModulator<'s>,
}

impl ModulatorOracle<'_> {
    /// Whether `process` runs inside the flush-to-zero scope (the default, as every oracle of the
    /// family). Off, the C++ runs in the default floating-point mode: an evidence switch for the
    /// test that asks whether the flush moves an output sample.
    pub fn set_flush(&mut self, flush: bool) {
        self.raw.set_flush(flush);
    }

    /// Processes one block of `input.len()` stereo frames (a multiple of 12 from 12 to
    /// [`MODULATOR_BLOCK_SIZE`]) into `out`.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty, too long, not a multiple of 12 or the lengths
    /// differ; [`Error::BadParameter`] when a parameter is out of range ([`parameters_in_range`]).
    pub fn process(&mut self, parameters: &Parameters, input: &[[i16; 2]], out: &mut [[i16; 2]]) -> Result<(), Error> {
        let n = input.len();
        if n == 0 || n > MODULATOR_BLOCK_SIZE || !n.is_multiple_of(12) || out.len() != n {
            return Err(Error::BadBlock);
        }
        if !parameters_in_range(parameters) {
            return Err(Error::BadParameter);
        }
        self.raw.process(&parameters.to_c(), input, out).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new modulator as the firmware's `Init(96000)` leaves it. Seed first: the internal
    /// oscillator's noise draws from the shared random state.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`] when the allocation fails.
    pub fn modulator(&self) -> Result<ModulatorOracle<'_>, Error> {
        let raw = RawModulator::new(self).ok_or(Error::OutOfMemory)?;
        Ok(ModulatorOracle { raw })
    }
}
