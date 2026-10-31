//! Bespoke panels for the built-in plug-ins that deserve more than a list of sliders: a small
//! layout description (rows of sections, each a titled group of knobs, toggles and choices) and
//! the code that draws it. A plug-in without a layout here keeps the generic rows.
//!
//! Every parameter of a bespoke plug-in appears in its layout (a test holds that), so nothing
//! becomes unreachable. Values go through `StudioApp::set_builtin_param`, like the sliders.

use crate::StudioApp;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::theme::{RADIUS, TOKENS};
use crate::widgets::knob;
use egui::{RichText, Ui};
use transit_dsp::{ParamInfo, PluginInfo};
use transit_model::{Insert, KeyFrom, TrackId};

/// One control in a section: parameter id and the label under it.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Knob(&'static str, &'static str),
    Toggle(&'static str, &'static str),
    Choice(&'static str, &'static str),
    /// A choice with long labels: a wide dropdown.
    Menu(&'static str, &'static str),
    /// The sidechain key dropdown (compressor and gate).
    Key,
    /// The voice activity lights of an instrument with voices (the Macro Oscillator).
    Lights,
    /// The Macro Oscillator's modulation matrix (#243): a compact grid of its sixteen rows
    /// (source, via, destination, curve, amount), every row parameter in it.
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

use Item::{Choice, Key, Knob, Lights, Matrix, Menu, Toggle};

const fn sec(title: &'static str, cols: usize, items: &'static [Item]) -> Section {
    Section { title, cols, items }
}

static COMPRESSOR: [Row; 2] = [
    &[
        sec(
            "Dynamics",
            3,
            &[
                Knob("threshold", "Threshold"),
                Knob("ratio", "Ratio"),
                Knob("attack", "Attack"),
                Knob("release", "Release"),
                Knob("knee", "Knee"),
                Knob("makeup", "Makeup"),
            ],
        ),
        sec("Output", 3, &[Toggle("limiter", "Limit"), Knob("lookahead", "Lookahead"), Knob("mix", "Mix")]),
    ],
    &[sec("Key", 2, &[Key, Toggle("sc_hpf_on", "SC HPF"), Knob("sc_hpf_freq", "HPF Freq")])],
];

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

/// The Macro Oscillator's id (ADR 0033, #195).
const MACRO_OSC: &str = "macro_oscillator";

static MACRO: [Row; 5] = [
    &[
        sec("Model", 1, &[Menu("model", "")]),
        sec("Tone", 3, &[Knob("harmonics", "Harmonics"), Knob("timbre", "Timbre"), Knob("morph", "Morph")]),
        sec("Gate", 2, &[Knob("decay", "Decay"), Knob("lpg_color", "LPG Color")]),
    ],
    &[
        sec("Pitch", 2, &[Knob("tune", "Tune"), Knob("bend_range", "Bend")]),
        sec("Control", 1, &[Choice("mod_wheel", "Mod Wheel")]),
        sec("Output", 2, &[Knob("aux_mix", "Aux Mix"), Knob("level", "Level")]),
        sec("Voices", 1, &[Choice("voices", "Voices"), Lights]),
    ],
    // From here on: transit's addition (#243), labeled as such above this row.
    &[
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
    ],
    &[
        sec(
            "LFO 1",
            7,
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
            7,
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
    ],
    &[sec("Modulation Matrix", 1, &[Matrix])],
];

/// The row of the Macro Oscillator's layout where transit's addition starts: the line above it
/// says so ([`MACRO_OSC_ADDITION`]) and shows or hides the rows from there on.
const ADDITION_ROW: usize = 2;

/// The line above the shaper and the matrix (#243: the panel labels them as transit's addition).
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
    use crate::help::Id;
    if plugin == FILTER {
        return filter_help(param);
    }
    if plugin != MACRO_OSC {
        return None;
    }
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
        _ => return macro_osc_numbered_help(param),
    })
}

/// The help entry of an LFO control (`lfo<n>_<field>`) or a matrix row's (`mod<n>_<field>`).
fn macro_osc_numbered_help(param: &str) -> Option<crate::help::Id> {
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
    head.strip_prefix("mod")?.parse::<usize>().ok().filter(|n| (1..=16).contains(n))?;
    Some(match field {
        "source" => Id::MacroOscMatrixSource,
        "via" => Id::MacroOscMatrixVia,
        "dest" => Id::MacroOscMatrixDest,
        "amount" => Id::MacroOscMatrixAmount,
        "curve" => Id::MacroOscMatrixCurve,
        _ => return None,
    })
}

