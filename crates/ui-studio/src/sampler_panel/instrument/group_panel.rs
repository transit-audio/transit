//! The Group panel (README, Sampler views, Instrument view and round 2; the boards
//! `SamplerInstrument` and `SamplerRound2`, answers 12 and 13): the selected zone's group.
//!
//! - **Header:** `GROUP E2` (the selected zone's root) and `D#2–F2 · 2 of 8 layers`; a zone in no
//!   group reads `D#2–F2 · no group`, a pad's zone `C1 · pad A1`.
//! - **Layers:** one row per velocity layer, the highest first: its range (and its first
//!   alternative's label, `64–127 ff`), its round-robin alternatives as 22 px chips (`rr1`, the
//!   selected zone's with a 2 px `signal` edge; a click selects it) and a dashed add chip. A zone in
//!   no group shows as its own one-chip row.
//! - **Round robin** as a segmented `Sequential · Random` (`mix.sampler_group_set`), live only for a
//!   zone in a group; under it the patch's **seed** (`mix.sampler_seed`): an integer field and the
//!   reroll button (Lucide `dices`) while Random, `—` in `ink.arrow` with the tooltip `Random only`
//!   while Sequential (the stored seed is kept).
//! - **Vel. sens.**: the selected zone's velocity sensitivity as a horizontal fader (0 to 100
//!   percent, `mix.sampler_zone_edit {playback: {velocity_sensitivity}}` on release).
//! - The closing line says that making a pad's zone a group unbinds it; while the add chip asks
//!   about a pad's zone, the Notice takes its place with **Cancel** and **Make group**.
//!
//! The panel draws and reports what was asked ([`Intent`]); the view sends it.

use super::color;
use super::state::{State, Zone};
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, fader as fd, layout, radius, sampler as tk};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use transit_model::sampler_patch::{LAYERS, RoundRobin};

pub const MODES: [&str; 2] = ["Sequential", "Random"];
pub const NOTE: &str = "Making a pad's zone a group unbinds it from its pad; switching views never does (ADR 0041).";
pub const RANDOM_ONLY: &str = "Random only";
pub const NO_GROUP: &str = "A zone in a group only: add an alternative to make one";
pub const REROLL: &str = "New seed";

const fn font(family: tokens::Family, (weight, size): (f32, f32)) -> tokens::TypeRole {
    tokens::TypeRole { family, weight: weight as u16, size, tracking: 0.0, uppercase: false, ink: None }
}
const CHIP_FONT: tokens::TypeRole = font(tokens::Family::Mono, tk::CHIP_FONT);
const CAPTION: tokens::TypeRole = font(tokens::Family::Sans, tk::CAPTION_FONT);

/// The Notice's words for pad `id`.
pub fn ask_text(id: &str) -> String {
    format!("Pad {id}'s zone becomes a group and leaves pad {id}, which will be empty. Its notes still play the group.")
}

/// One velocity layer as shown: its layer number (none for a zone in no group), its velocities,
/// its label and its alternatives' zones by alternative.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub layer: Option<u8>,
    pub velocities: [u8; 2],
    pub label: Option<String>,
    pub zones: Vec<u16>,
}

/// The layers of `z`'s group, highest first (or `z` alone).
pub fn layers(state: &State, z: &Zone) -> Vec<Layer> {
    let label = |n: u16| state.entry(n).and_then(|e| e.label.clone()).filter(|l| !l.is_empty());
    let Some(m) = z.membership else {
        let velocities = if z.pad.is_some() { [1, 127] } else { z.velocities };
        return vec![Layer { layer: None, velocities, label: label(z.zone), zones: vec![z.zone] }];
    };
    let members: Vec<&Zone> = state.zones.iter().filter(|o| o.membership.is_some_and(|p| p.group == m.group)).collect();
    let mut numbers: Vec<u8> = members.iter().filter_map(|o| o.membership.map(|p| p.layer)).collect();
    numbers.sort_unstable_by(|a, b| b.cmp(a));
    numbers.dedup();
    numbers
        .into_iter()
        .map(|l| {
            let mut alts: Vec<&Zone> = members.iter().copied().filter(|o| o.membership.is_some_and(|p| p.layer == l)).collect();
            alts.sort_by_key(|o| o.membership.map_or(0, |p| p.alternative));
            Layer { layer: Some(l), velocities: alts[0].velocities, label: label(alts[0].zone), zones: alts.iter().map(|o| o.zone).collect() }
        })
        .collect()
}

