//! The MIDI effects group of the device chain (#201, ADR 0036): an instrument track's MIDI
//! effects in their own card ahead of the instrument, listed top to bottom in processing order
//! (Logic's MIDI FX slots above the instrument slot; Ableton's MIDI effects left of the
//! instrument). Each effect has a row (power for bypass, its name, remove) and a row per
//! parameter; right-click a row to move it up or down. "+" in the card's title adds a built-in at
//! the end. Every edit is one undo step; a parameter drag is one step for the whole drag.

use crate::StudioApp;
use crate::arrange::R4;
use crate::icons::Icon;
use crate::mixer::MenuItem;
use crate::theme::TOKENS;
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use serde_json::json;
use transit_model::{MAX_MIDI_EFFECTS, Track, TrackId, TrackKind};

/// The card's width.
pub const CARD_W: f32 = 184.0;
const HEAD_H: f32 = 22.0;
const ROW_H: f32 = 20.0;
const PARAM_H: f32 = 18.0;
const PAD: f32 = 6.0;

/// The group as last drawn (for `ui.inspect` and the tests).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MidiFxLayout {
    pub rect: R4,
    /// "+": add a MIDI effect.
    pub add: R4,
    pub rows: Vec<MidiFxRow>,
}

/// One MIDI effect's rows as last drawn.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MidiFxRow {
    pub index: usize,
    pub plugin: String,
    pub name: String,
    pub bypass: bool,
    pub rect: R4,
    pub bypass_button: R4,
    pub remove_button: R4,
    /// Parameter id, its value as stored, and the value field.
    pub params: Vec<(String, f32, R4)>,
}

fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

/// Whether the track shows the group: instrument tracks (the chain feeds the instrument) and
/// racks (the chain feeds every member, ADR 0034).
#[must_use]
pub fn shows(t: &Track) -> bool {
    t.kind == TrackKind::Instrument || t.is_rack()
}

fn param_count(plugin: &str) -> usize {
    transit_dsp::midi_fx::info(plugin).map_or(0, |i| i.params.len())
}

/// The card's height for `t`: the title, each effect's row and parameter rows, and the padding.
#[must_use]
pub fn card_height(t: &Track) -> f32 {
    let rows: f32 = t.midi_effects.iter().map(|f| ROW_H + param_count(&f.plugin) as f32 * PARAM_H).sum();
    HEAD_H + rows.max(ROW_H) + PAD
}

/// The help entry of a built-in's parameter rows.
fn param_help(plugin: &str) -> crate::help::Id {
    if plugin == transit_dsp::midi_fx::velocity::VELOCITY_INFO.id {
        crate::help::Id::DeviceChainMidiVelocity
    } else if plugin == transit_dsp::midi_fx::transpose::TRANSPOSE_INFO.id {
        crate::help::Id::DeviceChainMidiTranspose
    } else {
        crate::help::Id::DeviceChainMidiEffect
    }
}

/// The device pane's height the focused track's MIDI effects group needs (0 without one): the
/// card and the pane's padding above and below it.
#[must_use]
pub fn pane_height(app: &StudioApp) -> f32 {
    let Some(t) = app.focus_track().and_then(|id| app.engine.session().track(id)).filter(|t| shows(t)) else { return 0.0 };
    card_height(t) + 16.0
}

/// The display name of MIDI effect `plugin` (its id when this build does not know it).
#[must_use]
pub fn name_of(plugin: &str) -> String {
    transit_dsp::midi_fx::info(plugin).map_or_else(|| plugin.to_string(), |i| i.name.to_string())
}

impl StudioApp {
    /// Adds built-in MIDI effect `plugin` at the end of `t`'s chain (one undo step).
    pub fn add_midi_effect(&mut self, t: TrackId, plugin: &str) -> Result<serde_json::Value, String> {
        self.run("mix.midi_effect", json!({"track": t.0, "plugin": plugin}))
    }

    /// Writes one MIDI effect parameter: a drag is merged into one undo step.
    fn set_midi_effect_param(&mut self, t: TrackId, index: usize, param: &str, value: f32) {
        let key = format!("midi_fx_param:{}:{index}:{param}", t.0);
        if let Err(e) =
            self.engine.execute_merged("mix.midi_effect_param", &json!({"track": t.0, "index": index, "param": param, "value": value}), &key)
        {
            self.status = e.to_string();
        }
    }
}

