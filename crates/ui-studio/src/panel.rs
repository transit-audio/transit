//! Bespoke panels for the built-in plug-ins that deserve more than a list of sliders: a small
//! layout description (rows of sections, each a titled group of knobs, toggles and choices) and
//! the code that draws it. A plug-in without a layout here keeps the generic rows.
//!
//! Every parameter of a bespoke plug-in appears in its layout (a test holds that), so nothing
//! becomes unreachable. Values go through `StudioApp::set_builtin_param`, like the sliders.

use crate::StudioApp;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::theme::{RADIUS, TOKENS};
use crate::widgets::param_knob as knob;
use egui::{RichText, Ui};
use transit_dsp::{ParamInfo, PluginInfo};
use transit_model::{Insert, KeyFrom, TrackId};

/// One control in a section: parameter id and the label under it.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Knob(&'static str, &'static str),
    Toggle(&'static str, &'static str),
    /// A button that holds its switch on while pressed (the Texture's Seed, #241).
    Hold(&'static str, &'static str),
    Choice(&'static str, &'static str),
    /// A choice with long labels: a wide dropdown.
    Menu(&'static str, &'static str),
    /// The sidechain key dropdown (the gate; the Compressor draws its own, 326-K).
    Key,
    /// The modulation matrix (#243, the Resonator's and the other ported voices'): a compact grid
    /// of its sixteen rows (source, via, destination, curve, amount), every row parameter in it.
    Matrix,
}

/// A titled group; `cols` items per line.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub title: &'static str,
    pub cols: usize,
    pub items: &'static [Item],
}

pub type Row = &'static [Section];

use Item::{Choice, Key, Knob, Matrix, Menu, Toggle};

const fn sec(title: &'static str, cols: usize, items: &'static [Item]) -> Section {
    Section { title, cols, items }
}

static GATE: [Row; 2] = [
    &[
        sec("Gate", 3, &[Knob("threshold", "Threshold"), Knob("ratio", "Ratio"), Knob("range", "Range")]),
        sec("Envelope", 3, &[Knob("attack", "Attack"), Knob("hold", "Hold"), Knob("release", "Release")]),
    ],
    &[sec("Key", 1, &[Key])],
];

static EQ7: [Row; 1] = [&[
    sec("In", 1, &[Knob("input_gain", "Input")]),
    sec("HPF", 1, &[Toggle("hpf_on", "On"), Knob("hpf_freq", "Freq"), Choice("hpf_slope", "Slope")]),
    sec("Low", 1, &[Knob("low_shelf_freq", "Freq"), Knob("low_shelf_gain", "Gain"), Knob("low_shelf_q", "Q")]),
    sec("Low Mid", 1, &[Knob("low_mid_freq", "Freq"), Knob("low_mid_gain", "Gain"), Knob("low_mid_q", "Q")]),
    sec("Mid", 1, &[Knob("mid_freq", "Freq"), Knob("mid_gain", "Gain"), Knob("mid_q", "Q")]),
    sec("High Mid", 1, &[Knob("high_mid_freq", "Freq"), Knob("high_mid_gain", "Gain"), Knob("high_mid_q", "Q")]),
    sec("High", 1, &[Knob("high_shelf_freq", "Freq"), Knob("high_shelf_gain", "Gain"), Knob("high_shelf_q", "Q")]),
    sec("LPF", 1, &[Toggle("lpf_on", "On"), Knob("lpf_freq", "Freq"), Choice("lpf_slope", "Slope")]),
    sec("Out", 1, &[Knob("output_gain", "Output")]),
]];

static PLATE: [Row; 1] = [&[
    sec("Space", 2, &[Knob("decay", "Decay"), Knob("size", "Size"), Knob("pre_delay", "Pre-Delay"), Knob("diffusion", "Diffusion")]),
    sec("Tone", 3, &[Knob("damping", "Damping"), Knob("low_cut", "Low Cut"), Knob("width", "Width")]),
    sec("Mix", 1, &[Knob("mix", "Mix")]),
]];