/// The header's summary for `z`.
pub fn summary(state: &State, z: &Zone, names: transit_midi::MiddleC) -> String {
    let n = |k: u8| transit_midi::note_name(k, names);
    if let Some(p) = z.pad {
        let note = state.pad_note(p).map(n).unwrap_or_default();
        return format!("{note} · pad {}", super::super::pad_id(p - 1));
    }
    let keys = if z.keys[0] == z.keys[1] { n(z.keys[0]) } else { format!("{}–{}", n(z.keys[0]), n(z.keys[1])) };
    if z.membership.is_some() { format!("{keys} · {} of {LAYERS} layers", layers(state, z).len()) } else { format!("{keys} · no group") }
}

/// What the panel asks for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Select(u16),
    /// The add chip of a layer (none: a zone in no group).
    Add(Option<u8>),
    Cancel,
    MakeGroup,
    RoundRobin(u8, RoundRobin),
    /// The seed as typed (a number when it reads as one).
    Seed(Value),
    Reroll,
    Sensitivity(f32),
}

/// One chip as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct ChipRecord {
    pub zone: u16,
    pub text: String,
    pub rect: [f32; 4],
    pub selected: bool,
    pub edge_width: f32,
}

/// One layer row as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRecord {
    pub layer: Option<u8>,
    pub row: [f32; 4],
    pub label: (String, [f32; 4]),
    pub chips: Vec<ChipRecord>,
    pub add: [f32; 4],
}

/// The Notice as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct NoticeRecord {
    pub rect: [f32; 4],
    pub text: String,
    pub cancel: [f32; 4],
    pub make: [f32; 4],
}

/// The panel as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupPanelLayout {
    pub panel: [f32; 4],
    pub label: (String, [f32; 4]),
    pub summary: (String, [f32; 4]),
    pub layers: Vec<LayerRecord>,
    pub divider: [f32; 4],
    pub rr_label: [f32; 4],
    pub rr: Option<segmented::SegmentedLayout>,
    /// The round-robin control's and the seed's opacity (the unused alpha when inert).
    pub rr_opacity: f32,
    pub seed_label: [f32; 4],
    pub seed: [f32; 4],
    pub seed_text: String,
    pub seed_live: bool,
    pub reroll: [f32; 4],
    pub vel_label: [f32; 4],
    pub fader: [f32; 4],
    pub cap: [f32; 4],
    pub vel_text: String,
    pub note: Option<(String, [f32; 4])>,
    pub notice: Option<NoticeRecord>,
    /// The seed field has the keyboard or holds typing.
    pub typing: bool,
}

/// What the panel shows beside the state.
pub struct GroupInputs<'a> {
    pub names: transit_midi::MiddleC,
    /// The add chip asked about this pad zone (the Notice shows).
    pub asking: bool,
    /// The seed field's typing and the fader's value while dragged.
    pub seed_text: &'a mut Option<String>,
    pub fader: &'a mut Option<f32>,
    /// The view's typing key survived since the last frame (the device window's Escape removes it).
    pub kept: bool,
}

fn line_h(p: &egui::Painter, role: &tokens::TypeRole) -> f32 {
    widgets::role_size(p, role, "Ag").y
}

fn header_h(p: &egui::Painter) -> f32 {
    line_h(p, &tokens::LABEL_SECTION).max(line_h(p, &tokens::READOUT_DISPLAY))
}

fn wrapped(p: &egui::Painter, role: &tokens::TypeRole, text: &str, width: f32, ink: egui::Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = role.job(p.ctx(), text, ink);
    job.wrap.max_width = width.max(40.0);
    p.layout_job(job)
}

fn notice_h(p: &egui::Painter, text: &str, width: f32) -> f32 {
    let [top, right, bottom, left] = chrome::NOTICE_PADDING;
    top + wrapped(p, &tokens::LABEL_CONTROL, text, width - left - right, TOKENS.text).size().y + SPACE[1] + layout::DENSE_CONTROL_HEIGHT + bottom
}

