//! The bench's playheads (#470, ADR 0041 amendment 2026-11-11): a 1 px `waveform/playhead` line
//! across the waveform at the source frame each sounding voice of the slice loop reads, whatever
//! started it (the bench, Space, a pad, MIDI or the song).
//!
//! The positions are the instrument's own: each block the Sampler publishes every sounding voice's
//! zone slot and source frame through its scope tap (`transit_dsp::VoicePos`,
//! `ScopeHub::voice_positions`), and the view draws exactly what the last snapshot says: no
//! wall-clock extrapolation between snapshots, so the line moves at audio-block steps. A voice
//! counts when its zone plays the loop's source; a voice outside the visible range draws nothing,
//! and the view never scrolls to follow one. A bypassed or inactive Sampler (or an inactive track)
//! shows none, as no bypassed device shows a moving meter. The view asks for a repaint only while
//! a voice of the loop's source sounds.

use super::state::Loop;
use super::viewport::Viewport;
use crate::StudioApp;
use crate::device_editor::Dev;
use egui::{Color32, Rect, pos2};
use transit_dsp::VoicePos;
use transit_model::sampler_loops::effective_region;

/// The source frames the loop's sounding voices read, in the tap's slot order (empty when the
/// Sampler is bypassed or inactive, its track is inactive, it has no tap or no slice loop).
pub(crate) fn sounding(app: &StudioApp, dev: Dev) -> Vec<u64> {
    let s = app.engine.session();
    let off = dev.ins.bypass || !dev.ins.active || s.track(dev.key.0).is_none_or(|t| t.inactive);
    if off {
        return Vec::new();
    }
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let Some(words) = app.scope_hub().and_then(|h| h.voice_positions(dev.key.0, uid)) else { return Vec::new() };
    let voices: Vec<(u16, u64)> = words.iter().filter_map(|w| VoicePos(*w).get()).collect();
    if voices.is_empty() {
        return Vec::new();
    }
    let Some(p) = super::super::patch_at(s, dev.key) else { return Vec::new() };
    let Some(source) = p.slicing.as_ref().map(|l| l.source) else { return Vec::new() };
    let plays_loop = |zone: u16| p.zones.iter().find(|z| z.id == zone).is_some_and(|z| effective_region(z, &p).source == source);
    voices.into_iter().filter(|(zone, _)| plays_loop(*zone)).map(|(_, frame)| frame).collect()
}

/// Draws a line per frame that the view shows, across `body` (the waveform under the flag strip),
/// clipped to `area` (the well's inside); returns each line's rect.
pub(crate) fn paint(p: &egui::Painter, viewport: &Viewport, l: &Loop, area: Rect, body: Rect, frames: &[u64], ink: Color32) -> Vec<[f32; 4]> {
    let (a, b) = viewport.range(l);
    let wp = p.with_clip_rect(area);
    let mut lines = Vec::new();
    for f in frames.iter().map(|f| *f as f64).filter(|f| *f >= a && *f < b) {
        let x = viewport.x_of(l, f, area.min.x, area.width());
        let line = Rect::from_min_max(pos2(x, body.min.y), pos2(x + 1.0, body.max.y));
        wp.rect_filled(line, 0.0, ink);
        lines.push(crate::widgets::r4(line));
    }
    lines
}