/// transit's addition (#243) in the older panel system, shared by the Resonator (#198) and the
/// other ported voices: the shaper, the filter and both LFOs in one row of two lines, so the
/// expanded window stays within 800 pt (1080p at 125% with the title bar). The Macro Oscillator
/// draws its own in the design language (326-M, `crate::macro_osc_panel`).
pub(crate) const SHAPER_ROW: Row = &[
    sec(
        "Shaper",
        3,
        &[
            Toggle("shaper", "Shaper"),
            Knob("amp_velocity", "Velocity"),
            Knob("amp_attack", "Attack"),
            Knob("amp_decay", "Decay"),
            Knob("amp_sustain", "Sustain"),
            Knob("amp_release", "Release"),
        ],
    ),
    // The filter on the first line, its envelope on the second.
    sec(
        "Filter",
        6,
        &[
            Choice("filter_mode", "Mode"),
            Choice("filter_slope", "Slope"),
            Knob("drive", "Drive"),
            Knob("cutoff", "Cutoff"),
            Knob("resonance", "Resonance"),
            Knob("key_track", "Key Track"),
            Knob("filter_env_amount", "Env Amount"),
            Knob("filter_attack", "Attack"),
            Knob("filter_decay", "Decay"),
            Knob("filter_sustain", "Sustain"),
            Knob("filter_release", "Release"),
        ],
    ),
    sec(
        "LFO 1",
        4,
        &[
            Choice("lfo1_shape", "Shape"),
            Knob("lfo1_rate", "Rate"),
            Toggle("lfo1_sync", "Sync"),
            Choice("lfo1_division", "Division"),
            Toggle("lfo1_retrigger", "Retrigger"),
            Knob("lfo1_fade", "Fade In"),
            Knob("lfo1_amount", "Amount"),
        ],
    ),
    sec(
        "LFO 2",
        4,
        &[
            Choice("lfo2_shape", "Shape"),
            Knob("lfo2_rate", "Rate"),
            Toggle("lfo2_sync", "Sync"),
            Choice("lfo2_division", "Division"),
            Toggle("lfo2_retrigger", "Retrigger"),
            Knob("lfo2_fade", "Fade In"),
            Knob("lfo2_amount", "Amount"),
        ],
    ),
];

/// The modulation matrix's row, below the shaper.
pub(crate) const MATRIX_ROW: Row = &[sec("Modulation Matrix", 1, &[Matrix])];

/// The line above the shaper and the matrix (#243: the panel labels them as transit's addition).
/// Shared by the panels still in the older system (the Resonator and the other ported voices);
/// the name stays from when the Macro Oscillator used it too (326-M moved it to section bars).
pub const MACRO_OSC_ADDITION: &str = "Shaper and modulation matrix: transit's addition, not part of the original module (off by default)";

/// The Filter's id (#242).
const FILTER: &str = "filter";

static FILTER_LAYOUT: [Row; 3] = [
    &[
        sec(
            "Filter",
            3,
            &[
                Choice("mode", "Mode"),
                Choice("slope", "Slope"),
                Knob("drive", "Drive"),
                Knob("cutoff", "Cutoff"),
                Knob("resonance", "Resonance"),
                Knob("output", "Output"),
            ],
        ),
        sec("Key", 1, &[Knob("key_track", "Key Track")]),
        sec("Follower", 1, &[Knob("follow_amount", "Amount"), Knob("follow_attack", "Attack"), Knob("follow_release", "Release")]),
    ],
    &[
        sec(
            "Envelope",
            4,
            &[
                Knob("env_amount", "Amount"),
                Knob("env_velocity", "Velocity"),
                Choice("retrigger", "Retrigger"),
                Knob("env_attack", "Attack"),
                Knob("env_decay", "Decay"),
                Knob("env_sustain", "Sustain"),
                Knob("env_release", "Release"),
            ],
        ),
        sec(
            "Amp",
            4,
            &[
                Toggle("amp_on", "Amp Env"),
                Knob("amp_velocity", "Velocity"),
                Knob("amp_attack", "Attack"),
                Knob("amp_decay", "Decay"),
                Knob("amp_sustain", "Sustain"),
                Knob("amp_release", "Release"),
            ],
        ),
    ],
    &[sec(
        "LFO",
        6,
        &[
            Knob("lfo_amount", "Amount"),
            Choice("lfo_shape", "Shape"),
            Knob("lfo_rate", "Rate"),
            Toggle("lfo_sync", "Sync"),
            Choice("lfo_division", "Division"),
            Toggle("lfo_retrigger", "Retrigger"),
        ],
    )],
];