/// The closing part's height: the Notice while asking, else the note line.
fn foot_h(p: &egui::Painter, width: f32, ask: Option<&str>) -> f32 {
    match ask {
        Some(t) => notice_h(p, t, width),
        None => wrapped(p, &CAPTION, NOTE, width, TOKENS.text_faint).size().y,
    }
}

/// The panel's height for `layers` rows across `width` (the Notice's text when asking).
pub fn height(p: &egui::Painter, width: f32, layers: usize, ask: Option<&str>) -> f32 {
    let inner = width - 2.0 * layout::PANEL_PADDING[1];
    let n = layers.max(1) as f32;
    let rows = n * tk::LAYER_HEIGHT + (n - 1.0) * tk::LAYER_GAP;
    // Header, rows, divider, Round robin, Seed and Vel. sens., the closing part: six gaps between.
    2.0 * layout::PANEL_PADDING[0] + header_h(p) + rows + 1.0 + 3.0 * layout::DENSE_CONTROL_HEIGHT + foot_h(p, inner, ask) + 6.0 * tk::GROUP_ROW_GAP
}

/// A dashed rect's edge (the add chip).
fn dashed(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let r = r.shrink(stroke.width / 2.0);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    let [dash, gap] = tk::CANDIDATE_DASH;
    p.extend(Shape::dashed_line(&pts, stroke, dash, gap));
}

