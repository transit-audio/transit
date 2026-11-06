//! The device UI design language's tokens (`docs/design/language/tokens.json`, ADR 0045) as Rust
//! constants: colors (the dark set is the design; the light set is derived), the arrangement and
//! transport colors the design does not name, type roles, the space scale, layout metrics, radii
//! and the component metrics the widget kit draws with.
//!
//! The JSON file is the source of truth. `tokens_tests.rs` reads it and asserts every value here
//! equals it, and that neither side has a name the other lacks, so the two never drift. Painting
//! code reads colors through [`crate::theme::TOKENS`] (which picks dark or light); this module is
//! the table those sets are built from.

use egui::Color32;

/// An opaque color from `0xRRGGBB`.
pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Declares one constant per color and a `(json name, color)` registry for the drift test.
macro_rules! colors {
    ($registry:ident; $($name:ident = $json:literal $hex:literal;)*) => {
        $(pub const $name: Color32 = hex($hex);)*
        /// Every color of this set by its `tokens.json` name.
        pub const $registry: &[(&str, Color32)] = &[$(($json, $name)),*];
    };
}

/// The design's color table (`color` in tokens.json): the dark theme.
pub mod color {
    use super::*;
    colors! { ALL;
        BG_WINDOW = "bg.window" 0x131518;
        BG_TITLEBAR = "bg.titlebar" 0x16181B;
        SURFACE_PANEL = "surface.panel" 0x1A1D21;
        SURFACE_RAISED = "surface.raised" 0x22262B;
        SURFACE_INSET = "surface.inset" 0x0D0F11;
        SURFACE_SELECTED = "surface.selected" 0x2A2F35;
        KNOB_CAP = "knob.cap" 0x262A30;
        KNOB_CAP_ACTIVE = "knob.capActive" 0x2E333A;
        LINE = "line" 0x2B3036;
        LINE_INSET = "line.inset" 0x23272C;
        LINE_STRONG = "line.strong" 0x383E46;
        GRID = "grid" 0x1C1F23;
        TEXT = "text" 0xE7E9EC;
        TEXT_MUTED = "text.muted" 0xA7ADB6;
        TEXT_FAINT = "text.faint" 0x8C939D;
        INK_GHOST = "ink.ghost" 0x4A5260;
        INK_ARROW = "ink.arrow" 0x585F69;
        SIGNAL = "signal" 0x4C9EFF;
        MOD = "mod" 0xF0A73A;
        METER_OK = "meter.ok" 0x5FCB8E;
        METER_HOT = "meter.hot" 0xFF6B5B;
    }
}

/// Arrangement and transport colors (`arrangement` in tokens.json), dark theme: the tokens the
/// design does not name, re-derived against the graphite ground at the contrast ratios they had
/// on the old ground, and the state colors that alias a design token. The grid lines are the lane
/// tinted toward white (toward black in light) so the gray Grid Over Clips overlay reproduces them.
pub mod arrangement {
    use super::*;
    colors! { OPAQUE;
        LANE = "lane" 0x171A1D;
        LANE_ALT = "lane.alt" 0x1A1E22;
        RULER = "ruler" 0x23282D;
        HEADER = "header" 0x23262C;
        HEADER_SELECTED = "header.selected" 0x333840;
        GRID_BAR = "grid.bar" 0x323537;
        GRID_BEAT = "grid.beat" 0x242729;
        GRID_SUB = "grid.sub" 0x1C1F21;
        LOOP_REGION = "loop" 0xE6AB27;
        CURSOR = "cursor" 0xE1E1E3;
        SELECTION_OUTLINE = "selection.outline" 0xEDEDF0;
        INK_DARK = "ink.dark" 0x131518;
        INK_LIGHT = "ink.light" 0xF6F6F8;
        RECORD = "record" 0xFF6B5B;
        MUTE = "mute" 0xE7E9EC;
        SOLO = "solo" 0x4C9EFF;
        MONITOR = "monitor" 0x5FCB8E;
        FOCUS_RING = "focus.ring" 0x4C9EFF;
        BUTTON_HOVER = "button.hover" 0x2E333A;
    }
    /// Dims what lies outside a range: black at 70/255 (`#00000046`).
    pub const OVERLAY: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 70);
}

