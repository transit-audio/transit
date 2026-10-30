//! Safe wrappers over the raw handles in `ffi`.

#![forbid(unsafe_code)]

use crate::Model;
use crate::ffi::{CEngineParameters, CModulations, CPatch, RawEngine, RawVoice, Session};

/// Why an oracle call did not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The firmware runs at 48 kHz only.
    UnsupportedSampleRate,
    /// The C++ allocation failed.
    OutOfMemory,
    /// A render asked for no frames or more than [`crate::MAX_BLOCK_SIZE`], or out and aux differ in length.
    BadBlock,
    /// A resonator setting outside the firmware's range (a polyphony outside 1 to 4, a chord
    /// outside the chord table).
    BadParameter,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnsupportedSampleRate => "the oracle runs at 48 kHz only",
            Self::OutOfMemory => "the oracle could not allocate a voice or engine",
            Self::BadBlock => "empty block or block longer than the maximum block size, or out and aux lengths differ",
            Self::BadParameter => "a resonator setting outside the firmware's range",
        })
    }
}

impl std::error::Error for Error {}

/// The voice's front-panel settings (`Patch` in `plaits/dsp/voice.h`), all normalized to 0..1
/// except `note` (MIDI semitones).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    /// Pitch in MIDI semitones.
    pub note: f32,
    /// Harmonics macro, 0..1.
    pub harmonics: f32,
    /// Timbre macro, 0..1.
    pub timbre: f32,
    /// Morph macro, 0..1.
    pub morph: f32,
    /// Attenuverter of the frequency modulation input, -1..1.
    pub frequency_modulation_amount: f32,
    /// Attenuverter of the timbre modulation input, -1..1.
    pub timbre_modulation_amount: f32,
    /// Attenuverter of the morph modulation input, -1..1.
    pub morph_modulation_amount: f32,
    /// The engine the voice runs.
    pub model: Model,
    /// Decay of the internal envelope and the low-pass gate, 0..1.
    pub decay: f32,
    /// Low-pass gate color, from VCA-like (0) to VCF-like (1).
    pub lpg_color: f32,
}

impl Patch {
    /// Every macro at 0.5, no modulation, note 60, the given engine.
    #[must_use]
    pub fn centered(model: Model) -> Self {
        Self {
            note: 60.0,
            harmonics: 0.5,
            timbre: 0.5,
            morph: 0.5,
            frequency_modulation_amount: 0.0,
            timbre_modulation_amount: 0.0,
            morph_modulation_amount: 0.0,
            model,
            decay: 0.5,
            lpg_color: 0.5,
        }
    }

    fn to_c(self) -> CPatch {
        CPatch {
            note: self.note,
            harmonics: self.harmonics,
            timbre: self.timbre,
            morph: self.morph,
            frequency_modulation_amount: self.frequency_modulation_amount,
            timbre_modulation_amount: self.timbre_modulation_amount,
            morph_modulation_amount: self.morph_modulation_amount,
            engine: i32::try_from(self.model.index()).unwrap_or(0),
            decay: self.decay,
            lpg_colour: self.lpg_color,
        }
    }
}

/// The voice's CV inputs (`Modulations` in `plaits/dsp/voice.h`). A `*_patched` flag says a cable
/// is in the jack; an unpatched input falls back to the internal envelope or a default.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Modulations {
    /// Engine selection offset.
    pub engine: f32,
    /// Pitch offset in semitones.
    pub note: f32,
    /// Frequency modulation input.
    pub frequency: f32,
    /// Harmonics offset.
    pub harmonics: f32,
    /// Timbre modulation input.
    pub timbre: f32,
    /// Morph modulation input.
    pub morph: f32,
    /// Trigger input (a rising edge above 0.3 fires; below 0.1 re-arms).
    pub trigger: f32,
    /// Level input (drives the low-pass gate when patched).
    pub level: f32,
    /// A cable is in the frequency input.
    pub frequency_patched: bool,
    /// A cable is in the timbre input.
    pub timbre_patched: bool,
    /// A cable is in the morph input.
    pub morph_patched: bool,
    /// A cable is in the trigger input (enables the internal envelope and the ping LPG).
    pub trigger_patched: bool,
    /// A cable is in the level input.
    pub level_patched: bool,
}

impl Modulations {
    fn to_c(self) -> CModulations {
        CModulations {
            engine: self.engine,
            note: self.note,
            frequency: self.frequency,
            harmonics: self.harmonics,
            timbre: self.timbre,
            morph: self.morph,
            trigger: self.trigger,
            level: self.level,
            frequency_patched: u8::from(self.frequency_patched),
            timbre_patched: u8::from(self.timbre_patched),
            morph_patched: u8::from(self.morph_patched),
            trigger_patched: u8::from(self.trigger_patched),
            level_patched: u8::from(self.level_patched),
        }
    }
}

