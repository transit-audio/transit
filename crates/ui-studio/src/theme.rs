//! Theme tokens (plan 06 slice 5, ADR 0045): the device design language's dark set and a light set
//! derived from it, picked by View > Appearance (System follows the OS). Flat: graphite grounds,
//! one signal color for what you set, one mod color for modulation, no gradients, no shadows
//! beyond a 1 px separator. The values come from [`crate::tokens`], which mirrors tokens.json.
//!
//! Every color the studio UI paints comes from [`TOKENS`], which reads the theme of the frame
//! being drawn ([`set_current`], per thread, so tests on parallel threads stay hermetic).

use egui::Color32;
use std::cell::Cell;

/// View > Appearance: follow the OS, or pin dark or light. Kept in `UiState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Dark, Appearance::Light];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }

    fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Dark => egui::ThemePreference::Dark,
            Appearance::Light => egui::ThemePreference::Light,
        }
    }
}

/// Color tokens. Everything the studio UI paints takes its colors from here. The design names
/// come first (`docs/design/language`, [`crate::tokens`]); the older names after them read the
/// same values where the design has one (ADR 0045).
pub struct Tokens {
    /// `bg.window`: the window ground.
    pub bg_window: Color32,
    /// `bg.titlebar`: title bars, routing-row fill.
    pub bg_titlebar: Color32,
    /// `surface.panel`: panels.
    pub surface_panel: Color32,
    /// `surface.raised`: chips, model buttons, dropdowns on panels.
    pub surface_raised: Color32,
    /// `surface.inset`: display wells, value fields, segmented tracks.
    pub surface_inset: Color32,
    /// `surface.selected`: the selected segment.
    pub surface_selected: Color32,
    /// `knob.cap`: the knob face.
    pub knob_cap: Color32,
    /// `knob.capActive`: the knob face while held; the fader cap.
    pub knob_cap_active: Color32,
    /// `line.inset`: display-well borders.
    pub line_inset: Color32,
    /// `line.strong`: knob rim, pills, dashed add-row, the switch's off track.
    pub line_strong: Color32,
    /// `grid`: grid lines inside displays.
    pub grid: Color32,
    /// `text.muted`: control labels, section labels.
    pub text_muted: Color32,
    /// `text.faint`: meta, units, column headers.
    pub text_faint: Color32,
    /// `ink.ghost`: secondary traces; a disabled knob's arc.
    pub ink_ghost: Color32,
    /// `ink.arrow`: routing arrows, unused source dots; a disabled knob's pointer.
    pub ink_arrow: Color32,
    /// `signal`: what you set (value arcs, curves, selected outline, switch on).
    pub signal: Color32,
    /// `mod` (a Rust keyword, hence the long name): modulation rings, LFO traces, amounts, GR.
    pub modulation: Color32,
    /// `meter.ok`: level meters.
    pub meter_ok: Color32,
    /// `meter.hot`: clip and over only.
    pub meter_hot: Color32,
    /// The monitor and option toggles' on fill (`toggle.on.option.fill`).
    pub toggle_option_fill: Color32,
    /// An unavailable toggle's fill (`toggle.unavailable.fill`).
    pub toggle_unavailable_fill: Color32,
    /// The switch's knob when on (`switch.onKnob`).
    pub switch_on_knob: Color32,
    /// `bg.window`.
    pub bg: Color32,
    /// `surface.panel`.
    pub panel: Color32,
    /// `surface.raised`.
    pub panel_raised: Color32,
    pub header: Color32,
    pub header_selected: Color32,
    pub lane: Color32,
    pub lane_alt: Color32,
    pub ruler: Color32,
    /// `line`: panel borders, arc tracks, and the 1 px separator between areas (the only
    /// "shadow" there is).
    pub line: Color32,
    pub grid_bar: Color32,
    pub grid_beat: Color32,
    /// Sub-beat grid lines (1/8 and finer): fainter than a beat line.
    pub grid_sub: Color32,
    /// `text`: primary text, values, pointers.
    pub text: Color32,
    /// `text.muted`.
    pub text_dim: Color32,
    /// `signal`, the one accent: selection, focus, active values.
    pub accent: Color32,
    /// The loop/cycle region: strip fill and handles, ruler cycle tint, the Cycle button when on.
    /// A yellowish amber (not the accent) so looping reads apart from selection; darker in light.
    pub loop_region: Color32,
    pub cursor: Color32,
    /// Record arm and the clip cap: `meter.hot`.
    pub record: Color32,
    /// Mute when on: `text` (the toggle is inverted).
    pub mute: Color32,
    /// Solo when on: `signal`.
    pub solo: Color32,
    /// Input monitoring and meter levels: `meter.ok`.
    pub monitor: Color32,
    /// `surface.raised`.
    pub button: Color32,
    /// The hover token: fill for hovered buttons and rows, one step above `surface.raised`
    /// (`knob.capActive`).
    pub button_hover: Color32,
    pub selection_outline: Color32,
    /// Keyboard focus ring (and the active pane's outline): `signal`.
    pub focus_ring: Color32,
    /// Dims what lies outside a range (piano roll past the clip end).
    pub overlay: Color32,
    /// Text on light fills and on dark fills ([`text_on`]); the dark ink is `bg.window`.
    pub ink_dark: Color32,
    pub ink_light: Color32,
    /// Light theme: clip bodies are the track color lightened, the name strip the track color.
    pub light_clips: bool,
}

