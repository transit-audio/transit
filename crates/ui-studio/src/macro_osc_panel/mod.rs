//! The Macro Oscillator's editor in the design language (#326, job 326-M): the maintainer's board
//! (`docs/design/language/mockups/Main.dc.html`) drawn entirely from the widget kit and the
//! tokens, in a floating window of its own ([`window`]).
//!
//! Anatomy (README, Device anatomy): A, the 52 px title bar ([`title_bar`]); B, the hero row: the
//! oscillator panel (model card, scope, Timbre/Morph pad, model picker) beside the module's own
//! sections Tone, Gate · LPG, Pitch and Output ([`hero`], [`sections`]); C, the Shaper behind its
//! section bar ([`shaper`]); D, the Modulation section bar, source chips and routing rows
//! ([`modulation`]); then the footer with the credit and the load readout.
//!
//! Every control writes through `StudioApp::set_builtin_param` (merged while the pointer is held,
//! so a drag is one undo step), takes MIDI Learn and its help entry, and records a
//! [`RowLayout`] in the window's [`EditorLayout`], so the editor's existing checks read it the way
//! they read the other panels. The panel's own record is [`MacroLayout`].

mod hero;
mod modulation;
mod sections;
mod shaper;
pub(crate) mod wave;
pub mod window;

#[cfg(test)]
mod hero_tests;
#[cfg(test)]
mod modulation_tests;
#[cfg(test)]
mod sections_tests;
#[cfg(test)]
mod shaper_tests;
#[cfg(test)]
mod test_rig;
#[cfg(test)]
mod title_bar_tests;
#[cfg(test)]
mod width_tests;
#[cfg(test)]
mod window_tests;

