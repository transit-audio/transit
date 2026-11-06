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
mod title_bar;
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
mod window_tests;

use crate::StudioApp;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, knob, r4};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_dsp::plugins::macro_osc::matrix::{ROWS, dst};
use transit_dsp::plugins::macro_osc::row_param;
use transit_dsp::{ParamInfo, PluginInfo, Unit};
use transit_model::{Insert, TrackId};

pub use modulation::{ChipRecord, RouteRecord};
pub use title_bar::TitleLayout;

/// The Macro Oscillator's plug-in id (ADR 0033, #195).
pub const MACRO_OSC: &str = "macro_oscillator";

/// The credit line in the footer (ADR 0033: the credit is required).
pub const CREDIT: &str = "A port of Émilie Gillet's open-source macro oscillator (MIT)";

/// The board's width: the content lays out on its 12-column grid at this width and narrower
/// windows shrink the fractional columns down to [`MIN_WIDTH`].
pub const DESIGN_WIDTH: f32 = 1440.0;
/// The narrowest the window's content lays out; below it the window keeps this width.
pub const MIN_WIDTH: f32 = 1040.0;
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

/// A display well as last drawn: `scope`, `pad`, `amp_env`, `filter`, `filter_env`, `lfo1`, `lfo2`.
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
    /// The scope well's readouts: the model on the left, the voice count on the right.
    pub readout: String,
    pub voices: String,
    pub scope: [f32; 4],
    /// The Timbre/Morph pad, its point and its T and M readouts.
    pub pad: [f32; 4],
    pub pad_point: [f32; 2],
    pub pad_trails: Vec<[f32; 4]>,
    pub timbre_text: String,
    pub morph_text: String,
    /// The model picker: model index, button rect, selected; and its group labels.
    pub picker: Vec<(usize, [f32; 4], bool)>,
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

/// The device an editor shows: its key, its insert and its descriptor.
#[derive(Clone, Copy)]
pub(crate) struct Dev<'a> {
    pub key: (TrackId, usize),
    pub ins: &'a Insert,
    pub info: &'static PluginInfo,
}

impl Dev<'_> {
    pub fn param(&self, id: &str) -> Option<&'static ParamInfo> {
        self.info.param(id)
    }

    /// The value a control shows: the automation it follows, else the insert's, else the default.
    pub fn value(&self, app: &StudioApp, id: &str) -> f32 {
        let Some(p) = self.param(id) else { return 0.0 };
        p.clamp(app.followed_param(self.key, id).or_else(|| self.ins.params.get(id).copied()).unwrap_or(p.default))
    }

    /// The value as its control prints it.
    pub fn text(&self, app: &StudioApp, id: &str) -> String {
        self.param(id).map(|p| p.format(self.value(app, id))).unwrap_or_default()
    }

    pub fn on(&self, app: &StudioApp, id: &str) -> bool {
        self.value(app, id) >= 0.5
    }

    pub fn index(&self, app: &StudioApp, id: &str) -> usize {
        self.value(app, id).round().max(0.0) as usize
    }

    pub fn uid(&self) -> u64 {
        self.ins.uid.get()
    }
}

/// The parameter value at a knob position: the taper honored, semitones and choices whole.
pub fn value_at(p: &ParamInfo, norm: f32) -> f32 {
    let v = p.from_normalized(norm);
    if matches!(p.unit, Unit::Semitones | Unit::Cents | Unit::Choice) { p.clamp(v.round()) } else { v }
}

/// A value typed into a knob's field: a number in the parameter's unit (`1.2k` and `1.2 kHz` for
/// kilohertz), or a choice by name. `None` when it does not read.
pub fn parse(p: &ParamInfo, text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    if !p.choices.is_empty() {
        return p.choices.iter().position(|c| c.to_lowercase() == t).map(|i| i as f32);
    }
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let mut v: f32 = num.parse().ok()?;
    if p.unit == Unit::Hz && t[num.len()..].trim_start().starts_with('k') {
        v *= 1000.0;
    }
    v.is_finite().then(|| p.clamp(v))
}

/// Whether a knob's arc grows from 12 o'clock: a linear range that spans zero.
pub fn bipolar(p: &ParamInfo) -> bool {
    p.min < 0.0 && p.max > 0.0 && p.taper == transit_dsp::Taper::Linear
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

/// Writes one parameter (merged into the gesture in progress) when it changed.
pub(crate) fn set(app: &mut StudioApp, dev: Dev, id: &str, value: f32) {
    let Some(p) = dev.param(id) else { return };
    let v = p.clamp(value);
    if (v - dev.value(app, id)).abs() > f32::EPSILON {
        app.set_builtin_param(dev.key, id, v);
    }
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
        egui::Popup::context_menu(resp).id(resp.id.with("learn_ctx")).show(|ui| app.midi_learn_menu(ui, dev.key.0, control.clone()));
        crate::midi_learn::mark(app, ui.painter(), resp.rect, dev.key.0, control);
    }
    if let Some(v) = new {
        set(app, dev, p.id, v);
    }
    let shown = p.format(new.map_or_else(|| dev.value(app, p.id), |v| p.clamp(v)));
    lay.rows.push(RowLayout { param: p.id.to_string(), control: r4(resp.rect), value: shown });
}