/// The help entry of a Filter control.
fn filter_help(param: &str) -> Option<crate::help::Id> {
    use crate::help::Id;
    Some(match param {
        "mode" => Id::FilterMode,
        "slope" => Id::FilterSlope,
        "cutoff" => Id::FilterCutoff,
        "resonance" => Id::FilterResonance,
        "drive" => Id::FilterDrive,
        "output" => Id::FilterOutput,
        "env_amount" => Id::FilterEnvAmount,
        "env_attack" | "env_decay" | "env_sustain" | "env_release" => Id::FilterEnvAdsr,
        "env_velocity" => Id::FilterEnvVelocity,
        "retrigger" => Id::FilterRetrigger,
        "lfo_amount" => Id::FilterLfoAmount,
        "lfo_shape" => Id::FilterLfoShape,
        "lfo_rate" => Id::FilterLfoRate,
        "lfo_sync" => Id::FilterLfoSync,
        "lfo_division" => Id::FilterLfoDivision,
        "lfo_retrigger" => Id::FilterLfoRetrigger,
        "follow_amount" => Id::FilterFollowAmount,
        "follow_attack" | "follow_release" => Id::FilterFollowTimes,
        "key_track" => Id::FilterKeyTrack,
        "amp_on" => Id::FilterAmpOn,
        "amp_attack" | "amp_decay" | "amp_sustain" | "amp_release" => Id::FilterAmpAdsr,
        "amp_velocity" => Id::FilterAmpVelocity,
        _ => return None,
    })
}

/// The help entry of a bespoke panel's control, for the plug-ins that have them.
fn help(plugin: &str, param: &str) -> Option<crate::help::Id> {
    if plugin == FILTER {
        return filter_help(param);
    }
    if plugin == crate::tuner_panel::TUNER {
        return crate::tuner_panel::help(param);
    }
    if plugin == crate::texture_panel::TEXTURE {
        return crate::texture_panel::help(param);
    }
    if plugin == crate::time_stretcher_panel::TIME_STRETCHER {
        return crate::time_stretcher_panel::help(param);
    }
    if plugin == crate::modulator_panel::CROSS_MODULATOR {
        return crate::modulator_panel::help(param);
    }
    if plugin == crate::resonator_panel::RESONATOR {
        return crate::resonator_panel::help(param);
    }
    if plugin == crate::texture_synth_panel::TEXTURE_SYNTH {
        return crate::texture_synth_panel::help(param);
    }
    if plugin == crate::modal_voice_panel::MODAL_VOICE {
        return crate::modal_voice_panel::help(param);
    }
    // The Macro Oscillator's help moved with its panel (326-M, `crate::macro_osc_panel::help`).
    None
}

/// The bespoke layout of a plug-in (`None`: it keeps the generic rows).
pub fn layout(plugin: &str) -> Option<&'static [Row]> {
    match plugin {
        "expander_gate" => Some(&GATE),
        "eq_7band" => Some(&EQ7),
        "plate_reverb" => Some(&PLATE),
        FILTER => Some(&FILTER_LAYOUT),
        crate::texture_panel::TEXTURE => Some(&crate::texture_panel::LAYOUT),
        crate::time_stretcher_panel::TIME_STRETCHER => Some(&crate::time_stretcher_panel::LAYOUT),
        crate::modulator_panel::CROSS_MODULATOR => Some(&crate::modulator_panel::LAYOUT),
        crate::tuner_panel::TUNER => Some(&crate::tuner_panel::LAYOUT),
        crate::resonator_panel::RESONATOR => Some(&crate::resonator_panel::LAYOUT),
        crate::texture_synth_panel::TEXTURE_SYNTH => Some(&crate::texture_synth_panel::LAYOUT),
        crate::modal_voice_panel::MODAL_VOICE => Some(&crate::modal_voice_panel::LAYOUT),
        _ => None,
    }
}