use crate::StudioApp;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, knob, r4};
use egui::{Align2, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_dsp::plugins::macro_osc::matrix::{ROWS, dst};
use transit_dsp::plugins::macro_osc::row_param;
use transit_model::TrackId;

#[cfg(test)]
pub(crate) use crate::device_editor::parse;
pub use crate::device_editor::title_bar::TitleLayout;
pub(crate) use crate::device_editor::{Dev, Lead, bipolar, child, dropdown, set, value_at};
pub use modulation::{ChipRecord, RouteRecord};

/// The Macro Oscillator's plug-in id (ADR 0033, #195).
pub const MACRO_OSC: &str = "macro_oscillator";

/// The credit line in the footer (ADR 0033: the credit is required).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source macro oscillator (MIT)";

/// The board's width: the content lays out on its 12-column grid at this width and narrower
/// windows shrink the fractional columns down to [`MIN_WIDTH`].
pub const DESIGN_WIDTH: f32 = 1440.0;
/// The narrowest the window's content lays out; below it the window keeps this width.
/// Set by the content (326-X, `width_tests`): the narrowest multiple of 8 at which, with every
/// section open and every knob at the widest value it prints, no label or readout meets its
/// neighbor or leaves its panel.
pub const MIN_WIDTH: f32 = 1192.0;
/// The footer under the content.
pub const FOOTER_H: f32 = 32.0;

/// Whether `plugin`'s editor is this panel (the other built-ins keep the older panel system until
/// their own migrations).
pub fn handles(plugin: &str) -> bool {
    plugin == MACRO_OSC
}

/// The help entry of a control, by parameter id (unchanged from the older panel: a control that
/// kept its place kept its id).
pub fn help(param: &str) -> Option<crate::help::Id> {
    use crate::help::Id;
    Some(match param {
        "model" => Id::MacroOscModel,
        "harmonics" => Id::MacroOscHarmonics,
        "timbre" => Id::MacroOscTimbre,
        "morph" => Id::MacroOscMorph,
        "decay" => Id::MacroOscDecay,
        "lpg_color" => Id::MacroOscLpgColor,
        "aux_mix" => Id::MacroOscAuxMix,
        "level" => Id::MacroOscLevel,
        "tune" => Id::MacroOscTune,
        "bend_range" => Id::MacroOscBendRange,
        "mod_wheel" => Id::MacroOscModWheel,
        "voices" => Id::MacroOscVoices,
        "shaper" => Id::MacroOscShaper,
        "amp_attack" | "amp_decay" | "amp_sustain" | "amp_release" => Id::MacroOscAmpAdsr,
        "amp_velocity" => Id::MacroOscAmpVelocity,
        "filter_mode" => Id::MacroOscFilterMode,
        "filter_slope" => Id::MacroOscFilterSlope,
        "cutoff" => Id::MacroOscCutoff,
        "resonance" => Id::MacroOscResonance,
        "drive" => Id::MacroOscDrive,
        "key_track" => Id::MacroOscKeyTrack,
        "filter_env_amount" => Id::MacroOscFilterEnvAmount,
        "filter_attack" | "filter_decay" | "filter_sustain" | "filter_release" => Id::MacroOscFilterAdsr,
        _ => return numbered_help(param),
    })
}

/// The help entry of an LFO control (`lfo<n>_<field>`) or a matrix row's (`mod<n>_<field>`).
fn numbered_help(param: &str) -> Option<crate::help::Id> {
    use crate::help::Id;
    let (head, field) = param.split_once('_')?;
    if head.strip_prefix("lfo").is_some_and(|n| n == "1" || n == "2") {
        return Some(match field {
            "shape" => Id::MacroOscLfoShape,
            "rate" => Id::MacroOscLfoRate,
            "sync" => Id::MacroOscLfoSync,
            "division" => Id::MacroOscLfoDivision,
            "retrigger" => Id::MacroOscLfoRetrigger,
            "fade" => Id::MacroOscLfoFade,
            "amount" => Id::MacroOscLfoAmount,
            _ => return None,
        });
    }
    head.strip_prefix("mod")?.parse::<usize>().ok().filter(|n| (1..=ROWS).contains(n))?;
    Some(match field {
        "source" => Id::MacroOscMatrixSource,
        "via" => Id::MacroOscMatrixVia,
        "dest" => Id::MacroOscMatrixDest,
        "amount" => Id::MacroOscMatrixAmount,
        "curve" => Id::MacroOscMatrixCurve,
        _ => return None,
    })
}

/// One knob as last drawn: the parameter it reads, the text it printed and the kit's record.
#[derive(Debug, Clone)]
pub struct KnobRecord {
    pub param: String,
    pub label: String,
    pub knob: knob::KnobLayout,
}

/// A segmented control as last drawn, by the parameter it sets.
#[derive(Debug, Clone)]
pub struct SegmentRecord {
    pub param: String,
    pub options: Vec<String>,
    pub segmented: widgets::segmented::SegmentedLayout,
}

/// A toggle as last drawn, by the parameter it sets (`lfo1_sync` for SYNC; HZ is `lfo1_sync`
/// off, recorded as `lfo1_sync:hz`).
#[derive(Debug, Clone)]
pub struct ToggleRecord {
    pub param: String,
    pub toggle: widgets::toggle::ToggleLayout,
}

/// A display well as last drawn: `scope`, `pad`, `amp_env`, `filter`, `lfo1`, `lfo2`.
#[derive(Debug, Clone)]
pub struct WellRecord {
    pub name: String,
    pub well: widgets::display_well::WellLayout,
}

/// The hero row's oscillator panel as last drawn.
#[derive(Debug, Clone, Default)]
pub struct HeroLayout {
    pub panel: [f32; 4],
    /// The model card's name and where it sits.
    pub model_name: String,
    pub model_rect: [f32; 4],
    /// The card's one-line description and where it sits.
    pub model_desc: String,
    pub model_desc_rect: [f32; 4],
    /// The pad's axis labels (the model's Timbre and Morph names).
    pub pad_labels: [String; 2],
    /// The well's readouts: `WAVE · <model>` (`BURST · <model>` for the noise and percussion
    /// group) on the left, the voice count on the right.
    pub readout: String,
    pub voices: String,
    pub scope: [f32; 4],
    /// The rendered picture the well draws (`None` before the first render lands), the note it
    /// was rendered at (`C3` at rest) and the points of its trace.
    pub wave_kind: Option<transit_dsp::plugins::macro_osc::preview::Kind>,
    pub wave_note: String,
    pub wave_points: usize,
    /// The Timbre/Morph pad, its point and its T and M readouts.
    pub pad: [f32; 4],
    pub pad_point: [f32; 2],
    pub pad_trails: Vec<[f32; 4]>,
    pub timbre_text: String,
    pub morph_text: String,
    /// The model picker: model index, button rect, selected; and its group labels.
    pub picker: Vec<(usize, [f32; 4], bool)>,
    /// Each picker button's short name and where it was drawn.
    pub picker_names: Vec<(String, [f32; 4])>,
    pub groups: Vec<(String, [f32; 4])>,
}

/// The whole panel as last drawn, for `ui.inspect`-style checks and the tests.
#[derive(Debug, Clone, Default)]
pub struct MacroLayout {
    pub window: [f32; 4],
    pub title: TitleLayout,
    pub hero: HeroLayout,
    /// Every panel the board draws: its name and rect.
    pub panels: Vec<(String, [f32; 4])>,
    pub knobs: Vec<KnobRecord>,
    pub segments: Vec<SegmentRecord>,
    pub toggles: Vec<ToggleRecord>,
    pub wells: Vec<WellRecord>,
    /// Dropdowns: the parameter, the rect, the text shown.
    pub dropdowns: Vec<(String, [f32; 4], String)>,
    /// The voice lights, one character per voice (`H` held, `R` releasing, `-` free), their rect,
    /// and the readout ("3 voices").
    pub lights: String,
    pub lights_rect: [f32; 4],
    pub shaper_bar: Option<widgets::section_bar::SectionBarLayout>,
    /// The Shaper's body (`None` while collapsed) and the opacity it drew at.
    pub shaper_body: Option<[f32; 4]>,
    pub shaper_opacity: f32,
    /// The filter well's modulation reach: the screen x of the lowest and the highest cutoff the
    /// modulation reaches (`None` when nothing modulates the cutoff, 326-X).
    pub filter_reach: Option<[f32; 2]>,
    /// The filter row's divider and the `FILTER ENV` caption over the envelope's knobs (326-X).
    pub filter_divider: [f32; 4],
    pub filter_env_caption: [f32; 4],
    pub modulation_bar: Option<widgets::section_bar::SectionBarLayout>,
    pub modulation_body: Option<[f32; 4]>,
    pub formula: String,
    pub chips: Vec<ChipRecord>,
    pub routes: Vec<RouteRecord>,
    pub add_route: Option<[f32; 4]>,
    pub credit: String,
    pub credit_rect: [f32; 4],
    pub load: String,
}

impl MacroLayout {
    /// The knob that reads `param`.
    pub fn knob(&self, param: &str) -> Option<&KnobRecord> {
        self.knobs.iter().find(|k| k.param == param)
    }
}

/// The matrix destination a knob's parameter is (`None`: no destination modulates it).
pub fn destination_of(param: &str) -> Option<usize> {
    Some(match param {
        "tune" => dst::PITCH,
        "harmonics" => dst::HARMONICS,
        "timbre" => dst::TIMBRE,
        "morph" => dst::MORPH,
        "decay" => dst::DECAY,
        "level" => dst::LEVEL,
        "cutoff" => dst::CUTOFF,
        "resonance" => dst::RESONANCE,
        "filter_env_amount" => dst::ENV_AMOUNT,
        "lfo1_rate" => dst::LFO1_RATE,
        "lfo2_rate" => dst::LFO2_RATE,
        "lfo1_amount" => dst::LFO1_AMOUNT,
        "lfo2_amount" => dst::LFO2_AMOUNT,
        _ => return None,
    })
}

/// A matrix row's fields as numbers: source, via, destination, amount (percent), curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Route {
    pub source: usize,
    pub via: usize,
    pub dest: usize,
    pub amount: f32,
    pub curve: usize,
}

