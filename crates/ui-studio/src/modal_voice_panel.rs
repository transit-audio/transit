//! The Modal Voice instrument's panel (#198 slice 2): its layout for [`crate::panel`], the help
//! entry of each control, the lines under it (how the gate works, the rate and latency) and the
//! credit line (ADR 0033).
//!
//! The first row is the module's exciter (the model, the contour, the bow, the blow and the
//! strike); the second its resonator and space, then the instrument's own controls (velocity, tune
//! and bend, level); then, behind the line that labels them as transit's addition, the Macro
//! Oscillator's shaper and matrix rows, the same sections under the same ids.

use crate::help::Id;
use crate::panel::Item::{Knob, Menu};
use crate::panel::{MATRIX_ROW, Row, SHAPER_ROW, Section};

/// The Modal Voice's id (#198 slice 2).
pub const MODAL_VOICE: &str = "modal_voice";

/// The credit line under the panel (ADR 0033: credit in the instrument's help and panel).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source modal synthesizer (MIT)";

/// The line on how the gate works.
pub const GATE: &str = "One voice: a key or the pedal holds the gate (bow and blow sound, the strike hits on the note) ·";

/// The line on the rate.
pub const RATE: &str = "Runs at 32 kHz in 16-frame blocks, converted to the session rate";

/// The row where transit's addition starts (the line above it labels it and shows or hides it).
pub const ADDITION_ROW: usize = 2;

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 4] = [
    &[
        sec("Model", 1, &[Menu("model", "")]),
        sec("Contour", 1, &[Knob("contour", "Contour")]),
        sec("Bow", 2, &[Knob("bow_level", "Level"), Knob("bow_timbre", "Timbre")]),
        sec("Blow", 3, &[Knob("blow_level", "Level"), Knob("flow", "Flow"), Knob("blow_timbre", "Timbre")]),
        sec("Strike", 3, &[Knob("strike_level", "Level"), Knob("mallet", "Mallet"), Knob("strike_timbre", "Timbre")]),
    ],
    &[
        sec(
            "Resonator",
            4,
            &[Knob("geometry", "Geometry"), Knob("brightness", "Brightness"), Knob("damping", "Damping"), Knob("position", "Position")],
        ),
        sec("Space", 1, &[Knob("space", "Space")]),
        sec("Play", 3, &[Knob("velocity", "Velocity"), Knob("tune", "Tune"), Knob("bend_range", "Bend")]),
        sec("Output", 1, &[Knob("level", "Level")]),
    ],
    SHAPER_ROW,
    MATRIX_ROW,
];

/// The help entry of a Modal Voice control: its own for the module's controls and for the shaper's
/// and matrix's entries that say something different here, the Macro Oscillator's for the rest
/// (the same controls doing the same thing).
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "model" => Id::ModalVoiceModel,
        "contour" => Id::ModalVoiceContour,
        "bow_level" => Id::ModalVoiceBowLevel,
        "bow_timbre" => Id::ModalVoiceBowTimbre,
        "blow_level" => Id::ModalVoiceBlowLevel,
        "flow" => Id::ModalVoiceFlow,
        "blow_timbre" => Id::ModalVoiceBlowTimbre,
        "strike_level" => Id::ModalVoiceStrikeLevel,
        "mallet" => Id::ModalVoiceMallet,
        "strike_timbre" => Id::ModalVoiceStrikeTimbre,
        "geometry" => Id::ModalVoiceGeometry,
        "brightness" => Id::ModalVoiceBrightness,
        "damping" => Id::ModalVoiceDamping,
        "position" => Id::ModalVoicePosition,
        "space" => Id::ModalVoiceSpace,
        "velocity" => Id::ModalVoiceVelocity,
        "level" => Id::ModalVoiceLevel,
        "tune" => Id::ModalVoiceTune,
        "bend_range" => Id::ModalVoiceBendRange,
        "shaper" => Id::ModalVoiceShaper,
        "amp_attack" | "amp_decay" | "amp_sustain" | "amp_release" => Id::ModalVoiceAmpAdsr,
        "amp_velocity" => Id::MacroOscAmpVelocity,
        "filter_mode" => Id::ModalVoiceFilterMode,
        "filter_slope" => Id::MacroOscFilterSlope,
        "cutoff" => Id::MacroOscCutoff,
        "resonance" => Id::MacroOscResonance,
        "drive" => Id::MacroOscDrive,
        "key_track" => Id::MacroOscKeyTrack,
        "filter_env_amount" => Id::MacroOscFilterEnvAmount,
        "filter_attack" | "filter_decay" | "filter_sustain" | "filter_release" => Id::ModalVoiceFilterAdsr,
        _ => return numbered_help(param),
    })
}

/// The help entry of an LFO control (`lfo<n>_<field>`) or a matrix row's (`mod<n>_<field>`).
fn numbered_help(param: &str) -> Option<Id> {
    let (head, field) = param.split_once('_')?;
    if head.strip_prefix("lfo").is_some_and(|n| n == "1" || n == "2") {
        return Some(match field {
            "shape" => Id::ModalVoiceLfoShape,
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
        "dest" => Id::ModalVoiceMatrixDest,
        "amount" => Id::MacroOscMatrixAmount,
        "curve" => Id::MacroOscMatrixCurve,
        _ => return None,
    })
}