/// Draws a plug-in's panel into `ui`, one row of sections after another.
pub fn show(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, rows: &[Row], lay: &mut EditorLayout) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
    if info.id == crate::tuner_panel::TUNER {
        crate::tuner_panel::show(app, ui, key, ins, lay);
    } else {
        crate::scope_strip::show(app, ui, key, ins, lay);
    }
    if info.id == FILTER {
        crate::filter_curve::show(app, ui, key, ins, info, lay);
    }
    for (i, row) in rows.iter().enumerate() {
        if addition_row(info.id) == Some(i) && !addition_header(ui, ins, lay) {
            break;
        }
        ui.horizontal_top(|ui| {
            for s in *row {
                section(app, ui, key, ins, info, s, lay);
            }
        });
    }
    if info.id == crate::texture_panel::TEXTURE {
        let r = ui.label(RichText::new(crate::texture_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::TextureCredits);
        lay.credit = crate::texture_panel::CREDIT.to_string();
    }
    if info.id == crate::time_stretcher_panel::TIME_STRETCHER {
        let r = ui.label(RichText::new(crate::time_stretcher_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::TimeStretcherCredits);
        lay.credit = crate::time_stretcher_panel::CREDIT.to_string();
    }
    if info.id == crate::resonator_panel::RESONATOR {
        // The strum and rate lines side by side, so the expanded window keeps within 800 pt.
        ui.horizontal(|ui| {
            for (text, id) in
                [(crate::resonator_panel::STRUMS, crate::help::Id::ResonatorStrums), (crate::resonator_panel::RATE, crate::help::Id::ResonatorRate)]
            {
                let r = ui.label(RichText::new(text).size(10.5).color(TOKENS.text_dim));
                crate::help::hover(ui, &r, id);
            }
        });
        let r = ui.label(RichText::new(crate::resonator_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::ResonatorCredits);
        lay.credit = crate::resonator_panel::CREDIT.to_string();
    }
    if info.id == crate::texture_synth_panel::TEXTURE_SYNTH {
        // The notes and rate lines side by side, so the expanded window keeps within 800 pt.
        ui.horizontal(|ui| {
            for (text, id) in [
                (crate::texture_synth_panel::NOTES, crate::help::Id::TextureSynthNotes),
                (crate::texture_synth_panel::RATE, crate::help::Id::TextureSynthRate),
            ] {
                let r = ui.label(RichText::new(text).size(10.5).color(TOKENS.text_dim));
                crate::help::hover(ui, &r, id);
            }
        });
        let r = ui.label(RichText::new(crate::texture_synth_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::TextureSynthCredits);
        lay.credit = crate::texture_synth_panel::CREDIT.to_string();
    }
    if info.id == crate::modal_voice_panel::MODAL_VOICE {
        // The gate and rate lines side by side, so the expanded window keeps within 800 pt.
        ui.horizontal(|ui| {
            for (text, id) in
                [(crate::modal_voice_panel::GATE, crate::help::Id::ModalVoiceGate), (crate::modal_voice_panel::RATE, crate::help::Id::ModalVoiceRate)]
            {
                let r = ui.label(RichText::new(text).size(10.5).color(TOKENS.text_dim));
                crate::help::hover(ui, &r, id);
            }
        });
        let r = ui.label(RichText::new(crate::modal_voice_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::ModalVoiceCredits);
        lay.credit = crate::modal_voice_panel::CREDIT.to_string();
    }
    if info.id == crate::modulator_panel::CROSS_MODULATOR {
        let v = app.followed_param(key, "algorithm").or_else(|| ins.params.get("algorithm").copied());
        let v = info.param("algorithm").map_or(0.0, |p| p.clamp(v.unwrap_or(p.default)));
        let r = ui.label(RichText::new(crate::modulator_panel::algorithm_line(v)).size(11.5).color(TOKENS.text));
        crate::help::hover(ui, &r, crate::help::Id::CrossModulatorAlgorithm);
        let r = ui.label(RichText::new(crate::modulator_panel::CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::CrossModulatorCredits);
        lay.credit = crate::modulator_panel::CREDIT.to_string();
    }
}

/// The row of a plug-in's layout where transit's addition (the shaper and the matrix) starts.
fn addition_row(plugin: &str) -> Option<usize> {
    match plugin {
        crate::resonator_panel::RESONATOR => Some(crate::resonator_panel::ADDITION_ROW),
        crate::texture_synth_panel::TEXTURE_SYNTH => Some(crate::texture_synth_panel::ADDITION_ROW),
        crate::modal_voice_panel::MODAL_VOICE => Some(crate::modal_voice_panel::ADDITION_ROW),
        _ => None,
    }
}

/// The line above the Resonator's (and the other ported voices') shaper and matrix: it labels them as transit's addition
/// and shows or hides them (a click). Until clicked, they show while they are in use (the shaper on
/// or a matrix row with a source), so a session of defaults keeps the module's own panel size and
/// a migrated mapping shows its rows. Returns whether they show.
fn addition_header(ui: &mut Ui, ins: &Insert, lay: &mut EditorLayout) -> bool {
    let id = egui::Id::new(("macro_osc_addition", ins.uid.get()));
    let in_use = ins.params.get("shaper").is_some_and(|v| *v >= 0.5)
        || ins.params.iter().any(|(k, v)| k.starts_with("mod") && k.ends_with("_source") && *v >= 0.5);
    let chosen: Option<bool> = ui.ctx().data(|d| d.get_temp(id));
    let open = chosen.unwrap_or(in_use);
    let line = ui.horizontal(|ui| {
        crate::icons::icon(ui, if open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight });
        ui.label(RichText::new(MACRO_OSC_ADDITION).size(10.5).italics().color(TOKENS.text_dim));
    });
    let r = ui.interact(line.response.rect, id.with("toggle"), egui::Sense::click());
    crate::help::hover(ui, &r, crate::help::Id::MacroOscAddition);
    if r.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(id, !open));
    }
    lay.addition = MACRO_OSC_ADDITION.to_string();
    lay.addition_rect = [r.rect.min.x, r.rect.min.y, r.rect.max.x, r.rect.max.y];
    open
}

fn section(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, s: &Section, lay: &mut EditorLayout) {
    egui::Frame::new().fill(TOKENS.panel_raised).corner_radius(RADIUS).inner_margin(8.0).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(s.title).size(10.5).strong().color(TOKENS.text_dim));
            for line in s.items.chunks(s.cols.max(1)) {
                ui.horizontal(|ui| {
                    for item in line {
                        draw(app, ui, key, ins, info, *item, lay);
                    }
                });
            }
        });
    });
}

