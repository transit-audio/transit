//! The Texture Synth instrument's panel (#241 slice 3): its layout for [`crate::panel`], the help
//! entry of each control, the lines under it (how notes play it, the rate and latency) and the
//! credit line (ADR 0033).
//!
//! The first row is the module in one line (each grain knob beside its random amount, then Density,
//! Bank, Dry/Wet and Reverb), so the expanded window stays within 800 pt; the second plays it (Freeze, the Seed
//! button held while pressed, its latch), sets the quality and the mono grains, and holds the
//! instrument's own controls (velocity, bend, level); then, behind the line that labels them as
//! transit's addition, the Macro Oscillator's shaper and matrix rows, the same sections under the
//! same ids.

use crate::help::Id;
use crate::panel::Item::{Choice, Hold, Knob, Toggle};
use crate::panel::{MATRIX_ROW, Row, SHAPER_ROW, Section};

/// The Texture Synth's id (#241).
pub const TEXTURE_SYNTH: &str = "texture_synth";

/// The credit line under the panel (ADR 0033: credit in the instrument's help and panel).
pub const CREDIT: &str = "A port of the synth in Émilie Gillet's open-source texture synthesizer (MIT)";

/// The line on how notes play it.
pub const NOTES: &str = "One pitch at a time: a key holds the seed gate, grains play out after it ·";

/// The line on the rate.
pub const RATE: &str = "Runs at 48 kHz in 12-frame blocks, converted at other session rates";

/// The row where transit's addition starts (the line above it labels it and shows or hides it).
pub const ADDITION_ROW: usize = 2;

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 4] = [
    &[
        sec(
            "Grains",
            8,
            &[
                Knob("time", "Time"),
                Knob("time_random", "Random"),
                Knob("size", "Size"),
                Knob("size_random", "Random"),
                Knob("shape", "Shape"),
                Knob("shape_random", "Random"),
                Knob("pitch", "Pitch"),
                Knob("pitch_random", "Random"),
            ],
        ),
        sec("Mix", 4, &[Knob("density", "Density"), Knob("feedback", "Bank"), Knob("dry_wet", "Dry/Wet"), Knob("reverb", "Reverb")]),
    ],
    &[
        sec("Play", 3, &[Toggle("freeze", "Freeze"), Hold("seed", "Seed"), Toggle("seed_latch", "Latch")]),
        sec("Synth", 2, &[Choice("quality", "Quality"), Toggle("mono_grains", "Mono Grains")]),
        sec("Output", 3, &[Knob("velocity", "Velocity"), Knob("bend_range", "Bend"), Knob("level", "Level")]),
    ],
    SHAPER_ROW,
    MATRIX_ROW,
];

/// The help entry of a Texture Synth control: its own for the module's controls and for the
/// shaper's and matrix's entries that say something different here, the Macro Oscillator's for the
/// rest (the same controls doing the same thing).
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "time" => Id::TextureSynthTime,
        "size" => Id::TextureSynthSize,
        "shape" => Id::TextureSynthShape,
        "pitch" => Id::TextureSynthPitch,
        "time_random" | "size_random" | "shape_random" | "pitch_random" => Id::TextureSynthRandom,
        "density" => Id::TextureSynthDensity,
        "feedback" => Id::TextureSynthBank,
        "dry_wet" => Id::TextureSynthDryWet,
        "reverb" => Id::TextureSynthReverb,
        "freeze" => Id::TextureSynthFreeze,
        "seed" => Id::TextureSynthSeed,
        "seed_latch" => Id::TextureSynthSeedLatch,
        "quality" => Id::TextureSynthQuality,
        "mono_grains" => Id::TextureSynthMonoGrains,
        "velocity" => Id::TextureSynthVelocity,
        "level" => Id::TextureSynthLevel,
        "bend_range" => Id::TextureSynthBendRange,
        "shaper" => Id::TextureSynthShaper,
        "amp_attack" | "amp_decay" | "amp_sustain" | "amp_release" => Id::TextureSynthAmpAdsr,
        "amp_velocity" => Id::MacroOscAmpVelocity,
        "filter_mode" => Id::TextureSynthFilterMode,
        "filter_slope" => Id::MacroOscFilterSlope,
        "cutoff" => Id::MacroOscCutoff,
        "resonance" => Id::MacroOscResonance,
        "drive" => Id::MacroOscDrive,
        "key_track" => Id::MacroOscKeyTrack,
        "filter_env_amount" => Id::MacroOscFilterEnvAmount,
        "filter_attack" | "filter_decay" | "filter_sustain" | "filter_release" => Id::TextureSynthFilterAdsr,
        _ => return numbered_help(param),
    })
}

/// The help entry of an LFO control (`lfo<n>_<field>`) or a matrix row's (`mod<n>_<field>`).
fn numbered_help(param: &str) -> Option<Id> {
    let (head, field) = param.split_once('_')?;
    if head.strip_prefix("lfo").is_some_and(|n| n == "1" || n == "2") {
        return Some(match field {
            "shape" => Id::TextureSynthLfoShape,
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
        "dest" => Id::TextureSynthMatrixDest,
        "amount" => Id::MacroOscMatrixAmount,
        "curve" => Id::MacroOscMatrixCurve,
        _ => return None,
    })
}
