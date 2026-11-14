//! Cached live-take peaks use the same bounded body as recorded clips.

use super::*;

/// Where the audio take being recorded shows, as it will be placed (#634): the timeline position
/// of its first sample and the span it covers, which in a cycle recording is only the lap under
/// way, restarting at the loop start at each wrap. Measured on the pass's unwrapped clock once it
/// has latched (the playhead wraps back into the loop), from the playhead before.
pub(super) fn span(app: &StudioApp) -> Option<(Samples, transit_time::Range)> {
    let pass = app.take_plan.as_ref().and_then(|p| p.pass);
    let wrap = app.take_cycle.filter(|_| pass.is_none());
    let (start, len) = app.live_clock().map_or((app.record_start, app.position() - app.record_start), |c| (c.start, c.reach - c.start));
    transit_engine::io::live_span(start, pass, wrap, app.punch, len.max(0))
}

pub(super) fn draw(app: &StudioApp, ui: &Ui, s: &Session, tl: Rect, track: TrackId, rect: Rect, origin: Samples) {
    let Some(recorder) = &app.recorder else { return };
    let Some(blocks) = recorder.live_take(track) else { return };
    draw_peaks(ui.painter(), s, tl, rect, origin, recorder.sample_rate, &blocks);
}

pub(super) fn draw_peaks(p: &egui::Painter, s: &Session, tl: Rect, rect: Rect, origin: Samples, rate: u32, blocks: &[Vec<(f32, f32)>]) {
    let strip_end = (rect.min.y + CLIP_NAME_H).min(rect.max.y);
    let body = Rect::from_min_max(pos2(rect.min.x, strip_end), rect.max).shrink2(vec2(0.0, 2.0));
    if rect.height() < 22.0 || body.height() <= 4.0 {
        return;
    }
    let block_len = transit_playback::record::LIVE_BLOCK as f64 * f64::from(s.sample_rate.hz()) / f64::from(rate.max(1));
    crate::waveform::draw_live(&p.with_clip_rect(body.intersect(tl)), s, tl, origin as f64, block_len, blocks, body, theme::text_on(TOKENS.record));
}

#[cfg(test)]
#[path = "recording_strip_tests.rs"]
mod tests;