/// The credit line under the Macro Oscillator's panel (ADR 0033: credit in the instrument's help).
pub const MACRO_OSC_CREDIT: &str = "A port of Émilie Gillet's open-source macro oscillator (MIT)";

/// The bespoke layout of a plug-in (`None`: it keeps the generic rows).
pub fn layout(plugin: &str) -> Option<&'static [Row]> {
    match plugin {
        "compressor" => Some(&COMPRESSOR),
        "expander_gate" => Some(&GATE),
        "eq_7band" => Some(&EQ7),
        "plate_reverb" => Some(&PLATE),
        MACRO_OSC => Some(&MACRO),
        FILTER => Some(&FILTER_LAYOUT),
        _ => None,
    }
}

/// Draws a plug-in's panel into `ui`, one row of sections after another.
pub fn show(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, rows: &[Row], lay: &mut EditorLayout) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
    crate::scope_strip::show(app, ui, key, ins, lay);
    if info.id == FILTER {
        crate::filter_curve::show(app, ui, key, ins, info, lay);
    }
    for (i, row) in rows.iter().enumerate() {
        if info.id == MACRO_OSC && i == ADDITION_ROW && !addition_header(ui, ins, lay) {
            break;
        }
        ui.horizontal_top(|ui| {
            for s in *row {
                section(app, ui, key, ins, info, s, lay);
            }
        });
    }
    if info.id == MACRO_OSC {
        let r = ui.label(RichText::new(MACRO_OSC_CREDIT).size(10.5).color(TOKENS.text_dim));
        crate::help::hover(ui, &r, crate::help::Id::MacroOscCredits);
        lay.credit = MACRO_OSC_CREDIT.to_string();
    }
}

/// The line above the Macro Oscillator's shaper and matrix: it labels them as transit's addition
/// and shows or hides them (a click). Until clicked, they show while they are in use (the shaper on
/// or a matrix row with a source), so a session of defaults keeps the module's own panel size and
/// a migrated mapping shows its rows. Returns whether they show.
fn addition_header(ui: &mut Ui, ins: &Insert, lay: &mut EditorLayout) -> bool {
    let id = egui::Id::new(("macro_osc_addition", ins.uid.get()));
    let in_use = ins.params.get("shaper").is_some_and(|v| *v >= 0.5)
        || ins.params.iter().any(|(k, v)| k.starts_with("mod") && k.ends_with("_source") && *v >= 0.5);
    let chosen: Option<bool> = ui.ctx().data(|d| d.get_temp(id));
    let open = chosen.unwrap_or(in_use);
    let text = format!("{} {MACRO_OSC_ADDITION}", if open { "▾" } else { "▸" });
    let r = ui.add(egui::Label::new(RichText::new(text).size(10.5).italics().color(TOKENS.text_dim)).sense(egui::Sense::click()));
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
        Knob(id, l) | Toggle(id, l) | Choice(id, l) | Menu(id, l) => (id, l),
        Key => return key_dropdown(app, ui, key),
        Lights => return lights(app, ui, key, ins, lay),
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
            let r = ui.selectable_label(v >= 0.5, label);
            let clicked = r.clicked().then_some(if v >= 0.5 { 0.0 } else { 1.0 });
            (r, clicked, p.format(v))
        }
        Menu(..) => choice(ui, ins, p, label, v, MENU_W),
        _ => choice(ui, ins, p, label, v, knob::CELL_W - 10.0),
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
        egui::Popup::context_menu(&resp).id(resp.id.with("learn_ctx")).show(|ui| app.midi_learn_menu(ui, key.0, control.clone()));
        crate::midi_learn::mark(app, ui.painter(), rect, key.0, control);
    }
    if let Some(x) = new.filter(|x| (x - v).abs() > f32::EPSILON) {
        app.set_builtin_param(key, id, p.clamp(x));
    }
    lay.rows.push(RowLayout { param: id.to_string(), control: [rect.min.x, rect.min.y, rect.max.x, rect.max.y], value: shown });
}

/// The width of a [`Item::Menu`] dropdown.
const MENU_W: f32 = 150.0;

