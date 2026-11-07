//! The module's own sections (README, Device anatomy C; the board's Row B right column): Tone with
//! the three hero knobs at size L, Gate · LPG and Pitch side by side at size M, and Output with
//! Aux mix and Level beside the Mod wheel and Voices dropdowns and the voice lights.
//!
//! The voice lights are the board's 10 px squares (radius 3), eight to a row: `signal` while a
//! key holds the voice, `signal` at the releasing alpha while its release fades, `line` while it
//! is free.

use super::{Dev, Lead, MacroLayout, dropdown, finish, knob_row, panel, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::{alpha, layout as space};
use crate::widgets::{knob, r4};
use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use transit_dsp::plugins::macro_osc::models::model_info;

/// A voice light, the gap between lights, and the lights per row.
pub const LIGHT: f32 = 10.0;
/// A voice light's corner (the board's 3).
pub const LIGHT_RADIUS: f32 = 3.0;
const LIGHT_GAP: f32 = 5.0;
const PER_ROW: usize = 8;
/// The Output panel's dropdowns.
pub const DROPDOWN_W: f32 = 104.0;
const DROPDOWN_H: f32 = space::DENSE_CONTROL_HEIGHT;
const LABEL_GAP: f32 = 10.0;

pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let pad = space::PANEL_PADDING;
    let l = knob::cell(knob::Size::L).y;
    let m = knob::cell(knob::Size::M).y;
    let tone_h = pad[0] * 2.0 + 14.0 + LABEL_GAP + l;
    let mid_h = pad[0] * 2.0 + 14.0 + 8.0 + m;
    // Tone.
    let tone = Rect::from_min_size(rect.min, vec2(rect.width(), tone_h));
    let inner = panel(ui, tone, "tone", out);
    section_label(ui, inner, "Tone");
    let row = Rect::from_min_max(pos2(inner.min.x, inner.min.y + 14.0 + LABEL_GAP), inner.max);
    // The three labels follow the selected model (326-N).
    let info = model_info(dev.index(app, "model"));
    knob_row(app, ui, dev, row, &[("harmonics", info.harmonics), ("timbre", info.timbre), ("morph", info.morph)], knob::Size::L, lay, out);
    // Gate · LPG and Pitch.
    let y = tone.max.y + space::PANEL_GAP;
    let half = (rect.width() - space::PANEL_GAP) / 2.0;
    for (i, (name, title, items)) in
        [("gate", "Gate · LPG", [("decay", "Decay"), ("lpg_color", "LPG color")]), ("pitch", "Pitch", [("tune", "Tune"), ("bend_range", "Bend")])]
            .into_iter()
            .enumerate()
    {
        let r = Rect::from_min_size(pos2(rect.min.x + i as f32 * (half + space::PANEL_GAP), y), vec2(half, mid_h));
        let inner = panel(ui, r.shrink2(vec2(0.0, 0.0)), name, out);
        let inner = Rect::from_min_max(pos2(r.min.x + 12.0, inner.min.y), pos2(r.max.x - 12.0, inner.max.y));
        section_label(ui, inner, title);
        let row = Rect::from_min_max(pos2(inner.min.x, inner.min.y + 14.0 + 8.0), inner.max);
        knob_row(app, ui, dev, row, &items, knob::Size::M, lay, out);
    }
    // Output: the rest of the column.
    let oy = y + mid_h + space::PANEL_GAP;
    let outr = Rect::from_min_max(pos2(rect.min.x, oy), rect.max);
    output(app, ui, dev, outr, lay, out);
}

/// The Output panel.
fn output(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "output", out);
    let inner = Rect::from_min_max(pos2(rect.min.x + 12.0, inner.min.y), pos2(rect.max.x - 12.0, inner.max.y));
    let cell = knob::cell(knob::Size::M);
    let knobs_w = 2.0 * cell.x.max(64.0) + 8.0;
    let ky = inner.center().y - cell.y / 2.0;
    knob_row(
        app,
        ui,
        dev,
        Rect::from_min_size(pos2(inner.min.x, ky), vec2(knobs_w, cell.y)),
        &[("aux_mix", "Aux mix"), ("level", "Level")],
        knob::Size::M,
        lay,
        out,
    );
    // The divider and the control column.
    let cx = inner.min.x + knobs_w + 8.0;
    ui.painter().line_segment([pos2(cx, inner.min.y), pos2(cx, inner.max.y)], Stroke::new(1.0, t.line));
    let col = Rect::from_min_max(pos2(cx + 8.0, inner.min.y), inner.max);
    let mut y = col.min.y;
    for (id, label, mono) in [("mod_wheel", "Mod wheel", false), ("voices", "Voices", true)] {
        let Some(p) = dev.param(id) else { continue };
        widgets_label(ui, label, pos2(col.min.x, y + DROPDOWN_H / 2.0));
        let r = Rect::from_min_size(pos2(col.max.x - DROPDOWN_W, y), vec2(DROPDOWN_W, DROPDOWN_H));
        let sel = dev.index(app, id);
        let text = dev.text(app, id);
        let (resp, pick) =
            dropdown(ui, egui::Id::new(("macro_osc_drop", dev.uid(), id)), r, t.surface_inset, Lead::None, &text, mono, p.choices, sel);
        finish(app, ui, dev, id, &resp, pick.map(|i| i as f32), lay);
        out.dropdowns.push((id.to_string(), r4(r), text));
        y += DROPDOWN_H + 8.0;
    }
    // The voice lights, right-aligned under the dropdowns.
    let states = out.lights.clone();
    let count = states.chars().count();
    let cols = count.min(PER_ROW);
    let w = cols as f32 * LIGHT + cols.saturating_sub(1) as f32 * LIGHT_GAP;
    let rows = count.div_ceil(PER_ROW).max(1);
    let lights = Rect::from_min_size(pos2(col.max.x - w, y + 2.0), vec2(w, rows as f32 * LIGHT + (rows - 1) as f32 * LIGHT_GAP));
    let resp = ui.interact(lights, egui::Id::new(("macro_osc_lights", dev.uid())), Sense::hover());
    crate::help::hover(ui, &resp, crate::help::Id::MacroOscVoiceLights);
    let p = ui.painter();
    for (i, s) in states.chars().enumerate() {
        let r = Rect::from_min_size(
            pos2(lights.min.x + (i % PER_ROW) as f32 * (LIGHT + LIGHT_GAP), lights.min.y + (i / PER_ROW) as f32 * (LIGHT + LIGHT_GAP)),
            vec2(LIGHT, LIGHT),
        );
        let fill = match s {
            'H' => t.signal,
            'R' => t.signal.gamma_multiply(alpha::VOICE_RELEASING),
            _ => t.line,
        };
        p.rect_filled(r, LIGHT_RADIUS, fill);
    }
    out.lights_rect = r4(lights);
}

fn widgets_label(ui: &Ui, text: &str, at: egui::Pos2) {
    crate::widgets::role_text(ui.painter(), &crate::tokens::LABEL_CONTROL, text, TOKENS.text_muted, Align2::LEFT_CENTER, at);
}