/// One token set from the constants in scope (the dark or the light names of [`crate::tokens`]).
macro_rules! token_set {
    ($light_clips:expr) => {
        Tokens {
            bg_window: BG_WINDOW,
            bg_titlebar: BG_TITLEBAR,
            surface_panel: SURFACE_PANEL,
            surface_raised: SURFACE_RAISED,
            surface_inset: SURFACE_INSET,
            surface_selected: SURFACE_SELECTED,
            knob_cap: KNOB_CAP,
            knob_cap_active: KNOB_CAP_ACTIVE,
            line_inset: LINE_INSET,
            line_strong: LINE_STRONG,
            grid: GRID,
            text_muted: TEXT_MUTED,
            text_faint: TEXT_FAINT,
            ink_ghost: INK_GHOST,
            ink_arrow: INK_ARROW,
            signal: SIGNAL,
            modulation: MOD,
            meter_ok: METER_OK,
            meter_hot: METER_HOT,
            toggle_option_fill: TOGGLE_OPTION_FILL,
            toggle_unavailable_fill: TOGGLE_UNAVAILABLE_FILL,
            switch_on_knob: SWITCH_ON_KNOB,
            bg: BG_WINDOW,
            panel: SURFACE_PANEL,
            panel_raised: SURFACE_RAISED,
            header: HEADER,
            header_selected: HEADER_SELECTED,
            lane: LANE,
            lane_alt: LANE_ALT,
            ruler: RULER,
            line: LINE,
            grid_bar: GRID_BAR,
            grid_beat: GRID_BEAT,
            grid_sub: GRID_SUB,
            text: TEXT,
            text_dim: TEXT_MUTED,
            accent: SIGNAL,
            loop_region: LOOP_REGION,
            cursor: CURSOR,
            record: RECORD,
            mute: MUTE,
            solo: SOLO,
            monitor: MONITOR,
            button: SURFACE_RAISED,
            button_hover: BUTTON_HOVER,
            selection_outline: SELECTION_OUTLINE,
            focus_ring: FOCUS_RING,
            overlay: OVERLAY,
            ink_dark: INK_DARK,
            ink_light: INK_LIGHT,
            light_clips: $light_clips,
        }
    };
}

/// The design (`color` and `arrangement` in tokens.json).
pub const DARK: Tokens = {
    use crate::tokens::arrangement::*;
    use crate::tokens::color::*;
    use crate::tokens::component::*;
    token_set!(false)
};

/// Derived from the design (`colorLight` in tokens.json): grounds mirrored, inks at least as
/// legible as in the dark set. Whether light ships in v0.7.0 is open (ADR 0045).
pub const LIGHT: Tokens = {
    use crate::tokens::color_light::*;
    token_set!(true)
};

thread_local! {
    static LIGHT_NOW: Cell<bool> = const { Cell::new(false) };
}

/// The tokens of the theme being drawn: `TOKENS.accent` reads dark or light.
pub struct Current;

impl std::ops::Deref for Current {
    type Target = Tokens;
    fn deref(&self) -> &Tokens {
        if LIGHT_NOW.with(Cell::get) { &LIGHT } else { &DARK }
    }
}

pub static TOKENS: Current = Current;

