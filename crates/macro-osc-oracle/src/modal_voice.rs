//! The modal voice oracle (#198 slice 2): the firmware's modal voice part (`elements::Part`),
//! rendered from Rust.
//!
//! Safe wrappers over [`crate::ffi::RawModalVoice`]. One [`ModalVoiceOracle`] is one part: the gate
//! envelope, the three exciters (bow, blow, strike), one of three resonator models (or the
//! easter-egg voice), the stereo spread and the reverb, at 32 kHz in blocks of up to 16 frames
//! ([`MODAL_VOICE_BLOCK_SIZE`]), float main and aux before the codec's int16 conversion.

#![forbid(unsafe_code)]

use crate::Error;
use crate::ffi::{CModalVoicePatch, CModalVoicePerformance, RawModalVoice, Session};

/// The modal voice's block size, `kMaxBlockSize` in `elements/dsp/dsp.h`: the codec renders 16
/// frames per call, and one call is one control block (the envelope and the exciters' gate edges
/// advance per call).
pub const MODAL_VOICE_BLOCK_SIZE: usize = 16;

/// The modal voice's only sample rate, `kSampleRate` in `elements/dsp/dsp.h`.
pub const MODAL_VOICE_SAMPLE_RATE: f32 = 32_000.0;

/// The largest value of every panel control but the strike timbre and the space: the module's CV
/// scaler caps them at 0.9995 (`elements/cv_scaler.cc`), and its pots never reach 1.0. The cap
/// keeps `Interpolate(lut_stiffness, geometry, 256)`, `Interpolate(lut_fm_frequency_quantizer, meta,
/// 128)` and `Interpolate(lut_detune_quantizer, bow_level, 64)` inside their tables.
pub const MAX_CONTROL: f32 = 0.9995;

/// The largest strike timbre: the CV scaler's cap (0.995).
pub const MAX_STRIKE_TIMBRE: f32 = 0.995;

/// The largest space: the CV scaler's cap (2.0; from 1.75 the reverb freezes).
pub const MAX_SPACE: f32 = 2.0;

/// The note (`PerformanceState::note`, MIDI semitones) the oracle accepts. The part converts
/// `(note + modulation + 48) * 256` to an integer and clamps it to its pitch table, and the
/// easter-egg voice does the same with the note plus at most 270 semitones of filter offset, so
/// any finite value in this range is defined.
pub const NOTE_RANGE: std::ops::RangeInclusive<f32> = -48.0..=160.0;

/// The modulation (the FM input, semitones) the oracle accepts: the CV scaler's range.
pub const MODULATION_RANGE: std::ops::RangeInclusive<f32> = -60.0..=60.0;

/// Whether the C++ can take these inputs without reading past a table or converting an
/// out-of-range float to an integer (invariant 4 of the FFI).
///
/// Every panel control indexes a table at `value * size` and the entry after it, so each stays
/// within the CV scaler's range ([`MAX_CONTROL`], [`MAX_STRIKE_TIMBRE`], [`MAX_SPACE`]); the
/// strength indexes `lut_accent_gain_coarse` at `strength * 256` (257 entries), so it stays within
/// 0 to 1; the note and modulation stay in [`NOTE_RANGE`] and [`MODULATION_RANGE`]; the audio inputs
/// stay within -1 to 1 (the codec's range: they reach the easter-egg voice's phase modulation as
/// `fm * 2^31` converted to an integer). Every value must be finite: a NaN reaching a float-to-int
/// conversion is undefined behavior in C++.
#[must_use]
pub fn inputs_in_range(performance: &Performance, patch: &ModalVoicePatch, blow: &[f32], strike: &[f32]) -> bool {
    let control = |x: f32| (0.0..=MAX_CONTROL).contains(&x);
    let audio = |x: &f32| (-1.0..=1.0).contains(x);
    NOTE_RANGE.contains(&performance.note)
        && MODULATION_RANGE.contains(&performance.modulation)
        && (0.0..=1.0).contains(&performance.strength)
        && [
            patch.envelope_shape,
            patch.bow_level,
            patch.bow_timbre,
            patch.blow_level,
            patch.blow_meta,
            patch.blow_timbre,
            patch.strike_level,
            patch.strike_meta,
            patch.geometry,
            patch.brightness,
            patch.damping,
            patch.position,
        ]
        .into_iter()
        .all(control)
        && (0.0..=MAX_STRIKE_TIMBRE).contains(&patch.strike_timbre)
        && (0.0..=MAX_SPACE).contains(&patch.space)
        && blow.iter().all(audio)
        && strike.iter().all(audio)
}

