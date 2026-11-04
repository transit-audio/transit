//! The arrangement's grid lines: one computation of where they fall, shared by the lane grid and
//! the optional overlay that strokes them again over clip bodies (#302).
//!
//! Ableton Live keeps the grid visible through clips; Logic Pro draws it behind regions and the
//! regions cover it. They disagree, so transit offers both: View > Grid Over Clips, default off.

use super::*;

/// Alpha of a bar line stroked over a clip body.
pub(super) const BAR_ALPHA: f32 = 0.45;
/// Alpha of a beat line stroked over a clip body.
pub(super) const BEAT_ALPHA: f32 = 0.25;

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

/// The bar and beat lines again over a clip body (`body`: below the name strip), in `ink`, at
/// reduced alpha. Off unless grid lines are shown and View > Grid Over Clips is on. `p` is
/// already clipped to the clip and the timeline.
pub(super) fn draw(p: &egui::Painter, s: &Session, map: Map, tl: Rect, body: Rect, ink: Color32) {
    if !(s.edit.grid_lines && s.edit.grid_over_clips) || body.height() <= 4.0 {
        return;
    }
    let vis = body.intersect(tl);
    if vis.min.x > vis.max.x {
        return;
    }
    for l in lines(s, map, vis.min.x, vis.max.x) {
        let alpha = match l.kind {
            Kind::Bar { .. } => BAR_ALPHA,
            Kind::Beat => BEAT_ALPHA,
            Kind::Sub => continue,
        };
        if l.x >= vis.min.x && l.x <= vis.max.x {
            p.line_segment([pos2(l.x, body.min.y), pos2(l.x, body.max.y)], Stroke::new(1.0, ink.gamma_multiply(alpha)));
        }
    }
}

#[cfg(test)]
#[path = "grid_overlay_tests.rs"]
mod tests;
