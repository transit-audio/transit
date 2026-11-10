//! The device UI design language's tokens (`docs/design/language/tokens.json`, ADR 0045) as Rust
//! constants: colors (the dark set and the designed light set), the arrangement and
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
/// on the old ground, and the state colors that alias a design token. A warning has no color of
/// its own (#326: `text` and a triangle; an error adds `meter.hot`). The grid lines are the lane
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
        LOOP_REGION = "loop" 0xD05DAD;
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
    /// Height of the loop strip at the top of the ruler (points): a Mono 9 label with 2 px or more above and below.
    pub const LOOP_STRIP_HEIGHT: f32 = 14.0;
    /// Dims what lies outside a range: black at 70/255 (`#00000046`).
    pub const OVERLAY: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 70);
}

/// Fills that the component specs give as literals (toggle option and unavailable fills, the
/// switch knob, the default button's hover and pressed fills), by their path in tokens.json; the
/// fader cap is `knob.capActive`.
pub mod component {
    use super::*;
    colors! { ALL;
        TOGGLE_OPTION_FILL = "toggle.on.option.fill" 0x1E2A38;
        TOGGLE_UNAVAILABLE_FILL = "toggle.unavailable.fill" 0x1E2125;
        SWITCH_ON_KNOB = "switch.onKnob" 0xFFFFFF;
        DEFAULT_BUTTON_HOVER = "chrome.defaultButton.hover" 0x66ACFF;
        DEFAULT_BUTTON_PRESSED = "chrome.defaultButton.pressed" 0x3E85D8;
    }
}

