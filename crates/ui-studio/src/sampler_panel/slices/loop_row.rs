//! The Slices view's sections row and its per-slice loop (README, Sampler views: Shared sections
//! and Per-slice loop; ADR 0041 amendment 2026-11-08, items 1 and 2; the board `SamplerSlices`).
//!
//! The row is the Pads view's Playback and Amp Envelope ([`super::super::zone`]) editing the
//! selected slice's zone: the zone its pad plays, else the first zone bound to it. Playback adds a
//! row under its knobs:
//!
//! - **Loop**, a segmented `Off · Loop · Ping-pong` showing the zone's stored loop mode; a chosen
//!   loop is filled `loop` with an `ink.dark` label. Loop and Ping-pong send the loop mode and Gate
//!   in one edit (a looping slice plays as Gate); Ping-pong also sends crossfade 0, so its undo
//!   restores the crossfade. Off sends the loop mode only.
//! - **Crossfade**, a value field beside it, live on a forward loop only (`xf —` in `ink.arrow`
//!   otherwise): the stored crossfade in milliseconds with one decimal at the zone source's native
//!   rate (the model's conversion, `X = floor(m * r / 1000 + 1/2)` frames, shown as `X * 1000 / r`),
//!   the frame count in its tooltip. Typing takes a bare number or `ms` as milliseconds and a
//!   trailing `s` as seconds; Enter or a click elsewhere commits, Escape cancels.
//! - **A warning line** (triangle, no color) when the crossfade playback uses is below the stored
//!   one on a loop in use: `Slice 7 is 9.4 ms long now: 4.7 ms of crossfade is used.`
//! - **An error banner** with a refusal in the engine's words (`Slice 4 is one frame long, too
//!   short to loop.`), or the One-shot press's sentence.
//!
//! While the zone's stored loop is on, the trigger's One-shot segment is ghosted to `ink.arrow`
//! with its reason as a tooltip, and pressing it sends nothing and shows the reason as the error
//! banner (a one-shot cannot loop; ADR 0041). Without a zone to edit the row is at the section's
//! disabled alpha and takes no input, as the sections are.

use super::edit;
use super::feedback::{self, CardRecord};
use super::state::{Loop, Zone};
use super::{SlicesView, bench};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::{Dev, child};
use crate::theme::TOKENS;
use crate::tokens::{self, chrome, radius};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use transit_model::sampler_loops::{crossfade_frames, crossfade_ms_text, tenths_of_ms};
use transit_model::sampler_patch::{LoopMode, PlayMode};

use super::super::{SamplerLayout, zone as sections};

/// The engine's refusal of a loop on a one-shot (ADR 0041), also the One-shot segment's tooltip
/// while the loop is on.
pub const ONE_SHOT_LOOP: &str = "A one-shot cannot loop: choose Gate, or turn the loop off.";
pub const LOOPS: [&str; 3] = ["Off", "Loop", "Ping-pong"];
pub const XF_NONE: &str = "xf —";
/// The crossfade field's tooltip when it takes no input.
pub const XF_FORWARD_ONLY: &str = "Crossfade: forward loops only";
/// The widest text the crossfade field holds at its natural width.
const XF_WIDEST: &str = "xf 000.0 ms";

/// What the row keeps between frames: the crossfade being typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayState {
    pub typing: Option<String>,
}

/// The loop row as drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoopRowLayout {
    /// The zone it edits (1-based), when there is one.
    pub zone: Option<u16>,
    pub track: [f32; 4],
    pub segments: Vec<[f32; 4]>,
    pub selected: usize,
    pub fills: Vec<Color32>,
    pub inks: Vec<Color32>,
    pub crossfade: [f32; 4],
    pub crossfade_text: String,
    pub crossfade_ink: Color32,
    pub crossfade_tip: String,
    pub crossfade_live: bool,
    pub warning: Option<(String, [f32; 4])>,
    pub error: Option<CardRecord>,
    /// The One-shot segment while the loop is on: its rect and ink.
    pub one_shot: Option<([f32; 4], Color32)>,
    /// The row's opacity (the section's disabled alpha without a zone).
    pub opacity: f32,
}