/// Draw with `theme`'s tokens from now on (this thread).
pub fn set_current(theme: egui::Theme) {
    LIGHT_NOW.with(|c| c.set(theme == egui::Theme::Light));
}

/// The theme the tokens currently read.
pub fn current() -> egui::Theme {
    if LIGHT_NOW.with(Cell::get) { egui::Theme::Light } else { egui::Theme::Dark }
}

/// The image tint that leaves a texture's own colors alone (the wordmark is never recolored).
pub const UNTINTED: Color32 = Color32::WHITE;

/// The spacing grid: margins and gaps are multiples of 8 px (4 px half steps inside dense rows).
pub const SPACE: f32 = 8.0;
/// Corner radius of buttons, cards, clips, menus and windows.
pub const RADIUS: f32 = 4.0;
/// Corner radius of small controls (header toggles, readouts, meters, send rows).
pub const RADIUS_SM: f32 = 2.0;
/// Keyboard focus ring width.
pub const FOCUS_W: f32 = 1.5;

/// Rounded top corners only (a card's title, a clip's name strip).
pub fn top_corners() -> egui::CornerRadius {
    let r = RADIUS as u8;
    egui::CornerRadius { nw: r, ne: r, sw: 0, se: 0 }
}

/// Track list row metrics (Cubase-like density).
pub const RULER_H: f32 = 34.0;
pub const TRANSPORT_H: f32 = 40.0;
pub const STATUS_H: f32 = 22.0;
pub const CLIP_NAME_H: f32 = 14.0;

/// egui's own widgets (menus, buttons, windows, combo boxes, scroll bars) in `t`'s colors, on the
/// design language's shapes (ADR 0045): flat graphite, 1 px `line` separators and borders, the
/// panel radius on windows and the control radius on menus and widgets, no shadows.
fn visuals(t: &Tokens, light: bool) -> egui::Visuals {
    use crate::tokens::radius;
    let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    let line = egui::Stroke::new(1.0, t.line);
    v.panel_fill = t.surface_panel;
    v.window_fill = t.surface_raised;
    // Text fields and scroll tracks are value fields: the inset well.
    v.extreme_bg_color = t.surface_inset;
    v.text_edit_bg_color = Some(t.surface_inset);
    v.code_bg_color = t.surface_inset;
    v.faint_bg_color = t.lane_alt;
    v.weak_text_color = Some(t.text_muted);
    v.selection.bg_fill = t.signal.gamma_multiply(if light { 0.35 } else { 0.6 });
    v.selection.stroke = egui::Stroke::new(FOCUS_W, t.focus_ring);
    v.hyperlink_color = t.signal;
    v.warn_fg_color = t.modulation;
    v.error_fg_color = t.meter_hot;
    v.window_stroke = line;
    v.widgets.noninteractive.bg_fill = t.surface_panel;
    v.widgets.noninteractive.weak_bg_fill = t.surface_panel;
    v.widgets.noninteractive.bg_stroke = line;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.surface_raised;
    v.widgets.inactive.weak_bg_fill = t.surface_raised;
    v.widgets.inactive.bg_stroke = line;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.button_hover;
    v.widgets.hovered.weak_bg_fill = t.button_hover;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, t.line_strong);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.active.bg_fill = t.surface_selected;
    v.widgets.active.weak_bg_fill = t.surface_selected;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, t.signal);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, t.text);
    v.widgets.open.bg_fill = t.button_hover;
    v.widgets.open.weak_bg_fill = t.button_hover;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0, t.line_strong);
    v.widgets.open.fg_stroke = egui::Stroke::new(1.0, t.text);
    // Flat: no shadows (the 1 px separator is the only edge).
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.window_corner_radius = egui::CornerRadius::same(radius::PANEL as u8);
    v.menu_corner_radius = egui::CornerRadius::same(radius::CONTROL as u8);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = egui::CornerRadius::same(radius::CONTROL as u8);
        w.expansion = 0.0;
    }
    v
}

