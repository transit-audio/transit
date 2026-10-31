//! The Resonator instrument's panel (#198 slice 3): its layout for [`crate::panel`], the help
//! entry of each control, the lines under it (how strums work, the rate and latency) and the
//! credit line (ADR 0033).
//!
//! The first row is the module (the model, its four macros, the polyphony); the second the
//! instrument's own controls (accent and tonic, tune and bend, spread and level); then, behind the
//! line that labels them as transit's addition, the Macro Oscillator's shaper and matrix rows, the
//! same sections under the same ids.

use crate::help::Id;
use crate::panel::Item::{Choice, Knob, Menu};
use crate::panel::{MATRIX_ROW, Row, SHAPER_ROW, Section};

/// The Resonator's id (#198).
pub const RESONATOR: &str = "resonator";

/// The credit line under the panel (ADR 0033: credit in the instrument's help and panel).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source resonator (MIT)";

/// The line on how strums work.
pub const STRUMS: &str = "Each note strikes the next voice; note-offs and the pedal leave it ringing ·";

/// The line on the rate.
pub const RATE: &str = "Runs at 48 kHz in 24-frame blocks, converted at other session rates";

/// The row where transit's addition starts (the line above it labels it and shows or hides it).
pub const ADDITION_ROW: usize = 2;

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 4] = [
    &[
        sec("Model", 1, &[Menu("model", "")]),
        sec("Resonator", 4, &[Knob("structure", "Structure"), Knob("brightness", "Brightness"), Knob("damping", "Damping"), Knob("position", "Position")]),
        sec("Voices", 1, &[Choice("polyphony", "Polyphony")]),
    ],
    &[
        sec("Strike", 2, &[Knob("accent", "Accent"), Choice("tonic", "Tonic")]),
        sec("Pitch", 2, &[Knob("tune", "Tune"), Knob("bend_range", "Bend")]),
        sec("Output", 2, &[Knob("spread", "Spread"), Knob("level", "Level")]),
    ],
    SHAPER_ROW,
    MATRIX_ROW,
];

/// The help entry of a Resonator control: its own for the module's controls and for the shaper's
/// and matrix's entries that say something different here, the Macro Oscillator's for the rest
/// (the same controls doing the same thing).
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "model" => Id::ResonatorModel,
        "polyphony" => Id::ResonatorPolyphony,
        "structure" => Id::ResonatorStructure,
        "brightness" => Id::ResonatorBrightness,
        "damping" => Id::ResonatorDamping,
        "position" => Id::ResonatorPosition,
        "accent" => Id::ResonatorAccent,
        "tonic" => Id::ResonatorTonic,
        "spread" => Id::ResonatorSpread,
        "level" => Id::ResonatorLevel,
        "tune" => Id::ResonatorTune,
        "bend_range" => Id::ResonatorBendRange,
        "shaper" => Id::ResonatorShaper,
        "amp_attack" | "amp_decay" | "amp_sustain" | "amp_release" => Id::ResonatorAmpAdsr,
        "amp_velocity" => Id::MacroOscAmpVelocity,
        "filter_mode" => Id::ResonatorFilterMode,
        "filter_slope" => Id::MacroOscFilterSlope,
        "cutoff" => Id::MacroOscCutoff,
        "resonance" => Id::MacroOscResonance,
        "drive" => Id::MacroOscDrive,
        "key_track" => Id::MacroOscKeyTrack,
        "filter_env_amount" => Id::MacroOscFilterEnvAmount,
        "filter_attack" | "filter_decay" | "filter_sustain" | "filter_release" => Id::ResonatorFilterAdsr,
        _ => return numbered_help(param),
    })
}

/// The help entry of an LFO control (`lfo<n>_<field>`) or a matrix row's (`mod<n>_<field>`).
fn numbered_help(param: &str) -> Option<Id> {
    let (head, field) = param.split_once('_')?;
    if head.strip_prefix("lfo").is_some_and(|n| n == "1" || n == "2") {
        return Some(match field {
            "shape" => Id::ResonatorLfoShape,
            "rate" => Id::MacroOscLfoRate,
            "sync" => Id::MacroOscLfoSync,
            "division" => Id::MacroOscLfoDivision,
            "retrigger" => Id::MacroOscLfoRetrigger,
            "fade" => Id::MacroOscLfoFade,
            "amount" => Id::MacroOscLfoAmount,
            _ => return None,
        });
    }
    head.strip_prefix("mod")?.parse::<usize>().ok().filter(|n| (1..=16).contains(n))?;
    Some(match field {
        "source" => Id::MacroOscMatrixSource,
        "via" => Id::MacroOscMatrixVia,
        "dest" => Id::ResonatorMatrixDest,
        "amount" => Id::MacroOscMatrixAmount,
        "curve" => Id::MacroOscMatrixCurve,
        _ => return None,
    })
}