/// The segment of a stored loop mode.
fn index(m: LoopMode) -> usize {
    match m {
        LoopMode::Off => 0,
        LoopMode::Forward => 1,
        LoopMode::PingPong => 2,
    }
}

/// The warning line's text when the crossfade playback uses is below the stored one on a loop in
/// use (a slice edit shortened the slice after the crossfade was set).
pub fn warning(z: &Zone, number: usize) -> Option<String> {
    if z.loop_used != LoopMode::Forward || z.crossfade_used >= z.crossfade {
        return None;
    }
    let len = z.region.map_or(0, |r| r.loop_end.saturating_sub(r.loop_start));
    Some(format!(
        "Slice {number} is {} ms long now: {} ms of crossfade is used.",
        crossfade_ms_text(len, z.rate),
        crossfade_ms_text(z.crossfade_used, z.rate)
    ))
}

/// A typed crossfade in tenths of a millisecond: a bare number or `ms` is milliseconds, a trailing
/// `s` seconds; a leading `xf` (the field's own text) is ignored. `None` when it does not read.
pub fn typed_tenths(text: &str) -> Option<u64> {
    let t = text.trim().to_lowercase();
    let t = t.strip_prefix("xf").unwrap_or(&t).trim();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let mut ms: f64 = num.parse().ok()?;
    let rest = t[num.len()..].trim();
    match rest {
        "" | "ms" => {}
        "s" => ms *= 1000.0,
        _ => return None,
    }
    tenths_of_ms(ms)
}

/// The Playback panel's inner width at row width `w` (the sections' own split).
fn playback_inner(w: f32) -> f32 {
    let slot = sections::slot_at(w);
    let amp = 4.0 * slot + 3.0 * sections::SLOT_GAP + 2.0 * tokens::layout::PANEL_PADDING[1];
    w - tokens::layout::PANEL_GAP - amp - 2.0 * tokens::layout::PANEL_PADDING[1]
}

/// The rows under the knobs: the loop row, then the warning line and the error banner when shown.
fn extra(ui: &Ui, inner_w: f32, warn: Option<&str>, error: Option<&str>) -> f32 {
    let mut h = sections::ROW_GAP + segmented::HEIGHT;
    if warn.is_some() {
        h += sections::ROW_GAP + crate::chrome::glyph_size(tokens::LABEL_CONTROL.size);
    }
    if let Some(e) = error {
        h += sections::ROW_GAP + feedback::card_height(ui, e, inner_w, true, None);
    }
    h
}

/// The sections row's height before it is first drawn (no warning, no banner).
pub fn height(p: &egui::Painter) -> f32 {
    sections::height(p) + sections::ROW_GAP + segmented::HEIGHT
}

