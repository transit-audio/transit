//! The Cross Modulator insert's panel (#200): its layout for [`crate::panel`], the help entry of
//! each control, the line naming the algorithm the knob sits at, and the credit line under it
//! (ADR 0033).
//!
//! The first row is the modulation (the algorithm and its timbre, the two inputs' levels); the
//! second the carrier (the sidechain key picker, the carrier mode, the swap, the internal
//! oscillator's shape and frequency) and the output (dry/wet, output level).

use crate::help::Id;
use crate::panel::Item::{Choice, Key, Knob, Toggle};
use crate::panel::{Row, Section};
use transit_dsp::plugins::modulator::{ALGORITHMS, algorithm_name};

/// The Cross Modulator's id (#200).
pub const CROSS_MODULATOR: &str = "cross_modulator";

/// The credit line under the panel (ADR 0033: credit in the device's help and panel).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source meta-modulator (MIT)";

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 2] = [
    &[
        sec("Modulation", 2, &[Knob("algorithm", "Algorithm"), Knob("timbre", "Timbre")]),
        sec("Levels", 2, &[Knob("carrier_level", "Carrier"), Knob("modulator_level", "Modulator")]),
        sec("Output", 2, &[Knob("dry_wet", "Dry/Wet"), Knob("output", "Output")]),
    ],
    &[
        sec("Carrier", 3, &[Key, Choice("carrier", "Carrier"), Toggle("swap", "Swap")]),
        sec("Oscillator", 2, &[Choice("shape", "Shape"), Knob("frequency", "Frequency")]),
    ],
];

/// The line naming where `Algorithm` sits, as the firmware reads it: one algorithm at a whole
/// position below 5, else the two it morphs between and how far; from 5 the comparator morphs
/// toward the dry modulator and, from 5.4 to 5.8, crossfades into the vocoder; from 6 the vocoder's
/// release grows, its bands freezing near 8 (where `r (2 - r)` passes 0.995, `r` the release).
pub fn algorithm_line(value: f32) -> String {
    let v = if value.is_finite() { value.clamp(0.0, 8.0) } else { 0.0 };
    if v < 5.0 {
        let below = v.floor();
        let t = v - below;
        let i = below as usize;
        return match (ALGORITHMS.get(i), ALGORITHMS.get(i + 1)) {
            (Some(a), Some(b)) if t >= 0.005 => format!("{a} → {b} {:.0}%", t * 100.0),
            _ => algorithm_name(v).to_string(),
        };
    }
    if v < 5.8 {
        return if v < 5.005 {
            ALGORITHMS[5].to_string()
        } else {
            format!("{} → {} {:.0}%", ALGORITHMS[5], ALGORITHMS[6], (v - 5.0) / 0.8 * 100.0)
        };
    }
    let r = (v / 2.0 - 3.0).clamp(0.0, 1.0);
    if r * (2.0 - r) > 0.995 {
        ALGORITHMS[8].to_string()
    } else if r < 0.005 {
        ALGORITHMS[6].to_string()
    } else {
        format!("{}, release {:.0}%", ALGORITHMS[6], r * 100.0)
    }
}

/// The help entry of a Cross Modulator control.
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "algorithm" => Id::CrossModulatorAlgorithm,
        "timbre" => Id::CrossModulatorTimbre,
        "carrier_level" => Id::CrossModulatorCarrierLevel,
        "modulator_level" => Id::CrossModulatorModulatorLevel,
        "carrier" => Id::CrossModulatorCarrier,
        "shape" => Id::CrossModulatorShape,
        "frequency" => Id::CrossModulatorFrequency,
        "swap" => Id::CrossModulatorSwap,
        "dry_wet" => Id::CrossModulatorDryWet,
        "output" => Id::CrossModulatorOutput,
        _ => return None,
    })
}
