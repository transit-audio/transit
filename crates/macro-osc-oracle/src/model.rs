//! The 24 engines, by the index the firmware's voice selects them with.

#![forbid(unsafe_code)]

/// One synthesis engine, in the voice's registration order (`Voice::Init` in
/// `plaits/dsp/voice.cc`). Names describe what each engine does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    /// Virtual analog with a resonant filter (engine2 bank).
    VirtualAnalogFilter,
    /// Phase distortion (engine2 bank).
    PhaseDistortion,
    /// Six-operator FM, first built-in patch bank (engine2 bank).
    SixOpFmBankA,
    /// Six-operator FM, second built-in patch bank (engine2 bank).
    SixOpFmBankB,
    /// Six-operator FM, third built-in patch bank (engine2 bank).
    SixOpFmBankC,
    /// Wave terrain synthesis (engine2 bank).
    WaveTerrain,
    /// String machine with chords and an ensemble effect (engine2 bank).
    StringMachine,
    /// Chiptune square voices with an arpeggiator (engine2 bank).
    Chiptune,
    /// Two virtual analog oscillators.
    VirtualAnalog,
    /// Waveshaping oscillator.
    Waveshaping,
    /// Two-operator FM.
    Fm,
    /// Granular formant oscillator.
    GrainFormant,
    /// Additive harmonic oscillator.
    Additive,
    /// Wavetable oscillator.
    Wavetable,
    /// Chords of divided-down oscillators.
    Chord,
    /// Vowel and speech synthesis.
    Speech,
    /// Swarm of sawtooth oscillators.
    Swarm,
    /// Filtered noise.
    FilteredNoise,
    /// Particle noise through resonant filters.
    ParticleNoise,
    /// Inharmonic string model.
    InharmonicString,
    /// Modal resonator.
    ModalResonator,
    /// Analog bass drum model.
    BassDrum,
    /// Analog snare drum model.
    SnareDrum,
    /// Analog hi-hat model.
    HiHat,
}

impl Model {
    /// Every engine, in index order.
    pub const ALL: [Self; crate::ENGINE_COUNT] = [
        Self::VirtualAnalogFilter,
        Self::PhaseDistortion,
        Self::SixOpFmBankA,
        Self::SixOpFmBankB,
        Self::SixOpFmBankC,
        Self::WaveTerrain,
        Self::StringMachine,
        Self::Chiptune,
        Self::VirtualAnalog,
        Self::Waveshaping,
        Self::Fm,
        Self::GrainFormant,
        Self::Additive,
        Self::Wavetable,
        Self::Chord,
        Self::Speech,
        Self::Swarm,
        Self::FilteredNoise,
        Self::ParticleNoise,
        Self::InharmonicString,
        Self::ModalResonator,
        Self::BassDrum,
        Self::SnareDrum,
        Self::HiHat,
    ];

    /// The engine index the voice selects this engine with (`Patch::engine`).
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    /// The engine at `index`, if any.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// A stable kebab-case label for reports and digests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::VirtualAnalogFilter => "virtual-analog-filter",
            Self::PhaseDistortion => "phase-distortion",
            Self::SixOpFmBankA => "six-op-fm-a",
            Self::SixOpFmBankB => "six-op-fm-b",
            Self::SixOpFmBankC => "six-op-fm-c",
            Self::WaveTerrain => "wave-terrain",
            Self::StringMachine => "string-machine",
            Self::Chiptune => "chiptune",
            Self::VirtualAnalog => "virtual-analog",
            Self::Waveshaping => "waveshaping",
            Self::Fm => "fm",
            Self::GrainFormant => "grain-formant",
            Self::Additive => "additive",
            Self::Wavetable => "wavetable",
            Self::Chord => "chord",
            Self::Speech => "speech",
            Self::Swarm => "swarm",
            Self::FilteredNoise => "filtered-noise",
            Self::ParticleNoise => "particle-noise",
            Self::InharmonicString => "inharmonic-string",
            Self::ModalResonator => "modal-resonator",
            Self::BassDrum => "bass-drum",
            Self::SnareDrum => "snare-drum",
            Self::HiHat => "hi-hat",
        }
    }
}
