//! The scales: the firmware's six presets (`preset_scales` in `marbles/settings.cc`: major, minor,
//! pentatonic, pelog, and two ragas) and transit's chromatic scale.

#![forbid(unsafe_code)]

use crate::quantizer::Scale;

/// A scale as a table: its interval and its degrees (voltage within one interval, weight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleTable {
    /// A stable name for menus and reports.
    pub name: &'static str,
    pub base_interval: f32,
    pub degrees: &'static [(f32, u8)],
}

impl ScaleTable {
    /// The quantizer's scale.
    #[must_use]
    pub fn to_scale(&self) -> Scale {
        Scale::new(self.base_interval, self.degrees)
    }
}

/// The firmware's six preset scales, in its order (the module's scale slots 0 to 5).
pub static PRESETS: [ScaleTable; 6] = [
    ScaleTable {
        name: "Major",
        base_interval: 1.0,
        degrees: &[
            (0.0000, 255),
            (0.0833, 16),
            (0.1667, 96),
            (0.2500, 24),
            (0.3333, 128),
            (0.4167, 64),
            (0.5000, 8),
            (0.5833, 192),
            (0.6667, 16),
            (0.7500, 96),
            (0.8333, 24),
            (0.9167, 128),
        ],
    },
    ScaleTable {
        name: "Minor",
        base_interval: 1.0,
        degrees: &[
            (0.0000, 255),
            (0.0833, 16),
            (0.1667, 96),
            (0.2500, 128),
            (0.3333, 8),
            (0.4167, 64),
            (0.5000, 4),
            (0.5833, 192),
            (0.6667, 96),
            (0.7500, 16),
            (0.8333, 128),
            (0.9167, 16),
        ],
    },
    ScaleTable {
        name: "Pentatonic",
        base_interval: 1.0,
        degrees: &[
            (0.0000, 255),
            (0.0833, 4),
            (0.1667, 96),
            (0.2500, 4),
            (0.3333, 4),
            (0.4167, 140),
            (0.5000, 4),
            (0.5833, 192),
            (0.6667, 4),
            (0.7500, 96),
            (0.8333, 4),
            (0.9167, 4),
        ],
    },
    ScaleTable {
        name: "Pelog",
        base_interval: 1.0,
        degrees: &[(0.0000, 255), (0.1275, 128), (0.2625, 32), (0.4600, 8), (0.5883, 192), (0.7067, 64), (0.8817, 16)],
    },
    ScaleTable {
        name: "Bhairav",
        base_interval: 1.0,
        degrees: &[
            (0.0000, 255),
            (0.0752, 128),
            (0.1699, 4),
            (0.2630, 4),
            (0.3219, 128),
            (0.4150, 64),
            (0.4918, 4),
            (0.5850, 192),
            (0.6601, 64),
            (0.7549, 4),
            (0.8479, 4),
            (0.9069, 64),
        ],
    },
    ScaleTable {
        name: "Shri",
        base_interval: 1.0,
        degrees: &[
            (0.0000, 255),
            (0.0752, 4),
            (0.1699, 128),
            (0.2630, 64),
            (0.3219, 4),
            (0.4150, 128),
            (0.4918, 4),
            (0.5850, 192),
            (0.6601, 4),
            (0.7549, 64),
            (0.8479, 128),
            (0.9069, 4),
        ],
    },
];

/// transit's chromatic scale: the twelve semitones at equal weight, so every steps level keeps them
/// all (Steps then only chooses between quantized and free pitches).
pub static CHROMATIC: ScaleTable = ScaleTable {
    name: "Chromatic",
    base_interval: 1.0,
    degrees: &[
        (0.0 / 12.0, 255),
        (1.0 / 12.0, 255),
        (2.0 / 12.0, 255),
        (3.0 / 12.0, 255),
        (4.0 / 12.0, 255),
        (5.0 / 12.0, 255),
        (6.0 / 12.0, 255),
        (7.0 / 12.0, 255),
        (8.0 / 12.0, 255),
        (9.0 / 12.0, 255),
        (10.0 / 12.0, 255),
        (11.0 / 12.0, 255),
    ],
};
