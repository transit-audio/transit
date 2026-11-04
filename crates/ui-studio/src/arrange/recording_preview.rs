//! Cached live-take peaks use the same bounded body as recorded clips.

use super::*;

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
