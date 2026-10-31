//! The Time Stretcher insert's panel (#199): its layout for [`crate::panel`], the help entry of each
//! control, and the credit line under it (ADR 0033).
//!
//! The first row shifts (Pitch, Fine, Window) and colors (Diffusion, Tone, Feedback, Reverb); the
//! second stretches (Position, Freeze, the Trigger button held while pressed), sets the buffer
//! (Quality, Input Level) and the mix.

use crate::help::Id;
use crate::panel::Item::{Choice, Hold, Knob, Toggle};
use crate::panel::{Row, Section};

/// The Time Stretcher's id (#199).
pub const TIME_STRETCHER: &str = "time_stretcher";

/// The credit line under the panel (ADR 0033: credit in the device's help and panel).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source texture processor (MIT)";

const fn sec(title: &'static str, cols: usize, items: &'static [crate::panel::Item]) -> Section {
    Section { title, cols, items }
}

pub static LAYOUT: [Row; 2] = [
    &[
        sec("Shift", 3, &[Knob("pitch", "Pitch"), Knob("fine", "Fine"), Knob("window", "Window")]),
        sec("Color", 4, &[Knob("diffusion", "Diffusion"), Knob("tone", "Tone"), Knob("feedback", "Feedback"), Knob("reverb", "Reverb")]),
    ],
    &[
        sec("Stretch", 3, &[Knob("position", "Position"), Toggle("freeze", "Freeze"), Hold("trigger", "Trigger")]),
        sec("Buffer", 2, &[Choice("quality", "Quality"), Knob("input_level", "Input")]),
        sec("Mix", 1, &[Knob("dry_wet", "Dry/Wet")]),
    ],
];

/// The help entry of a Time Stretcher control.
pub fn help(param: &str) -> Option<Id> {
    Some(match param {
        "pitch" => Id::TimeStretcherPitch,
        "fine" => Id::TimeStretcherFine,
        "window" => Id::TimeStretcherWindow,
        "position" => Id::TimeStretcherPosition,
        "diffusion" => Id::TimeStretcherDiffusion,
        "tone" => Id::TimeStretcherTone,
        "feedback" => Id::TimeStretcherFeedback,
        "dry_wet" => Id::TimeStretcherDryWet,
        "reverb" => Id::TimeStretcherReverb,
        "freeze" => Id::TimeStretcherFreeze,
        "trigger" => Id::TimeStretcherTrigger,
        "quality" => Id::TimeStretcherQuality,
        "input_level" => Id::TimeStretcherInputLevel,
        _ => return None,
    })
}