fn draw(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, item: Item, lay: &mut EditorLayout) {
    let (id, label) = match item {
        Knob(id, l) | Toggle(id, l) | Item::Hold(id, l) | Choice(id, l) | Menu(id, l) => (id, l),
        Key => return key_dropdown(app, ui, key),
        Matrix => return crate::panel_matrix::show(app, ui, key, ins, info, lay),
    };
    let Some(p) = info.param(id) else { return };
    let v = p.clamp(app.followed_param(key, id).or_else(|| ins.params.get(id).copied()).unwrap_or(p.default));
    let (resp, new, shown) = match item {
        Knob(..) => {
            let k = knob::show(ui, egui::Id::new(("knob", ins.uid.get(), id)), label, p, v);
            (k.response, k.value, p.format(k.value.unwrap_or(v)))
        }
        Toggle(..) => {
            let r = crate::chrome_pick::option_label(ui, v >= 0.5, label);
            let clicked = r.clicked().then_some(if v >= 0.5 { 0.0 } else { 1.0 });
            (r, clicked, p.format(v))
        }
        Item::Hold(..) => {
            // On while the pointer holds it down, off when it lets go (wherever it is then). It
            // senses drags as well: a click-only widget loses the press once the pointer moves
            // off it with the button still down.
            let r = crate::chrome_pick::option_label(ui, v >= 0.5, label).interact(egui::Sense::click_and_drag());
            let down = r.is_pointer_button_down_on();
            let changed = (down != (v >= 0.5)).then_some(if down { 1.0 } else { 0.0 });
            (r, changed, p.format(v))
        }
        Menu(..) => choice(ui, ins, p, label, v, MENU_W, None),
        _ => choice(ui, ins, p, label, v, knob::CELL_W - 10.0, None),
    };
    finish(app, ui, key, info, id, (resp, new, shown), v, lay);
}

