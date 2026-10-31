//! The Filter's response curve (#242): the magnitude response of its mode, slope, cutoff and
//! resonance (`transit_dsp::filter_response`, the analytic transfer function the DSP is tested
//! against), on a log frequency axis, with the reach of the modulation under it: the band the
//! envelope and the LFO move the cutoff over at their amounts (six octaves at 100%).

use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::{RADIUS_SM, TOKENS};
use egui::{Pos2, Rect, Sense, Shape, Stroke, Ui, vec2};
use transit_dsp::PluginInfo;
use transit_model::{Insert, TrackId};

pub const CURVE_W: f32 = 400.0;
pub const CURVE_H: f32 = 96.0;
const F_LO: f32 = 20.0;
const F_HI: f32 = 20_000.0;
const DB_TOP: f32 = 30.0;
const DB_BOTTOM: f32 = -48.0;
/// Octaves a modulation amount of 100% moves the cutoff (as the DSP does).
const MOD_OCTAVES: f32 = transit_dsp::plugins::filter::MOD_OCTAVES;

/// 0..1 across the plot for `freq` (log axis).
#[must_use]
pub fn x_of(freq: f32) -> f32 {
    ((freq.max(F_LO) / F_LO).ln() / (F_HI / F_LO).ln()).clamp(0.0, 1.0)
}

/// The frequency at 0..1 across the plot.
#[must_use]
pub fn freq_at(x: f32) -> f32 {
    F_LO * (F_HI / F_LO).powf(x.clamp(0.0, 1.0))
}

/// 0..1 down the plot for `db` (top: +30 dB, bottom: -48 dB, clamped).
#[must_use]
pub fn y_of(db: f32) -> f32 {
    ((DB_TOP - db) / (DB_TOP - DB_BOTTOM)).clamp(0.0, 1.0)
}

/// The lowest and highest cutoff the envelope and the LFO reach at their amounts, from `cutoff`.
#[must_use]
pub fn reach(cutoff: f32, env_amount_pct: f32, lfo_amount_pct: f32) -> (f32, f32) {
    let env = MOD_OCTAVES * env_amount_pct / 100.0;
    let lfo = MOD_OCTAVES * (lfo_amount_pct / 100.0).abs();
    let lo = env.min(0.0) - lfo;
    let hi = env.max(0.0) + lfo;
    ((cutoff * lo.exp2()).clamp(F_LO, F_HI), (cutoff * hi.exp2()).clamp(F_LO, F_HI))
}

/// Draws the curve for the insert's current values.
pub fn show(app: &StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, lay: &mut EditorLayout) {
    let value = |id: &str| {
        let p = info.param(id);
        let d = p.map_or(0.0, |p| p.default);
        p.map_or(d, |p| p.clamp(app.followed_param(key, id).or_else(|| ins.params.get(id).copied()).unwrap_or(d)))
    };
    let params: Vec<(&str, f32)> = info.params.iter().map(|p| (p.id, value(p.id))).collect();
    let (rect, resp) = ui.allocate_exact_size(vec2(CURVE_W, CURVE_H), Sense::hover());
    crate::help::hover(ui, &resp, crate::help::Id::FilterCurve);
    lay.curve = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
    let p = ui.painter_at(rect);
    p.rect_filled(rect, RADIUS_SM, TOKENS.lane);
    let at = |x: f32, y: f32| Pos2::new(rect.min.x + x * rect.width(), rect.min.y + y * rect.height());
    // Decade lines and the 0 dB line.
    for f in [100.0, 1000.0, 10_000.0] {
        let x = x_of(f);
        p.line_segment([at(x, 0.0), at(x, 1.0)], Stroke::new(1.0, TOKENS.grid_beat));
    }
    p.line_segment([at(0.0, y_of(0.0)), at(1.0, y_of(0.0))], Stroke::new(1.0, TOKENS.grid_bar));
    // The modulation's reach, under the curve.
    let cutoff = value("cutoff");
    let (lo, hi) = reach(cutoff, value("env_amount"), value("lfo_amount"));
    if hi > lo {
        let band = Rect::from_min_max(at(x_of(lo), 0.0), at(x_of(hi), 1.0));
        p.rect_filled(band, 0.0, TOKENS.accent.gamma_multiply(0.12));
    }
    p.line_segment([at(x_of(cutoff), 0.0), at(x_of(cutoff), 1.0)], Stroke::new(1.0, TOKENS.accent.gamma_multiply(0.5)));
    let cols = rect.width().max(2.0) as usize;
    let freqs: Vec<f32> = (0..cols).map(|i| freq_at(i as f32 / (cols - 1) as f32)).collect();
    let db = transit_dsp::filter_response(&params, &freqs, 48_000.0);
    let line: Vec<Pos2> = db.iter().enumerate().map(|(i, d)| at(i as f32 / (cols - 1) as f32, y_of(*d))).collect();
    p.add(Shape::line(line, Stroke::new(1.5, TOKENS.accent)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axes_map_and_clamp() {
        assert_eq!(x_of(20.0), 0.0);
        assert!((x_of(20_000.0) - 1.0).abs() < 1e-6);
        assert!((freq_at(x_of(1000.0)) - 1000.0).abs() < 0.1);
        assert_eq!(y_of(DB_TOP + 10.0), 0.0);
        assert_eq!(y_of(-200.0), 1.0);
    }

    #[test]
    fn the_reach_covers_the_envelope_and_both_sides_of_the_lfo() {
        assert_eq!(reach(1000.0, 0.0, 0.0), (1000.0, 1000.0));
        let (lo, hi) = reach(100.0, 50.0, 0.0);
        assert!((lo - 100.0).abs() < 1e-3 && (hi - 800.0).abs() < 0.1, "{lo} {hi}");
        let (lo, hi) = reach(1000.0, -50.0, 100.0 / 6.0);
        assert!((lo - 62.5).abs() < 0.1 && (hi - 2000.0).abs() < 0.1, "{lo} {hi}");
    }
}