/// The spacing grid on egui's own widgets.
fn spacing(st: &mut egui::Style) {
    let sp = &mut st.spacing;
    sp.item_spacing = egui::vec2(SPACE, SPACE / 2.0);
    sp.button_padding = egui::vec2(SPACE, SPACE / 4.0);
    sp.menu_margin = egui::Margin::same(SPACE as i8);
    sp.window_margin = egui::Margin::same(SPACE as i8);
    sp.indent = 2.0 * SPACE;
    // egui's window close button is `icon_width` square (so are checkboxes and radio buttons).
    sp.icon_width = WINDOW_CLOSE_SIZE;
    // egui sizes a window's title bar, and the close button inside it, from `TextStyle::Heading`.
    // transit draws no other heading, so Heading is the window-title size too (#206).
    let title = egui::FontId::proportional(WINDOW_TITLE_SIZE);
    st.text_styles.insert(window_title_style(), title.clone());
    st.text_styles.insert(egui::TextStyle::Heading, title);
}

/// Window title text: Body (13) plus one point, so a title reads as a label of the window and
/// not as a heading, at 1.0 and at 1.5 scale (the bar scales with the text and the margins).
pub const WINDOW_TITLE_SIZE: f32 = 14.0;
/// The close button's hit area, points (egui's default is 14; 22 would also grow every checkbox).
pub const WINDOW_CLOSE_SIZE: f32 = 18.0;
/// Margin above a window's title text (the content margin stays [`SPACE`]). The bar is 26 px at
/// 1.0 scale: the title row (17 px) plus [`WINDOW_TITLE_PAD_TOP`] plus [`WINDOW_TITLE_PAD_BOTTOM`]
/// plus the 2 px of stroke and slack egui adds (#232; #206 had 2 and 2, a 23 px bar).
pub const WINDOW_TITLE_PAD_TOP: i8 = 3;
/// Margin below a window's title text, see [`WINDOW_TITLE_PAD_TOP`].
pub const WINDOW_TITLE_PAD_BOTTOM: i8 = 4;
/// Margin between a window's side edge and the chevron or close hit box (was [`SPACE`], 8, in
/// #206). The title text keeps egui's item spacing (8 px) from the chevron.
pub const WINDOW_TITLE_PAD_X: i8 = 10;

/// The text style of window titles (registered by [`apply`]).
pub fn window_title_style() -> egui::TextStyle {
    egui::TextStyle::Name("window-title".into())
}

/// Every internal window in ui-studio starts here (a source-scan test forbids the plain
/// `egui::Window` constructor elsewhere): its title is in the window-title text style and its
/// title bar has a tight vertical margin; the content keeps the theme's window margin (#206). Its
/// collapse and close controls are Lucide icons ([`crate::title_bar`], #227).
pub fn window<'a>(ctx: &egui::Context, title: impl Into<String>) -> crate::title_bar::Window<'a> {
    let style = ctx.global_style();
    let title_frame = egui::Frame::window(&style).inner_margin(egui::Margin {
        left: WINDOW_TITLE_PAD_X,
        right: WINDOW_TITLE_PAD_X,
        top: WINDOW_TITLE_PAD_TOP,
        bottom: WINDOW_TITLE_PAD_BOTTOM,
    });
    // A context the theme was never applied to (a fresh one in a test) has no such style.
    let text = egui::RichText::new(title);
    let text =
        if style.text_styles.contains_key(&window_title_style()) { text.text_style(window_title_style()) } else { text.size(WINDOW_TITLE_SIZE) };
    crate::title_bar::Window::new(egui::Window::new(text).title_frame(title_frame))
}

/// A dialog: [`window`] over a full-screen scrim that takes the pointer (#244). The scrim is an
/// invisible, interactable `Area` in the background order (below every window, above the panels),
/// so the views behind the dialog see another layer under the pointer: their wheel, zoom and drag
/// handling asks `Ui::rect_contains_pointer`, which is false there, and clicks cannot reach them
/// either. A wheel over the dialog itself reaches only the dialog's own scroll areas. Call it each
/// frame the dialog is shown, as `window`.
pub fn modal_window<'a>(ctx: &egui::Context, title: impl Into<String>) -> crate::title_bar::Window<'a> {
    egui::Area::new(egui::Id::new("modal_scrim"))
        .order(egui::Order::Background)
        .fixed_pos(ctx.content_rect().min)
        .sense(egui::Sense::click_and_drag())
        .show(ctx, |ui| {
            let r = ctx.content_rect();
            ui.set_clip_rect(r);
            ui.allocate_rect(r, egui::Sense::click_and_drag());
        });
    window(ctx, title)
}

