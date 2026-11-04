//! The arrangement's grid lines: one computation of where they fall and one painting of them, in
//! the layer View > Grid Over Clips picks (#302).
//!
//! Ableton Live keeps the grid visible through clips; Logic Pro draws it behind regions and the
//! regions cover it. They disagree, so transit offers both: View > Grid Over Clips, default off.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A bar line; `labeled` when the ruler numbers it (the lane draws it stronger).
    Bar {
        labeled: bool,
    },
    Beat,
    /// A line at the grid step finer than the beat.
    Sub,
}

/// The stroke color of a line of `kind`.
pub(super) fn color(kind: Kind) -> Color32 {
    match kind {
        Kind::Bar { labeled: true } => TOKENS.grid_bar,
        Kind::Bar { labeled: false } | Kind::Beat => TOKENS.grid_beat,
        Kind::Sub => TOKENS.grid_sub,
    }
}

/// Multiplies the alpha of the grid painted over a clip body (the lane segments keep the lane's
/// own contrast). At 1.0 the sixteenth lines are not findable on a bright clip; 3.0 is the
/// maintainer's pick (4.0 reads stronger; both make them findable on the brightest demo clip, #302).
pub(super) const OVERLAY_GAIN: f32 = 3.0;

/// The most alpha a stroke over a clip body may have (a fraction of opaque), after the gain, in
/// both themes: the light theme's bar line would otherwise read as the harsh opaque line again.
pub(super) const OVERLAY_BODY_ALPHA_MAX: f32 = 0.40;

/// The alpha a stroke over a clip body gets for a lane alpha `a`.
pub(super) fn body_alpha(a: u8) -> u8 {
    (f32::from(a) * OVERLAY_GAIN).min(OVERLAY_BODY_ALPHA_MAX * 255.0).round() as u8
}

/// The stroke of a line of `kind` painted above the clips: white (dark theme) or black (light
/// theme) at the alpha that reproduces the lane's token over an empty lane, so a clip body is
/// tinted by the same fraction the lane is, never cut by an opaque line.
pub(super) fn overlay_color(kind: Kind) -> Color32 {
    translucent(color(kind), TOKENS.lane, 1.0)
}

/// `token` over `lane` as an extreme (white or black) at an alpha: the direction is the sign of
/// the luma difference, the alpha is the mean per-channel distance from the lane as a fraction of
/// the mean distance from the lane to that extreme, times `gain` (clamped to opaque).
pub(super) fn translucent(token: Color32, lane: Color32, gain: f32) -> Color32 {
    let ch = |c: Color32| [c.r(), c.g(), c.b()].map(f32::from);
    let (t, l) = (ch(token), ch(lane));
    let toward_white = crate::theme::luminance(token) >= crate::theme::luminance(lane);
    let extreme = if toward_white { 255.0 } else { 0.0 };
    let mean = |f: &dyn Fn(usize) -> f32| (0..3).map(f).sum::<f32>() / 3.0;
    let dist = mean(&|i| (t[i] - l[i]).abs());
    let room = mean(&|i| (extreme - l[i]).abs());
    let a = if room > 0.0 { (dist / room * gain).clamp(0.0, 1.0) } else { 0.0 };
    let a8 = (a * 255.0).round() as u8;
    crate::theme::tint(toward_white, a8)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Line {
    pub x: f32,
    pub kind: Kind,
    pub bar: i64,
}

/// Every grid line between `x_min` and `x_max` in paint order (per bar: its beats, its sub-beat
/// lines, then the bar line). Beat lines are skipped where they would be closer than 10 points,
/// bar lines between labels where bars are closer than 6, and sub-beat lines where they would be
/// denser than `MIN_DRAW_PX` (the snap still uses the step).
pub(super) fn lines(s: &Session, map: Map, x_min: f32, x_max: f32) -> Vec<Line> {
    let mut out = Vec::new();
    let first = s.tempo.bar_beat_at(map.at(x_min).max(0), s.sample_rate).bar.max(1);
    let bar_px = (map.x(bar_at(s, first + 1)) - map.x(bar_at(s, first))).max(0.01);
    // Label every n bars so labels keep about 40 points apart.
    let every = [1, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f32 >= 40.0).unwrap_or(256);
    let ppt = crate::grid::px_per_tick(s, map.spp);
    let step = crate::grid::ticks(ppt, crate::grid::bar_ticks(s), &map.grid);
    let sub_step = crate::grid::sub_line_step(step, ppt, transit_time::TICKS_PER_QUARTER);
    for bar in (first..).take(4000) {
        let x = map.x(bar_at(s, bar));
        if x > x_max {
            break;
        }
        let beats = s.tempo.meter_at_tick(s.tempo.bar_start_tick(bar)).numerator.max(1);
        let next = map.x(bar_at(s, bar + 1));
        if (next - x) / beats as f32 >= 10.0 {
            for b in 1..beats {
                let bx = map.x(s.tempo.samples_at_bar_beat(BarBeat { bar, beat: i64::from(b) + 1, tick: 0 }, s.sample_rate));
                out.push(Line { x: bx, kind: Kind::Beat, bar });
            }
        }
        if let Some(step) = sub_step {
            let (t0, t1) = (s.tempo.bar_start_tick(bar), s.tempo.bar_start_tick(bar + 1));
            let beat = s.tempo.meter_at_tick(t0).ticks_per_beat().max(1);
            // Multiples of the step from tick 0, as `snap` rounds to them.
            for tick in ((t0 + step - 1).div_euclid(step) * step..t1).step_by(step as usize) {
                if (tick - t0) % beat == 0 {
                    continue;
                }
                let sx = map.x(s.tempo.tick_to_samples(tick, s.sample_rate));
                if sx >= x_min - 1.0 && sx <= x_max {
                    out.push(Line { x: sx, kind: Kind::Sub, bar });
                }
            }
        }
        if (bar - 1) % every == 0 {
            out.push(Line { x, kind: Kind::Bar { labeled: true }, bar });
        } else if bar_px >= 6.0 {
            out.push(Line { x, kind: Kind::Bar { labeled: false }, bar });
        }
    }
    out
}

/// A clip body the overlay crosses: its rect and body color.
#[derive(Clone, Copy, Debug)]
pub(super) struct Body {
    pub rect: Rect,
    pub color: Color32,
}

/// The bodies the overlay crosses: for each clip intersecting `tl`, its name strip in the strip
/// color, then the body below it in the body color, sorted by top edge. The two are separate
/// because the strip is a different color and can sit on the other side of the 0.5 luma line.
pub(super) fn bodies(clips: &[ClipRect], tl: Rect) -> Vec<Body> {
    let mut out = Vec::new();
    for c in clips {
        let r = Rect::from_min_max(pos2(c.rect[0], c.rect[1]), pos2(c.rect[2], c.rect[3]));
        if !r.intersects(tl) {
            continue;
        }
        let split = (r.min.y + CLIP_NAME_H).min(r.max.y);
        out.push(Body { rect: Rect::from_min_max(r.min, pos2(r.max.x, split)), color: c.strip });
        out.push(Body { rect: Rect::from_min_max(pos2(r.min.x, split), r.max), color: c.body });
    }
    out.sort_by(|a, b| a.rect.min.y.total_cmp(&b.rect.min.y));
    out
}

/// Paints the grid across the timeline `tl`, once per frame: beneath the clips by default
/// (`above` is `None`), above them with View > Grid Over Clips (`above` is the clip bodies that
/// intersect `tl`, sorted by top edge). The lane and the overlay are this one pass, never two.
/// Above the clips each line is split at the bodies it crosses and stroked per [`segments_into`],
/// an emulated overlay blend mode.
pub(super) fn paint(p: &egui::Painter, tl: Rect, lines: &[Line], above: Option<&[Body]>) {
    let gp = p.with_clip_rect(tl);
    let mut scratch = Vec::with_capacity(8);
    for l in lines {
        match above {
            None => {
                gp.line_segment([pos2(l.x, tl.min.y), pos2(l.x, tl.max.y)], Stroke::new(1.0, color(l.kind)));
            }
            Some(bodies) => {
                segments_into(l, tl, bodies, &mut scratch);
                for &(y0, y1, c) in &scratch {
                    gp.line_segment([pos2(l.x, y0), pos2(l.x, y1)], Stroke::new(1.0, c));
                }
            }
        }
    }
}

/// Whether a body reads as bright: perceived luma (Rec. 601 on the stored values) above 0.5.
pub(super) fn bright(c: Color32) -> bool {
    0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b()) > 127.5
}

