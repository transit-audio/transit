//! The Texture insert's panel (#241): its layout for [`crate::panel`], the help entry of each
//! control, and the credit line under it (ADR 0033).
//!
//! The grains' four knobs sit over their random amounts (the module's attenuverters); Density,
//! Feedback, Dry/Wet and Reverb beside them. The second row plays it (Freeze, the Seed button held
//! while pressed, its latch, the trigger gate and whether it is patched), sets the buffer (quality,
//! length, mono input and mono grains) and the input stage.

use crate::help::Id;
use crate::panel::Item::{Choice, Hold, Knob, Toggle};
use crate::panel::{Row, Section};

/// The Texture's id (#241).
pub const TEXTURE: &str = "texture";

/// The credit line under the panel (ADR 0033: credit in the device's help and panel).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source texture synthesizer (MIT)";

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 2] = [
    &[
        sec(
            "Grains",
            4,
            &[
                Knob("time", "Time"),
                Knob("size", "Size"),
                Knob("shape", "Shape"),
                Knob("pitch", "Pitch"),
                Knob("time_random", "Random"),
                Knob("size_random", "Random"),
                Knob("shape_random", "Random"),
                Knob("pitch_random", "Random"),
            ],
        ),
        sec("Mix", 2, &[Knob("density", "Density"), Knob("feedback", "Feedback"), Knob("dry_wet", "Dry/Wet"), Knob("reverb", "Reverb")]),
    ],
    &[
        sec(
            "Play",
            3,
            &[
                Toggle("freeze", "Freeze"),
                Hold("seed", "Seed"),
                Toggle("seed_latch", "Latch"),
                Toggle("trigger", "Trigger"),
                Toggle("trigger_input", "Trig In"),
            ],
        ),
        sec(
            "Buffer",
            2,
            &[Choice("quality", "Quality"), Choice("buffer", "Length"), Toggle("mono_input", "Mono In"), Toggle("mono_grains", "Mono Grains")],
        ),
        sec("Input", 2, &[Choice("input_gain", "Gain"), Knob("input_level", "Level")]),
    ],
];

/// The help entry of a Texture control.
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "time" => Id::TextureTime,
        "size" => Id::TextureSize,
        "shape" => Id::TextureShape,
        "pitch" => Id::TexturePitch,
        "time_random" | "size_random" | "shape_random" | "pitch_random" => Id::TextureRandom,
        "density" => Id::TextureDensity,
        "feedback" => Id::TextureFeedback,
        "dry_wet" => Id::TextureDryWet,
        "reverb" => Id::TextureReverb,
        "freeze" => Id::TextureFreeze,
        "seed" => Id::TextureSeed,
        "seed_latch" => Id::TextureSeedLatch,
        "trigger" => Id::TextureTrigger,
        "trigger_input" => Id::TextureTriggerInput,
        "quality" => Id::TextureQuality,
        "buffer" => Id::TextureBuffer,
        "mono_input" => Id::TextureMonoInput,
        "mono_grains" => Id::TextureMonoGrains,
        "input_gain" => Id::TextureInputGain,
        "input_level" => Id::TextureInputLevel,
        _ => return None,
    })
}
