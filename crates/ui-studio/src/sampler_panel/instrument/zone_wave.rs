//! The zone waveform (README, Sampler views, Instrument view, "Zone waveform"; the board
//! `SamplerInstrument`): a display well over the whole source. The region between start and end is
//! `signal` at 55 percent, the rest `ink.ghost`; start and end are 2 px `signal` lines with `S` and
//! `E` flag tabs at the foot, dragged to move (one `mix.sampler_zone_edit` on release, Escape
//! sends nothing). The loop playback uses takes the `loop` hue: a brace across the top with its
//! mode and crossfade (`loop · fwd · xf 12.0 ms`), a 10 percent band down the well and the crossfade
//! as a hatch at the loop's end. A slice-bound zone shows its slice's derived region and loop, its
//! handles locked (the key map's lock beside each flag); they do not drag.
//!
//! Positions are the source's own frames at its native rate; the waveform comes from the source's
//! existing peaks ([`super::super::slices::wave`]).

use super::color;
use super::state::Span;
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, radius, sampler as tk};
use crate::widgets::{self, r4};
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_model::sampler_patch::LoopMode;

const FLAG_LETTER: tokens::TypeRole =
    tokens::TypeRole { family: tokens::Family::Sans, weight: tk::TYPE[4].2 as u16, size: tk::TYPE[4].3, tracking: 0.0, uppercase: false, ink: None };
const BRACE_LABEL: tokens::TypeRole =
    tokens::TypeRole { family: tokens::Family::Mono, weight: tk::TYPE[5].2 as u16, size: tk::TYPE[5].3, tracking: 0.0, uppercase: false, ink: None };

/// A handle of the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

/// A handle being dragged: the zone (from 1), the handle, the frame under the pointer and whether
/// Escape canceled it.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveDrag {
    pub zone: u16,
    pub handle: Handle,
    pub frame: u64,
    pub cancelled: bool,
}

/// What the well shows beside the zone's span.
pub struct WaveInputs<'a> {
    pub zone: u16,
    pub span: Span,
    /// The source's length in frames.
    pub frames: u64,
    pub rate: u32,
    pub loop_used: LoopMode,
    pub crossfade_used: u64,
    /// A slice-bound zone: its handles do not drag.
    pub locked: bool,
    pub audio: Option<&'a transit_model::SourceAudio>,
    pub handle_tip: &'a str,
}

/// The well as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WaveLayout {
    pub well: [f32; 4],
    /// The loop brace and its label, the band under it, the crossfade hatch.
    pub brace: Option<([f32; 4], String)>,
    pub band: Option<[f32; 4]>,
    pub hatch: Option<[f32; 4]>,
    pub start_line: [f32; 4],
    pub start_flag: [f32; 4],
    pub end_line: [f32; 4],
    pub end_flag: [f32; 4],
    /// The locks beside the flags (a slice-bound zone).
    pub locks: Vec<[f32; 4]>,
    /// `0.004 – 3.410 s`.
    pub readout: (String, [f32; 4]),
    /// Waveform columns drawn and how many inside the region.
    pub columns: usize,
    pub inside: usize,
}

/// The handles' hit areas: each line with a band of [`super::key_map::EDGE`] either side, and its
/// flag.
pub struct Grabs {
    pub s_line: Rect,
    pub s_flag: Rect,
    pub e_line: Rect,
    pub e_flag: Rect,
}

impl Grabs {
    /// The handle a press at `at` holds. The flags, which are what shows, come first, E's before
    /// S's (E draws over S where they meet); then the bands around the lines, the nearer line
    /// where both reach. One answer per point, so neither handle takes the other's presses.
    pub fn handle_at(&self, at: egui::Pos2) -> Option<Handle> {
        if self.e_flag.contains(at) {
            return Some(Handle::End);
        }
        if self.s_flag.contains(at) {
            return Some(Handle::Start);
        }
        let band = |line: Rect| line.expand2(vec2(super::key_map::EDGE, 0.0));
        let dist = |line: Rect| (at.x - line.center().x).abs();
        match (band(self.s_line).contains(at), band(self.e_line).contains(at)) {
            (true, true) if dist(self.e_line) < dist(self.s_line) => Some(Handle::End),
            (true, _) => Some(Handle::Start),
            (false, true) => Some(Handle::End),
            (false, false) => None,
        }
    }
}

fn seconds(frame: u64, rate: u32) -> f64 {
    frame as f64 / f64::from(rate.max(1))
}