/// One resonator model, in the firmware's order (`ResonatorModel` in `elements/dsp/voice.h`).
/// Names describe what each model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModalVoiceModel {
    /// A bank of 52 band-pass modes with eight bowed waveguides (`RESONATOR_MODEL_MODAL`).
    Modal,
    /// One string with dispersion (`RESONATOR_MODEL_STRING`).
    String,
    /// Five strings tuned to a chord picked by the geometry (`RESONATOR_MODEL_STRINGS`).
    Strings,
}

impl ModalVoiceModel {
    /// Every model, in index order.
    pub const ALL: [Self; 3] = [Self::Modal, Self::String, Self::Strings];

    /// The firmware's model index.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// A stable kebab-case label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Modal => "modal",
            Self::String => "string",
            Self::Strings => "strings",
        }
    }
}

/// The panel controls (the fields of `Patch` in `elements/dsp/patch.h` the CV scaler writes every
/// block). The other five fields (the side channel's modulation, the reverb's diffusion and
/// low-pass, the exciter signature) are the part's own, set by `Init` and [`ModalVoiceOracle::seed`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModalVoicePatch {
    /// The gate envelope's shape: short decay, sustain, long attack and release (0 to 1).
    pub envelope_shape: f32,
    /// The bow's level.
    pub bow_level: f32,
    /// The bow's timbre (its noise filter).
    pub bow_timbre: f32,
    /// The blow's level (past two thirds, the tube model joins in).
    pub blow_level: f32,
    /// The blow's meta control: where the granular noise sample restarts.
    pub blow_meta: f32,
    /// The blow's timbre: the noise sample's playback rate.
    pub blow_timbre: f32,
    /// The strike's level (past 80%, the raw strike bleeds into the output).
    pub strike_level: f32,
    /// The strike's meta control: the sample player, the mallet, the plectrum, the particles.
    pub strike_meta: f32,
    /// The strike's timbre (its filter or playback rate).
    pub strike_timbre: f32,
    /// The resonator's geometry (inharmonicity; the strings' dispersion or chord).
    pub geometry: f32,
    /// The resonator's brightness.
    pub brightness: f32,
    /// The resonator's damping.
    pub damping: f32,
    /// The excitation or pickup position.
    pub position: f32,
    /// Space: the dry level, the stereo spread and the reverb (0 to 2; frozen from 1.75).
    pub space: f32,
}

impl Default for ModalVoicePatch {
    /// The values `Part::Init` writes (`elements/dsp/part.cc`), the envelope shape capped at
    /// [`MAX_CONTROL`] (Init writes 1.0, which the CV scaler overwrites before the first block).
    fn default() -> Self {
        Self {
            envelope_shape: MAX_CONTROL,
            bow_level: 0.0,
            bow_timbre: 0.5,
            blow_level: 0.0,
            blow_meta: 0.5,
            blow_timbre: 0.5,
            strike_level: 0.8,
            strike_meta: 0.5,
            strike_timbre: 0.5,
            geometry: 0.2,
            brightness: 0.5,
            damping: 0.25,
            position: 0.3,
            space: 0.5,
        }
    }
}

