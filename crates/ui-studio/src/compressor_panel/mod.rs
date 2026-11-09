//! The Compressor's editor in the design language (#326, job 326-K), the second device after the
//! Macro Oscillator: the maintainer's board (`docs/design/language/mockups/Compressor.dc.html`)
//! drawn from the widget kit and the tokens, in the shared device window
//! ([`crate::device_editor::window`], content-sized, no resize handle).
//!
//! Anatomy (README, Device anatomy): A, the shared 52 px title bar
//! ([`crate::device_editor::title_bar`]); B, the hero panel ([`hero`]): the transfer curve in a
//! display well with the unity diagonal and the operating point, the level and gain-reduction
//! history well ([`history`]), and the gain-reduction meter with its readout; C, the controls
//! panel ([`sections`]): the dynamics knobs (Threshold, Ratio, Knee,
//! Attack, Release, Makeup, Mix, the board's order), then beside a divider the sidechain key and
//! the mode (Peak or Limit); then the Detector section bar (lookahead and the sidechain's
//! high-pass, the parameters the board does not place), collapsed while they are at rest; then the
//! footer with the load readout.
//!
//! Every control writes through `StudioApp::set_builtin_param` (merged while the pointer is held,
//! so a drag is one undo step), takes MIDI Learn, and records a [`RowLayout`] in the window's
//! [`EditorLayout`], so the editor's existing checks read it the way they read the other panels.
//! The panel's own record is [`CompressorLayout`].

mod hero;
pub mod history;
mod sections;
pub mod window;

#[cfg(test)]
mod collapse_tests;
#[cfg(test)]
mod hero_tests;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod sections_tests;
#[cfg(test)]
mod test_rig;
#[cfg(test)]
mod transfer_tests;
#[cfg(test)]
mod width_tests;
#[cfg(test)]
mod window_tests;

use crate::StudioApp;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::device_editor::title_bar::TitleLayout;
use crate::device_editor::{Dev, bipolar, child, set};
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, knob, r4};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_model::TrackId;

pub use hero::{DotRecord, UnityRecord, transfer_db, transfer_gr_db};

/// The Compressor's plug-in id.
pub const COMPRESSOR: &str = "compressor";

/// The board's width: the content lays out at this width and narrower windows shrink the flexible
/// columns (the history well, the sidechain column) down to [`MIN_WIDTH`].
pub const DESIGN_WIDTH: f32 = 960.0;
/// The narrowest the window's content lays out; below it the window keeps this width. Set by the
/// content (`width_tests`): the narrowest multiple of 8 at which, with the Detector section open
/// and every knob at the widest value it prints, no label or readout meets its neighbor or leaves
/// its panel and the sidechain label clears its dropdown.
pub const MIN_WIDTH: f32 = 888.0;
/// The footer under the content.
pub const FOOTER_H: f32 = 32.0;

/// Whether `plugin`'s editor is this panel.
pub fn handles(plugin: &str) -> bool {
    plugin == COMPRESSOR
}

/// One knob as last drawn: the parameter it reads, its label and the kit's record.
#[derive(Debug, Clone)]
pub struct KnobRecord {
    pub param: String,
    pub label: String,
    pub knob: knob::KnobLayout,
}

/// A display well as last drawn: `transfer` or `history`.
#[derive(Debug, Clone)]
pub struct WellRecord {
    pub name: String,
    pub well: widgets::display_well::WellLayout,
    /// The grid lines drawn inside it (the quarters, in `grid`).
    pub grid: Vec<[f32; 4]>,
}

/// The hero panel as last drawn.
#[derive(Debug, Clone, Default)]
pub struct HeroLayout {
    pub panel: [f32; 4],
    /// The column labels (`TRANSFER`, `HISTORY`) and where they sit.
    pub labels: Vec<(String, [f32; 4])>,
    /// The transfer curve's points (normalized: input across, output up, both −60 to 0 dB).
    pub curve: Vec<[f32; 2]>,
    /// The gain-reduction meter (the kit's bar), its displayed reduction in dB, the readout and
    /// where the readout sits.
    pub gr_meter: Option<widgets::level_meter::MeterLayout>,
    pub gr_db: f32,
    pub gr_readout: String,
    pub gr_readout_rect: [f32; 4],
    /// The transfer well's unity diagonal.
    pub unity: Option<UnityRecord>,
    /// The operating point (`None` while hidden: nothing published yet, or below −60 dB).
    pub dot: Option<DotRecord>,
    /// The history well's traces, threshold, time labels and legend.
    pub history: history::HistoryRecord,
}

