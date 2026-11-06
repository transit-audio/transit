//! The hero row's oscillator panel (README, Device anatomy B; the board's Row B left): the model
//! card, the scope well with its readouts, the Timbre/Morph pad and the model picker.
//!
//! - **Scope well:** the instrument's output envelope from its scope tap (the tap carries each
//!   frame's peak, so the trace is the envelope above and below the center line), `signal` at 2 px
//!   over an 8 percent fill; readouts `WAVE · <model>` on the left and the voice count on the right.
//! - **Pad:** a 212 px display well; x is Timbre, y is Morph. Drag sets both (the point follows the
//!   pointer), Shift drags fine (a tenth of the pointer's travel), double-click resets both. The
//!   point is a 7 px `signal` dot with a 2 px `text` rim over a 16 px halo; a route to Timbre or
//!   Morph draws its reach as a dashed `mod` trail. T and M print below in `value`.
//! - **Model picker:** the 24 models numbered in the firmware's order, eight to a row, grouped
//!   Pitched (1 to 16) and Noise · Perc (17 to 24, the board's group from Swarm on). The current
//!   model is outlined in `signal` on the option fill; a click selects, and with the picker
//!   focused the arrow keys move (left and right one, up and down a row).

use super::{Dev, MacroLayout, finish, panel, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::{self, alpha, layout as space, radius};
use crate::widgets::{self, display_well, r4};
use egui::{Align2, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_dsp::plugins::macro_osc::MODELS;
use transit_dsp::plugins::macro_osc::matrix::dst;

/// The pad's side and the gap above its readouts.
pub const PAD: f32 = 212.0;
const PAD_READOUT: f32 = 15.0;
/// The display row: the pad column's height.
pub const DISPLAY_H: f32 = PAD + 6.0 + PAD_READOUT;
const HEADER_H: f32 = 26.0;
/// A model button's height, the grid's gaps, and the group label column.
pub const MODEL_H: f32 = 30.0;
const MODEL_GAP: f32 = 4.0;
const GROUP_GAP: f32 = 8.0;
const GROUP_W: f32 = 92.0;
const PER_ROW: usize = 8;
/// The pad point's dot, rim and halo radii.
const DOT: f32 = 7.0;
const HALO: f32 = 16.0;
/// Models from here on are the Noise · Perc group.
pub const NOISE_FROM: usize = 16;

/// The picker's height: two rows of Pitched, one of Noise · Perc.
fn picker_h() -> f32 {
    3.0 * MODEL_H + MODEL_GAP + GROUP_GAP
}

/// The hero row's height (the oscillator panel; the sections beside it share it).
pub fn height(_width: f32) -> f32 {
    space::PANEL_PADDING[0] + HEADER_H + space::PANEL_GAP + DISPLAY_H + space::PANEL_GAP + picker_h() + space::PANEL_PADDING[1]
}

/// The voices sounding, from the tap's activity bits: (held, releasing) per voice.
pub fn voice_states(activity: u32, count: usize) -> String {
    (0..count)
        .map(|i| match (activity >> (16 + i) & 1 == 1, activity >> i & 1 == 1) {
            (true, _) => 'H',
            (false, true) => 'R',
            _ => '-',
        })
        .collect()
}

/// "1 voice", "3 voices": the voices sounding (held or releasing).
pub fn voices_text(states: &str) -> String {
    let n = states.chars().filter(|c| *c != '-').count();
    if n == 1 { "1 voice".to_string() } else { format!("{n} voices") }
}

/// The model after an arrow key: left and right step one, up and down a row, within the list.
pub fn model_step(model: usize, key: egui::Key) -> Option<usize> {
    let n = MODELS.len();
    let next = match key {
        egui::Key::ArrowLeft => model.checked_sub(1)?,
        egui::Key::ArrowRight => model + 1,
        egui::Key::ArrowUp => model.checked_sub(PER_ROW)?,
        egui::Key::ArrowDown => model + PER_ROW,
        _ => return None,
    };
    (next < n).then_some(next)
}

/// The pad's normalized position after a drag: absolute (the pointer's place in the pad) or,
/// with Shift, the previous position plus a tenth of the pointer's travel.
pub fn pad_drag(prev: [f32; 2], at: [f32; 2], delta: [f32; 2], fine: bool) -> [f32; 2] {
    if fine { [(prev[0] + delta[0] / 10.0).clamp(0.0, 1.0), (prev[1] + delta[1] / 10.0).clamp(0.0, 1.0)] } else { at }
}

pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "oscillator", out);
    out.hero.panel = r4(rect);
    // Header: the section label and the model's name.
    let label = section_label(ui, Rect::from_min_size(inner.min + vec2(0.0, 6.0), vec2(inner.width(), HEADER_H)), "Oscillator");
    let model = dev.index(app, "model").min(MODELS.len() - 1);
    out.hero.model_name = MODELS[model].to_string();
    let r =
        widgets::role_text(ui.painter(), &tokens::LABEL_HERO, MODELS[model], t.text, Align2::LEFT_CENTER, pos2(label.max.x + 10.0, label.center().y));
    out.hero.model_rect = r4(r);
    let y = inner.min.y + HEADER_H + space::PANEL_GAP;
    let pad_x = inner.max.x - PAD;
    scope(app, ui, dev, Rect::from_min_max(pos2(inner.min.x, y), pos2(pad_x - space::PANEL_GAP, y + DISPLAY_H)), model, out);
    pad(app, ui, dev, Rect::from_min_size(pos2(pad_x, y), vec2(PAD, PAD)), out);
    let py = y + DISPLAY_H + space::PANEL_GAP;
    picker(app, ui, dev, Rect::from_min_max(pos2(inner.min.x, py), pos2(inner.max.x, py + picker_h())), model, lay, out);
}