/// Draws the panel into `rect` for instance `uid` and the selected zone `z`.
#[allow(clippy::too_many_lines)]
pub(crate) fn show(ui: &mut Ui, uid: u64, state: &State, z: &Zone, inputs: GroupInputs, rect: Rect) -> (GroupPanelLayout, Option<Intent>) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mut out = GroupPanelLayout { panel: r4(rect), rr_opacity: 1.0, ..GroupPanelLayout::default() };
    let mut intent = None;
    p.rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]));
    let gap = tk::GROUP_ROW_GAP;
    // Header.
    let hh = header_h(&p);
    let cy = inner.min.y + hh / 2.0;
    let grouped = z.membership.is_some();
    let label = if grouped { format!("Group {}", transit_midi::note_name(z.root, inputs.names)) } else { "Group".to_string() };
    let lr = widgets::role_text(&p, &tokens::LABEL_SECTION, &label, t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, cy));
    out.label = (label, r4(lr));
    let s = summary(state, z, inputs.names);
    let sr = widgets::role_text(&p, &tokens::READOUT_DISPLAY, &s, t.text_faint, Align2::LEFT_CENTER, pos2(lr.max.x + gap, cy));
    out.summary = (s, r4(sr));
    // Layer rows.
    let mut y = inner.min.y + hh + gap;
    let selected_layer = z.membership.map(|m| m.layer);
    for (i, l) in layers(state, z).into_iter().enumerate() {
        if i > 0 {
            y += tk::LAYER_GAP;
        }
        let row = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), tk::LAYER_HEIGHT));
        let rcy = row.center().y;
        let mine = l.layer == selected_layer;
        let ink = if mine { t.text } else { t.text_muted };
        let text = match &l.label {
            Some(lb) => format!("{}–{} {lb}", l.velocities[0], l.velocities[1]),
            None => format!("{}–{}", l.velocities[0], l.velocities[1]),
        };
        let label_r = widgets::role_text(&p, &tokens::VALUE, &text, ink, Align2::LEFT_CENTER, pos2(row.min.x, rcy));
        let mut x = row.min.x + tk::LAYER_LABEL_WIDTH + tk::LAYER_ITEM_GAP;
        let mut chips = Vec::new();
        for (a, zone) in l.zones.iter().enumerate() {
            let ctext = format!("rr{}", a + 1);
            let w = (widgets::role_size(&p, &CHIP_FONT, &ctext).x + 2.0 * tk::CHIP_PADDING_X).max(tk::CHIP_MIN_WIDTH);
            let cr = Rect::from_min_size(pos2(x, rcy - tk::CHIP / 2.0), vec2(w, tk::CHIP));
            let selected = *zone == z.zone;
            let (edge, ew, cink) =
                if selected { (t.signal, tk::SELECTED_BORDER_WIDTH, t.text) } else { (color(tk::CHIP_EDGE), chrome::FIELD_EDGE_WIDTH, t.text_muted) };
            p.rect(cr, tk::CHIP_RADIUS, color(tk::CHIP_FILL), Stroke::new(ew, edge), StrokeKind::Inside);
            widgets::role_text(&p, &CHIP_FONT, &ctext, cink, Align2::CENTER_CENTER, cr.center());
            if ui.interact(cr, egui::Id::new(("sampler_group_chip", uid, *zone)), Sense::click()).clicked() {
                intent = Some(Intent::Select(*zone));
            }
            chips.push(ChipRecord { zone: *zone, text: ctext, rect: r4(cr), selected, edge_width: ew });
            x = cr.max.x + tk::LAYER_ITEM_GAP;
        }
        let aw = widgets::role_size(&p, &CAPTION, "+").x + 2.0 * tk::ADD_CHIP_PADDING_X + 2.0 * chrome::FIELD_EDGE_WIDTH;
        let ar = Rect::from_min_size(pos2(x, rcy - tk::CHIP / 2.0), vec2(aw, tk::CHIP));
        dashed(&p, ar, Stroke::new(chrome::FIELD_EDGE_WIDTH, color(tk::ADD_CHIP_EDGE)));
        widgets::role_text(&p, &CAPTION, "+", color(tk::ADD_CHIP_INK), Align2::CENTER_CENTER, ar.center());
        let tip = if l.layer.is_some() { "Add an alternative to this layer: choose a sample" } else { "Make this zone a group" };
        if ui.interact(ar, egui::Id::new(("sampler_group_add", uid, l.layer.map_or(-1, i32::from))), Sense::click()).on_hover_text(tip).clicked() {
            intent = Some(Intent::Add(l.layer));
        }
        out.layers.push(LayerRecord { layer: l.layer, row: r4(row), label: (text, r4(label_r)), chips, add: r4(ar) });
        y += tk::LAYER_HEIGHT;
    }
    // The divider, then Round robin.
    y += gap;
    let div = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 1.0));
    p.rect_filled(div, 0.0, color(tk::GROUP_DIVIDER));
    out.divider = r4(div);
    y += 1.0 + gap;
    let row = |y: f32| Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), layout::DENSE_CONTROL_HEIGHT));
    let control_x = inner.min.x + tk::GROUP_LABEL_WIDTH + gap;
    let r = row(y);
    out.rr_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Round robin", t.text_muted, Align2::LEFT_CENTER, pos2(r.min.x, r.center().y)));
    let group = z.membership.map(|m| m.group);
    let mode = group.and_then(|g| state.groups.iter().find(|e| e.group == g)).map_or(RoundRobin::Sequential, |e| e.round_robin);
    let seg = super::super::slices::segmented_at(
        ui,
        egui::Id::new(("sampler_group_rr", uid)),
        pos2(control_x, r.min.y),
        &MODES,
        usize::from(mode == RoundRobin::Random),
        grouped,
    );
    if !grouped {
        out.rr_opacity = tk::UNUSED_CONTROL_ALPHA;
        ui.interact(super::super::grid_rect(seg.layout.track), egui::Id::new(("sampler_group_rr_tip", uid)), Sense::hover()).on_hover_text(NO_GROUP);
    }
    if let (Some(i), Some(g)) = (seg.picked, group) {
        intent = Some(Intent::RoundRobin(g, if i == 1 { RoundRobin::Random } else { RoundRobin::Sequential }));
    }
    out.rr = Some(seg.layout);
    // The seed: live while the group is Random.
    y += layout::DENSE_CONTROL_HEIGHT + gap;
    let r = row(y);
    out.seed_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Seed", t.text_muted, Align2::LEFT_CENTER, pos2(r.min.x, r.center().y)));
    let field = Rect::from_min_size(pos2(control_x, r.min.y), vec2(tk::SEED_WIDTH, layout::DENSE_CONTROL_HEIGHT));
    out.seed = r4(field);
    let live = grouped && mode == RoundRobin::Random;
    out.seed_live = live;
    let stored = state.seed.to_string();
    if live {
        let mut text = inputs.seed_text.clone().unwrap_or_else(|| stored.clone());
        let resp = ui
            .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                let edit = egui::TextEdit::singleline(&mut text)
                    .id(seed_id(uid))
                    .font(tokens::VALUE.font_id(ui.ctx()))
                    .horizontal_align(egui::Align::RIGHT)
                    .desired_width(field.width())
                    .min_size(field.size())
                    .vertical_align(egui::Align::Center);
                crate::chrome::text_field_enabled(ui, true, edit)
            })
            .inner
            .on_hover_text(format!("Round-robin seed, {} to {}", tk::SEED_MIN, tk::SEED_MAX));
        let had_id = seed_id(uid).with("had");
        let had = ui.data(|d| d.get_temp::<bool>(had_id).unwrap_or(false));
        let focused = resp.has_focus();
        ui.data_mut(|d| d.insert_temp(had_id, focused));
        if focused || resp.changed() {
            *inputs.seed_text = Some(text.clone());
        }
        if resp.lost_focus() && had {
            let typed = inputs.seed_text.take();
            let escape = !inputs.kept || ui.input(|i| crate::shortcuts::hit(i, crate::shortcuts::Action::CloseEditor));
            if let Some(typed) = typed.filter(|s| !escape && s.trim() != stored) {
                let s = typed.trim();
                intent = Some(Intent::Seed(match serde_json::from_str::<Value>(s) {
                    Ok(Value::Number(n)) => Value::Number(n),
                    _ => json!(s),
                }));
            }
        }
        out.typing = focused || inputs.seed_text.is_some();
        out.seed_text = inputs.seed_text.clone().unwrap_or(stored);
    } else {
        // Sequential (or no group): `—`, disabled, the stored seed kept.
        *inputs.seed_text = None;
        p.rect(field, chrome::RADIUS_CONTROL, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line), StrokeKind::Inside);
        let pad = chrome::FIELD_PADDING[1];
        widgets::role_text(&p, &tokens::VALUE, "—", color(tk::SEED_SEQUENTIAL), Align2::RIGHT_CENTER, pos2(field.max.x - pad, field.center().y));
        ui.interact(field, seed_id(uid), Sense::hover()).on_hover_text(RANDOM_ONLY);
        out.seed_text = "—".into();
    }
    // The reroll button.
    let b = Rect::from_min_size(pos2(field.max.x + gap, r.min.y), vec2(layout::DENSE_CONTROL_HEIGHT, layout::DENSE_CONTROL_HEIGHT));
    out.reroll = r4(b);
    let resp = ui.interact(b, egui::Id::new(("sampler_group_reroll", uid)), if live { Sense::click() } else { Sense::hover() });
    let a = if live { 1.0 } else { tk::UNUSED_CONTROL_ALPHA };
    if live && (resp.hovered() || resp.is_pointer_button_down_on()) {
        p.rect_filled(b, chrome::RADIUS_CONTROL, if resp.is_pointer_button_down_on() { t.surface_selected } else { t.button_hover });
    }
    crate::icons::paint(&p, b.center(), crate::icons::Icon::Dices, chrome::ICON_GLYPH, t.text_muted.gamma_multiply(a));
    let resp = resp.on_hover_text(if live { REROLL } else { RANDOM_ONLY });
    if live && resp.clicked() {
        intent = Some(Intent::Reroll);
    }
    // Velocity sensitivity: a horizontal fader, 0 to 100 percent.
    y += layout::DENSE_CONTROL_HEIGHT + gap;
    let r = row(y);
    out.vel_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Vel. sens.", t.text_muted, Align2::LEFT_CENTER, pos2(r.min.x, r.center().y)));
    let track = Rect::from_center_size(pos2(control_x + tk::GROUP_FADER_LENGTH / 2.0, r.center().y), vec2(tk::GROUP_FADER_LENGTH, fd::TRACK_WIDTH));
    let [cw, ch] = fd::CAP_HORIZONTAL;
    let hit = Rect::from_min_max(pos2(track.min.x - cw / 2.0, r.min.y), pos2(track.max.x + cw / 2.0, r.max.y));
    out.fader = r4(track);
    let stored = state.entry(z.zone).map_or(0.7, |e| e.velocity_sensitivity);
    let resp = ui
        .interact(hit, egui::Id::new(("sampler_group_vel", uid)), Sense::click_and_drag())
        .on_hover_text("Velocity sensitivity of the selected zone");
    if let Some(at) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked() || resp.drag_stopped()) {
        *inputs.fader = Some((((at.x - track.min.x) / track.width()).clamp(0.0, 1.0) * 100.0).round() / 100.0);
    }
    let value = inputs.fader.unwrap_or(stored);
    if (resp.drag_stopped() || resp.clicked())
        && let Some(v) = inputs.fader.take()
        && (v - stored).abs() > 0.001
    {
        intent = Some(Intent::Sensitivity(v));
    }
    if !resp.dragged() && !resp.drag_stopped() && !resp.clicked() {
        *inputs.fader = None;
    }
    let cx = track.min.x + value * track.width();
    p.rect_filled(track, fd::TRACK_RADIUS, t.line);
    p.rect_filled(Rect::from_min_max(track.min, pos2(cx, track.max.y)), fd::TRACK_RADIUS, t.signal);
    let cap = Rect::from_center_size(pos2(cx, track.center().y), vec2(cw, ch));
    p.rect_filled(cap, fd::CAP_RADIUS, t.knob_cap_active);
    p.rect_stroke(cap, fd::CAP_RADIUS, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.ink_arrow), StrokeKind::Inside);
    out.cap = r4(cap);
    out.vel_text = format!("{:.0}%", value * 100.0);
    widgets::role_text(&p, &tokens::VALUE, &out.vel_text, t.text, Align2::LEFT_CENTER, pos2(track.max.x + gap, r.center().y));
    // The closing line, or the Notice while asking.
    y += layout::DENSE_CONTROL_HEIGHT + gap;
    match z.pad.filter(|_| inputs.asking) {
        Some(pad) => {
            let text = ask_text(&super::super::pad_id(pad - 1));
            let h = notice_h(&p, &text, inner.width());
            let card = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), h));
            let [top, right, bottom, left] = chrome::NOTICE_PADDING;
            p.rect(card, chrome::NOTICE_RADIUS, t.surface_raised, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_strong), StrokeKind::Inside);
            let galley = wrapped(&p, &tokens::LABEL_CONTROL, &text, inner.width() - left - right, t.text);
            p.galley(pos2(card.min.x + left, card.min.y + top), galley, t.text);
            let by = card.max.y - bottom - layout::DENSE_CONTROL_HEIGHT;
            let mw = super::super::slices::detect_panel::default_w(&p, "Make group");
            let make = Rect::from_min_size(pos2(card.max.x - right - mw, by), vec2(mw, layout::DENSE_CONTROL_HEIGHT));
            let cwd = super::super::slices::bench::button_w(&p, "Cancel");
            let cancel = Rect::from_min_size(pos2(make.min.x - SPACE[1] - cwd, by), vec2(cwd, layout::DENSE_CONTROL_HEIGHT));
            if super::super::slices::bench::dense_button(ui, egui::Id::new(("sampler_group_cancel", uid)), cancel, "Cancel", true).clicked() {
                intent = Some(Intent::Cancel);
            }
            if super::super::slices::detect_panel::default_button(ui, egui::Id::new(("sampler_group_make", uid)), make, "Make group").clicked() {
                intent = Some(Intent::MakeGroup);
            }
            out.notice = Some(NoticeRecord { rect: r4(card), text, cancel: r4(cancel), make: r4(make) });
        }
        None => {
            let galley = wrapped(&p, &CAPTION, NOTE, inner.width(), t.text_faint);
            let nr = Rect::from_min_size(pos2(inner.min.x, y), galley.size());
            p.galley(nr.min, galley, t.text_faint);
            out.note = Some((NOTE.to_string(), r4(nr)));
        }
    }
    (out, intent)
}

/// The seed field's id on instance `uid`.
pub fn seed_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_group_seed", uid))
}