/// The sidechain key as last drawn: its label, the dropdown and the text it shows.
#[derive(Debug, Clone, Default)]
pub struct KeyRecord {
    pub label: [f32; 4],
    pub dropdown: [f32; 4],
    pub text: String,
    /// The list's rows while it is open (their text and rect), empty while closed.
    pub list: Vec<(String, [f32; 4])>,
}

/// The whole panel as last drawn, for `ui.inspect`-style checks and the tests.
#[derive(Debug, Clone, Default)]
pub struct CompressorLayout {
    pub window: [f32; 4],
    pub title: TitleLayout,
    pub hero: HeroLayout,
    /// Every panel card: its name (`hero`, `controls`, `detector`) and rect.
    pub panels: Vec<(String, [f32; 4])>,
    /// The sections of the controls panel (`dynamics`, `sidechain`, `mode`) and the Detector
    /// section's body: name and rect.
    pub sections: Vec<(String, [f32; 4])>,
    pub knobs: Vec<KnobRecord>,
    pub wells: Vec<WellRecord>,
    /// The mode's segmented control (Peak, Limit).
    pub mode: Option<widgets::segmented::SegmentedLayout>,
    pub mode_options: Vec<String>,
    /// The Detector section's SC HPF toggle.
    pub hpf_toggle: Option<widgets::toggle::ToggleLayout>,
    pub key: KeyRecord,
    /// The 1 px divider between the knobs and the sidechain column.
    pub divider: [f32; 4],
    pub detector_bar: Option<widgets::section_bar::SectionBarLayout>,
    /// The Detector section's body (`None` while collapsed).
    pub detector_body: Option<[f32; 4]>,
    pub load: String,
    pub footer: [f32; 4],
}

impl CompressorLayout {
    /// The knob that reads `param`.
    pub fn knob(&self, param: &str) -> Option<&KnobRecord> {
        self.knobs.iter().find(|k| k.param == param)
    }

    /// The well named `name`.
    pub fn well(&self, name: &str) -> Option<&WellRecord> {
        self.wells.iter().find(|w| w.name == name)
    }

    /// The section or panel named `name`.
    pub fn section(&self, name: &str) -> Option<[f32; 4]> {
        self.sections.iter().chain(&self.panels).find(|s| s.0 == name).map(|s| s.1)
    }
}

/// A color the compressor's tokens name (`compressor` in tokens.json), as the theme draws it.
pub(crate) fn named(name: &str) -> Color32 {
    let t = &TOKENS;
    match name {
        "line" => t.line,
        "grid" => t.grid,
        "text" => t.text,
        "surface.inset" => t.surface_inset,
        "ink.ghost" => t.ink_ghost,
        "signal" => t.signal,
        "mod" => t.modulation,
        _ => Color32::MAGENTA,
    }
}

/// What every control does after drawing: its MIDI Learn menu and mark, the value it set, and its
/// row in the window's record (`control` is the rect a row's click lands on).
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish(
    app: &mut StudioApp,
    ui: &Ui,
    dev: Dev,
    id: &str,
    resp: &egui::Response,
    control: Rect,
    new: Option<f32>,
    lay: &mut EditorLayout,
) {
    let Some(p) = dev.param(id) else { return };
    if let Some(c) = crate::midi_learn::plugin_control(dev.key.1, p.id) {
        crate::chrome::context_menu(resp, app.learn_checks(dev.key.0, &c))
            .id(resp.id.with("learn_ctx"))
            .show(|ui| app.midi_learn_menu(ui, dev.key.0, c.clone()));
        crate::midi_learn::mark(app, ui.painter(), resp.rect, dev.key.0, c);
    }
    if let Some(v) = new {
        set(app, dev, p.id, v);
    }
    let shown = p.format(new.map_or_else(|| dev.value(app, p.id), |v| p.clamp(v)));
    lay.rows.push(RowLayout { param: p.id.to_string(), control: r4(control), value: shown });
}

/// A panel card: `surface.panel`, 1 px `line`, the panel radius. Returns the inner rect (the
/// panel padding, 14 by 16, inside).
pub(crate) fn panel(ui: &Ui, rect: Rect, name: &str, out: &mut CompressorLayout) -> Rect {
    let t = &TOKENS;
    ui.painter().rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    out.panels.push((name.to_string(), r4(rect)));
    rect.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]))
}