/// Matrix row `row` (0-based) of the device.
pub(crate) fn route(app: &StudioApp, dev: Dev, row: usize) -> Route {
    let f = |field: usize| row_param(row, field).map_or(0.0, |id| dev.value(app, id));
    let i = |field: usize| f(field).round().max(0.0) as usize;
    Route { source: i(0), via: i(1), dest: i(2), amount: f(3), curve: i(4) }
}

/// A row is in use (shown) when its source or its destination is set.
pub fn active(r: &Route) -> bool {
    r.source != 0 || r.dest != 0
}

/// The modulation ring of a knob whose destination is `dest`: from the base value to base plus the
/// summed amounts of the rows that route a source to it (normalized: 100% is the knob's full
/// travel). `None` when no row routes to it.
pub(crate) fn ring_depth(app: &StudioApp, dev: Dev, dest: usize) -> Option<f32> {
    let mut depth = None;
    for row in 0..ROWS {
        let r = route(app, dev, row);
        if r.source != 0 && r.dest == dest {
            *depth.get_or_insert(0.0) += r.amount / 100.0;
        }
    }
    depth
}

/// Writes parameters together into the gesture in progress under one merge key (the pad's Timbre
/// and Morph), so a drag is one undo step that ends on release.
pub(crate) fn set_together(app: &mut StudioApp, dev: Dev, values: &[(&str, f32)], merge: &str) {
    let (t, slot) = dev.key;
    let changed: Vec<(&str, f32)> = values
        .iter()
        .filter_map(|(id, v)| dev.param(id).map(|p| (p.id, p.clamp(*v))))
        .filter(|(id, v)| (v - dev.value(app, id)).abs() > f32::EPSILON)
        .collect();
    if changed.is_empty() {
        return;
    }
    app.builtin_editor.gesture = true;
    let merge = format!("{merge}:{}:{slot}", t.0);
    for (id, v) in changed {
        app.touch_param(t, slot, id, false);
        let r = if slot == transit_mix::INSTRUMENT_SLOT {
            app.execute_ui_merged("mix.instrument_param", &serde_json::json!({"track": t.0, "param": id, "value": v}), &merge)
        } else {
            app.execute_ui_merged("mix.insert_param", &serde_json::json!({"track": t.0, "slot": slot, "param": id, "value": v}), &merge)
        };
        if let Err(e) = r {
            app.status = e.to_string();
        }
    }
}