/// Draws the sections row from `at` across `width` for the selected slice's zone; returns its
/// height.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    v: &mut SlicesView,
    l: Option<&Loop>,
    at: egui::Pos2,
    width: f32,
    lay: &mut EditorLayout,
    out: &mut SamplerLayout,
) -> (LoopRowLayout, f32) {
    let mut row = LoopRowLayout { opacity: 1.0, ..LoopRowLayout::default() };
    let Some(patch) = super::super::patch_at(app.engine.session(), dev.key) else { return (row, 0.0) };
    let zs = l.and_then(|l| l.zone_of(v.selected)).cloned();
    let entry = zs.as_ref().and_then(|z| patch.zone(z.zone.saturating_sub(1)).cloned());
    let zs = zs.filter(|_| entry.is_some());
    row.zone = zs.as_ref().map(|z| z.zone);
    let number = zs.as_ref().and_then(|z| z.slice.as_ref().and_then(|b| b.slice)).unwrap_or(v.selected + 1);
    let warn = zs.as_ref().and_then(|z| warning(z, number));
    let pads_note = super::super::view(app, dev.uid()).note.clone();
    let error = v.feedback.play_error.clone().or(pads_note);
    let inner_w = playback_inner(width);
    let p = ui.painter().clone();
    let h = sections::height(&p) + extra(ui, inner_w, warn.as_deref(), error.as_deref());
    let pad = entry.map(|z| super::super::PadData { entry: Default::default(), note: 0, zone: Some(z), name: String::new() });
    let rect = Rect::from_min_size(at, vec2(width, h));
    sections::show(app, ui, dev, &patch, pad.as_ref(), rect, lay, out);
    let Some(play) = out.panel("playback") else { return (row, h) };
    let inner = super::super::grid_rect(play).shrink2(vec2(tokens::layout::PANEL_PADDING[1], tokens::layout::PANEL_PADDING[0]));
    let mut body = child(ui, rect);
    if zs.is_none() {
        row.opacity = tokens::alpha::DISABLED_SECTION;
        body.multiply_opacity(tokens::alpha::DISABLED_SECTION);
        body.disable();
    }
    let label_h = super::super::line_h(&p, &tokens::LABEL_SECTION);
    let mut y = inner.min.y + label_h + sections::LABEL_GAP + widgets::knob::cell(widgets::knob::Size::M).y + sections::ROW_GAP;
    let uid = dev.uid();
    let stored = zs.as_ref().map_or(LoopMode::Off, |z| z.loop_mode);
    if let Some(i) = segments(&mut body, uid, pos2(inner.min.x, y), index(stored), &mut row)
        && let Some(z) = &zs
    {
        let fields = match i {
            0 => json!({"region": {"loop_mode": "off"}}),
            1 => json!({"region": {"loop_mode": "forward"}, "playback": {"mode": "gate"}}),
            _ => json!({"region": {"loop_mode": "ping_pong", "crossfade": 0}, "playback": {"mode": "gate"}}),
        };
        edit::zone(app, dev, v, z.zone, fields);
    }
    crossfade(app, &mut body, dev, v, zs.as_ref(), pos2(row.track[2] + tokens::SPACE[1], y), &mut row);
    y += segmented::HEIGHT;
    if let Some(w) = warn {
        y += sections::ROW_GAP;
        let font = tokens::LABEL_CONTROL.font_id(ui.ctx());
        let gh = crate::chrome::glyph_size(font.size);
        let r = crate::chrome::status_text(body.painter(), pos2(inner.min.x, y + gh / 2.0), &w, font, crate::chrome::Severity::Warning);
        row.warning = Some((w, r4(r)));
        y += gh;
    }
    if let Some(e) = &error {
        y += sections::ROW_GAP;
        let r = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 0.0));
        row.error = Some(feedback::card(&mut body, egui::Id::new(("sampler_slice_play_error", uid)), r, e, true, None).0);
    }
    if let Some(z) = zs.as_ref().filter(|z| z.loop_mode != LoopMode::Off) {
        one_shot(&mut body, uid, v, z, out, &mut row);
    }
    (row, h)
}