/// The scope well and its readouts.
fn scope(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, model: usize, out: &mut MacroLayout) {
    let t = &TOKENS;
    let mut c = super::child(ui, rect);
    let well = display_well::show(&mut c, egui::Id::new(("macro_osc_scope", dev.uid())), rect.size(), &display_well::Well::default());
    let plot = display_well::plot_rect(rect, true);
    let p = ui.painter().clone();
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let hub = app.scope_hub();
    let count = dev.index(app, "voices") + 1;
    let activity = hub.as_ref().map_or(0, |h| h.voice_activity(dev.key.0, uid));
    let states = voice_states(activity, count);
    out.hero.readout = format!("WAVE · {}", MODELS[model]);
    out.hero.voices = voices_text(&states);
    let top = rect.min.y + display_well::PADDING + display_well::READOUT_H / 2.0;
    widgets::role_text(&p, &tokens::READOUT_DISPLAY, &out.hero.readout, t.text_faint, Align2::LEFT_CENTER, pos2(plot.min.x, top));
    widgets::role_text(&p, &tokens::READOUT_DISPLAY, &out.hero.voices, t.text_faint, Align2::RIGHT_CENTER, pos2(plot.max.x, top));
    // Grid: the center line and two verticals (the board's 40 and 80 percent).
    let mid = plot.center().y;
    p.line_segment([pos2(plot.min.x, mid), pos2(plot.max.x, mid)], Stroke::new(1.0, t.line_inset));
    for f in [0.4, 0.8] {
        let x = plot.min.x + plot.width() * f;
        p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], Stroke::new(1.0, t.grid));
    }
    let mut frames = std::mem::take(&mut app.builtin_editor.frames);
    frames.resize(transit_mix::SCOPE_FRAMES, 0.0);
    if let Some(h) = hub.as_ref() {
        h.frames(dev.key.0, uid, &mut frames);
    }
    let cols = crate::scope_strip::reduce(&frames, plot.width().max(1.0) as usize);
    app.builtin_editor.frames = frames;
    let half = plot.height() / 2.0 - 2.0;
    let (mut upper, mut lower) = (Vec::with_capacity(cols.len()), Vec::with_capacity(cols.len()));
    for (x, (_, hi)) in cols.iter().enumerate() {
        let h = crate::scope_strip::height(*hi) * half;
        let px = plot.min.x + x as f32 + 0.5;
        upper.push(pos2(px, mid - h));
        lower.push(pos2(px, mid + h));
    }
    let fill = t.signal.gamma_multiply(alpha::SCOPE_FILL);
    let mut mesh = Mesh::default();
    for (a, b) in upper.windows(2).zip(lower.windows(2)) {
        let base = mesh.vertices.len() as u32;
        for v in [a[0], a[1], b[1], b[0]] {
            mesh.colored_vertex(v, fill);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(upper, Stroke::new(display_well::CURVE_STROKE, t.signal)));
    p.add(Shape::line(lower, Stroke::new(display_well::CURVE_STROKE, t.signal)));
    out.hero.scope = r4(rect);
    out.wells.push(super::WellRecord { name: "scope".into(), well: well.layout });
    out.lights = states;
}

