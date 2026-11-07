//! The Macro Oscillator's model content: per model, the short name the panel's picker prints, the
//! description on the model card, and what the three macro controls (Harmonics, Timbre, Morph) do
//! in that model. It is data for the panel and the user guide; no entry changes a parameter or a
//! sound. The wording is transit's own, written from the port's engine documentation in
//! `crates/macro-osc/src/engine*/` (a port of Émilie Gillet's open-source firmware, ADR 0033).

use super::MODELS;

/// The longest short name, in characters.
pub const SHORT_NAME_MAX: usize = 12;
/// The longest description, in characters.
pub const DESCRIPTION_MAX: usize = 70;
/// The longest macro label, in characters.
pub const LABEL_MAX: usize = 14;

/// One model's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// What the picker button prints (unique, at most [`SHORT_NAME_MAX`] characters).
    pub short_name: &'static str,
    /// The full name, the one in [`MODELS`] (ADR 0033), unchanged.
    pub name: &'static str,
    /// One line for the model card (at most [`DESCRIPTION_MAX`] characters).
    pub description: &'static str,
    /// What Harmonics, Timbre and Morph control in this model (each at most [`LABEL_MAX`]).
    pub harmonics: &'static str,
    pub timbre: &'static str,
    pub morph: &'static str,
}

const fn m(
    short_name: &'static str,
    name: &'static str,
    description: &'static str,
    harmonics: &'static str,
    timbre: &'static str,
    morph: &'static str,
) -> ModelInfo {
    ModelInfo { short_name, name, description, harmonics, timbre, morph }
}

/// The 24 models in [`MODELS`]' order.
pub const MODEL_INFO: [ModelInfo; 24] = [
    m("VCF", MODELS[0], "Analog waves through a resonant low-pass filter", "Resonance", "Cutoff", "Wave and sub"),
    m("Phase", MODELS[1], "A sine bent by an asymmetric triangle modulator", "Bend pitch", "Bend amount", "Skew"),
    m("6-op 1", MODELS[2], "Six-operator FM, first bank of 32 patches", "Patch", "Mod level", "Env stretch"),
    m("6-op 2", MODELS[3], "Six-operator FM, second bank of 32 patches", "Patch", "Mod level", "Env stretch"),
    m("6-op 3", MODELS[4], "Six-operator FM, third bank of 32 patches", "Patch", "Mod level", "Env stretch"),
    m("Terrain", MODELS[5], "A 2D surface read along an elliptical path", "Terrain", "Path radius", "Path offset"),
    m("Strings", MODELS[6], "Divide-down string-machine chords with filter and chorus", "Chord", "Filter, chorus", "Waveform"),
    m("Chip", MODELS[7], "Square-wave chords with an arpeggiator and a triangle voice", "Chord", "Arp, inversion", "Pulse or sync"),
    m("Analog", MODELS[8], "Two classic waves with sync and crossfade", "Detune", "Square shape", "Saw shape"),
    m("Shaper", MODELS[9], "A triangle through a waveshaper and a wavefolder", "Shaper wave", "Fold amount", "Asymmetry"),
    m("FM", MODELS[10], "Two sine operators modulating each other's phase", "Ratio", "FM index", "Feedback"),
    m("Grain", MODELS[11], "A formant oscillator built from windowed sine segments", "Formant ratio", "Formant pitch", "Formant width"),
    m("Add", MODELS[12], "Sine harmonics shaped into a movable spectral bump", "Bump count", "Peak harmonic", "Bump width"),
    m("Table", MODELS[13], "Four banks of 8 by 8 waveforms, scanned by row and column", "Bank", "Row", "Column"),
    m("Chord", MODELS[14], "Four-note chords from analog or wavetable oscillators", "Chord type", "Inversion", "Waveform"),
    m("Voice", MODELS[15], "Vowel and word synthesis in several speech styles", "Speech style", "Formant shift", "Phoneme, word"),
    m("Swarm", MODELS[16], "Eight enveloped saw grains in a drifting cloud", "Pitch scatter", "Density", "Grain length"),
    m("Noise", MODELS[17], "Clocked noise through a resonant filter", "Filter type", "Clock rate", "Resonance"),
    m("Dust", MODELS[18], "Random dust clicks through resonant filter networks", "Pitch scatter", "Density", "Filter type"),
    m("Pluck", MODELS[19], "A struck or plucked string, from tuned to inharmonic", "Material", "Brightness", "Decay"),
    m("Modal", MODELS[20], "A modal resonator struck by noise or dust", "Material", "Brightness", "Decay"),
    m("Kick", MODELS[21], "Analog-style kick: a punchy attack and a tuned body", "Attack, drive", "Brightness", "Decay"),
    m("Snare", MODELS[22], "Analog-style snare: shell modes plus filtered noise", "Noise balance", "Mode balance", "Decay"),
    m("Hi-hat", MODELS[23], "Metallic square oscillators with a noise blend", "Metal or noise", "High-pass", "Decay"),
];

/// The content of `model`, clamped to the last model.
pub fn model_info(model: usize) -> &'static ModelInfo {
    &MODEL_INFO[model.min(MODEL_INFO.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Why `e` breaks the table's rules, if it does.
    fn problem(e: &ModelInfo) -> Option<String> {
        for (what, s, max) in [
            ("short_name", e.short_name, SHORT_NAME_MAX),
            ("description", e.description, DESCRIPTION_MAX),
            ("harmonics", e.harmonics, LABEL_MAX),
            ("timbre", e.timbre, LABEL_MAX),
            ("morph", e.morph, LABEL_MAX),
        ] {
            let n = s.chars().count();
            if n == 0 || n > max {
                return Some(format!("{what} `{s}` is {n} characters (1 to {max})"));
            }
            if s != s.trim() {
                return Some(format!("{what} `{s}` has stray spaces"));
            }
        }
        None
    }

    #[test]
    fn every_model_has_content_within_its_limits() {
        assert_eq!(MODEL_INFO.len(), 24);
        assert_eq!(MODEL_INFO.len(), MODELS.len(), "no model missing");
        for (i, e) in MODEL_INFO.iter().enumerate() {
            assert_eq!(e.name, MODELS[i], "model {i}: the ADR 0033 name, unchanged");
            assert_eq!(problem(e), None, "model {i}");
        }
        let mut short: Vec<_> = MODEL_INFO.iter().map(|e| e.short_name).collect();
        short.sort_unstable();
        short.dedup();
        assert_eq!(short.len(), 24, "short names are unique");
    }

    #[test]
    fn the_check_rejects_entries_over_their_limits() {
        // Negative controls: a 13-character short name, an empty label and a long description.
        let ok = MODEL_INFO[0];
        assert_eq!(problem(&ok), None);
        assert!(problem(&ModelInfo { short_name: "Twelve chars", ..ok }).is_none(), "12 characters is allowed");
        assert!(problem(&ModelInfo { short_name: "Thirteen char", ..ok }).is_some());
        assert!(problem(&ModelInfo { morph: "", ..ok }).is_some());
        assert!(problem(&ModelInfo { description: "x".repeat(71).leak(), ..ok }).is_some());
        assert_eq!(model_info(99).name, MODELS[23], "an index past the end clamps");
    }
}