/// What every control of a panel does after drawing: its help entry, its MIDI Learn menu and mark,
/// the value it set (through `set_builtin_param`), and its row in the layout record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish(
    app: &mut StudioApp,
    ui: &mut Ui,
    key: (TrackId, usize),
    info: &'static PluginInfo,
    id: &'static str,
    (resp, new, shown): (egui::Response, Option<f32>, String),
    v: f32,
    lay: &mut EditorLayout,
) {
    let Some(p) = info.param(id) else { return };
    if let Some(h) = help(info.id, id) {
        crate::help::hover(ui, &resp, h);
    }
    let rect = resp.rect;
    if let Some(control) = crate::midi_learn::plugin_control(key.1, id) {
        // Its own popup id: `Popup::context_menu` closes whatever popup shares the response's id on
        // a primary click, which was the dropdown's own list (a combo's popup uses its button's id).
        crate::chrome::context_menu(&resp, app.learn_checks(key.0, &control))
            .id(resp.id.with("learn_ctx"))
            .show(|ui| app.midi_learn_menu(ui, key.0, control.clone()));
        crate::midi_learn::mark(app, ui.painter(), rect, key.0, control);
    }
    if let Some(x) = new.filter(|x| (x - v).abs() > f32::EPSILON) {
        app.set_builtin_param(key, id, p.clamp(x));
    }
    lay.rows.push(RowLayout { param: id.to_string(), control: [rect.min.x, rect.min.y, rect.max.x, rect.max.y], value: shown });
}

/// The width of a [`Item::Menu`] dropdown.
const MENU_W: f32 = 150.0;

pub(crate) fn choice(
    ui: &mut Ui,
    ins: &Insert,
    p: &ParamInfo,
    label: &str,
    v: f32,
    width: f32,
    row: Option<f32>,
) -> (egui::Response, Option<f32>, String) {
    let mut idx = (v.round().max(0.0) as usize).min(p.choices.len().saturating_sub(1));
    let r = ui
        .vertical(|ui| {
            let combo =
                crate::chrome_pick::Combo::new(("builtin_choice", ins.uid.get(), p.id), p.choices.get(idx).copied().unwrap_or("")).width(width);
            let c = if let Some(h) = row { combo.row(h) } else { combo.dense() }.show(ui, |ui| {
                for (i, name) in p.choices.iter().enumerate() {
                    crate::chrome_pick::choice_value(ui, &mut idx, i, *name);
                }
            });
            if !label.is_empty() {
                ui.label(RichText::new(label).size(10.5).color(TOKENS.text_dim));
            }
            c.response
        })
        .inner;
    (r, (idx as f32 != v).then_some(idx as f32), p.format(idx as f32))
}