/// Fills that the component specs give as literals (toggle option and unavailable fills, the
/// switch knob), by their path in tokens.json; the fader cap is `knob.capActive`.
pub mod component {
    use super::*;
    colors! { ALL;
        TOGGLE_OPTION_FILL = "toggle.on.option.fill" 0x1E2A38;
        TOGGLE_UNAVAILABLE_FILL = "toggle.unavailable.fill" 0x1E2125;
        SWITCH_ON_KNOB = "switch.onKnob" 0xFFFFFF;
    }
}

/// The light theme (`colorLight` in tokens.json): grounds mirror the dark grounds (each keeps its
/// contrast ratio to `bg.window`, in the opposite direction, with `surface.inset` white); inks keep
/// their hue and at least their dark contrast against `bg.window`.
pub mod color_light {
    use super::*;
    colors! { ALL;
        BG_WINDOW = "bg.window" 0xF9FAFA;
        BG_TITLEBAR = "bg.titlebar" 0xF6F6F8;
        SURFACE_PANEL = "surface.panel" 0xEFF1F3;
        SURFACE_RAISED = "surface.raised" 0xE3E5E9;
        SURFACE_INSET = "surface.inset" 0xFFFFFF;
        SURFACE_SELECTED = "surface.selected" 0xD5D9DE;
        KNOB_CAP = "knob.cap" 0xDCE0E4;
        KNOB_CAP_ACTIVE = "knob.capActive" 0xCED3D9;
        LINE = "line" 0xD4D7DD;
        LINE_INSET = "line.inset" 0xE1E4E8;
        LINE_STRONG = "line.strong" 0xBDC3CB;
        GRID = "grid" 0xEDEEF1;
        TEXT = "text" 0x202329;
        TEXT_MUTED = "text.muted" 0x484D56;
        TEXT_FAINT = "text.faint" 0x5B616B;
        INK_GHOST = "ink.ghost" 0x9EA6B5;
        INK_ARROW = "ink.arrow" 0x8F96A1;
        SIGNAL = "signal" 0x0055BB;
        MOD = "mod" 0x633E08;
        METER_OK = "meter.ok" 0x1A4F31;
        METER_HOT = "meter.hot" 0xB51200;
        LANE = "lane" 0xF3F4F6;
        LANE_ALT = "lane.alt" 0xEEF0F2;
        RULER = "ruler" 0xE0E3E7;
        HEADER = "header" 0xE2E5E9;
        HEADER_SELECTED = "header.selected" 0xC6CBD2;
        GRID_BAR = "grid.bar" 0xCFCFD1;
        GRID_BEAT = "grid.beat" 0xE3E4E5;
        GRID_SUB = "grid.sub" 0xEEEEF0;
        LOOP_REGION = "loop" 0x5B430B;
        CURSOR = "cursor" 0x28282B;
        SELECTION_OUTLINE = "selection.outline" 0x1F1F26;
        INK_DARK = "ink.dark" 0x131518;
        INK_LIGHT = "ink.light" 0xF6F6F8;
        RECORD = "record" 0xB51200;
        MUTE = "mute" 0x202329;
        SOLO = "solo" 0x0055BB;
        MONITOR = "monitor" 0x1A4F31;
        FOCUS_RING = "focus.ring" 0x0055BB;
        BUTTON_HOVER = "button.hover" 0xCED3D9;
        TOGGLE_OPTION_FILL = "toggle.on.option.fill" 0xD9E1EB;
        TOGGLE_UNAVAILABLE_FILL = "toggle.unavailable.fill" 0xEAECEF;
        SWITCH_ON_KNOB = "switch.onKnob" 0xFFFFFF;
    }
    /// Dims what lies outside a range: black at 28/255 (`#0000001C`).
    pub const OVERLAY: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 28);
}