/// The Timbre/Morph pad and its readouts.
fn pad(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, out: &mut MacroLayout) {
    let t = &TOKENS;
    let (Some(tp), Some(mp)) = (dev.param("timbre"), dev.param("morph")) else { return };
    let norm = [tp.to_normalized(dev.value(app, "timbre")), mp.to_normalized(dev.value(app, "morph"))];
    let p = ui.painter().clone();
    p.rect(rect, radius::DISPLAY, t.surface_inset, Stroke::new(1.0, t.line_inset), StrokeKind::Inside);
    let plot = rect.shrink(1.0);
    for f in [0.25, 0.5, 0.75] {
        let x = plot.min.x + plot.width() * f;
        let y = plot.min.y + plot.height() * f;
        p.line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], Stroke::new(1.0, t.grid));
        p.line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.0, t.grid));
    }
    let resp = ui.interact(rect, egui::Id::new(("macro_osc_pad", dev.uid())), Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::MacroOscTimbre);
    let mut new = None;
    if resp.dragged() || resp.drag_started() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = ui.input(|i| i.pointer.delta());
        if let Some(at) = resp.interact_pointer_pos() {
            let n = display_well::to_norm(plot, at);
            new = Some(pad_drag(norm, n, [d.x / plot.width(), -d.y / plot.height()], fine));
        }
    }
    if resp.double_clicked() {
        new = Some([tp.to_normalized(tp.default), mp.to_normalized(mp.default)]);
    }
    if let Some([x, y]) = new {
        super::set_together(app, dev, &[("timbre", super::value_at(tp, x)), ("morph", super::value_at(mp, y))], "macro_osc_pad");
    }
    let shown = new.unwrap_or(norm);
    let c = display_well::to_screen(plot, shown);
    // Route trails: the reach of the rows that modulate Timbre (across) and Morph (up).
    let mut trails = Vec::new();
    for (d, axis) in [(dst::TIMBRE, 0usize), (dst::MORPH, 1)] {
        if let Some(depth) = super::ring_depth(app, dev, d) {
            let mut end = shown;
            end[axis] = (end[axis] + depth).clamp(0.0, 1.0);
            let e = display_well::to_screen(plot, end);
            p.extend(Shape::dashed_line(&[c, e], Stroke::new(3.0, t.modulation), 1.0, 5.0));
            if axis == 0 {
                p.circle_stroke(e, 4.0, Stroke::new(1.5, t.modulation));
            }
            trails.push([c.x, c.y, e.x, e.y]);
        }
    }
    p.circle_filled(c, HALO, t.signal.gamma_multiply(alpha::PAD_HALO));
    p.circle(c, DOT, t.signal, Stroke::new(2.0, t.text));
    let lbl = |text: &str, anchor: Align2, at: Pos2| widgets::role_text(&p, &tokens::READOUT_DISPLAY, text, t.text_faint, anchor, at);
    lbl("TIMBRE →", Align2::LEFT_BOTTOM, pos2(rect.min.x + 8.0, rect.max.y - 6.0));
    lbl("↑ MORPH", Align2::LEFT_TOP, pos2(rect.min.x + 8.0, rect.min.y + 6.0));
    let ry = rect.max.y + 6.0 + PAD_READOUT / 2.0;
    out.hero.timbre_text = tp.format(super::value_at(tp, shown[0]));
    out.hero.morph_text = mp.format(super::value_at(mp, shown[1]));
    for (letter, text, anchor, x) in
        [("T", &out.hero.timbre_text, Align2::LEFT_CENTER, rect.min.x), ("M", &out.hero.morph_text, Align2::RIGHT_CENTER, rect.max.x)]
    {
        let w = widgets::role_size(&p, &tokens::VALUE, text).x;
        let lw = widgets::role_size(&p, &tokens::VALUE, letter).x + 4.0;
        let x0 = if anchor == Align2::LEFT_CENTER { x } else { x - w - lw };
        widgets::role_text(&p, &tokens::VALUE, letter, t.text_faint, Align2::LEFT_CENTER, pos2(x0, ry));
        widgets::role_text(&p, &tokens::VALUE, text, t.text, Align2::LEFT_CENTER, pos2(x0 + lw, ry));
    }
    out.hero.pad = r4(rect);
    out.hero.pad_point = [c.x, c.y];
    out.hero.pad_trails = trails;
}