impl ModalVoicePatch {
    pub(crate) fn to_c(self) -> CModalVoicePatch {
        CModalVoicePatch {
            exciter_envelope_shape: self.envelope_shape,
            exciter_bow_level: self.bow_level,
            exciter_bow_timbre: self.bow_timbre,
            exciter_blow_level: self.blow_level,
            exciter_blow_meta: self.blow_meta,
            exciter_blow_timbre: self.blow_timbre,
            exciter_strike_level: self.strike_level,
            exciter_strike_meta: self.strike_meta,
            exciter_strike_timbre: self.strike_timbre,
            resonator_geometry: self.geometry,
            resonator_brightness: self.brightness,
            resonator_damping: self.damping,
            resonator_position: self.position,
            space: self.space,
        }
    }
}

/// What the module's CV scaler hands the part each block (`PerformanceState` in
/// `elements/dsp/part.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Performance {
    /// The gate (the gate input or the panel's play button).
    pub gate: bool,
    /// The note, MIDI semitones (the V/OCT input plus the coarse and fine knobs).
    pub note: f32,
    /// The FM input, semitones.
    pub modulation: f32,
    /// The strength input, 0 to 1 (`1 - pressure CV` on the module).
    pub strength: f32,
}

impl Performance {
    pub(crate) fn to_c(self) -> CModalVoicePerformance {
        CModalVoicePerformance { note: self.note, modulation: self.modulation, strength: self.strength, gate: u8::from(self.gate) }
    }
}

/// One modal voice part of the oracle at 32 kHz.
pub struct ModalVoiceOracle<'s> {
    raw: RawModalVoice<'s>,
}

impl ModalVoiceOracle<'_> {
    /// `Part::Seed` with three words (the module seeds it with its chip's serial number): the side
    /// channel's modulation rate and offset, the reverb's diffusion and low-pass and the exciter
    /// signature.
    pub fn seed(&mut self, words: [u32; 3]) {
        self.raw.seed(words);
    }

    /// Selects the resonator model, as the calibration menu does.
    pub fn set_model(&mut self, model: ModalVoiceModel) {
        // Every model index is in range, so the wrapper never refuses it.
        let index = i32::try_from(model.index()).unwrap_or(0);
        let _ = self.raw.set_model(index);
    }

    /// The easter-egg voice (two FM oscillators instead of the exciters and the resonator).
    pub fn set_easter_egg(&mut self, easter_egg: bool) {
        self.raw.set_easter_egg(easter_egg);
    }

    /// Bypass: the strike input goes to main, the blow input to aux.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.raw.set_bypass(bypass);
    }

    /// Processes one block of `blow.len()` frames (1 to [`MODAL_VOICE_BLOCK_SIZE`]) into `main` and
    /// `aux`.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty or too long or the lengths differ;
    /// [`Error::BadParameter`] when an input is outside what the C++ can take ([`inputs_in_range`]).
    pub fn process(
        &mut self,
        performance: &Performance,
        patch: &ModalVoicePatch,
        blow: &[f32],
        strike: &[f32],
        main: &mut [f32],
        aux: &mut [f32],
    ) -> Result<(), Error> {
        let n = blow.len();
        if n == 0 || n > MODAL_VOICE_BLOCK_SIZE || strike.len() != n || main.len() != n || aux.len() != n {
            return Err(Error::BadBlock);
        }
        if !inputs_in_range(performance, patch, blow, strike) {
            return Err(Error::BadParameter);
        }
        self.raw.process(performance, patch, blow, strike, main, aux).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new modal voice part, as the firmware's `Part::Init` leaves it (the modal model, unseeded)
    /// on zero-initialized memory. Seed the session first: the exciters' noise and the strings'
    /// dispersion draw from the shared random state.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`].
    pub fn modal_voice(&self) -> Result<ModalVoiceOracle<'_>, Error> {
        let raw = RawModalVoice::new(self).ok_or(Error::OutOfMemory)?;
        Ok(ModalVoiceOracle { raw })
    }
}