/// Fill alphas (`alpha` in tokens.json).
pub mod alpha {
    /// Fill under a curve in a display well.
    pub const CURVE_FILL: f32 = 0.12;
    /// Fill under the oscilloscope trace.
    pub const SCOPE_FILL: f32 = 0.08;
    /// A modulation range band.
    pub const MOD_BAND: f32 = 0.1;
    /// A disabled but open section.
    pub const DISABLED_SECTION: f32 = 0.4;
    pub const ALL: &[(&str, f32)] =
        &[("curveFill", CURVE_FILL), ("scopeFill", SCOPE_FILL), ("modBand", MOD_BAND), ("disabledSection", DISABLED_SECTION)];
}

/// The two type families (`font` in tokens.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Sans,
    Mono,
}

impl Family {
    /// The family's key in tokens.json.
    pub fn key(self) -> &'static str {
        match self {
            Family::Sans => "sans",
            Family::Mono => "mono",
        }
    }
}

/// The bundled faces: IBM Plex Sans for interface text, IBM Plex Mono for every number.
pub const FONT_SANS: &str = "IBM Plex Sans";
pub const FONT_MONO: &str = "IBM Plex Mono";

/// The color token a type role is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Text,
    TextMuted,
    TextFaint,
}

impl Ink {
    /// The token's name in tokens.json.
    pub fn key(self) -> &'static str {
        match self {
            Ink::Text => "text",
            Ink::TextMuted => "text.muted",
            Ink::TextFaint => "text.faint",
        }
    }

    /// The token in the theme being drawn.
    pub fn color(self) -> Color32 {
        let t = &crate::theme::TOKENS;
        match self {
            Ink::Text => t.text,
            Ink::TextMuted => t.text_muted,
            Ink::TextFaint => t.text_faint,
        }
    }
}

/// A type role: family, weight, size (points), tracking (a fraction of the size), case and ink.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeRole {
    pub family: Family,
    pub weight: u16,
    pub size: f32,
    pub tracking: f32,
    pub uppercase: bool,
    /// `None`: the role names no ink (a tag takes its caller's).
    pub ink: Option<Ink>,
}

const fn role(family: Family, weight: u16, size: f32, tracking: f32, uppercase: bool, ink: Option<Ink>) -> TypeRole {
    TypeRole { family, weight, size, tracking, uppercase, ink }
}

pub const TITLE_BRAND: TypeRole = role(Family::Sans, 600, 15.0, 0.0, false, None);
pub const TITLE_DEVICE: TypeRole = role(Family::Sans, 500, 14.0, 0.0, false, None);
pub const TITLE_SECTION_BAR: TypeRole = role(Family::Sans, 600, 14.0, 0.0, false, None);
pub const LABEL_SECTION: TypeRole = role(Family::Sans, 600, 11.0, 0.08, true, Some(Ink::TextMuted));
pub const LABEL_CONTROL: TypeRole = role(Family::Sans, 400, 12.0, 0.0, false, Some(Ink::TextMuted));
pub const LABEL_HERO: TypeRole = role(Family::Sans, 500, 13.0, 0.0, false, Some(Ink::Text));
pub const VALUE: TypeRole = role(Family::Mono, 500, 11.0, 0.0, false, Some(Ink::Text));
pub const READOUT_DISPLAY: TypeRole = role(Family::Mono, 400, 11.0, 0.0, false, Some(Ink::TextFaint));
pub const TAG: TypeRole = role(Family::Mono, 500, 10.0, 0.06, false, None);

/// Every type role by its tokens.json name.
pub const TYPE_ROLES: &[(&str, TypeRole)] = &[
    ("title.brand", TITLE_BRAND),
    ("title.device", TITLE_DEVICE),
    ("title.sectionBar", TITLE_SECTION_BAR),
    ("label.section", LABEL_SECTION),
    ("label.control", LABEL_CONTROL),
    ("label.hero", LABEL_HERO),
    ("value", VALUE),
    ("readout.display", READOUT_DISPLAY),
    ("tag", TAG),
];