/// The model picker.
#[allow(clippy::too_many_arguments)]
fn picker(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, model: usize, lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let focus_id = egui::Id::new(("macro_osc_picker", dev.uid()));
    let area = ui.interact(rect, focus_id, Sense::hover());
    let mut new = None;
    // The picker holds the arrow keys from a click on a model until a press lands elsewhere.
    let pressed_elsewhere = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p)));
    if pressed_elsewhere && app.builtin_editor.macro_osc.picker == Some(dev.uid()) {
        app.builtin_editor.macro_osc.picker = None;
    }
    let focused = app.builtin_editor.macro_osc.picker == Some(dev.uid());
    if focused && !ui.ctx().egui_wants_keyboard_input() {
        for key in [egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::ArrowUp, egui::Key::ArrowDown] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
                && let Some(m) = model_step(new.unwrap_or(model), key)
            {
                new = Some(m);
            }
        }
    }
    let p = ui.painter().clone();
    let grid_x = rect.min.x + GROUP_W + 10.0;
    let col_w = (rect.max.x - grid_x - MODEL_GAP * (PER_ROW - 1) as f32) / PER_ROW as f32;
    for (g, (name, first, rows)) in [("Pitched", 0usize, 2usize), ("Noise · Perc", NOISE_FROM, 1)].into_iter().enumerate() {
        let gy = rect.min.y + g as f32 * (2.0 * MODEL_H + MODEL_GAP + GROUP_GAP);
        let lr = widgets::role_text(&p, &tokens::LABEL_SECTION, name, t.text_faint, Align2::LEFT_CENTER, pos2(rect.min.x, gy + MODEL_H / 2.0));
        out.hero.groups.push((name.to_string(), r4(lr)));
        for i in 0..rows * PER_ROW {
            let m = first + i;
            if m >= MODELS.len() {
                break;
            }
            let (row, col) = (i / PER_ROW, i % PER_ROW);
            let r =
                Rect::from_min_size(pos2(grid_x + col as f32 * (col_w + MODEL_GAP), gy + row as f32 * (MODEL_H + MODEL_GAP)), vec2(col_w, MODEL_H));
            let resp = ui.interact(r, focus_id.with(m), Sense::click());
            crate::help::hover(ui, &resp, crate::help::Id::MacroOscModel);
            if resp.clicked() {
                new = Some(m);
                app.builtin_editor.macro_osc.picker = Some(dev.uid());
            }
            let sel = new.unwrap_or(model) == m;
            let (fill, border, ink, num) = if sel {
                (t.toggle_option_fill, t.signal, t.text, t.signal)
            } else {
                (if resp.hovered() { t.button_hover } else { t.surface_raised }, t.line, t.text_muted, t.text_faint)
            };
            p.rect(r, radius::CONTROL, fill, Stroke::new(1.0, border), StrokeKind::Inside);
            let clip = p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect()));
            let number = format!("{:02}", m + 1);
            let nw = widgets::role_size(&clip, &tokens::TAG, &number).x;
            let name_w = widgets::role_size(&clip, &tokens::chrome::BUTTON_FONT, MODELS[m]).x;
            let x0 = (r.center().x - (nw + 6.0 + name_w) / 2.0).max(r.min.x + 6.0);
            widgets::role_text(&clip, &tokens::TAG, &number, num, Align2::LEFT_CENTER, pos2(x0, r.center().y));
            widgets::role_text(&clip, &tokens::chrome::BUTTON_FONT, MODELS[m], ink, Align2::LEFT_CENTER, pos2(x0 + nw + 6.0, r.center().y));
            out.hero.picker.push((m, r4(r), sel));
        }
    }
    if app.builtin_editor.macro_osc.picker == Some(dev.uid()) {
        // The keyboard focus ring (Application chrome, Buttons): the arrows move the model.
        use crate::tokens::chrome as c;
        let ring = rect.expand(c::FOCUS_RING_OFFSET + c::FOCUS_RING_WIDTH / 2.0);
        p.rect_stroke(ring, c::RADIUS_CONTROL + c::FOCUS_RING_OFFSET, Stroke::new(c::FOCUS_RING_WIDTH, t.focus_ring), StrokeKind::Middle);
    }
    finish(app, ui, dev, "model", &area, new.map(|m| m as f32), lay);
}