/// Draws the group card for `t` at `r`; returns its layout.
pub fn card(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, lane: Rect, t: &Track, r: Rect, menu: &mut Vec<MenuItem>) -> MidiFxLayout {
    let pc = p.with_clip_rect(lane);
    let tid = t.id;
    let mut layout = MidiFxLayout { rect: r4(r), ..MidiFxLayout::default() };
    pc.rect_filled(r, crate::theme::RADIUS, TOKENS.panel_raised);
    let title = Rect::from_min_size(r.min, vec2(r.width(), HEAD_H));
    pc.rect_filled(title, crate::theme::top_corners(), TOKENS.button);
    pc.text(pos2(r.min.x + 8.0, title.center().y), Align2::LEFT_CENTER, "MIDI Effects", FontId::proportional(12.0), TOKENS.text);
    let head = ui.interact(
        Rect::from_min_max(title.min, pos2(title.max.x - 24.0, title.max.y)).intersect(lane),
        ui.id().with(("midi_fx_head", tid.0)),
        Sense::hover(),
    );
    crate::help::hover(ui, &head, crate::help::Id::DeviceChainMidiEffects);
    head.on_hover_text("MIDI effects change the notes before the instrument plays them, top to bottom");
    // "+": the built-ins.
    let full = t.midi_effects.len() >= MAX_MIDI_EFFECTS;
    let add = Rect::from_min_size(pos2(title.max.x - 22.0, title.min.y + 2.0), vec2(18.0, 18.0));
    layout.add = r4(add);
    let resp = ui.interact(add.intersect(lane), ui.id().with(("midi_fx_add", tid.0)), Sense::click());
    crate::icons::paint(
        &pc,
        add.center(),
        Icon::Plus,
        14.0,
        if full {
            TOKENS.text_dim
        } else if resp.hovered() {
            TOKENS.accent
        } else {
            TOKENS.text
        },
    );
    crate::help::hover(ui, &resp, crate::help::Id::DeviceChainMidiEffectAdd);
    let resp = resp.on_hover_text(if full { "The track holds eight MIDI effects, the most it can" } else { "Add a MIDI effect" });
    if !full {
        let mut picked = None;
        egui::Popup::menu(&resp).width(180.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            for info in transit_dsp::midi_fx::REGISTRY {
                let b = ui.button(info.name);
                menu.push(MenuItem::at(ui, info.name.to_string(), info.id.to_string(), b.rect));
                if b.clicked() {
                    picked = Some(info.id);
                    ui.close();
                }
            }
        });
        if let Some(id) = picked {
            let _ = app.add_midi_effect(tid, id);
        }
    }
    let mut y = title.max.y;
    if t.midi_effects.is_empty() {
        pc.text(pos2(r.min.x + 8.0, y + ROW_H / 2.0), Align2::LEFT_CENTER, "None: notes go straight in", FontId::proportional(10.5), TOKENS.text_dim);
    }
    let n = t.midi_effects.len();
    for (i, fx) in t.midi_effects.iter().enumerate() {
        let info = transit_dsp::midi_fx::info(&fx.plugin);
        let on = !fx.bypass;
        let rows = param_count(&fx.plugin);
        let block = Rect::from_min_size(pos2(r.min.x, y), vec2(r.width(), ROW_H + rows as f32 * PARAM_H));
        let row = Rect::from_min_size(block.min, vec2(r.width(), ROW_H));
        let mut out =
            MidiFxRow { index: i, plugin: fx.plugin.clone(), name: name_of(&fx.plugin), bypass: fx.bypass, rect: r4(block), ..MidiFxRow::default() };
        if i > 0 {
            pc.hline(egui::Rangef::new(r.min.x + 4.0, r.max.x - 4.0), y, egui::Stroke::new(1.0, TOKENS.line));
        }
        // The row: right-click moves the effect.
        let body = ui.interact(row.intersect(lane), ui.id().with(("midi_fx_row", tid.0, i)), Sense::click());
        crate::help::hover(ui, &body, crate::help::Id::DeviceChainMidiEffect);
        egui::Popup::context_menu(&body).id(body.id.with("ctx")).show(|ui| {
            for (label, to, ok) in [("Move Up", i.wrapping_sub(1), i > 0), ("Move Down", i + 1, i + 1 < n)] {
                let b = ui.add_enabled(ok, egui::Button::new(label));
                menu.push(MenuItem::at(ui, label.into(), to.to_string(), b.rect));
                if b.clicked() {
                    let _ = app.run("mix.midi_effect_move", json!({"track": tid.0, "from": i, "to": to}));
                    ui.close();
                }
            }
        });
        let pb = Rect::from_min_size(pos2(row.min.x + 4.0, row.min.y + 2.0), vec2(16.0, 16.0));
        let resp = ui.interact(pb.intersect(lane), ui.id().with(("midi_fx_bypass", tid.0, i)), Sense::click());
        crate::icons::paint(
            &pc,
            pb.center(),
            Icon::Power,
            13.0,
            if on {
                TOKENS.accent
            } else if resp.hovered() {
                TOKENS.text
            } else {
                TOKENS.text_dim
            },
        );
        crate::help::hover(ui, &resp, crate::help::Id::DeviceChainBypass);
        if resp.on_hover_text(if on { "Bypass" } else { "Turn on" }).clicked() {
            let _ = app.run("mix.midi_effect_bypass", json!({"track": tid.0, "index": i}));
        }
        out.bypass_button = r4(pb);
        let xb = Rect::from_min_size(pos2(row.max.x - 20.0, row.min.y + 2.0), vec2(16.0, 16.0));
        let resp = ui.interact(xb.intersect(lane), ui.id().with(("midi_fx_remove", tid.0, i)), Sense::click());
        crate::icons::paint(&pc, xb.center(), Icon::Remove, 13.0, if resp.hovered() { TOKENS.record } else { TOKENS.text_dim });
        crate::help::hover(ui, &resp, crate::help::Id::DeviceChainRemove);
        if resp.on_hover_text("Remove").clicked() {
            let _ = app.run("mix.midi_effect_remove", json!({"track": tid.0, "index": i}));
        }
        out.remove_button = r4(xb);
        let name_r = Rect::from_min_max(pos2(pb.max.x + 5.0, row.min.y), pos2(xb.min.x - 4.0, row.max.y));
        let label = if fx.bypass { format!("{} (bypassed)", out.name) } else { out.name.clone() };
        pc.with_clip_rect(name_r.intersect(lane)).text(
            name_r.left_center(),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.0),
            if on { TOKENS.text } else { TOKENS.text_dim },
        );
        // One row per parameter: its name and a value field.
        for (k, pi) in info.map_or(&[][..], |i| i.params).iter().enumerate() {
            let pr = Rect::from_min_size(pos2(r.min.x + 10.0, row.max.y + k as f32 * PARAM_H), vec2(r.width() - 16.0, PARAM_H));
            pc.text(pos2(pr.min.x, pr.center().y), Align2::LEFT_CENTER, pi.name, FontId::proportional(10.5), TOKENS.text_dim);
            let field = Rect::from_min_max(pos2(pr.max.x - 64.0, pr.min.y + 1.0), pos2(pr.max.x, pr.max.y - 1.0));
            let stored = fx.params.get(pi.id).copied().unwrap_or(pi.default);
            let mut v = stored;
            let suffix = match pi.unit {
                transit_dsp::Unit::Percent => " %",
                transit_dsp::Unit::Semitones => " st",
                _ => "",
            };
            let resp = if field.intersects(lane) {
                Some(ui.put(field, egui::DragValue::new(&mut v).range(pi.min..=pi.max).speed(0.25).max_decimals(0).suffix(suffix)))
            } else {
                None
            };
            if let Some(resp) = resp {
                crate::help::hover(ui, &resp, param_help(&fx.plugin));
                if resp.changed() && v.to_bits() != stored.to_bits() {
                    app.set_midi_effect_param(tid, i, pi.id, v);
                }
                if resp.drag_stopped() || resp.lost_focus() {
                    app.engine.end_merge();
                }
            }
            out.params.push((pi.id.to_string(), stored, r4(field)));
        }
        layout.rows.push(out);
        y = block.max.y;
    }
    layout
}