/// The brace's label: the mode in use and its crossfade (milliseconds at the source's rate).
pub fn brace_label(mode: LoopMode, crossfade: u64, rate: u32) -> String {
    match mode {
        LoopMode::Forward if crossfade > 0 => format!("loop · fwd · xf {} ms", transit_model::sampler_loops::crossfade_ms_text(crossfade, rate)),
        LoopMode::Forward => "loop · fwd".to_string(),
        _ => "loop · ping-pong".to_string(),
    }
}

/// Draws the well into `well` for instance `uid`; returns what it drew and a handle moved this frame.
pub(crate) fn show(ui: &mut Ui, uid: u64, w: &WaveInputs, drag: &mut Option<WaveDrag>, well: Rect) -> (WaveLayout, Option<(Handle, u64)>) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mut out = WaveLayout { well: r4(well), ..WaveLayout::default() };
    p.rect(well, radius::DISPLAY, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_inset), StrokeKind::Inside);
    let area = well.shrink(chrome::FIELD_EDGE_WIDTH);
    let wp = p.with_clip_rect(area);
    let len = w.frames.max(w.span.end).max(1);
    let x_of = |f: u64| area.min.x + (f as f64 / len as f64) as f32 * area.width();
    let frame_at = |x: f32| ((((x - area.min.x) / area.width()).clamp(0.0, 1.0)) as f64 * len as f64).round() as u64;
    let body = Rect::from_min_max(pos2(area.min.x, area.min.y + tk::WAVE_LOOP_BRACE), area.max);
    let live = drag.as_ref().filter(|d| !d.cancelled && d.zone == w.zone);
    let start = live.filter(|d| d.handle == Handle::Start).map_or(w.span.start, |d| d.frame);
    let end = live.filter(|d| d.handle == Handle::End).map_or(w.span.end, |d| d.frame);
    // The loop: brace, band and crossfade hatch.
    if w.loop_used != LoopMode::Off {
        let hue = t.loop_region;
        let (x0, x1) = (x_of(w.span.loop_start), x_of(w.span.loop_end));
        let brace = Rect::from_min_max(pos2(x0, area.min.y), pos2(x1, body.min.y));
        let br = tk::BRACE_RADIUS as u8;
        wp.rect_filled(brace, CornerRadius { nw: 0, ne: 0, sw: br, se: br }, hue);
        let text = brace_label(w.loop_used, w.crossfade_used, w.rate);
        wp.with_clip_rect(brace.intersect(area)).text(
            brace.center(),
            Align2::CENTER_CENTER,
            &text,
            BRACE_LABEL.font_id(ui.ctx()),
            color(tk::BRACE_INK),
        );
        out.brace = Some((r4(brace), text));
        let band = Rect::from_min_max(pos2(x0, body.min.y), pos2(x1, body.max.y));
        wp.rect_filled(band, 0.0, hue.gamma_multiply(tk::WAVE_LOOP_BAND_ALPHA));
        out.band = Some(r4(band));
        if w.crossfade_used > 0 {
            let hx = x_of(w.span.loop_end.saturating_sub(w.crossfade_used));
            let hatch = Rect::from_min_max(pos2(hx, body.min.y), pos2(x1, body.max.y));
            let hp = wp.with_clip_rect(hatch.intersect(area));
            let stroke = Stroke::new(tk::HATCH_STROKE, hue.gamma_multiply(tk::WAVE_CROSSFADE_HATCH_ALPHA));
            let mut x = hatch.min.x - hatch.height();
            while x < hatch.max.x {
                hp.line_segment([pos2(x, hatch.max.y), pos2(x + hatch.height(), hatch.min.y)], stroke);
                x += tk::HATCH_SPACING;
            }
            out.hatch = Some(r4(hatch));
        }
    }
    // The waveform: one column per physical pixel, the region in `signal`.
    let ppp = ui.ctx().pixels_per_point();
    let cols = (area.width() * ppp).floor().max(1.0) as usize;
    if let Some(spans) = super::super::slices::wave::columns(w.audio, 0.0, len as f64, cols) {
        let mid = body.center().y;
        let half = body.height() / 2.0;
        let inside_ink = color(tk::WAVE_REGION).gamma_multiply(tk::WAVE_REGION_ALPHA);
        let outside_ink = color(tk::WAVE_OUTSIDE);
        for (c, s) in spans.iter().enumerate() {
            let Some((lo, hi)) = s else { continue };
            let x = area.min.x + (c as f32 + 0.5) / ppp;
            let f = frame_at(x);
            let inside = f >= start && f < end;
            out.inside += usize::from(inside);
            out.columns += 1;
            let (y0, y1) = (mid - hi.clamp(-1.0, 1.0) * half, mid - lo.clamp(-1.0, 1.0) * half);
            wp.line_segment([pos2(x, y0), pos2(x, y1.max(y0 + 1.0 / ppp))], Stroke::new(1.0 / ppp, if inside { inside_ink } else { outside_ink }));
        }
    }
    // The handles.
    let hw = tk::WAVE_HANDLE_WIDTH;
    let fh = tk::WAVE_FLAG_HEIGHT;
    let ink = color(tk::WAVE_HANDLE);
    let flag_w = |letter: &str| widgets::role_size(&wp, &FLAG_LETTER, letter).x + 2.0 * tk::ZONE_FLAG_PADDING_X;
    let fr = tk::ZONE_FLAG_RADIUS as u8;
    let sx = x_of(start);
    let s_line = Rect::from_min_max(pos2(sx, body.min.y), pos2(sx + hw, body.max.y));
    let s_flag = Rect::from_min_max(pos2(sx, body.max.y - fh), pos2(sx + flag_w("S"), body.max.y));
    let ex = x_of(end);
    let e_line = Rect::from_min_max(pos2(ex - hw, body.min.y), pos2(ex, body.max.y));
    let e_flag = Rect::from_min_max(pos2(ex - flag_w("E"), body.max.y - fh), pos2(ex, body.max.y));
    for (line, flag, letter, cr) in
        [(s_line, s_flag, "S", CornerRadius { nw: 0, ne: fr, sw: 0, se: 0 }), (e_line, e_flag, "E", CornerRadius { nw: fr, ne: 0, sw: 0, se: 0 })]
    {
        wp.rect_filled(line, 0.0, ink);
        wp.rect_filled(flag, cr, ink);
        widgets::role_text(&wp, &FLAG_LETTER, letter, color(tk::ZONE_FLAG_INK), Align2::CENTER_CENTER, flag.center());
    }
    (out.start_line, out.start_flag, out.end_line, out.end_flag) = (r4(s_line), r4(s_flag), r4(e_line), r4(e_flag));
    if w.locked {
        let s = tk::LOCK_GLYPH_SIZE;
        for at in [pos2(s_flag.max.x + SPACE[0] + s / 2.0, s_flag.center().y), pos2(e_flag.min.x - SPACE[0] - s / 2.0, e_flag.center().y)] {
            crate::icons::paint(&wp, at, crate::icons::Icon::Lock, s, color(tk::LOCK_GLYPH_COLOR));
            out.locks.push(r4(Rect::from_center_size(at, vec2(s, s))));
        }
    }
    let text = format!("{:.3} – {:.3} s", seconds(start, w.rate), seconds(end, w.rate));
    let r = widgets::role_text(
        &wp,
        &super::key_map::AXIS,
        &text,
        t.text_faint,
        Align2::RIGHT_TOP,
        pos2(area.max.x - tk::TAG_INSET, area.min.y + tk::READOUT_TOP),
    );
    out.readout = (text, r4(r));
    // Dragging: one widget over the well, so the two handles' hit areas never overlap and the
    // release is seen wherever it happens. A press holds the handle under it (see `handle_at`).
    let grabs = Grabs { s_line, s_flag, e_line, e_flag };
    let mut moved = None;
    let id = egui::Id::new(("sampler_zone_handles", uid));
    let resp = ui.interact(area, id, if w.locked { Sense::hover() } else { Sense::drag() });
    let over = resp.hover_pos().and_then(|at| grabs.handle_at(at));
    let resp = if over.is_some() { resp.on_hover_text(w.handle_tip) } else { resp };
    if w.locked {
        return (out, None);
    }
    if over.is_some() || drag.as_ref().is_some_and(|d| d.zone == w.zone) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started()
        && let Some(handle) = ui.input(|i| i.pointer.press_origin()).and_then(|at| grabs.handle_at(at))
    {
        let now = if handle == Handle::Start { w.span.start } else { w.span.end };
        *drag = Some(WaveDrag { zone: w.zone, handle, frame: now, cancelled: false });
    }
    if resp.dragged()
        && let (Some(d), Some(at)) = (drag.as_mut(), resp.interact_pointer_pos())
        && d.zone == w.zone
    {
        d.frame = frame_at(at.x);
    }
    if resp.drag_stopped()
        && let Some(d) = drag.take()
        && !d.cancelled
        && d.zone == w.zone
    {
        let now = if d.handle == Handle::Start { w.span.start } else { w.span.end };
        if d.frame != now {
            moved = Some((d.handle, d.frame));
        }
    }
    (out, moved)
}