/// The egui font families apps/transit installs (with the system and Hangul fallbacks behind
/// each). `bold` stays as an alias of the semibold family.
pub const FAMILY_SANS: &str = "sans";
pub const FAMILY_SANS_MEDIUM: &str = "sans-medium";
pub const FAMILY_SANS_SEMIBOLD: &str = "sans-semibold";
pub const FAMILY_MONO: &str = "mono";
pub const FAMILY_MONO_MEDIUM: &str = "mono-medium";
pub const FAMILIES: [&str; 5] = [FAMILY_SANS, FAMILY_SANS_MEDIUM, FAMILY_SANS_SEMIBOLD, FAMILY_MONO, FAMILY_MONO_MEDIUM];

impl TypeRole {
    /// The installed family for this role's face (the nearest weight the app bundles).
    pub fn family_name(&self) -> &'static str {
        match (self.family, self.weight) {
            (Family::Sans, ..=450) => FAMILY_SANS,
            (Family::Sans, 451..=550) => FAMILY_SANS_MEDIUM,
            (Family::Sans, _) => FAMILY_SANS_SEMIBOLD,
            (Family::Mono, ..=450) => FAMILY_MONO,
            (Family::Mono, _) => FAMILY_MONO_MEDIUM,
        }
    }

    /// The font: the role's family where the app installed it ([`crate::theme::probe_fonts`]),
    /// else egui's Proportional or Monospace (tests and `--no-fonts` runs).
    pub fn font_id(&self, ctx: &egui::Context) -> egui::FontId {
        let name = self.family_name();
        if crate::theme::has_family(ctx, name) {
            egui::FontId::new(self.size, egui::FontFamily::Name(name.into()))
        } else if self.family == Family::Mono {
            egui::FontId::monospace(self.size)
        } else {
            egui::FontId::proportional(self.size)
        }
    }

    /// The ink in the theme being drawn (text when the role names none).
    pub fn color(&self) -> Color32 {
        self.ink.unwrap_or(Ink::Text).color()
    }

    /// `text` as this role sets it: uppercased when the role is.
    pub fn text(&self, text: &str) -> String {
        if self.uppercase { text.to_uppercase() } else { text.to_string() }
    }

    /// A one-line layout job in this role, with its tracking as extra letter spacing.
    pub fn job(&self, ctx: &egui::Context, text: &str, color: Color32) -> egui::text::LayoutJob {
        let format = egui::TextFormat { font_id: self.font_id(ctx), color, extra_letter_spacing: self.tracking * self.size, ..Default::default() };
        egui::text::LayoutJob::single_section(self.text(text), format)
    }
}

/// The 4 px space scale (`space` in tokens.json).
pub const SPACE: [f32; 6] = [4.0, 8.0, 12.0, 16.0, 24.0, 32.0];

/// Layout metrics (`layout` in tokens.json).
pub mod layout {
    pub const WINDOW_PADDING: f32 = 16.0;
    pub const PANEL_GAP: f32 = 12.0;
    /// Vertical, horizontal.
    pub const PANEL_PADDING: [f32; 2] = [14.0, 16.0];
    pub const GRID_COLUMNS: u32 = 12;
    pub const TITLE_BAR_HEIGHT: f32 = 52.0;
    pub const ROUTING_ROW_HEIGHT: f32 = 40.0;
    /// Smallest and largest control height.
    pub const CONTROL_HEIGHT: [f32; 2] = [26.0, 32.0];
    pub const SEGMENT_HEIGHT: [f32; 2] = [22.0, 24.0];
}