/// The child `Ui` that draws into `rect` without moving the parent's cursor.
pub(crate) fn child(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Min)))
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
    let text = p.format(v);
    let dest = destination_of(id);
    let k = knob::Knob {
        size,
        label,
        value_text: &text,
        value: p.to_normalized(v),
        default: p.to_normalized(p.default),
        bipolar: bipolar(p),
        modulation: dest.and_then(|d| ring_depth(app, dev, d)),
        disabled: false,
    };
    let mut c = child(ui, cell);
    let shown = knob::show(&mut c, egui::Id::new(("macro_osc_knob", dev.uid(), p.id)), &k);
    let new = shown.value.map(|n| value_at(p, n)).or_else(|| shown.typed.as_deref().and_then(|t| parse(p, t)));
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

/// What a dropdown shows before its text.
#[derive(Clone, Copy)]
pub(crate) enum Lead<'a> {
    None,
    /// A faint group or operator (`Tone`, `×`).
    Text(&'a str),
    /// An 8 px dot (a routing row's source).
    Dot(Color32),
    /// Room the caller paints into (a curve icon).
    Gap(f32),
}

/// A dropdown (README, Dropdown): `fill` (`surface.inset` on panels, `surface.raised` in routing
/// rows), 1 px `line`, the control radius, the chevron in `text.faint`, `lead` before the text.
/// Its list is a popup of check rows. Returns the response and the choice the user made.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dropdown(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    fill: Color32,
    lead: Lead,
    text: &str,
    mono: bool,
    choices: &[&str],
    selected: usize,
) -> (egui::Response, Option<usize>) {
    let t = &TOKENS;
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    p.rect(rect, radius::CONTROL, if resp.hovered() { t.button_hover } else { fill }, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let mut x = rect.min.x + 8.0;
    let cy = rect.center().y;
    match lead {
        Lead::None => {}
        Lead::Text(pre) => {
            let r = widgets::role_text(p, &tokens::LABEL_CONTROL, pre, t.text_faint, Align2::LEFT_CENTER, pos2(x, cy));
            x = r.max.x + 8.0;
        }
        Lead::Dot(c) => {
            p.circle_filled(pos2(x + 4.0, cy), 4.0, c);
            x += 16.0;
        }
        Lead::Gap(w) => x += w + 8.0,
    }
    let role = if mono { tokens::VALUE } else { tokens::chrome::BUTTON_FONT };
    let clip = p.with_clip_rect(Rect::from_x_y_ranges(x..=rect.max.x - 18.0, rect.y_range()).intersect(p.clip_rect()));
    widgets::role_text(&clip, &role, text, t.text, Align2::LEFT_CENTER, pos2(x, cy));
    chevron(p, pos2(rect.max.x - 11.0, cy), 8.0, t.text_faint);
    let mut pick = None;
    egui::Popup::menu(&resp).show(|ui| {
        crate::chrome::menu_style(ui.style_mut());
        for (i, c) in choices.iter().enumerate() {
            if crate::menus::check_button(ui, i == selected, *c).clicked() && i != selected {
                pick = Some(i);
            }
        }
    });
    (resp, pick)
}

/// A downward chevron `w` wide centered at `c`.
pub(crate) fn chevron(p: &egui::Painter, c: Pos2, w: f32, color: Color32) {
    let h = w / 2.0;
    p.add(egui::Shape::line(vec![c + vec2(-h, -h / 2.0), c + vec2(0.0, h / 2.0), c + vec2(h, -h / 2.0)], Stroke::new(1.5, color)));
}

/// The panel's content height at `width`, for the window's default size.
pub fn content_height(app: &StudioApp, key: (TrackId, usize), width: f32) -> f32 {
    let Some(ins) = app.builtin_at(key) else { return 0.0 };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return 0.0 };
    let dev = Dev { key, ins: &ins, info };
    let w = width.max(MIN_WIDTH) - 2.0 * space::WINDOW_PADDING;
    let mut h = 2.0 * space::WINDOW_PADDING;
    h += hero::height(w);
    h += space::PANEL_GAP + shaper::height(app, dev, w);
    h += space::PANEL_GAP + modulation::height(app, dev, w);
    h
}

/// Draws the panel's body (below the title bar, above the footer) into `ui` at `width`.
pub(crate) fn body(app: &mut StudioApp, ui: &mut Ui, dev: Dev, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let width = ui.available_width().max(MIN_WIDTH);
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