/// Writes several parameters as one undo step (a route made or removed).
pub(crate) fn set_many(app: &mut StudioApp, dev: Dev, values: &[(&str, f32)]) {
    let (t, slot) = dev.key;
    app.engine.end_merge();
    let merge = format!("macro_osc_route:{}:{slot}", t.0);
    for (id, v) in values {
        let Some(p) = dev.param(id) else { continue };
        app.touch_param(t, slot, id, false);
        let v = p.clamp(*v);
        let r = if slot == transit_mix::INSTRUMENT_SLOT {
            app.execute_ui_merged("mix.instrument_param", &serde_json::json!({"track": t.0, "param": id, "value": v}), &merge)
        } else {
            app.execute_ui_merged("mix.insert_param", &serde_json::json!({"track": t.0, "slot": slot, "param": id, "value": v}), &merge)
        };
        if let Err(e) = r {
            app.status = e.to_string();
        }
    }
    app.engine.end_merge();
}

/// What every control does after drawing: its help entry, its MIDI Learn menu and mark, the value
/// it set, and its row in the window's record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish(app: &mut StudioApp, ui: &Ui, dev: Dev, id: &str, resp: &egui::Response, new: Option<f32>, lay: &mut EditorLayout) {
    let Some(p) = dev.param(id) else { return };
    if let Some(h) = help(id) {
        crate::help::hover(ui, resp, h);
    }
    if let Some(control) = crate::midi_learn::plugin_control(dev.key.1, p.id) {
        crate::chrome::context_menu(resp, app.learn_checks(dev.key.0, &control))
            .id(resp.id.with("learn_ctx"))
            .show(|ui| app.midi_learn_menu(ui, dev.key.0, control.clone()));
        crate::midi_learn::mark(app, ui.painter(), resp.rect, dev.key.0, control);
    }
    if let Some(v) = new {
        set(app, dev, p.id, v);
    }
    let shown = crate::middle_c::param_text(p, new.map_or_else(|| dev.value(app, p.id), |v| p.clamp(v)), app.ui.middle_c);
    lay.rows.push(RowLayout { param: p.id.to_string(), control: r4(resp.rect), value: shown });
}