/// The loop's segmented control at `at` (the Segmented control's track and segments at their text;
/// a chosen loop filled `loop` with an `ink.dark` label, Off chosen as any segment). Returns the
/// segment clicked when it is not the chosen one.
fn segments(ui: &mut Ui, uid: u64, at: egui::Pos2, selected: usize, out: &mut LoopRowLayout) -> Option<usize> {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let track = Rect::from_min_size(at, vec2(bench::segmented_w(&p, &LOOPS), segmented::HEIGHT));
    p.rect(track, radius::DISPLAY, t.surface_inset, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = track.shrink(segmented::PADDING);
    let mut x = inner.min.x;
    let mut picked = None;
    for (i, o) in LOOPS.iter().enumerate() {
        let w = widgets::role_size(&p, &tokens::VALUE, o).x + 2.0 * segmented::SEGMENT_PAD;
        let seg = Rect::from_min_size(pos2(x, inner.min.y), vec2(w, inner.height()));
        x += w;
        if ui.interact(seg, egui::Id::new(("sampler_slice_loop_mode", uid, i)), Sense::click()).clicked() && i != selected {
            picked = Some(i);
        }
        let (fill, ink) = match (i == selected, i) {
            (true, 0) => (t.surface_selected, t.text),
            (true, _) => (t.loop_region, t.ink_dark),
            (false, _) => (Color32::TRANSPARENT, t.text_faint),
        };
        p.rect_filled(seg, radius::SEGMENT, fill);
        widgets::role_text(&p, &tokens::VALUE, o, ink, Align2::CENTER_CENTER, seg.center());
        out.segments.push(r4(seg));
        out.fills.push(fill);
        out.inks.push(ink);
    }
    out.track = r4(track);
    out.selected = selected;
    picked
}

/// The crossfade field at `at`: live on a stored forward loop, `xf —` in `ink.arrow` otherwise.
fn crossfade(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, z: Option<&Zone>, at: egui::Pos2, out: &mut LoopRowLayout) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let live = z.filter(|z| z.loop_mode == LoopMode::Forward);
    let shown = match live {
        Some(z) => v.play.typing.clone().unwrap_or_else(|| format!("xf {} ms", crossfade_ms_text(z.crossfade, z.rate))),
        None => XF_NONE.to_string(),
    };
    let text_w = widgets::role_size(&p, &tokens::VALUE, XF_WIDEST).x.max(widgets::role_size(&p, &tokens::VALUE, &shown).x);
    let r = Rect::from_min_size(at, vec2((text_w + 2.0 * chrome::FIELD_PADDING[1]).ceil(), segmented::HEIGHT));
    out.crossfade = r4(r);
    out.crossfade_live = live.is_some();
    let Some(z) = live else {
        p.rect(r, chrome::RADIUS_CONTROL, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line), StrokeKind::Inside);
        widgets::role_text(&p, &tokens::VALUE, &shown, t.ink_arrow, Align2::RIGHT_CENTER, pos2(r.max.x - chrome::FIELD_PADDING[1], r.center().y));
        ui.interact(r, egui::Id::new(("sampler_slice_crossfade_tip", dev.uid())), Sense::hover()).on_hover_text(XF_FORWARD_ONLY);
        (out.crossfade_text, out.crossfade_ink, out.crossfade_tip) = (shown, t.ink_arrow, XF_FORWARD_ONLY.to_string());
        return;
    };
    let tip = format!("{} frames at the source's {} Hz · forward loops only", z.crossfade, z.rate);
    let mut text = shown.clone();
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            let edit = egui::TextEdit::singleline(&mut text)
                .font(tokens::VALUE.font_id(ui.ctx()))
                .horizontal_align(egui::Align::Max)
                .desired_width(r.width())
                .min_size(r.size())
                .vertical_align(egui::Align::Center)
                .id(egui::Id::new(("sampler_slice_crossfade", dev.uid())));
            crate::chrome::text_field_enabled(ui, true, edit)
        })
        .inner
        .on_hover_text(&tip);
    (out.crossfade_text, out.crossfade_ink, out.crossfade_tip) = (text.clone(), t.text, tip);
    if resp.has_focus() || resp.changed() {
        v.play.typing = Some(text.clone());
    }
    if resp.lost_focus() {
        v.play.typing = None;
        // Escape cancels; a field left without a change sends nothing (its text is the stored
        // value rounded to a tenth, which need not convert back to the same frames).
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) || text.trim() == format!("xf {} ms", crossfade_ms_text(z.crossfade, z.rate)) {
            return;
        }
        if let Some(frames) = typed_tenths(&text).map(|tenths| crossfade_frames(tenths, z.rate)).filter(|f| *f != z.crossfade) {
            edit::zone(app, dev, v, z.zone, json!({"region": {"crossfade": frames}}));
        }
    }
}

/// While the loop is on: the trigger's One-shot segment ghosted to `ink.arrow` with its reason as
/// a tooltip, over the sections' own segment so that a press sends nothing and shows the reason.
fn one_shot(ui: &mut Ui, uid: u64, v: &mut SlicesView, z: &Zone, out: &SamplerLayout, row: &mut LoopRowLayout) {
    let t = &TOKENS;
    let Some(mode) = &out.mode else { return };
    let Some(seg) = mode.segments.first().map(|s| super::super::grid_rect(*s)) else { return };
    let p = ui.painter().clone();
    p.rect_filled(seg, radius::SEGMENT, if z.mode == PlayMode::OneShot { t.surface_selected } else { t.surface_inset });
    widgets::role_text(&p, &tokens::VALUE, sections::MODES[0], t.ink_arrow, Align2::CENTER_CENTER, seg.center());
    let resp = ui.interact(seg, egui::Id::new(("sampler_slice_one_shot", uid)), Sense::click()).on_hover_text(ONE_SHOT_LOOP);
    if resp.clicked() {
        v.feedback.play_error = Some(ONE_SHOT_LOOP.to_string());
    }
    row.one_shot = Some((r4(seg), t.ink_arrow));
}
