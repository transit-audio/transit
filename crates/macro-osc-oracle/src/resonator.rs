//! The resonator oracle (#198): the firmware's resonator part (`rings::Part`), rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawResonator`]. One [`ResonatorOracle`] is one part: up to four
//! voices of one of six models, the excitation filters, the strummed note filter, the reverb of the
//! string-and-reverb model and the output limiter, at 48 kHz in blocks of up to 24 frames
//! ([`RESONATOR_BLOCK_SIZE`]), float out and aux before the codec's int16 conversion.

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CPerformanceState, CResonatorPatch, RawResonator, Session};

/// The resonator's block size, `kMaxBlockSize` in `rings/dsp/dsp.h`: the codec renders 24 frames
/// per call, and one call is one control block (the note filter and the strum advance per call).
pub const RESONATOR_BLOCK_SIZE: usize = 24;

/// The largest polyphony, `kMaxPolyphony` in `rings/dsp/part.h`.
pub const MAX_POLYPHONY: usize = 4;

/// The number of chords of the quantized sympathetic strings, `kNumChords`.
pub const CHORD_COUNT: i32 = 11;

/// One resonator model, in the firmware's order (`ResonatorModel` in `rings/dsp/part.h`). Names
/// describe what each model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResonatorModel {
    /// A bank of up to 64 band-pass modes (`RESONATOR_MODEL_MODAL`).
    Modal,
    /// A plucked string with sympathetic strings tuned by structure
    /// (`RESONATOR_MODEL_SYMPATHETIC_STRING`).
    SympatheticStrings,
    /// A string with dispersion: stiffness, a curved bridge or noise in its delay
    /// (`RESONATOR_MODEL_STRING`).
    InharmonicString,
    /// A two-operator FM voice with an envelope follower (`RESONATOR_MODEL_FM_VOICE`).
    FmVoice,
    /// Sympathetic strings tuned to a chord from the chord table
    /// (`RESONATOR_MODEL_SYMPATHETIC_STRING_QUANTIZED`).
    QuantizedSympatheticStrings,
    /// The inharmonic string through a reverb (`RESONATOR_MODEL_STRING_AND_REVERB`).
    StringAndReverb,
}

impl ResonatorModel {
    /// Every model, in index order.
    pub const ALL: [Self; 6] =
        [Self::Modal, Self::SympatheticStrings, Self::InharmonicString, Self::FmVoice, Self::QuantizedSympatheticStrings, Self::StringAndReverb];

    /// The firmware's model index.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// The model at `index`, if any.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// A stable kebab-case label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Modal => "modal",
            Self::SympatheticStrings => "sympathetic-strings",
            Self::InharmonicString => "inharmonic-string",
            Self::FmVoice => "fm-voice",
            Self::QuantizedSympatheticStrings => "quantized-sympathetic-strings",
            Self::StringAndReverb => "string-and-reverb",
        }
    }
}

/// The four macros (`Patch` in `rings/dsp/patch.h`), each 0..1 (the module's structure tops out at
/// 0.9995).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResonatorPatch {
    /// Structure: inharmonicity, string stiffness, the sympathetic strings' intervals, the FM ratio.
    pub structure: f32,
    /// Brightness.
    pub brightness: f32,
    /// Damping (decay time).
    pub damping: f32,
    /// Position of the excitation or pickup.
    pub position: f32,
}

impl ResonatorPatch {
    fn to_c(self) -> CResonatorPatch {
        CResonatorPatch { structure: self.structure, brightness: self.brightness, damping: self.damping, position: self.position }
    }
}

/// What the module's CV scaler and strummer hand the part each block (`PerformanceState` in
/// `rings/dsp/performance_state.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerformanceState {
    /// A strum in this block: the active voice rotates and the internal exciter fires.
    pub strum: bool,
    /// Nothing patched in the audio input: the part excites itself (a pulse or a noise burst).
    pub internal_exciter: bool,
    /// Nothing patched in the strum input (read by the strummer, not by the part).
    pub internal_strum: bool,
    /// Nothing patched in the V/OCT input (read by the CV scaler, not by the part).
    pub internal_note: bool,
    /// The frequency knob, semitones (`12 + transpose` on the module).
    pub tonic: f32,
    /// The V/OCT input, semitones over the tonic.
    pub note: f32,
    /// The FM input, semitones (-48 to 48).
    pub fm: f32,
    /// The chord of the quantized sympathetic strings, 0 to [`CHORD_COUNT`] - 1.
    pub chord: i32,
}

impl PerformanceState {
    fn to_c(self) -> CPerformanceState {
        CPerformanceState {
            tonic: self.tonic,
            note: self.note,
            fm: self.fm,
            chord: self.chord,
            strum: u8::from(self.strum),
            internal_exciter: u8::from(self.internal_exciter),
            internal_strum: u8::from(self.internal_strum),
            internal_note: u8::from(self.internal_note),
        }
    }
}

/// One resonator part of the oracle at 48 kHz.
pub struct ResonatorOracle<'s> {
    raw: RawResonator<'s>,
}

impl ResonatorOracle<'_> {
    /// Selects the model, as the panel does; the part re-initializes its resonators on the next
    /// block when the model changes.
    pub fn set_model(&mut self, model: ResonatorModel) {
        // Every `ResonatorModel` index is in range, so the wrapper never refuses it.
        let index = i32::try_from(model.index()).unwrap_or(0);
        let _ = self.raw.set_model(index);
    }

    /// Sets the polyphony (1 to [`MAX_POLYPHONY`]), as the panel does.
    ///
    /// # Errors
    ///
    /// [`Error::BadParameter`] outside 1 to 4.
    pub fn set_polyphony(&mut self, polyphony: usize) -> Result<(), Error> {
        let p = i32::try_from(polyphony).map_err(|_| Error::BadParameter)?;
        self.raw.set_polyphony(p).ok_or(Error::BadParameter)
    }

    /// Bypass: the input is copied to both outputs.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.raw.set_bypass(bypass);
    }

    /// Processes one block of `input.len()` frames (1 to [`RESONATOR_BLOCK_SIZE`]) into `out` and
    /// `aux`.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty or too long or the lengths differ;
    /// [`Error::BadParameter`] when the chord is outside the chord table.
    pub fn process(
        &mut self,
        performance: &PerformanceState,
        patch: &ResonatorPatch,
        input: &[f32],
        out: &mut [f32],
        aux: &mut [f32],
    ) -> Result<(), Error> {
        let n = input.len();
        if n == 0 || n > RESONATOR_BLOCK_SIZE || out.len() != n || aux.len() != n {
            return Err(Error::BadBlock);
        }
        if !(0..CHORD_COUNT).contains(&performance.chord) {
            return Err(Error::BadParameter);
        }
        self.raw.process(&performance.to_c(), &patch.to_c(), input, out, aux).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new resonator part, as the firmware's `Part::Init` leaves it (polyphony 1, the modal
    /// model) on zero-initialized memory. Seed first: the noise bursts and the dispersion draw from
    /// the shared random state.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`].
    pub fn resonator(&self) -> Result<ResonatorOracle<'_>, Error> {
        let raw = RawResonator::new(self).ok_or(Error::OutOfMemory)?;
        Ok(ResonatorOracle { raw })
    }
}
