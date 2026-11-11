//! A looping slice on the bench (README, Sampler views, Per-slice loop; the board
//! `SamplerSlices`): a `sampler.bench.sliceLoop.band` (4 px) `loop` band right under the flag
//! strip across the slice, a `loop` wash at `washAlpha` (7 percent) over its body, `↻` (forward) or
//! `⇄` (ping-pong) after its flag number, and its crossfade as a `loop`-hue hatch at its end (the
//! hatch's lines, pitch and stroke in the loop hue at `crossfadeHatchAlpha`).
//!
//! The bench draws what plays: the loop mode and crossfade playback uses (`loop_used`,
//! `crossfade_used`), as the state query reports them. The stored values are the Playback
//! section's ([`super::loop_row`]); where a slice edit left them over the limit, the two differ
//! (checked when set, clamped when used). The loop spans the whole slice and has no handles of its
//! own: loop points inside a slice are deferred (ADR 0041 amendment 2026-11-08, items 1 and 12).

use super::state::Loop;
use crate::theme::TOKENS;
use crate::tokens::{hatch as h, sampler as tk};
use crate::widgets::r4;
use egui::{Color32, Rect, Stroke, pos2};
use transit_model::sampler_patch::LoopMode;

/// A looping slice as drawn: its band, wash, flag glyph and (forward, with a crossfade in use)
/// the crossfade's hatch.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopMark {
    pub index: usize,
    pub mode: LoopMode,
    pub band: [f32; 4],
    pub wash: [f32; 4],
    pub color: Color32,
    pub wash_color: Color32,
    pub glyph: &'static str,
    /// The crossfade's region and how many hatch lines crossed it.
    pub hatch: Option<([f32; 4], usize)>,
}

/// The loop mode and the crossfade (source frames) slice `index` of the showing set plays with,
/// when it loops.
pub fn used(l: &Loop, index: usize) -> Option<(LoopMode, u64)> {
    l.zone_of(index).filter(|z| z.loop_used != LoopMode::Off).map(|z| (z.loop_used, z.crossfade_used))
}

/// The glyph after a looping slice's flag number.
pub fn glyph(mode: LoopMode) -> &'static str {
    if mode == LoopMode::PingPong { tk::SLICE_LOOP_GLYPH_PING_PONG } else { tk::SLICE_LOOP_GLYPH_FORWARD }
}

/// The flag's text: `S 1` or the slice's number, then the glyph when it loops.
pub fn flag_text(index: usize, mode: Option<LoopMode>) -> String {
    let n = if index == 0 { "S 1".to_string() } else { (index + 1).to_string() };
    match mode {
        Some(m) => format!("{n} {}", glyph(m)),
        None => n,
    }
}

/// Paints slice `index`'s loop between `a` and `b` (its start and end x) under `strip`, over
/// `body`; `xf_left` is the x where its crossfade in use begins.
#[allow(clippy::too_many_arguments)]
pub fn paint(wp: &egui::Painter, index: usize, mode: LoopMode, a: f32, b: f32, xf_left: Option<f32>, strip: Rect, body: Rect) -> LoopMark {
    let color = TOKENS.loop_region;
    let wash_color = color.gamma_multiply(tk::SLICE_LOOP_WASH_ALPHA);
    let wash = Rect::from_min_max(pos2(a, body.min.y), pos2(b, body.max.y));
    wp.rect_filled(wash, 0.0, wash_color);
    let band = Rect::from_min_max(pos2(a, strip.max.y), pos2(b, strip.max.y + tk::SLICE_LOOP_BAND));
    wp.rect_filled(band, 0.0, color);
    let hatch = xf_left.filter(|x| *x < b).map(|x| {
        let r = Rect::from_min_max(pos2(x.max(a), band.max.y), pos2(b, body.max.y));
        (r4(r), lines(wp, r, color.gamma_multiply(tk::SLICE_LOOP_HATCH_ALPHA)))
    });
    LoopMark { index, mode, band: r4(band), wash: r4(wash), color, wash_color, glyph: glyph(mode), hatch }
}

/// The hatch's lines (README, Hatch: 1.5 px at −45 degrees, 8 px apart, anchored to the window) in
/// `color`, clipped to `rect`; returns how many crossed it. A crossfade narrower than the hatch's
/// least size still draws: it is a value, not an inert region.
fn lines(wp: &egui::Painter, rect: Rect, color: Color32) -> usize {
    let step = crate::widgets::hatch::step();
    let p = wp.with_clip_rect(rect.intersect(wp.clip_rect()));
    let stroke = Stroke::new(h::STROKE, color);
    let mut n = 0;
    let mut k = ((rect.min.x + rect.min.y) / step).floor();
    while k * step < rect.max.x + rect.max.y + step {
        let c = k * step;
        let x0 = rect.min.x.max(c - rect.max.y);
        let x1 = rect.max.x.min(c - rect.min.y);
        if x1 > x0 {
            p.line_segment([pos2(x0, c - x0), pos2(x1, c - x1)], stroke);
            n += 1;
        }
        k += 1.0;
    }
    n
}