pub(crate) fn choice(ui: &mut Ui, ins: &Insert, p: &ParamInfo, label: &str, v: f32, width: f32) -> (egui::Response, Option<f32>, String) {
    let mut idx = (v.round().max(0.0) as usize).min(p.choices.len().saturating_sub(1));
    let r = ui
        .vertical(|ui| {
            let c = egui::ComboBox::from_id_salt(("builtin_choice", ins.uid.get(), p.id))
                .width(width)
                .selected_text(p.choices.get(idx).copied().unwrap_or(""))
                .show_ui(ui, |ui| {
                    for (i, name) in p.choices.iter().enumerate() {
                        ui.selectable_value(&mut idx, i, *name);
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

/// Diameter of one voice light and the lights per line.
const LIGHT: f32 = 9.0;
const LIGHTS_PER_LINE: usize = 8;

/// The voice activity lights: one per voice of the instrument's `voices` count, filled while a key
/// holds the voice, half-lit while its release tail plays, an outline while it is free. Read from
/// the instrument's scope tap (the render stores `Plugin::voice_activity` while the panel is open).
fn lights(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, lay: &mut EditorLayout) {
    let count = ins.params.get("voices").map_or(8, |v| v.round().clamp(0.0, 15.0) as usize + 1);
    let uid = crate::scope_strip::hub_uid(key, ins);
    let activity = app.scope_hub().map_or(0, |h| h.voice_activity(key.0, uid));
    let lines = count.div_ceil(LIGHTS_PER_LINE);
    let cols = count.min(LIGHTS_PER_LINE);
    let gap = 4.0;
    let size = egui::vec2(cols as f32 * (LIGHT + gap), lines as f32 * (LIGHT + gap));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    crate::help::hover(ui, &resp, crate::help::Id::MacroOscVoiceLights);
    let mut text = String::with_capacity(count);
    for i in 0..count {
        let (on, held) = (activity >> i & 1 == 1, activity >> (16 + i) & 1 == 1);
        let c = rect.min
            + egui::vec2((i % LIGHTS_PER_LINE) as f32 * (LIGHT + gap) + LIGHT / 2.0, (i / LIGHTS_PER_LINE) as f32 * (LIGHT + gap) + LIGHT / 2.0);
        let p = ui.painter();
        if held {
            p.circle_filled(c, LIGHT / 2.0, TOKENS.accent);
        } else if on {
            p.circle_filled(c, LIGHT / 2.0, TOKENS.accent.gamma_multiply(0.45));
        } else {
            p.circle_stroke(c, LIGHT / 2.0 - 0.5, egui::Stroke::new(1.0, TOKENS.text_dim));
        }
        text.push(if held {
            'H'
        } else if on {
            'R'
        } else {
            '-'
        });
    }
    lay.lights = text;
    lay.lights_rect = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
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
        egui::ComboBox::from_id_salt(("panel_key", t.0, slot)).width(150.0).selected_text(label).show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "None").clicked() {
                pick = Some(None);
            }
            for (id, name) in &tracks {
                if ui.selectable_label(current.is_some_and(|k| k.from == KeyFrom::Track(*id)), name).clicked() {
                    pick = Some(Some(KeyFrom::Track(*id)));
                }
            }
            for (id, name) in &busses {
                if ui.selectable_label(current.is_some_and(|k| k.from == KeyFrom::Bus(*id)), name).clicked() {
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
            Knob(id, _) | Toggle(id, _) | Choice(id, _) | Menu(id, _) => vec![id],
            Matrix => crate::panel_matrix::params().collect(),
            Key | Lights => Vec::new(),
        }
    }

    #[test]
    fn every_layout_places_each_parameter_of_its_plugin_exactly_once() {
        for plugin in ["compressor", "expander_gate", "eq_7band", "plate_reverb", MACRO_OSC, FILTER] {
            let rows = layout(plugin).unwrap_or_else(|| panic!("a layout for {plugin}"));
            let mut ids: Vec<&str> = rows.iter().flat_map(|r| r.iter()).flat_map(|s| s.items.iter()).flat_map(|i| item_ids(*i)).collect();
            ids.sort_unstable();
            let mut want: Vec<&str> = transit_dsp::plugin_info(plugin).unwrap().params.iter().map(|p| p.id).collect();
            want.sort_unstable();
            assert_eq!(ids, want, "{plugin}");
        }
    }

    #[test]
    fn every_macro_oscillator_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(MACRO_OSC).unwrap().params {
            assert!(help(MACRO_OSC, p.id).is_some(), "{}", p.id);
        }
        assert!(help("compressor", "threshold").is_none());
    }

    #[test]
    fn every_filter_control_has_a_help_entry() {
        for p in transit_dsp::plugin_info(FILTER).unwrap().params {
            assert!(help(FILTER, p.id).is_some(), "{}", p.id);
        }
    }
}