/// A column label (`label.section`) at `at` (its top left); returns its rect.
pub(crate) fn section_label(ui: &Ui, at: Pos2, text: &str) -> Rect {
    widgets::role_text(ui.painter(), &tokens::LABEL_SECTION, text, TOKENS.text_muted, Align2::LEFT_TOP, at)
}

/// The id of the knob of `param` on the insert `uid` (its value field is `id.with("editing")`).
pub fn knob_id(uid: u64, param: &str) -> egui::Id {
    egui::Id::new(("compressor_knob", uid, param))
}

/// A knob (size M) for parameter `id` with its top left at `at`, mapped through the parameter's
/// range and taper.
#[allow(clippy::too_many_arguments)]
pub(crate) fn knob_at(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    id: &str,
    label: &str,
    at: Pos2,
    lay: &mut EditorLayout,
    out: &mut CompressorLayout,
) -> Rect {
    let size = knob::Size::M;
    let cell = Rect::from_min_size(at, knob::cell(size));
    let Some(p) = dev.param(id) else { return cell };
    let v = dev.value(app, id);
    let text = p.format(v);
    let k = knob::Knob {
        size,
        label,
        value_text: &text,
        value: p.to_normalized(v),
        default: p.to_normalized(p.default),
        bipolar: bipolar(p),
        modulation: None,
        disabled: false,
    };
    let mut c = child(ui, cell);
    let shown = knob::show(&mut c, knob_id(dev.uid(), p.id), &k);
    let new = crate::device_editor::knob_value(p, &shown);
    if shown.wheeled {
        // A wheel gesture's nudges merge into one undo step while the wheel keeps coming.
        app.builtin_editor.devices.wheel_now = true;
    }
    finish(app, &c, dev, p.id, &shown.response, shown.response.rect, new, lay);
    out.knobs.push(KnobRecord { param: p.id.to_string(), label: label.to_string(), knob: shown.layout });
    cell
}

/// The panel's content height at `width` (the window's inside), for the window's size (every
/// frame).
pub fn content_height(app: &StudioApp, _ctx: &egui::Context, key: (TrackId, usize), _width: f32) -> f32 {
    let Some(ins) = app.builtin_at(key) else { return 0.0 };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return 0.0 };
    let dev = Dev { key, ins: &ins, info };
    2.0 * space::WINDOW_PADDING + hero::HEIGHT + space::PANEL_GAP + sections::controls_height() + sections::detector_height(app, dev)
}

/// Draws the panel's body (below the title bar, above the footer) into `ui`.
pub(crate) fn body(app: &mut StudioApp, ui: &mut Ui, dev: Dev, lay: &mut EditorLayout, out: &mut CompressorLayout) {
    // The window's inside: its width less the frame's edge (never under the minimum's).
    let width = ui.available_width().max(window::min_width(app) - crate::device_window::edge().x);
    let origin = ui.cursor().min;
    let w = width - 2.0 * space::WINDOW_PADDING;
    let x = origin.x + space::WINDOW_PADDING;
    let mut y = origin.y + space::WINDOW_PADDING;
    hero::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, hero::HEIGHT)), lay, out);
    y += hero::HEIGHT + space::PANEL_GAP;
    sections::controls(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, sections::controls_height())), lay, out);
    y += sections::controls_height();
    y += sections::detector(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, 0.0)), lay, out);
    y += space::WINDOW_PADDING;
    ui.allocate_rect(Rect::from_min_max(origin, pos2(origin.x + width, y)), Sense::hover());
}

/// The footer: the audio load on the right (`text.faint`, 11). The Compressor is transit's own,
/// so the footer carries no credit.
pub(crate) fn footer(app: &StudioApp, ui: &mut Ui, rect: Rect, _lay: &mut EditorLayout, out: &mut CompressorLayout) {
    let t = &TOKENS;
    let p = ui.painter();
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.line));
    out.footer = r4(rect);
    out.load = format!("Audio load {}", crate::load_readout::percent_text(app.telemetry.snap.current_load_ppm));
    widgets::role_text(
        p,
        &tokens::READOUT_DISPLAY,
        &out.load,
        t.text_faint,
        Align2::RIGHT_CENTER,
        pos2(rect.max.x - space::WINDOW_PADDING, rect.center().y),
    );
}