/// An engine's trigger state for one block (`TriggerState` in `plaits/dsp/engine/engine.h`, a bit
/// set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trigger(pub i32);

impl Trigger {
    /// Gate low.
    pub const LOW: Self = Self(0);
    /// A rising edge in this block.
    pub const RISING_EDGE: Self = Self(1);
    /// No cable in the trigger input: the engine free-runs.
    pub const UNPATCHED: Self = Self(2);
    /// Gate high.
    pub const HIGH: Self = Self(4);

    /// Both bit sets.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One engine's parameters for a block (`EngineParameters` in `plaits/dsp/engine/engine.h`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineParameters {
    /// Trigger state for this block.
    pub trigger: Trigger,
    /// Pitch in MIDI semitones.
    pub note: f32,
    /// Timbre, 0..1.
    pub timbre: f32,
    /// Morph, 0..1.
    pub morph: f32,
    /// Harmonics, 0..1.
    pub harmonics: f32,
    /// Accent, 0..1 (the voice passes 0.8 unless the level input is patched).
    pub accent: f32,
}

impl EngineParameters {
    fn to_c(self) -> CEngineParameters {
        CEngineParameters {
            trigger: self.trigger.0,
            note: self.note,
            timbre: self.timbre,
            morph: self.morph,
            harmonics: self.harmonics,
            accent: self.accent,
        }
    }
}

/// One output frame of the voice, as the firmware's DAC sees it (`Voice::Frame`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Main output.
    pub out: i16,
    /// Auxiliary output.
    pub aux: i16,
}

/// A complete voice of the oracle at 48 kHz.
pub struct Voice<'s> {
    raw: RawVoice<'s>,
    scratch: [i16; 2 * crate::MAX_BLOCK_SIZE],
}

impl Voice<'_> {
    /// Renders `frames.len()` frames (1 to [`crate::MAX_BLOCK_SIZE`]).
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty or too long.
    pub fn render(&mut self, patch: &Patch, modulations: &Modulations, frames: &mut [Frame]) -> Result<(), Error> {
        let n = frames.len();
        if n > crate::MAX_BLOCK_SIZE {
            return Err(Error::BadBlock);
        }
        let scratch = &mut self.scratch[..2 * n];
        self.raw.render(&patch.to_c(), &modulations.to_c(), scratch).ok_or(Error::BadBlock)?;
        for (frame, [out, aux]) in frames.iter_mut().zip(scratch.as_chunks::<2>().0) {
            *frame = Frame { out: *out, aux: *aux };
        }
        Ok(())
    }
}

/// One engine of the oracle, rendered on its own (float out and aux, before the voice's gain,
/// limiter and low-pass gate).
pub struct EngineOracle<'s> {
    raw: RawEngine<'s>,
    model: Model,
}

impl EngineOracle<'_> {
    /// The engine this oracle runs.
    #[must_use]
    pub fn model(&self) -> Model {
        self.model
    }

    /// Renders `out.len()` frames (1 to [`crate::MAX_BLOCK_SIZE`]) into `out` and `aux`, and
    /// returns whether the engine reports its output as already enveloped.
    ///
    /// # Errors
    ///
    /// [`Error::BadBlock`] when the block is empty or too long, or the lengths differ.
    pub fn render(&mut self, parameters: &EngineParameters, out: &mut [f32], aux: &mut [f32]) -> Result<bool, Error> {
        self.raw.render(&parameters.to_c(), out, aux).ok_or(Error::BadBlock)
    }
}

impl Session {
    /// A new voice. Initialization may draw from the random state, so seed first.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedSampleRate`] for any rate but 48 kHz; [`Error::OutOfMemory`].
    pub fn voice(&self, sample_rate: f32) -> Result<Voice<'_>, Error> {
        #[allow(clippy::float_cmp)] // The firmware compares exactly; 48 kHz is the only valid value.
        if sample_rate != crate::SAMPLE_RATE {
            return Err(Error::UnsupportedSampleRate);
        }
        let raw = RawVoice::new(self, sample_rate).ok_or(Error::OutOfMemory)?;
        Ok(Voice { raw, scratch: [0; 2 * crate::MAX_BLOCK_SIZE] })
    }

    /// A new engine, initialized as the voice does when it switches to it. Seed first.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfMemory`].
    pub fn engine(&self, model: Model) -> Result<EngineOracle<'_>, Error> {
        let index = i32::try_from(model.index()).map_err(|_| Error::OutOfMemory)?;
        let raw = RawEngine::new(self, index).ok_or(Error::OutOfMemory)?;
        Ok(EngineOracle { raw, model })
    }
}