/// A panel card: `surface.panel`, 1 px `line`, the panel radius. Returns the inner rect (the
/// panel padding, 14 by 16, inside).
pub(crate) fn panel(ui: &Ui, rect: Rect, name: &str, out: &mut MacroLayout) -> Rect {
    let t = &TOKENS;
    ui.painter().rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    out.panels.push((name.to_string(), r4(rect)));
    rect.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]))
}

/// A section label (`label.section`) at the top left of `inner`; returns its rect.
pub(crate) fn section_label(ui: &Ui, inner: Rect, text: &str) -> Rect {
    widgets::role_text(ui.painter(), &tokens::LABEL_SECTION, text, TOKENS.text_muted, Align2::LEFT_TOP, inner.min)
}

/// Whether `id` is a LFO's sync division (a Choice whose table order is not the order of time).
pub(crate) fn division_param(id: &str) -> bool {
    matches!(id, "lfo1_division" | "lfo2_division")
}

/// The division table's indices from the longest cycle to the shortest, triplets and dotted
/// values where their durations fall (the order the sync-rate lists of Ableton and Logic use).
/// A display and drag mapping only: the stored index and saved sessions keep the table's order.
pub fn division_order() -> Vec<usize> {
    use transit_dsp::plugins::lfo::{DIVISIONS, division_quarters};
    let mut order: Vec<usize> = (0..DIVISIONS.len()).collect();
    order.sort_by(|a, b| division_quarters(*b, 4.0).total_cmp(&division_quarters(*a, 4.0)));
    order
}

/// The knob position (0..1) of the stored division index `v`.
fn division_norm(v: f32) -> f32 {
    let order = division_order();
    let rank = order.iter().position(|i| *i == v.round() as usize).unwrap_or(0);
    rank as f32 / (order.len() - 1) as f32
}

/// The stored division index at knob position `norm`.
fn division_at(norm: f32) -> f32 {
    let order = division_order();
    let rank = (norm.clamp(0.0, 1.0) * (order.len() - 1) as f32).round() as usize;
    order[rank] as f32
}

/// A knob for parameter `id` at `at` (the top left of its cell), mapped through the parameter's
/// range and taper; a source chip dropped on it makes a route to its destination.
#[allow(clippy::too_many_arguments)]
pub(crate) fn knob_at(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    id: &str,
    label: &str,
    size: knob::Size,
    at: Pos2,
    lay: &mut EditorLayout,
    out: &mut MacroLayout,
) -> Rect {
    let cell = Rect::from_min_size(at, knob::cell(size));
    let Some(p) = dev.param(id) else { return cell };
    let v = dev.value(app, id);
    let text = crate::middle_c::param_text(p, v, app.ui.middle_c);
    let dest = destination_of(id);
    let synced = division_param(id);
    let k = knob::Knob {
        size,
        label,
        value_text: &text,
        value: if synced { division_norm(v) } else { p.to_normalized(v) },
        default: p.to_normalized(p.default),
        bipolar: bipolar(p),
        modulation: dest.and_then(|d| ring_depth(app, dev, d)),
        disabled: false,
    };
    let mut c = child(ui, cell);
    let shown = knob::show(&mut c, egui::Id::new(("macro_osc_knob", dev.uid(), p.id)), &k);
    let new = match shown.value {
        // A sync division's knob travels the divisions in duration order, not table order.
        Some(n) if synced && !shown.reset => Some(division_at(n)),
        _ => crate::device_editor::knob_value(p, &shown, app.ui.middle_c),
    };
    if shown.wheeled {
        // A wheel gesture's nudges merge into one undo step while the wheel keeps coming.
        app.builtin_editor.devices.wheel_now = true;
    }
    if let (Some(d), Some(chip)) = (dest, shown.response.dnd_release_payload::<modulation::Chip>()) {
        modulation::drop_route(app, dev, chip.0, d);
    }
    finish(app, &c, dev, p.id, &shown.response, new, lay);
    out.knobs.push(KnobRecord { param: p.id.to_string(), label: label.to_string(), knob: shown.layout });
    cell
}