/// The per-segment rule of the overlay, an overlay blend mode emulated. With `a` the alpha of
/// [`overlay_color`] for the line's kind: over the empty lane the stroke is [`overlay_color`]
/// itself (the lane looks as it does with the option off); over a clip body (the name strip and
/// the body below it are two bodies, each in its own color) whose luma is above 0.5 it is black
/// at [`body_alpha`] (darkening, where the lane lightens); over a body at 0.5 or below it is
/// white at [`body_alpha`]. A line is split at the top and bottom edge of each body it crosses;
/// neighbors of one color are merged.
pub(super) fn segments(l: &Line, tl: Rect, bodies: &[Body]) -> Vec<(f32, f32, Color32)> {
    let mut sorted = bodies.to_vec();
    sorted.sort_by(|a, b| a.rect.min.y.total_cmp(&b.rect.min.y));
    let mut out = Vec::new();
    segments_into(l, tl, &sorted, &mut out);
    out
}

/// [`segments`] into a reused buffer; `bodies` must be sorted by top edge.
pub(super) fn segments_into(l: &Line, tl: Rect, bodies: &[Body], out: &mut Vec<(f32, f32, Color32)>) {
    out.clear();
    let lane = overlay_color(l.kind);
    let a = body_alpha(lane.a());
    let mut push = |out: &mut Vec<(f32, f32, Color32)>, y0: f32, y1: f32, c: Color32| match out.last_mut() {
        Some(last) if last.2 == c && last.1 >= y0 => last.1 = y1,
        _ => out.push((y0, y1, c)),
    };
    let mut y = tl.min.y;
    for b in bodies.iter().filter(|b| b.rect.min.x <= l.x && l.x <= b.rect.max.x) {
        let (y0, y1) = (b.rect.min.y.max(tl.min.y).max(y), b.rect.max.y.min(tl.max.y));
        if y1 <= y0 {
            continue;
        }
        if y0 > y {
            push(out, y, y0, lane);
        }
        push(out, y0, y1, crate::theme::tint(!bright(b.color), a));
        y = y1;
    }
    if y < tl.max.y {
        push(out, y, tl.max.y, lane);
    }
}

#[cfg(test)]
#[path = "grid_overlay_tests.rs"]
mod tests;