/// The light theme (`colorLight` in tokens.json): the maintainer's designed set, not a formula.
/// It keeps every role and hue of the dark set and meets targets: text roles at 4.5:1 or more on
/// the panel and the inset well, marks at 3:1 on the panel, `mod` a mark-only orange, the accents
/// kept apart for every color-vision type, `surface.raised` and `knob.cap` white on a mid-light
/// graphite window.
pub mod color_light {
    use super::*;
    colors! { ALL;
        BG_WINDOW = "bg.window" 0xE6E8EB;
        BG_TITLEBAR = "bg.titlebar" 0xDFE2E6;
        SURFACE_PANEL = "surface.panel" 0xF7F8F9;
        SURFACE_RAISED = "surface.raised" 0xFFFFFF;
        SURFACE_INSET = "surface.inset" 0xECEEF1;
        SURFACE_SELECTED = "surface.selected" 0xDCE0E5;
        KNOB_CAP = "knob.cap" 0xFFFFFF;
        KNOB_CAP_ACTIVE = "knob.capActive" 0xEEF0F3;
        LINE = "line" 0xD3D7DD;
        LINE_INSET = "line.inset" 0xDCE0E5;
        LINE_STRONG = "line.strong" 0xB4BBC4;
        GRID = "grid" 0xDFE2E7;
        TEXT = "text" 0x1B1E23;
        TEXT_MUTED = "text.muted" 0x4A515B;
        TEXT_FAINT = "text.faint" 0x5F6670;
        INK_GHOST = "ink.ghost" 0xA2A9B4;
        INK_ARROW = "ink.arrow" 0x7D8591;
        SIGNAL = "signal" 0x1764D8;
        MOD = "mod" 0xE17000;
        METER_OK = "meter.ok" 0x1A7943;
        METER_HOT = "meter.hot" 0xA8200E;
        LANE = "lane" 0xF3F4F6;
        LANE_ALT = "lane.alt" 0xEDEFF2;
        RULER = "ruler" 0xDDE1E5;
        HEADER = "header" 0xF0F2F4;
        HEADER_SELECTED = "header.selected" 0xDCE1E8;
        GRID_BAR = "grid.bar" 0xCDD1D6;
        GRID_BEAT = "grid.beat" 0xDDE0E4;
        GRID_SUB = "grid.sub" 0xE6E8EB;
        LOOP_REGION = "loop" 0xB73590;
        CURSOR = "cursor" 0x28282B;
        SELECTION_OUTLINE = "selection.outline" 0x1F1F26;
        INK_DARK = "ink.dark" 0x131518;
        INK_LIGHT = "ink.light" 0xF6F6F8;
        RECORD = "record" 0xA8200E;
        MUTE = "mute" 0x1B1E23;
        SOLO = "solo" 0x1764D8;
        MONITOR = "monitor" 0x1A7943;
        FOCUS_RING = "focus.ring" 0x1764D8;
        BUTTON_HOVER = "button.hover" 0xEEF0F3;
        TOGGLE_OPTION_FILL = "toggle.on.option.fill" 0xE3ECFA;
        TOGGLE_UNAVAILABLE_FILL = "toggle.unavailable.fill" 0xEDEFF2;
        SWITCH_ON_KNOB = "switch.onKnob" 0xFFFFFF;
        DEFAULT_BUTTON_HOVER = "chrome.defaultButton.hover" 0x1457BE;
        DEFAULT_BUTTON_PRESSED = "chrome.defaultButton.pressed" 0x4A86E0;
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
    /// The loop brace while Cycle is off: its fill and its 1 px edge, in the loop hue.
    pub const LOOP_BRACE_OFF: f32 = 0.3;
    pub const LOOP_BRACE_OFF_EDGE: f32 = 0.55;
    /// The ruler under the loop while Cycle is on.
    pub const LOOP_RULER_TINT: f32 = 0.2;
    /// The loop's edges down the ruler and the lanes while Cycle is on.
    pub const LOOP_EDGE: f32 = 0.75;
    /// Out-of-scale rows in the piano roll.
    pub const LOOP_SCALE_ROW: f32 = 0.12;
    /// The halo under an XY pad's point (`signal`, 326-M, the board's 14 percent).
    pub const PAD_HALO: f32 = 0.14;
    /// A voice light while its voice's release fades ("half lit", the older panel's 45 percent).
    pub const VOICE_RELEASING: f32 = 0.45;
    pub const ALL: &[(&str, f32)] = &[
        ("curveFill", CURVE_FILL),
        ("scopeFill", SCOPE_FILL),
        ("modBand", MOD_BAND),
        ("disabledSection", DISABLED_SECTION),
        ("loopBraceOff", LOOP_BRACE_OFF),
        ("loopBraceOffEdge", LOOP_BRACE_OFF_EDGE),
        ("loopRulerTint", LOOP_RULER_TINT),
        ("loopEdge", LOOP_EDGE),
        ("loopScaleRow", LOOP_SCALE_ROW),
        ("padHalo", PAD_HALO),
        ("voiceReleasing", VOICE_RELEASING),
    ];
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
    /// Every control inside a device panel or strip (combo, dropdown, field).
    pub const DENSE_CONTROL_HEIGHT: f32 = 24.0;
    /// The application chrome's own fields and buttons.
    pub const CHROME_CONTROL_HEIGHT: f32 = 26.0;
    /// List and menu rows.
    pub const LIST_ROW_HEIGHT: f32 = 22.0;
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
    /// The order the Channel scale's labels are placed in when the travel is short (Labels by
    /// travel): each is skipped when its center would fall under [`LABEL_MIN_GAP`] from one placed.
    pub const LABEL_PRIORITY: [Option<f32>; 8] = [Some(0.0), None, Some(-12.0), Some(-24.0), Some(6.0), Some(-48.0), Some(-6.0), Some(-36.0)];
    pub const LABEL_MIN_GAP: f32 = 13.0;
    /// The Frame: the travel's top (Y0) is the level block's top plus this; the clip cells sit
    /// [`FRAME_CLIP_CELL_GAP`] above Y0; every strip in a row shares Y0 and Y1.
    pub const FRAME_INSET: f32 = 10.0;
    pub const FRAME_CLIP_CELL_GAP: f32 = 6.0;
    pub const FRAME_SHARED_ACROSS_ROW: bool = true;
    /// The Channel variant's columns, in x from the level block's left edge: the fader column,
    /// the labels' right edge, the tick, the meter's left edge, and the block widths (a channel's
    /// and the main bus's).
    pub const COLUMN_FADER: [f32; 2] = [0.0, 36.0];
    pub const COLUMN_LABEL_RIGHT: f32 = 60.0;
    pub const COLUMN_TICK: [f32; 2] = [63.0, 67.0];
    pub const COLUMN_METER_X: f32 = 72.0;
    pub const BLOCK_WIDTH: f32 = 116.0;
    pub const MAIN_BLOCK_WIDTH: f32 = 144.0;
    /// The Channel tick: 4 long, 3 after its label's right edge; `line.strong`, `text.muted` at 0 dB.
    pub const TICK_LENGTH: f32 = 4.0;
    pub const TICK_GAP_AFTER_LABEL: f32 = 3.0;
    pub const TICK_COLOR: &str = "line.strong";
    pub const TICK_ZERO_COLOR: &str = "text.muted";
    /// The meter's 0 dB grid line (the others are `line.inset`, [`METER_GRID_COLOR`]).
    pub const ZERO_GRID_LINE: &str = "line.strong";
    /// The order a meter well paints its layers in: nothing is drawn over the level.
    pub const METER_DRAW_ORDER: [&str; 5] = ["well", "grid", "peak", "rms", "hold"];
    /// The meter grid (`fader.meterGrid`): a line only where a label is placed (the set the
    /// Channel scale's ticks use), `line.inset`, the 0 dB line `line.strong`.
    pub const METER_GRID_AT: &str = "placedLabels";
    pub const METER_GRID_COLOR: &str = "line.inset";
    pub const METER_GRID_ZERO_COLOR: &str = "line.strong";
    /// Where a main bus without a fader offers to add one (`fader.mainAddFader`): its routing rows.
    pub const MAIN_ADD_FADER: &str = "routingRow";
    /// The fill runs from this gain to the cap (`fader.fill.fromDb`): gain is bipolar around unity.
    pub const FILL_FROM_DB: f32 = 0.0;
    /// The unity notch across the track at 0 dB (`fader.unityNotch`): 12 by 1 in `ink.arrow`.
    pub const UNITY_NOTCH: [f32; 2] = [12.0, 1.0];
    pub const UNITY_NOTCH_COLOR: &str = "ink.arrow";
    /// A meter source with no RMS reading draws its peak at this alpha (`fader.meter.peakAlphaWithoutRms`).
    pub const METER_PEAK_ALPHA_WITHOUT_RMS: f32 = 1.0;
    /// The meter fills' and hold line's corner radius, and the well's (`fader.meter.fillRadius`,
    /// `fader.meter.wellRadius`).
    pub const METER_FILL_RADIUS: f32 = 0.0;
    pub const METER_WELL_RADIUS: f32 = 1.5;
    /// The keys of `fader` in tokens.json, all mirrored here (the drift test checks both ways).
    pub const KEYS: [&str; 21] = [
        "taper",
        "taperNote",
        "scaleLabels",
        "labelPriority",
        "labelMinGap",
        "frame",
        "channelColumns",
        "tick",
        "zeroGridLine",
        "meterDrawOrder",
        "meterGrid",
        "mainAddFader",
        "unityNotch",
        "inset",
        "track",
        "fill",
        "cap",
        "capCompact",
        "capHorizontal",
        "variants",
        "meter",
    ];
}

/// The application chrome (`chrome` in tokens.json): egui's own widgets outside device panels,
/// from the maintainer's application chrome document. [`crate::theme`] maps the global rules onto
/// egui's style; [`crate::chrome`] draws what egui's shared style cannot (fields, checks, the
/// default button, dialog frames and footers, the focus ring).
pub mod chrome {
    use super::{Family, Ink, TypeRole, role};
    /// A standard button's label.
    pub const BUTTON_FONT: TypeRole = role(Family::Sans, 500, 12.0, 0.0, false, Some(Ink::Text));
    /// The default button's label (its ink is `surface.inset`).
    pub const DEFAULT_BUTTON_FONT: TypeRole = role(Family::Sans, 600, 12.0, 0.0, false, None);
    /// A modal's question, which is its title.
    pub const QUESTION_FONT: TypeRole = role(Family::Sans, 600, 13.0, 0.0, false, Some(Ink::Text));
    /// A modal's detail line.
    pub const DETAIL_FONT: TypeRole = role(Family::Sans, 400, 12.0, 0.0, false, Some(Ink::TextMuted));
    /// A tool window's title.
    pub const TITLE_FONT: TypeRole = role(Family::Sans, 500, 14.0, 0.0, false, Some(Ink::Text));
    /// A menu row and a menu bar item.
    pub const ROW_FONT: TypeRole = role(Family::Sans, 400, 12.0, 0.0, false, Some(Ink::Text));
    /// A menu row's shortcut, right-aligned.
    pub const SHORTCUT_FONT: TypeRole = role(Family::Mono, 400, 11.0, 0.0, false, Some(Ink::TextFaint));
    /// A number in a text field (right-aligned, the unit after it in `text.faint`).
    pub const NUMBER_FONT: TypeRole = role(Family::Mono, 500, 11.0, 0.0, false, Some(Ink::Text));
    /// Every chrome type role by its name under `chrome.type` in tokens.json.
    pub const TYPE_ROLES: &[(&str, TypeRole)] = &[
        ("button", BUTTON_FONT),
        ("defaultButton", DEFAULT_BUTTON_FONT),
        ("question", QUESTION_FONT),
        ("detail", DETAIL_FONT),
        ("title", TITLE_FONT),
        ("row", ROW_FONT),
        ("shortcut", SHORTCUT_FONT),
        ("number", NUMBER_FONT),
    ];
    /// Window and dialog corners (`radius.panel`).
    pub const RADIUS_WINDOW: f32 = 8.0;
    /// Popups (menus, combo lists, tooltips) and controls (`radius.control`).
    pub const RADIUS_POPUP: f32 = 5.0;
    pub const RADIUS_CONTROL: f32 = 5.0;
    /// Menu rows and menu bar items.
    pub const RADIUS_ROW: f32 = 4.0;
    pub const RADIUS_CHECKBOX: f32 = 3.0;
    /// Buttons, text fields and combo boxes.
    pub const CONTROL_HEIGHT: f32 = 26.0;
    pub const MENU_BAR_HEIGHT: f32 = 32.0;
    pub const MENU_BAR_ITEM_HEIGHT: f32 = 22.0;
    /// A popup row (menu, submenu, context menu, combo list).
    pub const ROW_HEIGHT: f32 = 24.0;
    /// App windows and dialogs; devices use the 52 px A bar ([`super::layout::TITLE_BAR_HEIGHT`]).
    pub const TITLE_BAR_HEIGHT: f32 = 26.0;
    pub const DEVICE_TITLE_BAR_HEIGHT: f32 = 52.0;
    /// Vertical, horizontal.
    pub const BUTTON_PADDING: [f32; 2] = [0.0, 12.0];
    pub const DEFAULT_BUTTON_PADDING: [f32; 2] = [0.0, 16.0];
    /// An icon button is this square, its glyph this size.
    pub const ICON_BUTTON: f32 = 26.0;
    pub const ICON_GLYPH: f32 = 14.0;
    pub const MENU_BAR_ITEM_PADDING: [f32; 2] = [0.0, 8.0];
    pub const POPUP_PADDING: f32 = 4.0;
    pub const SUBMENU_OVERLAP: f32 = 4.0;
    pub const CHECK_GUTTER: f32 = 16.0;
    /// A menu row's inset inside its hover fill: left (before the check gutter), right (after the
    /// shortcut or the text).
    pub const ROW_PADDING: [f32; 2] = [4.0, 8.0];
    /// Between a menu row's check gutter, text and shortcut.
    pub const ROW_GAP: f32 = 4.0;
    pub const CHECK_GLYPH: f32 = 12.0;
    pub const CHEVRON: f32 = 12.0;
    /// A list that opens from a field starts this far below it; a menu-bar popup opens the same
    /// distance below the bar ([`MENU_BAR_GAP`]).
    pub const COMBO_POPUP_GAP: f32 = 4.0;
    /// A menu-bar popup opens this far below the bar (`popup.menuBarGap`), aligned to the item's left
    /// edge. Mirrored ahead of its behavior (#418): no caller reads it yet.
    pub const MENU_BAR_GAP: f32 = 4.0;
    /// Which popups carry the check gutter (`popup.checkGutterWhen`): only one with at least one
    /// checkable row (check or radio); each popup decides for itself. Mirrored ahead of #418.
    pub const CHECK_GUTTER_WHEN: &str = "anyCheckableRow";
    /// A row's left and right padding in a popup without the gutter (`popup.rowPaddingNoGutter`).
    pub const ROW_PADDING_NO_GUTTER: [f32; 2] = [8.0, 8.0];
    /// Where a row's text starts, from the popup's edge: with the gutter, and without it
    /// (`popup.textInset`: 4 popup padding + 8 row padding).
    pub const TEXT_INSET_GUTTER: f32 = 28.0;
    pub const TEXT_INSET_NO_GUTTER: f32 = 12.0;
    pub const SEPARATOR_GAP: f32 = 4.0;
    pub const MODAL_WIDTH: f32 = 380.0;
    /// Top, sides, bottom.
    pub const MODAL_PADDING: [f32; 3] = [18.0, 18.0, 16.0];
    /// Space between a modal's text and its footer.
    pub const MODAL_FOOTER_GAP: f32 = 14.0;
    /// Vertical, horizontal.
    pub const TOOL_BODY_PADDING: [f32; 2] = [14.0, 16.0];
    pub const TOOL_FOOTER_PADDING: [f32; 2] = [10.0, 16.0];
    pub const CLOSE_HIT: f32 = 18.0;
    pub const CHECKBOX_BOX: f32 = 14.0;
    pub const CHECKBOX_HIT: f32 = 18.0;
    pub const CHECKBOX_MARK: f32 = 8.0;
    pub const RADIO_DOT: f32 = 6.0;
    pub const SCROLL_REST: f32 = 4.0;
    pub const SCROLL_HOVER: f32 = 8.0;
    pub const SCROLL_INSET: f32 = 2.0;
    pub const SCROLL_MIN_HANDLE: f32 = 24.0;
    pub const TOOLTIP_DELAY_MS: u32 = 500;
    /// Vertical, horizontal.
    pub const TOOLTIP_PADDING: [f32; 2] = [5.0, 8.0];
    pub const TOOLTIP_OFFSET: f32 = 4.0;
    pub const FOCUS_RING_WIDTH: f32 = 1.5;
    pub const FOCUS_RING_OFFSET: f32 = 2.0;
    /// A text field (`field`): its rest and hover edge is this wide; its rings (dragging, typing,
    /// error) take [`FOCUS_RING_WIDTH`] on the field's own edge, inside its bounds, at this offset
    /// (buttons keep [`FOCUS_RING_OFFSET`]); its text sits this far in (vertical, horizontal), on
    /// both the 26 px chrome field and the 24 px value fields of device panels and strips.
    pub const FIELD_EDGE_WIDTH: f32 = 1.0;
    pub const FIELD_RING_OFFSET: f32 = 0.0;
    pub const FIELD_PADDING: [f32; 2] = [0.0, 8.0];
    /// Text selection: `signal` at this alpha.
    pub const SELECTION_ALPHA_DARK: f32 = 0.6;
    pub const SELECTION_ALPHA_LIGHT: f32 = 0.35;
    /// Every number under `chrome` in tokens.json (the type roles aside) by its path; the drift
    /// test walks the file and this list both ways.
    /// The Notice pattern (an info line with one action, no color) and the Success pattern (a
    /// last-edit line with a check, no color, no banner): Sampler round 2.
    pub const NOTICE_FILL: &str = "surface.raised";
    pub const NOTICE_BORDER: &str = "line.strong";
    pub const NOTICE_RADIUS: f32 = 5.0;
    pub const NOTICE_PADDING: [f32; 4] = [6.0, 8.0, 6.0, 10.0];
    pub const NOTICE_ICON: &str = "info";
    pub const NOTICE_ICON_SIZE: f32 = 13.0;
    pub const NOTICE_INK: &str = "text";
    pub const NOTICE_ACTION_FILL: &str = "button.hover";
    pub const SUCCESS_ICON: &str = "check";
    pub const SUCCESS_ICON_SIZE: f32 = 12.0;
    pub const SUCCESS_INK: &str = "text";
    /// The icons chrome rules name (Lucide names, not colors), by path.
    pub const ICONS: &[(&str, &str)] = &[("notice/icon", NOTICE_ICON), ("success/icon", SUCCESS_ICON)];
    /// The colors the Notice and Success patterns name, by path.
    pub const PATTERN_COLORS: &[(&str, &str)] = &[
        ("notice/fill", NOTICE_FILL),
        ("notice/border", NOTICE_BORDER),
        ("notice/ink", NOTICE_INK),
        ("notice/actionFill", NOTICE_ACTION_FILL),
        ("success/ink", SUCCESS_INK),
    ];
    pub const METRICS: &[(&str, f32)] = &[
        ("notice/radius", NOTICE_RADIUS),
        ("notice/padding/0", NOTICE_PADDING[0]),
        ("notice/padding/1", NOTICE_PADDING[1]),
        ("notice/padding/2", NOTICE_PADDING[2]),
        ("notice/padding/3", NOTICE_PADDING[3]),
        ("notice/iconSize", NOTICE_ICON_SIZE),
        ("success/iconSize", SUCCESS_ICON_SIZE),
        ("radius/window", RADIUS_WINDOW),
        ("radius/popup", RADIUS_POPUP),
        ("radius/control", RADIUS_CONTROL),
        ("radius/row", RADIUS_ROW),
        ("radius/checkbox", RADIUS_CHECKBOX),
        ("height/control", CONTROL_HEIGHT),
        ("height/menuBar", MENU_BAR_HEIGHT),
        ("height/menuBarItem", MENU_BAR_ITEM_HEIGHT),
        ("height/row", ROW_HEIGHT),
        ("height/titleBar", TITLE_BAR_HEIGHT),
        ("height/deviceTitleBar", DEVICE_TITLE_BAR_HEIGHT),
        ("button/padding/0", BUTTON_PADDING[0]),
        ("button/padding/1", BUTTON_PADDING[1]),
        ("button/defaultPadding/0", DEFAULT_BUTTON_PADDING[0]),
        ("button/defaultPadding/1", DEFAULT_BUTTON_PADDING[1]),
        ("button/icon", ICON_BUTTON),
        ("button/glyph", ICON_GLYPH),
        ("menuBarItem/padding/0", MENU_BAR_ITEM_PADDING[0]),
        ("menuBarItem/padding/1", MENU_BAR_ITEM_PADDING[1]),
        ("popup/padding", POPUP_PADDING),
        ("popup/submenuOverlap", SUBMENU_OVERLAP),
        ("popup/checkGutter", CHECK_GUTTER),
        ("popup/rowPadding/0", ROW_PADDING[0]),
        ("popup/rowPadding/1", ROW_PADDING[1]),
        ("popup/rowGap", ROW_GAP),
        ("popup/checkGlyph", CHECK_GLYPH),
        ("popup/chevron", CHEVRON),
        ("comboPopupGap", COMBO_POPUP_GAP),
        ("popup/menuBarGap", MENU_BAR_GAP),
        ("popup/rowPaddingNoGutter/0", ROW_PADDING_NO_GUTTER[0]),
        ("popup/rowPaddingNoGutter/1", ROW_PADDING_NO_GUTTER[1]),
        ("popup/textInset/gutter", TEXT_INSET_GUTTER),
        ("popup/textInset/noGutter", TEXT_INSET_NO_GUTTER),
        ("popup/separatorGap", SEPARATOR_GAP),
        ("modal/width", MODAL_WIDTH),
        ("modal/padding/0", MODAL_PADDING[0]),
        ("modal/padding/1", MODAL_PADDING[1]),
        ("modal/padding/2", MODAL_PADDING[2]),
        ("modal/footerGap", MODAL_FOOTER_GAP),
        ("toolWindow/bodyPadding/0", TOOL_BODY_PADDING[0]),
        ("toolWindow/bodyPadding/1", TOOL_BODY_PADDING[1]),
        ("toolWindow/footerPadding/0", TOOL_FOOTER_PADDING[0]),
        ("toolWindow/footerPadding/1", TOOL_FOOTER_PADDING[1]),
        ("toolWindow/closeHit", CLOSE_HIT),
        ("checkbox/box", CHECKBOX_BOX),
        ("checkbox/hit", CHECKBOX_HIT),
        ("checkbox/mark", CHECKBOX_MARK),
        ("checkbox/radioDot", RADIO_DOT),
        ("scrollBar/rest", SCROLL_REST),
        ("scrollBar/hover", SCROLL_HOVER),
        ("scrollBar/inset", SCROLL_INSET),
        ("scrollBar/minHandle", SCROLL_MIN_HANDLE),
        ("tooltip/delayMs", TOOLTIP_DELAY_MS as f32),
        ("tooltip/padding/0", TOOLTIP_PADDING[0]),
        ("tooltip/padding/1", TOOLTIP_PADDING[1]),
        ("tooltip/offset", TOOLTIP_OFFSET),
        ("focusRing/width", FOCUS_RING_WIDTH),
        ("focusRing/offset", FOCUS_RING_OFFSET),
        ("field/edgeWidth", FIELD_EDGE_WIDTH),
        ("field/ringOffset", FIELD_RING_OFFSET),
        ("field/padding/0", FIELD_PADDING[0]),
        ("field/padding/1", FIELD_PADDING[1]),
        ("selectionAlpha/dark", SELECTION_ALPHA_DARK),
        ("selectionAlpha/light", SELECTION_ALPHA_LIGHT),
    ];
}

/// The hatch (`hatch` in tokens.json): a region that cannot take input right now
/// ([`crate::widgets::hatch`]).
pub mod hatch {
    /// `text` at this alpha (dark) ...
    pub const ALPHA: f32 = 0.07;
    /// ... and at this one in the light set.
    pub const ALPHA_LIGHT: f32 = 0.06;
    pub const STROKE: f32 = 1.5;
    /// Degrees; the lines rise left to right.
    pub const ANGLE: f32 = -45.0;
    /// The distance between lines, across them.
    pub const PITCH: f32 = 8.0;
    /// Nothing under this (either way) is hatched: only slivers; track headers and lanes hatch at
    /// every height.
    pub const MIN_SIZE: f32 = 16.0;
    /// A clip on an inactive track: a ghost outline in `ink.ghost` with a fill of it at this alpha.
    pub const GHOST_FILL: f32 = 0.18;
    /// Every number under `hatch` in tokens.json by its path.
    pub const METRICS: &[(&str, f32)] = &[
        ("alpha", ALPHA),
        ("alphaLight", ALPHA_LIGHT),
        ("stroke", STROKE),
        ("angle", ANGLE),
        ("pitch", PITCH),
        ("minSize", MIN_SIZE),
        ("ghostClip/fillAlpha", GHOST_FILL),
    ];
}

/// The content palette (`palette` in tokens.json): the colors a user picks for track bars, clip
/// bodies and name strips, and choke-group tags. Sixteen OKLCH hues 22.5 degrees apart from 11,
/// plus Graphite; each has a bar, a body and a strip value per theme, so a project's colors survive
/// a theme switch. Documented ahead of its use: the fixed track colors move to it in a later job.
pub mod palette {
    use super::*;
    pub const NAMES: [&str; 17] = [
        "Red",
        "Vermilion",
        "Orange",
        "Amber",
        "Yellow",
        "Lime",
        "Green",
        "Jade",
        "Teal",
        "Cyan",
        "Sky",
        "Blue",
        "Violet",
        "Purple",
        "Magenta",
        "Pink",
        "Graphite",
    ];
    pub const DARK_BAR: [Color32; 17] = [
        hex(0xF08696),
        hex(0xF18A74),
        hex(0xE99454),
        hex(0xD7A03D),
        hex(0xBDAD3E),
        hex(0x9BB856),
        hex(0x71C178),
        hex(0x3FC59C),
        hex(0x00C4BE),
        hex(0x00BFDC),
        hex(0x49B7F1),
        hex(0x77ACFC),
        hex(0x9DA0FB),
        hex(0xBC95EE),
        hex(0xD58DD7),
        hex(0xE787B8),
        hex(0xA5ACB2),
    ];
    pub const DARK_BODY: [Color32; 17] = [
        hex(0x934955),
        hex(0x934C3C),
        hex(0x8E5223),
        hex(0x825B0B),
        hex(0x70640C),
        hex(0x586C25),
        hex(0x39723F),
        hex(0x037559),
        hex(0x007571),
        hex(0x007185),
        hex(0x166B94),
        hex(0x3E639C),
        hex(0x5A5B9B),
        hex(0x6F5492),
        hex(0x804E82),
        hex(0x8C4A6D),
        hex(0x5E646A),
    ];
    pub const DARK_STRIP: [Color32; 17] = [
        hex(0x793943),
        hex(0x793B2E),
        hex(0x744117),
        hex(0x6A4900),
        hex(0x5B5100),
        hex(0x465818),
        hex(0x2B5D30),
        hex(0x005F47),
        hex(0x005F5C),
        hex(0x005C6D),
        hex(0x06567A),
        hex(0x2F5081),
        hex(0x484980),
        hex(0x5A4278),
        hex(0x693D6A),
        hex(0x733958),
        hex(0x4B5056),
    ];
    pub const LIGHT_BAR: [Color32; 17] = [
        hex(0xC8536A),
        hex(0xCA5840),
        hex(0xC16300),
        hex(0xAE7300),
        hex(0x948100),
        hex(0x6F8E00),
        hex(0x369744),
        hex(0x009A70),
        hex(0x009692),
        hex(0x0092AD),
        hex(0x008BCD),
        hex(0x447FD8),
        hex(0x7272D7),
        hex(0x9366C8),
        hex(0xAD5CB0),
        hex(0xBF558F),
        hex(0x7B8187),
    ];
    pub const LIGHT_BODY: [Color32; 17] = [
        hex(0xF9C5CA),
        hex(0xF9C7BB),
        hex(0xF4CBAF),
        hex(0xEAD0A8),
        hex(0xDCD6A9),
        hex(0xCCDBB0),
        hex(0xBCDFBD),
        hex(0xAEE2CD),
        hex(0xA7E1DE),
        hex(0xA7DFEC),
        hex(0xAFDBF7),
        hex(0xBDD6FC),
        hex(0xCDD0FC),
        hex(0xDCCBF5),
        hex(0xEAC7EA),
        hex(0xF4C5DB),
        hex(0xCED5DC),
    ];
    pub const LIGHT_STRIP: [Color32; 17] = [
        hex(0xF0ACB4),
        hex(0xF0AF9F),
        hex(0xEAB48F),
        hex(0xDDBB86),
        hex(0xCCC386),
        hex(0xB6CA91),
        hex(0xA0CFA2),
        hex(0x8CD2B8),
        hex(0x81D2CD),
        hex(0x82CFE0),
        hex(0x8ECAEE),
        hex(0xA2C3F5),
        hex(0xB7BCF4),
        hex(0xCBB5EC),
        hex(0xDDAFDD),
        hex(0xE9ACCA),
        hex(0xBBC2C9),
    ];
    /// The ink on a palette fill, per theme.
    pub const INK_DARK: Color32 = hex(0xF6F6F8);
    pub const INK_LIGHT: Color32 = hex(0x1B1E23);
    /// Every number under `palette.oklch` by its path (lightness, chroma pairs; the hue rotation).
    pub const OKLCH: &[(&str, f32)] = &[
        ("hueStart", 11.0),
        ("hueStep", 22.5),
        ("dark/bar/0", 0.74),
        ("dark/bar/1", 0.13),
        ("dark/body/0", 0.50),
        ("dark/body/1", 0.1),
        ("dark/strip/0", 0.43),
        ("dark/strip/1", 0.09),
        ("light/bar/0", 0.60),
        ("light/bar/1", 0.15),
        ("light/body/0", 0.87),
        ("light/body/1", 0.06),
        ("light/strip/0", 0.81),
        ("light/strip/1", 0.08),
        ("graphiteChroma", 0.012),
        ("graphiteHue", 250.0),
    ];
}

/// The pad (`pad` in tokens.json): every number of its spec, by its path. The kit's pad widget
/// (`widgets/pad.rs`, 326-S) uses these numbers; the drift test keeps this table and the file in step.
pub mod pad {
    pub const METRICS: &[(&str, f32)] = &[
        ("sizes/grid", 56.0),
        ("sizes/focus", 80.0),
        ("gap", 4.0),
        ("radius", 6.0),
        ("hit/alpha", 0.26),
        ("hit/edge", 2.0),
        ("hit/decayMs", 250.0),
        ("selected/width", 2.0),
        ("muted/opacity", 0.4),
        ("tag/height", 3.0),
        ("id/size/grid", 9.0),
        ("id/size/focus", 10.0),
        ("id/weight", 400.0),
        ("chokeBadge/minWidth", 16.0),
        ("chokeBadge/height", 14.0),
        ("chokeBadge/radius", 3.0),
        ("chokeBadge/weight", 500.0),
        ("chokeBadge/size", 9.0),
        ("chokeBadge/chokedMs", 200.0),
        ("peer/width", 1.0),
        ("peer/inset", 3.0),
        ("peer/radius", 4.0),
        ("bankPeerDot/size", 5.0),
        ("chip/size", 14.0),
        ("chip/radius", 3.0),
        ("soloExcluded/opacity", 0.4),
    ];
}

/// The mixer's channel strip (`channelStrip` in tokens.json): its widths, the fixed section
/// heights that keep every fader aligned, and the insert slot. Every number by its path for the
/// drift test.
pub mod channel_strip {
    pub const WIDTH: f32 = 132.0;
    pub const WIDTH_MAIN: f32 = 160.0;
    pub const GAP: f32 = 4.0;
    /// The extra space before the main bus strip.
    pub const GAP_MAIN: f32 = 8.0;
    pub const COLOR_BAR: f32 = 3.0;
    /// Padding inside the strip: top (under the color bar), sides, bottom.
    pub const PADDING: [f32; 3] = [7.0, 8.0, 8.0];
    /// Between sections, top to bottom.
    pub const SECTION_GAP: f32 = 6.0;
    pub const NAME_HEIGHT: f32 = 18.0;
    /// The input and output dropdowns and the toggles.
    pub const ROW_HEIGHT: f32 = 24.0;
    /// The inserts and sends sections' bodies (under a 1 px `line.inset` and 6 px of padding).
    pub const INSERTS_HEIGHT: f32 = 112.0;
    pub const SENDS_HEIGHT: f32 = 86.0;
    /// The padding between a section's top line and its body.
    pub const SECTION_PADDING: f32 = 6.0;
    /// The short variant's `FX n` / `Snd n` row.
    pub const SHORT_ROW: f32 = 22.0;
    pub const PAN_KNOB: f32 = 36.0;
    pub const PAN_ROW: f32 = 40.0;
    pub const PAN_VALUE_WIDTH: f32 = 30.0;
    /// The MIDI OUT button beside the input.
    pub const MIDI_OUT_WIDTH: f32 = 32.0;
    /// A send row's bar fills from the left (`sendBarAnchor`): a send's rest is off, so the bar
    /// reads as how much is sent. No notch. No code change; the constant records the decision.
    pub const SEND_BAR_ANCHOR: &str = "left";
    pub const READOUT_HEIGHT: f32 = 26.0;
    pub const PEAK_WIDTH: f32 = 46.0;
    pub const PEAK_WIDTH_MAIN: f32 = 56.0;
    pub const INSERT_SLOT_HEIGHT: f32 = 22.0;
    pub const INSERT_SLOT_RADIUS: f32 = 4.0;
    pub const INSERT_SLOT_GAP: f32 = 3.0;
    pub const INDICATOR_SIZE: f32 = 12.0;
    pub const INDICATOR_STROKE: f32 = 1.5;
    /// The strip's overflow edge fades (px) and the hidden-count font (Mono 400 10).
    pub const OVERFLOW_FADE: f32 = 12.0;
    pub const OVERFLOW_COUNT_WEIGHT: f32 = 400.0;
    pub const OVERFLOW_COUNT_SIZE: f32 = 10.0;
    /// The unresponsive-insert glyph (`circle-alert` in `meter.hot`).
    pub const UNRESPONSIVE_SIZE: f32 = 10.0;
    pub const METRICS: &[(&str, f32)] = &[
        ("width", WIDTH),
        ("widthMain", WIDTH_MAIN),
        ("gap", GAP),
        ("gapMain", GAP_MAIN),
        ("colorBar", COLOR_BAR),
        ("padding/0", PADDING[0]),
        ("padding/1", PADDING[1]),
        ("padding/2", PADDING[2]),
        ("sectionGap", SECTION_GAP),
        ("nameHeight", NAME_HEIGHT),
        ("rowHeight", ROW_HEIGHT),
        ("insertsHeight", INSERTS_HEIGHT),
        ("sendsHeight", SENDS_HEIGHT),
        ("sectionPadding", SECTION_PADDING),
        ("shortRow", SHORT_ROW),
        ("panKnob", PAN_KNOB),
        ("panRow", PAN_ROW),
        ("panValueWidth", PAN_VALUE_WIDTH),
        ("midiOutWidth", MIDI_OUT_WIDTH),
        ("readoutHeight", READOUT_HEIGHT),
        ("peakWidth", PEAK_WIDTH),
        ("peakWidthMain", PEAK_WIDTH_MAIN),
        ("insertSlot/height", INSERT_SLOT_HEIGHT),
        ("insertSlot/radius", INSERT_SLOT_RADIUS),
        ("insertSlot/gap", INSERT_SLOT_GAP),
        ("insertSlot/indicatorSize", INDICATOR_SIZE),
        ("insertSlot/indicatorStroke", INDICATOR_STROKE),
        ("overflow/fade", OVERFLOW_FADE),
        ("overflow/hiddenCountFont/weight", OVERFLOW_COUNT_WEIGHT),
        ("overflow/hiddenCountFont/size", OVERFLOW_COUNT_SIZE),
        ("unresponsive/size", UNRESPONSIVE_SIZE),
    ];
}

/// The compressor's displays (`compressor` in tokens.json): the unity diagonal, the operating
/// point and the history well. The numbers, and the color names and labels as the design writes
/// them (the panel draws each name with its theme token). Checked both ways by the drift test.
pub mod compressor {
    pub const UNITY_COLOR: &str = "line";
    pub const OPERATING_POINT_FILL: &str = "text";
    pub const OPERATING_POINT_RING: &str = "surface.inset";
    pub const OPERATING_POINT_SOURCE: &str = "detector input level, dB, at UI rate";
    pub const HISTORY_SECOND_LINES: &str = "grid";
    pub const HISTORY_QUARTER_LINES: &str = "grid";
    pub const HISTORY_TIME_LABELS: [&str; 2] = ["\u{2212}4 s", "now"];
    pub const HISTORY_INPUT_FILL: &str = "ink.ghost";
    pub const HISTORY_OUTPUT_STROKE: &str = "signal";
    pub const HISTORY_GR_FILL: &str = "mod";
    pub const HISTORY_GR_STROKE: &str = "mod";
    pub const HISTORY_THRESHOLD_COLOR: &str = "text";
    /// Every string under `compressor` in tokens.json by its path.
    pub const STRINGS: &[(&str, &str)] = &[
        ("unity/color", UNITY_COLOR),
        ("operatingPoint/fill", OPERATING_POINT_FILL),
        ("operatingPoint/ring", OPERATING_POINT_RING),
        ("operatingPoint/source", OPERATING_POINT_SOURCE),
        ("history/secondLines", HISTORY_SECOND_LINES),
        ("history/quarterLines", HISTORY_QUARTER_LINES),
        ("history/timeLabels/0", HISTORY_TIME_LABELS[0]),
        ("history/timeLabels/1", HISTORY_TIME_LABELS[1]),
        ("history/input/fill", HISTORY_INPUT_FILL),
        ("history/output/stroke", HISTORY_OUTPUT_STROKE),
        ("history/gainReduction/fill", HISTORY_GR_FILL),
        ("history/gainReduction/stroke", HISTORY_GR_STROKE),
        ("history/threshold/color", HISTORY_THRESHOLD_COLOR),
    ];
    /// The gain-reduction band hangs from the top (`history/gainReduction/fromTop`).
    pub const HISTORY_GR_FROM_TOP: bool = true;
    pub const UNITY_WIDTH: f32 = 1.0;
    pub const UNITY_DASH: [f32; 2] = [3.0, 3.0];
    pub const OPERATING_POINT_SIZE: f32 = 8.0;
    pub const OPERATING_POINT_RING_WIDTH: f32 = 2.0;
    pub const OPERATING_POINT_HIDE_BELOW_DB: f32 = -60.0;
    pub const HISTORY_WINDOW_SECONDS: f32 = 4.0;
    pub const HISTORY_INPUT_ALPHA: f32 = 0.35;
    pub const HISTORY_OUTPUT_WIDTH: f32 = 1.75;
    pub const HISTORY_GR_ALPHA: f32 = 0.22;
    pub const HISTORY_GR_WIDTH: f32 = 1.75;
    pub const HISTORY_GR_RANGE_DB: f32 = 24.0;
    pub const HISTORY_THRESHOLD_ALPHA: f32 = 0.45;
    pub const HISTORY_THRESHOLD_WIDTH: f32 = 1.0;
    pub const HISTORY_THRESHOLD_DASH: [f32; 2] = [4.0, 4.0];
    pub const METRICS: &[(&str, f32)] = &[
        ("unity/width", UNITY_WIDTH),
        ("unity/dash/0", UNITY_DASH[0]),
        ("unity/dash/1", UNITY_DASH[1]),
        ("operatingPoint/size", OPERATING_POINT_SIZE),
        ("operatingPoint/ringWidth", OPERATING_POINT_RING_WIDTH),
        ("operatingPoint/hideBelowDb", OPERATING_POINT_HIDE_BELOW_DB),
        ("history/windowSeconds", HISTORY_WINDOW_SECONDS),
        ("history/input/alpha", HISTORY_INPUT_ALPHA),
        ("history/output/width", HISTORY_OUTPUT_WIDTH),
        ("history/gainReduction/alpha", HISTORY_GR_ALPHA),
        ("history/gainReduction/width", HISTORY_GR_WIDTH),
        ("history/gainReduction/rangeDb", HISTORY_GR_RANGE_DB),
        ("history/threshold/alpha", HISTORY_THRESHOLD_ALPHA),
        ("history/threshold/width", HISTORY_THRESHOLD_WIDTH),
        ("history/threshold/dash/0", HISTORY_THRESHOLD_DASH[0]),
        ("history/threshold/dash/1", HISTORY_THRESHOLD_DASH[1]),
    ];
}

/// The Sampler views (`sampler` in tokens.json): the view bar, the key map, the zone waveform and
/// the slice bench. The numbers, and the color names, font roles and labels as the design writes
/// them. Checked both ways by the drift test.
pub mod sampler {
    pub const VIEW_BAR_FONT: &str = "sans";
    pub const VIEW_BAR_VIEWS: [&str; 3] = ["Instrument", "Pads", "Slices"];
    pub const ZONE_FILL: &str = "surface.raised";
    pub const ZONE_BORDER: &str = "line.strong";
    pub const SELECTED_BORDER: &str = "signal";
    pub const PAD_ZONE_BORDER: &str = "ink.arrow";
    pub const PAD_ZONE_STYLE: &str = "dashed";
    pub const WHITE_KEY: &str = "text.muted";
    pub const PAD_KEY: &str = "text.faint";
    pub const BLACK_KEY: &str = "bg.titlebar";
    pub const WAVE_REGION: &str = "signal";
    pub const WAVE_OUTSIDE: &str = "ink.ghost";
    pub const WAVE_HANDLE: &str = "signal";
    pub const WAVE_PLAYHEAD: &str = "text";
    pub const BENCH_FLAG_FONT: &str = "mono";
    pub const BENCH_WAVE: &str = "text.faint";
    pub const BENCH_END: &str = "ink.arrow";
    pub const BENCH_AUTO_LINE: &str = "ink.arrow";
    pub const BENCH_MANUAL_LINE: &str = "text.muted";
    pub const BENCH_GHOST_LINE: &str = "text";
    pub const BENCH_GHOST_STYLE: &str = "dashed";
    pub const BENCH_GHOST_REFUSED: &str = "meter.hot";
    pub const SLICE_LOOP_COLOR: &str = "loop";
    pub const SLICE_LOOP_GLYPH_FORWARD: &str = "\u{21bb}";
    pub const SLICE_LOOP_GLYPH_PING_PONG: &str = "\u{21c4}";
    /// Round 2 (ADR 0041): the flag strip's line, the type roles, the candidate line, the lock
    /// glyph, the seed field and the view switch.
    pub const BENCH_FLAG_STRIP_LINE: &str = "line.inset";
    /// The Sampler's type roles: (name, family, weight, size).
    pub const TYPE: [(&str, &str, f32, f32); 6] = [
        ("zoneLabel", "mono", 500.0, 9.0),
        ("rrTag", "mono", 500.0, 8.0),
        ("octaveName", "mono", 500.0, 8.0),
        ("axis", "mono", 400.0, 10.0),
        ("flagLetter", "sans", 600.0, 9.0),
        ("braceLabel", "mono", 500.0, 9.0),
    ];
    pub const CANDIDATE_LINE: &str = "text.faint";
    pub const CANDIDATE_FLAG: bool = false;
    pub const LOCK_GLYPH_ICON: &str = "lock";
    pub const LOCK_GLYPH_COLOR: &str = "text.faint";
    pub const SEED_REROLL_ICON: &str = "dices";
    pub const SEED_SEQUENTIAL: &str = "ink.arrow";
    pub const VIEW_SWITCH_FONT: &str = "sans";
    /// The Pads bar's Start note field (`startNote`; `sampler_panel::bar` draws it).
    pub const START_NOTE_LABEL: &str = "Start note";
    pub const START_NOTE_FONT: &str = "mono";
    pub const START_NOTE_DEFAULT: &str = "C1";
    pub const START_NOTE_LABEL_GAP: f32 = 6.0;
    pub const START_NOTE_WIDTH: f32 = 48.0;
    pub const START_NOTE_HEIGHT: f32 = 24.0;
    pub const START_NOTE_FONT_WEIGHT: f32 = 500.0;
    pub const START_NOTE_FONT_SIZE: f32 = 11.0;
    /// The start notes the field shows (`startNote.range`, MIDI numbers, inclusive): display only.
    /// The model's `MAX_PAD_START` is the authority for behavior; a test keeps the two equal.
    pub const START_NOTE_RANGE: [u8; 2] = [0, 64];
    pub const START_NOTE_DRAG_PX_PER_SEMITONE: f32 = 6.0;
    /// Every string under `sampler` in tokens.json by its path.
    pub const STRINGS: &[(&str, &str)] = &[
        ("viewBar/font/font", VIEW_BAR_FONT),
        ("viewBar/views/0", VIEW_BAR_VIEWS[0]),
        ("viewBar/views/1", VIEW_BAR_VIEWS[1]),
        ("viewBar/views/2", VIEW_BAR_VIEWS[2]),
        ("keyMap/zoneFill", ZONE_FILL),
        ("keyMap/zoneBorder", ZONE_BORDER),
        ("keyMap/selectedBorder", SELECTED_BORDER),
        ("keyMap/padZoneBorder", PAD_ZONE_BORDER),
        ("keyMap/padZoneStyle", PAD_ZONE_STYLE),
        ("keyMap/whiteKey", WHITE_KEY),
        ("keyMap/padKey", PAD_KEY),
        ("keyMap/blackKey", BLACK_KEY),
        ("waveform/region", WAVE_REGION),
        ("waveform/outside", WAVE_OUTSIDE),
        ("waveform/handle", WAVE_HANDLE),
        ("waveform/playhead", WAVE_PLAYHEAD),
        ("bench/flagFont/font", BENCH_FLAG_FONT),
        ("bench/wave", BENCH_WAVE),
        ("bench/end", BENCH_END),
        ("bench/autoLine", BENCH_AUTO_LINE),
        ("bench/manualLine", BENCH_MANUAL_LINE),
        ("bench/ghost/line", BENCH_GHOST_LINE),
        ("bench/ghost/style", BENCH_GHOST_STYLE),
        ("bench/ghost/refused", BENCH_GHOST_REFUSED),
        ("bench/sliceLoop/color", SLICE_LOOP_COLOR),
        ("bench/sliceLoop/glyphForward", SLICE_LOOP_GLYPH_FORWARD),
        ("bench/sliceLoop/glyphPingPong", SLICE_LOOP_GLYPH_PING_PONG),
        ("bench/flagStripLine", BENCH_FLAG_STRIP_LINE),
        ("type/zoneLabel/font", TYPE[0].1),
        ("type/rrTag/font", TYPE[1].1),
        ("type/octaveName/font", TYPE[2].1),
        ("type/axis/font", TYPE[3].1),
        ("type/flagLetter/font", TYPE[4].1),
        ("type/braceLabel/font", TYPE[5].1),
        ("candidate/line", CANDIDATE_LINE),
        ("lockGlyph/icon", LOCK_GLYPH_ICON),
        ("lockGlyph/color", LOCK_GLYPH_COLOR),
        ("seed/rerollIcon", SEED_REROLL_ICON),
        ("seed/sequential", SEED_SEQUENTIAL),
        ("viewSwitch/font/font", VIEW_SWITCH_FONT),
        ("startNote/label", START_NOTE_LABEL),
        ("startNote/font/font", START_NOTE_FONT),
        ("startNote/default", START_NOTE_DEFAULT),
    ];
    /// The view bar's row; its controls take the dense `VIEW_BAR_CONTROL_HEIGHT`, centred, and its
    /// items (a label and its field are one) stand `VIEW_BAR_GAP` apart.
    pub const VIEW_BAR_HEIGHT: f32 = 28.0;
    pub const VIEW_BAR_CONTROL_HEIGHT: f32 = 24.0;
    pub const VIEW_BAR_GAP: f32 = 12.0;
    pub const VIEW_BAR_FONT_WEIGHT: f32 = 500.0;
    pub const VIEW_BAR_FONT_SIZE: f32 = 12.0;
    pub const ZONE_RADIUS: f32 = 4.0;
    pub const SELECTED_BORDER_WIDTH: f32 = 2.0;
    pub const SELECTED_FILL_ALPHA: f32 = 0.14;
    pub const ROOT_DOT: f32 = 7.0;
    pub const HELD_COLUMN_ALPHA: f32 = 0.1;
    pub const WAVE_REGION_ALPHA: f32 = 0.55;
    pub const WAVE_HANDLE_WIDTH: f32 = 2.0;
    pub const WAVE_FLAG_HEIGHT: f32 = 14.0;
    pub const WAVE_LOOP_BRACE: f32 = 14.0;
    pub const WAVE_LOOP_BAND_ALPHA: f32 = 0.1;
    pub const WAVE_CROSSFADE_HATCH_ALPHA: f32 = 0.28;
    pub const BENCH_FLAG_STRIP: f32 = 18.0;
    pub const BENCH_FLAG_FONT_WEIGHT: f32 = 500.0;
    pub const BENCH_FLAG_FONT_SIZE: f32 = 9.0;
    pub const BENCH_ALT_SLICE_ALPHA: f32 = 0.03;
    pub const BENCH_SELECTED_ALPHA: f32 = 0.12;
    pub const BENCH_SELECTED_WAVE_ALPHA: f32 = 0.6;
    pub const BENCH_WAVE_ALPHA: f32 = 0.45;
    pub const BENCH_HANDLE: f32 = 13.0;
    pub const SLICE_LOOP_BAND: f32 = 4.0;
    pub const SLICE_LOOP_WASH_ALPHA: f32 = 0.07;
    pub const SLICE_LOOP_HATCH_ALPHA: f32 = 0.28;
    pub const BENCH_ZOOMS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
    /// The flag's corner radii (top left, top right, bottom right, bottom left).
    pub const BENCH_FLAG_RADIUS: [f32; 4] = [0.0, 3.0, 3.0, 0.0];
    pub const CHIP: f32 = 22.0;
    pub const PREVIEW_CELL: f32 = 12.0;
    pub const UNUSED_CONTROL_ALPHA: f32 = 0.4;
    pub const CANDIDATE_DASH: [f32; 2] = [3.0, 3.0];
    pub const FADE_FLOOR_ALPHA: f32 = 0.25;
    pub const LOCK_GLYPH_SIZE: f32 = 10.0;
    /// The seed field's range (ADR 0041: 1 to 4 294 967 295, 0 normalized to 1).
    pub const SEED_MIN: u32 = 1;
    pub const SEED_MAX: u32 = u32::MAX;
    pub const VIEW_SWITCH_FONT_WEIGHT: f32 = 500.0;
    pub const VIEW_SWITCH_FONT_SIZE: f32 = 12.0;
    pub const VIEW_SWITCH_PADDING_X: f32 = 12.0;
    pub const METRICS: &[(&str, f32)] = &[
        ("viewBar/height", VIEW_BAR_HEIGHT),
        ("viewBar/controlHeight", VIEW_BAR_CONTROL_HEIGHT),
        ("viewBar/gap", VIEW_BAR_GAP),
        ("viewBar/font/weight", VIEW_BAR_FONT_WEIGHT),
        ("viewBar/font/size", VIEW_BAR_FONT_SIZE),
        ("keyMap/zoneRadius", ZONE_RADIUS),
        ("keyMap/selectedBorderWidth", SELECTED_BORDER_WIDTH),
        ("keyMap/selectedFillAlpha", SELECTED_FILL_ALPHA),
        ("keyMap/rootDot", ROOT_DOT),
        ("keyMap/heldColumnAlpha", HELD_COLUMN_ALPHA),
        ("waveform/regionAlpha", WAVE_REGION_ALPHA),
        ("waveform/handleWidth", WAVE_HANDLE_WIDTH),
        ("waveform/flagHeight", WAVE_FLAG_HEIGHT),
        ("waveform/loopBrace", WAVE_LOOP_BRACE),
        ("waveform/loopBandAlpha", WAVE_LOOP_BAND_ALPHA),
        ("waveform/crossfadeHatchAlpha", WAVE_CROSSFADE_HATCH_ALPHA),
        ("bench/flagStrip", BENCH_FLAG_STRIP),
        ("bench/flagFont/weight", BENCH_FLAG_FONT_WEIGHT),
        ("bench/flagFont/size", BENCH_FLAG_FONT_SIZE),
        ("bench/altSliceAlpha", BENCH_ALT_SLICE_ALPHA),
        ("bench/selectedAlpha", BENCH_SELECTED_ALPHA),
        ("bench/selectedWaveAlpha", BENCH_SELECTED_WAVE_ALPHA),
        ("bench/waveAlpha", BENCH_WAVE_ALPHA),
        ("bench/handle", BENCH_HANDLE),
        ("bench/sliceLoop/band", SLICE_LOOP_BAND),
        ("bench/sliceLoop/washAlpha", SLICE_LOOP_WASH_ALPHA),
        ("bench/sliceLoop/crossfadeHatchAlpha", SLICE_LOOP_HATCH_ALPHA),
        ("bench/zooms/0", BENCH_ZOOMS[0]),
        ("bench/zooms/1", BENCH_ZOOMS[1]),
        ("bench/zooms/2", BENCH_ZOOMS[2]),
        ("bench/zooms/3", BENCH_ZOOMS[3]),
        ("bench/flagRadius/0", BENCH_FLAG_RADIUS[0]),
        ("bench/flagRadius/1", BENCH_FLAG_RADIUS[1]),
        ("bench/flagRadius/2", BENCH_FLAG_RADIUS[2]),
        ("bench/flagRadius/3", BENCH_FLAG_RADIUS[3]),
        ("type/zoneLabel/weight", TYPE[0].2),
        ("type/zoneLabel/size", TYPE[0].3),
        ("type/rrTag/weight", TYPE[1].2),
        ("type/rrTag/size", TYPE[1].3),
        ("type/octaveName/weight", TYPE[2].2),
        ("type/octaveName/size", TYPE[2].3),
        ("type/axis/weight", TYPE[3].2),
        ("type/axis/size", TYPE[3].3),
        ("type/flagLetter/weight", TYPE[4].2),
        ("type/flagLetter/size", TYPE[4].3),
        ("type/braceLabel/weight", TYPE[5].2),
        ("type/braceLabel/size", TYPE[5].3),
        ("chip", CHIP),
        ("previewCell", PREVIEW_CELL),
        ("unusedControlAlpha", UNUSED_CONTROL_ALPHA),
        ("candidate/dash/0", CANDIDATE_DASH[0]),
        ("candidate/dash/1", CANDIDATE_DASH[1]),
        ("fadeFloorAlpha", FADE_FLOOR_ALPHA),
        ("lockGlyph/size", LOCK_GLYPH_SIZE),
        ("seed/min", SEED_MIN as f32),
        ("seed/max", SEED_MAX as f32),
        ("viewSwitch/font/weight", VIEW_SWITCH_FONT_WEIGHT),
        ("viewSwitch/font/size", VIEW_SWITCH_FONT_SIZE),
        ("viewSwitch/paddingX", VIEW_SWITCH_PADDING_X),
        ("startNote/labelGap", START_NOTE_LABEL_GAP),
        ("startNote/width", START_NOTE_WIDTH),
        ("startNote/height", START_NOTE_HEIGHT),
        ("startNote/font/weight", START_NOTE_FONT_WEIGHT),
        ("startNote/font/size", START_NOTE_FONT_SIZE),
        ("startNote/dragPxPerSemitone", START_NOTE_DRAG_PX_PER_SEMITONE),
        ("startNote/range/0", START_NOTE_RANGE[0] as f32),
        ("startNote/range/1", START_NOTE_RANGE[1] as f32),
    ];
}