/// Knobs spread across `rect` (centered in equal slots, the board's `space-around`), from its top.
#[allow(clippy::too_many_arguments)]
pub(crate) fn knob_row(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    rect: Rect,
    items: &[(&str, &str)],
    size: knob::Size,
    lay: &mut EditorLayout,
    out: &mut MacroLayout,
) -> f32 {
    let cell = knob::cell(size);
    let slot = rect.width() / items.len().max(1) as f32;
    for (i, (id, label)) in items.iter().enumerate() {
        let x = rect.min.x + slot * i as f32 + (slot - cell.x) / 2.0;
        knob_at(app, ui, dev, id, label, size, pos2(x, rect.min.y), lay, out);
    }
    cell.y
}

/// The panel's content height at `width` (the window's inside), for the window's size (326-X: every
/// frame).
pub fn content_height(app: &StudioApp, ctx: &egui::Context, key: (TrackId, usize), width: f32) -> f32 {
    let Some(ins) = app.builtin_at(key) else { return 0.0 };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return 0.0 };
    let dev = Dev { key, ins: &ins, info };
    let w = width.max(window::min_width(app) - crate::device_window::edge().x) - 2.0 * space::WINDOW_PADDING;
    let mut h = 2.0 * space::WINDOW_PADDING;
    h += hero::height(w);
    h += space::PANEL_GAP + shaper::height(app, dev, w);
    h += space::PANEL_GAP + modulation::height(app, ctx, dev, w);
    h
}

/// Draws the panel's body (below the title bar, above the footer) into `ui` at `width`.
pub(crate) fn body(app: &mut StudioApp, ui: &mut Ui, dev: Dev, lay: &mut EditorLayout, out: &mut MacroLayout) {
    // The window's inside: its width less the frame's edge (never under the minimum's).
    let width = ui.available_width().max(window::min_width(app) - crate::device_window::edge().x);
    let origin = ui.cursor().min;
    let w = width - 2.0 * space::WINDOW_PADDING;
    let mut y = origin.y + space::WINDOW_PADDING;
    let x = origin.x + space::WINDOW_PADDING;
    let hero_h = hero::height(w);
    let left_w = (w - space::PANEL_GAP) * 2.0 / 3.0;
    hero::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(left_w, hero_h)), lay, out);
    let right = Rect::from_min_size(pos2(x + left_w + space::PANEL_GAP, y), vec2(w - left_w - space::PANEL_GAP, hero_h));
    sections::show(app, ui, dev, right, lay, out);
    y += hero_h + space::PANEL_GAP;
    y += shaper::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, 0.0)), lay, out) + space::PANEL_GAP;
    y += modulation::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, 0.0)), lay, out);
    y += space::WINDOW_PADDING;
    ui.allocate_rect(Rect::from_min_max(origin, pos2(origin.x + width, y)), Sense::hover());
}

/// The footer: the credit on the left, the audio load on the right (`text.faint`, 11).
pub(crate) fn footer(app: &StudioApp, ui: &mut Ui, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let p = ui.painter();
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.line));
    let at = pos2(rect.min.x + space::WINDOW_PADDING, rect.center().y);
    let r = widgets::role_text(p, &tokens::LABEL_CONTROL, CREDIT, t.text_faint, Align2::LEFT_CENTER, at);
    let resp = ui.interact(r, egui::Id::new(("macro_osc_credit", out.title.uid)), Sense::hover());
    crate::help::hover(ui, &resp, crate::help::Id::MacroOscCredits);
    out.credit = CREDIT.to_string();
    out.credit_rect = r4(r);
    lay.credit = CREDIT.to_string();
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