/// Corner radii (`radius` in tokens.json).
pub mod radius {
    pub const PANEL: f32 = 8.0;
    pub const DISPLAY: f32 = 6.0;
    pub const CONTROL: f32 = 5.0;
    pub const SEGMENT: f32 = 4.0;
    pub const CHIP: f32 = 13.0;
    pub const SWITCH: f32 = 10.0;
    pub const ALL: &[(&str, f32)] =
        &[("panel", PANEL), ("display", DISPLAY), ("control", CONTROL), ("segment", SEGMENT), ("chip", CHIP), ("switch", SWITCH)];
}

/// Knob geometry (`knob` in tokens.json), in a 100-unit box scaled to the knob's size.
pub mod knob {
    /// Diameters: hero (L), default (M), dense (S).
    pub const L: f32 = 72.0;
    pub const M: f32 = 44.0;
    pub const S: f32 = 38.0;
    pub const BOX: f32 = 100.0;
    pub const SWEEP_DEGREES: f32 = 270.0;
    /// Where the sweep starts, degrees clockwise from 12 o'clock (7 o'clock).
    pub const START_DEGREES: f32 = -135.0;
    pub const TRACK_RADIUS: f32 = 40.0;
    pub const TRACK_STROKE: f32 = 6.0;
    pub const MOD_RING_RADIUS: f32 = 47.0;
    pub const MOD_RING_STROKE: f32 = 3.5;
    pub const CAP_RADIUS: f32 = 28.0;
    pub const CAP_RIM_STROKE: f32 = 1.5;
    pub const POINTER_FROM: f32 = 9.0;
    pub const POINTER_TO: f32 = 24.0;
    pub const POINTER_STROKE: f32 = 4.0;
}

/// The section switch (`switch` in tokens.json).
pub mod switch {
    pub const WIDTH: f32 = 34.0;
    pub const HEIGHT: f32 = 20.0;
    pub const KNOB: f32 = 14.0;
}

/// Latching toggles (`toggle` in tokens.json).
pub mod toggle {
    use super::{Family, TypeRole, role};
    pub const HEIGHT: f32 = 24.0;
    pub const RADIUS: f32 = 4.0;
    pub const FONT: TypeRole = role(Family::Sans, 600, 11.0, 0.0, false, None);
}

/// Faders and meters (`fader` in tokens.json).
pub mod fader {
    /// dB to the fraction of travel from the top; `None` is minus infinity.
    pub const TAPER: [(Option<f32>, f32); 9] = [
        (Some(6.0), 0.0),
        (Some(0.0), 0.14),
        (Some(-6.0), 0.28),
        (Some(-12.0), 0.42),
        (Some(-24.0), 0.6),
        (Some(-36.0), 0.74),
        (Some(-48.0), 0.86),
        (Some(-60.0), 0.94),
        (None, 1.0),
    ];
    pub const SCALE_LABELS: [Option<f32>; 8] = [Some(6.0), Some(0.0), Some(-6.0), Some(-12.0), Some(-24.0), Some(-36.0), Some(-48.0), None];
    /// Track inset at the top and bottom.
    pub const INSET: f32 = 10.0;
    pub const TRACK_WIDTH: f32 = 4.0;
    pub const TRACK_RADIUS: f32 = 2.0;
    pub const CAP: [f32; 2] = [32.0, 16.0];
    pub const CAP_RADIUS: f32 = 3.0;
    pub const CAP_CENTER_LINE: f32 = 2.0;
    pub const CAP_COMPACT: [f32; 2] = [28.0, 14.0];
    pub const CAP_HORIZONTAL: [f32; 2] = [12.0, 18.0];
    pub const CHANNEL_WIDTH: f32 = 116.0;
    pub const COMPACT_WIDTH: f32 = 40.0;
    pub const HORIZONTAL_HEIGHT: f32 = 24.0;
    pub const METER_BAR: f32 = 10.0;
    pub const METER_BAR_MAIN: f32 = 14.0;
    pub const METER_GAP: f32 = 3.0;
    pub const METER_PEAK_ALPHA: f32 = 0.35;
    pub const METER_HOLD: f32 = 2.0;
    pub const METER_CLIP_CELL: f32 = 4.0;
}