/// Install both themes' visuals and follow `appearance` (System: the OS setting, through egui's
/// `system_theme`). Call when the appearance changes; [`follow`] runs every frame.
pub fn apply(ctx: &egui::Context, appearance: Appearance) {
    for (theme, tokens) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        ctx.style_mut_of(theme, |st| {
            st.visuals = visuals(tokens, theme == egui::Theme::Light);
            spacing(st);
        });
    }
    ctx.set_theme(appearance.preference());
    set_current(ctx.theme());
}

/// Point the tokens at the theme egui resolved for this frame (System follows the OS live).
pub fn follow(ctx: &egui::Context) {
    set_current(ctx.theme());
}

/// White (`toward_white`) or black at `alpha`: a translucent tint that lightens or darkens whatever
/// it is painted over.
pub fn tint(toward_white: bool, alpha: u8) -> Color32 {
    if toward_white { Color32::from_white_alpha(alpha) } else { Color32::from_black_alpha(alpha) }
}

/// WCAG relative luminance of an opaque color.
pub fn luminance(c: Color32) -> f32 {
    let lin = |x: u8| {
        let v = f32::from(x) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two opaque colors (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A readable text color on `bg`: whichever of the dark and light inks contrasts more.
pub fn text_on(bg: Color32) -> Color32 {
    let t = &TOKENS;
    if contrast(t.ink_dark, bg) >= contrast(t.ink_light, bg) { t.ink_dark } else { t.ink_light }
}

/// A clip's colors from its track color: (body, name strip, name text). Dark theme: the track
/// color as the body under a darker strip; light theme: a lightened body under a strip in the
/// track color. The strip is nudged darker or lighter until its name reads at 4.5:1.
pub fn clip_colors(base: Color32, selected: bool) -> (Color32, Color32, Color32) {
    let t = &TOKENS;
    let (body, mut strip) = if t.light_clips {
        let body = lighten(base, if selected { 0.25 } else { 0.4 });
        (body, if selected { darken(base, 0.08) } else { base })
    } else {
        let body = if selected { lighten(base, 0.12) } else { base };
        (body, darken(body, 0.22))
    };
    for _ in 0..20 {
        let ink = text_on(strip);
        if contrast(ink, strip) >= 4.5 {
            break;
        }
        strip = if ink == t.ink_light { darken(strip, 0.06) } else { lighten(strip, 0.06) };
    }
    (body, strip, text_on(strip))
}

/// `c` mixed toward black by `k` (0..1).
pub fn darken(c: Color32, k: f32) -> Color32 {
    let f = (1.0 - k).clamp(0.0, 1.0);
    Color32::from_rgb((f32::from(c.r()) * f) as u8, (f32::from(c.g()) * f) as u8, (f32::from(c.b()) * f) as u8)
}

/// `c` mixed toward white by `k` (0..1).
pub fn lighten(c: Color32, k: f32) -> Color32 {
    let k = k.clamp(0.0, 1.0);
    let m = |x: u8| (f32::from(x) + (255.0 - f32::from(x)) * k) as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The family apps/transit installs for numbers (IBM Plex Mono Medium).
const MONO_MEDIUM: &str = crate::tokens::FAMILY_MONO_MEDIUM;

/// Notes, once a frame, which of the design's font families exist (an egui font id naming a
/// family that was not installed panics, and tests and `--no-fonts` runs have only the defaults).
pub fn probe_fonts(ctx: &egui::Context) {
    let mask = ctx.fonts(|f| {
        let families = f.families();
        crate::tokens::FAMILIES
            .iter()
            .enumerate()
            .filter(|(_, n)| families.contains(&egui::FontFamily::Name((**n).into())))
            .fold(0u8, |m, (i, _)| m | 1 << i)
    });
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("transit-font-families"), mask));
}

/// Whether the app installed the font family `name` (as of the last [`probe_fonts`]).
pub fn has_family(ctx: &egui::Context, name: &str) -> bool {
    let mask = ctx.data(|d| d.get_temp::<u8>(egui::Id::new("transit-font-families"))).unwrap_or(0);
    crate::tokens::FAMILIES.iter().position(|n| *n == name).is_some_and(|i| mask & 1 << i != 0)
}

/// Digits for clocks and readouts: `mono-medium` where the app installed it, else Monospace.
pub fn mono(ctx: &egui::Context, size: f32) -> egui::FontId {
    if has_family(ctx, MONO_MEDIUM) { egui::FontId::new(size, egui::FontFamily::Name(MONO_MEDIUM.into())) } else { egui::FontId::monospace(size) }
}