/// The sidechain key: None, or another track (or a bus), at the tap the device card last used.
fn key_dropdown(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize)) {
    let (t, slot) = key;
    let s = app.engine.session_arc();
    let Some(ins) = s.track(t).and_then(|x| x.mixer.inserts.get(slot)).and_then(Option::as_ref) else { return };
    let label = crate::devices::key_label(&s, ins);
    let (tracks, busses) = crate::devices::key_sources(&s, t);
    let current = ins.key;
    let pre = current.map_or(app.mixer.key_pre, |k| k.tap == transit_model::Tap::Pre);
    let mut pick: Option<Option<KeyFrom>> = None;
    ui.vertical(|ui| {
        crate::chrome_pick::Combo::new(("panel_key", t.0, slot), label).width(150.0).dense().show(ui, |ui| {
            if crate::chrome_pick::choice_label(ui, current.is_none(), "None").clicked() {
                pick = Some(None);
            }
            for (id, name) in &tracks {
                if crate::chrome_pick::choice_label(ui, current.is_some_and(|k| k.from == KeyFrom::Track(*id)), name).clicked() {
                    pick = Some(Some(KeyFrom::Track(*id)));
                }
            }
            for (id, name) in &busses {
                if crate::chrome_pick::choice_label(ui, current.is_some_and(|k| k.from == KeyFrom::Bus(*id)), name).clicked() {
                    pick = Some(Some(KeyFrom::Bus(*id)));
                }
            }
        });
        ui.label(RichText::new("Sidechain key").size(10.5).color(TOKENS.text_dim));
    });
    if let Some(from) = pick {
        let _ = app.set_insert_key(t, slot, from, pre);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_ids(i: Item) -> Vec<&'static str> {
        match i {
            Knob(id, _) | Toggle(id, _) | Item::Hold(id, _) | Choice(id, _) | Menu(id, _) => vec![id],
            Matrix => crate::panel_matrix::params().collect(),
            Key => Vec::new(),
        }
    }

    #[test]
    fn every_layout_places_each_parameter_of_its_plugin_exactly_once() {
        for plugin in [
            "expander_gate",
            "eq_7band",
            "plate_reverb",
            FILTER,
            crate::texture_panel::TEXTURE,
            crate::time_stretcher_panel::TIME_STRETCHER,
            crate::modulator_panel::CROSS_MODULATOR,
            crate::tuner_panel::TUNER,
            crate::resonator_panel::RESONATOR,
            crate::texture_synth_panel::TEXTURE_SYNTH,
            crate::modal_voice_panel::MODAL_VOICE,
        ] {
            let rows = layout(plugin).unwrap_or_else(|| panic!("a layout for {plugin}"));
            let mut ids: Vec<&str> = rows.iter().flat_map(|r| r.iter()).flat_map(|s| s.items.iter()).flat_map(|i| item_ids(*i)).collect();
            ids.sort_unstable();
            let mut want: Vec<&str> = transit_dsp::plugin_info(plugin).unwrap().params.iter().map(|p| p.id).collect();
            want.sort_unstable();
            assert_eq!(ids, want, "{plugin}");
        }
    }

    #[test]
    fn every_texture_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::texture_panel::TEXTURE).unwrap().params {
            assert!(help(crate::texture_panel::TEXTURE, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_time_stretcher_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::time_stretcher_panel::TIME_STRETCHER).unwrap().params {
            assert!(help(crate::time_stretcher_panel::TIME_STRETCHER, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_cross_modulator_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::modulator_panel::CROSS_MODULATOR).unwrap().params {
            assert!(help(crate::modulator_panel::CROSS_MODULATOR, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_texture_synth_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::texture_synth_panel::TEXTURE_SYNTH).unwrap().params {
            assert!(help(crate::texture_synth_panel::TEXTURE_SYNTH, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_resonator_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::resonator_panel::RESONATOR).unwrap().params {
            assert!(help(crate::resonator_panel::RESONATOR, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_modal_voice_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(crate::modal_voice_panel::MODAL_VOICE).unwrap().params {
            assert!(help(crate::modal_voice_panel::MODAL_VOICE, p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn every_filter_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(FILTER).unwrap().params {
            assert!(help(FILTER, p.id).is_some(), "{}", p.id);
        }
        assert!(help("compressor", "threshold").is_none());
    }
}
