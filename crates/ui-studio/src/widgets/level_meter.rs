//! The level meter (`docs/design/language`, Components > Level meter and the fader's meter): a
//! flat bar in a `surface.inset` well with a 1 px `line.inset`, on the fader's dB taper so it
//! lines up with a fader beside it. `meter.ok` fills, `meter.hot` only above the reference (0 dBFS
//! for a level meter, 0 VU for the VU reading) and in the latched clip cell; a 2 px `text`
//! peak-hold line. A gain-reduction bar fills from the top in `mod`, with no over zone.
//!
//! Ballistics are a pair of time constants ([`Ballistics`]): the default, [`Ballistics::PEAK_FALLOFF`],
//! jumps to any higher value within the frame and falls off over 300 ms, so transients are never
//! hidden; [`Ballistics::VU`] rises and falls over 300 ms and reads against a reference (0 VU at
//! −18 dBFS by default). The widget is stateless: the caller holds the displayed value and steps
//! it with [`meter_step`] each frame with the frame's `dt`. The peak-hold line and the clip cell
//! are never smoothed. A slow reading is a ballistics mode of this bar, never a needle on a dial.

use super::{fader, r4};
use crate::theme::TOKENS;
use crate::tokens::{self, fader as spec};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// How the displayed value follows the signal: the time to reach 99 percent of a step up
/// (attack) and down (release), in milliseconds; 0 is instantaneous.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Ballistics {
    pub attack_ms: f32,
    pub release_ms: f32,
}

impl Ballistics {
    /// The default: instant attack, a 300 ms fall-off.
    pub const PEAK_FALLOFF: Ballistics = Ballistics { attack_ms: 0.0, release_ms: 300.0 };
    /// The averaging reading: 300 ms up and down (the VU standard's rise time).
    pub const VU: Ballistics = Ballistics { attack_ms: 300.0, release_ms: 300.0 };
}

impl Default for Ballistics {
    fn default() -> Ballistics {
        Ballistics::PEAK_FALLOFF
    }
}

/// Where 0 VU sits by default, in dBFS.
pub const VU_REFERENCE_DBFS: f32 = -18.0;
/// The tag under the well.
pub const TAG_H: f32 = 14.0;

/// One frame of ballistics: `prev` (the displayed dB) moves toward `target` by a one-pole step
/// whose time constant reaches 99 percent of a step in the attack (rising) or release (falling)
/// time. `dt` is the frame's duration in seconds, from the frame clock.
pub fn meter_step(prev: f32, target: f32, dt: f32, b: Ballistics) -> f32 {
    let floor = fader::FLOOR_DB;
    let (prev, target) = (prev.max(floor), target.max(floor));
    let ms = if target > prev { b.attack_ms } else { b.release_ms };
    if ms <= 0.0 {
        return target;
    }
    // 99 percent in `ms`: what remains of the step after `dt` is 0.01^(dt / ms) (computed in f64
    // so a frame landing exactly on `ms` sees the full 99 percent).
    let remain = 0.01f64.powf(f64::from(dt.max(0.0)) * 1000.0 / f64::from(ms));
    let exact = f64::from(target) + (f64::from(prev) - f64::from(target)) * remain;
    // Round toward the target, so the f32 state the caller keeps never lags the curve.
    let out = exact as f32;
    if target > prev && f64::from(out) < exact {
        out.next_up()
    } else if target < prev && f64::from(out) > exact {
        out.next_down()
    } else {
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Kind {
    Level,
    /// Gain reduction: fills down from the top, in `mod`.
    GainReduction,
}

/// The inputs of one meter bar.
#[derive(Debug, Clone, Copy)]
pub struct Meter {
    pub kind: Kind,
    /// The displayed reading in dBFS (already stepped by the caller); for gain reduction, the
    /// reduction in dB as a positive number.
    pub level_db: f32,
    /// The instantaneous peak drawn behind the reading at 35 percent (level meters only).
    pub peak_db: Option<f32>,
    /// The peak-hold line.
    pub hold_db: Option<f32>,
    /// The latched clip cell is lit.
    pub clip: bool,
    pub ballistics: Ballistics,
    /// The level that reads 0 on the scale: 0 dBFS, or the VU reference.
    pub reference_dbfs: f32,
}

impl Meter {
    pub fn level(level_db: f32) -> Meter {
        Meter { kind: Kind::Level, level_db, peak_db: None, hold_db: None, clip: false, ballistics: Ballistics::PEAK_FALLOFF, reference_dbfs: 0.0 }
    }

    pub fn vu(level_db: f32) -> Meter {
        Meter { ballistics: Ballistics::VU, reference_dbfs: VU_REFERENCE_DBFS, ..Meter::level(level_db) }
    }

    pub fn gain_reduction(reduction_db: f32) -> Meter {
        Meter { kind: Kind::GainReduction, ..Meter::level(reduction_db) }
    }

    /// The tag under the well: `VU` for the averaging reading, `GR` for gain reduction.
    pub fn tag(&self) -> Option<&'static str> {
        match self.kind {
            Kind::GainReduction => Some("GR"),
            Kind::Level if self.ballistics == Ballistics::VU => Some("VU"),
            Kind::Level => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MeterLayout {
    pub well: [f32; 4],
    /// The reading's bar (below the reference).
    pub fill: [f32; 4],
    /// The part of the reading above the reference, in `meter.hot`.
    pub over: Option<[f32; 4]>,
    pub peak: Option<[f32; 4]>,
    pub hold: Option<[f32; 4]>,
    pub clip_cell: Option<[f32; 4]>,
    /// The y of the scale's 0 (the reference).
    pub reference_y: f32,
    pub reference_dbfs: f32,
    pub tag: Option<(&'static str, [f32; 4])>,
    #[serde(skip)]
    pub fill_color: Color32,
    #[serde(skip)]
    pub over_color: Color32,
    #[serde(skip)]
    pub clip_color: Color32,
}

/// Draws a meter `width` by `height` (the tag, when there is one, takes [`TAG_H`] of it).
pub fn show(ui: &mut Ui, width: f32, height: f32, m: &Meter) -> MeterLayout {
    let t = &TOKENS;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let tag_text = m.tag();
    let body = if tag_text.is_some() { Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - TAG_H)) } else { rect };
    let clip_cell = (m.kind == Kind::Level).then(|| Rect::from_min_size(body.min, vec2(body.width(), spec::METER_CLIP_CELL)));
    let well = if let Some(c) = clip_cell { Rect::from_min_max(pos2(body.min.x, c.max.y + 1.0), body.max) } else { body };
    let inner = well.shrink(1.0);
    let y_of = |frac: f32| inner.min.y + frac.clamp(0.0, 1.0) * inner.height();
    let reference_y = y_of(fader::taper(0.0));
    let p = ui.painter().clone();
    p.rect_filled(well, 0.0, t.surface_inset);
    p.rect_stroke(well, 0.0, Stroke::new(1.0, t.line_inset), StrokeKind::Inside);
    let (mut over, mut peak, mut hold) = (None, None, None);
    let fill_color = if m.kind == Kind::GainReduction { t.modulation } else { t.meter_ok };
    let fill = match m.kind {
        Kind::Level => {
            let y = y_of(fader::taper(m.level_db - m.reference_dbfs));
            if let Some(pk) = m.peak_db {
                let r = Rect::from_min_max(pos2(inner.min.x, y_of(fader::taper(pk - m.reference_dbfs)).max(reference_y)), inner.max);
                p.rect_filled(r, 0.0, t.meter_ok.gamma_multiply(spec::METER_PEAK_ALPHA));
                peak = Some(r4(r));
            }
            let below = Rect::from_min_max(pos2(inner.min.x, y.max(reference_y)), inner.max);
            p.rect_filled(below, 0.0, t.meter_ok);
            if y < reference_y {
                let r = Rect::from_min_max(pos2(inner.min.x, y), pos2(inner.max.x, reference_y));
                p.rect_filled(r, 0.0, t.meter_hot);
                over = Some(r4(r));
            }
            if let Some(h) = m.hold_db {
                let hy = y_of(fader::taper(h - m.reference_dbfs));
                let r = Rect::from_min_max(pos2(inner.min.x, hy - spec::METER_HOLD / 2.0), pos2(inner.max.x, hy + spec::METER_HOLD / 2.0));
                p.rect_filled(r, 0.0, t.text);
                hold = Some(r4(r));
            }
            below
        }
        Kind::GainReduction => {
            // 0 dB of reduction at the top; the taper below 0 dB spread over the whole well.
            let z = fader::taper(0.0);
            let frac = (fader::taper(-m.level_db.max(0.0)) - z) / (1.0 - z);
            let r = Rect::from_min_max(inner.min, pos2(inner.max.x, inner.min.y + frac * inner.height()));
            p.rect_filled(r, 0.0, fill_color);
            r
        }
    };
    if let Some(c) = clip_cell {
        p.rect_filled(c, 0.0, if m.clip { t.meter_hot } else { t.surface_inset });
    }
    let tag = tag_text.map(|text| {
        let r = super::role_text(&p, &tokens::TAG, text, t.text_muted, Align2::CENTER_CENTER, pos2(rect.center().x, rect.max.y - TAG_H / 2.0));
        (text, r4(r))
    });
    MeterLayout {
        well: r4(well),
        fill: r4(fill),
        over,
        peak,
        hold,
        clip_cell: clip_cell.map(r4),
        reference_y,
        reference_dbfs: m.reference_dbfs,
        tag,
        fill_color,
        over_color: t.meter_hot,
        clip_color: if m.clip { t.meter_hot } else { t.surface_inset },
    }
}

/// One channel of the vertical channel meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    /// The peak reading in dBFS, already stepped by the caller (peak fall-off by default); drawn
    /// in `meter.ok` at 35 percent.
    pub peak_db: f32,
    /// The RMS reading drawn solid on top; `None` where the source publishes peaks only (the
    /// layer is left out, never faked).
    pub rms_db: Option<f32>,
    /// The peak-hold line (never smoothed).
    pub hold_db: Option<f32>,
}

/// Where the channel meter drew, per channel, and with which inks.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ChannelMeterLayout {
    pub wells: Vec<[f32; 4]>,
    /// The 35 percent peak fill of each channel (absent while silent).
    pub peaks: Vec<Option<[f32; 4]>>,
    /// The solid RMS fill of each channel (absent while silent or without an RMS source).
    pub rms: Vec<Option<[f32; 4]>>,
    /// The part of an RMS reading above 0 dBFS, in `meter.hot`.
    pub over: Vec<Option<[f32; 4]>>,
    pub holds: Vec<Option<[f32; 4]>>,
    pub clip_cells: Vec<[f32; 4]>,
    pub clip: bool,
    /// The grid lines across the wells, one per taper mark, on whole physical pixels.
    pub grid: Vec<[f32; 4]>,
    /// The y of the 0 dBFS line.
    pub reference_y: f32,
    #[serde(skip)]
    pub peak_color: egui::Color32,
    #[serde(skip)]
    pub clip_color: egui::Color32,
}

/// The gap between the clip cell and the top of the well.
pub const CHANNEL_CLIP_GAP: f32 = 4.0;

/// The vertical channel meter (Fader > Meter, the mixer's stereo meter): one `surface.inset` well
/// per channel, `bar` wide with the meter gap between, spanning `travel` (the fader's track, so
/// the bar shares the taper and every mark lines up); grid lines in `line` at the scale's marks;
/// the peak fill in `meter.ok` at 35 percent, the RMS fill solid on top (`meter.hot` only above
/// 0 dBFS), a 2 px `text` peak-hold line; above each well, in the fader's top inset, a 4 px clip
/// cell, `meter.hot` while `clip` is latched and `line` otherwise. Paints only: the caller owns
/// the clicks and steps the readings with [`meter_step`].
pub fn paint_channel(p: &egui::Painter, x: f32, travel: Rect, bar: f32, channels: &[Channel], clip: bool) -> ChannelMeterLayout {
    let t = &TOKENS;
    let mut out = ChannelMeterLayout { clip, peak_color: t.meter_ok.gamma_multiply(spec::METER_PEAK_ALPHA), ..Default::default() };
    out.clip_color = if clip { t.meter_hot } else { t.line };
    let y_of = |db: f32, well: Rect| well.min.y + fader::taper(db) * well.height();
    for (i, ch) in channels.iter().enumerate() {
        let x0 = x + i as f32 * (bar + spec::METER_GAP);
        let well = Rect::from_min_max(pos2(x0, travel.min.y), pos2(x0 + bar, travel.max.y));
        let reference_y = y_of(0.0, well);
        out.reference_y = reference_y;
        p.rect_filled(well, 0.0, t.surface_inset);
        for m in spec::SCALE_LABELS {
            let (top, bottom) = fader::snap_rows(p, y_of(m.unwrap_or(f32::NEG_INFINITY), well), 1.0);
            let line = Rect::from_min_max(pos2(well.min.x, top), pos2(well.max.x, bottom));
            p.rect_filled(line, 0.0, t.line);
            if i == 0 {
                out.grid
                    .push(r4(Rect::from_min_max(pos2(x, top), pos2(x + channels.len() as f32 * (bar + spec::METER_GAP) - spec::METER_GAP, bottom))));
            }
        }
        let fill_to = |db: f32| (db > fader::FLOOR_DB).then(|| Rect::from_min_max(pos2(well.min.x, y_of(db, well).max(reference_y)), well.max));
        let peak = fill_to(ch.peak_db);
        if let Some(r) = peak {
            p.rect_filled(r, 0.0, out.peak_color);
        }
        let rms = ch.rms_db.and_then(fill_to);
        if let Some(r) = rms {
            p.rect_filled(r, 0.0, t.meter_ok);
        }
        let over = ch.rms_db.filter(|db| *db > 0.0).map(|db| Rect::from_min_max(pos2(well.min.x, y_of(db, well)), pos2(well.max.x, reference_y)));
        if let Some(r) = over {
            p.rect_filled(r, 0.0, t.meter_hot);
        }
        let hold = ch.hold_db.filter(|db| *db > fader::FLOOR_DB).map(|db| {
            let y = y_of(db, well).clamp(well.min.y + spec::METER_HOLD / 2.0, well.max.y - spec::METER_HOLD / 2.0);
            Rect::from_min_max(pos2(well.min.x, y - spec::METER_HOLD / 2.0), pos2(well.max.x, y + spec::METER_HOLD / 2.0))
        });
        if let Some(r) = hold {
            p.rect_filled(r, 0.0, t.text);
        }
        p.rect_stroke(well, 0.0, Stroke::new(1.0, t.line_inset), StrokeKind::Inside);
        let cell = Rect::from_min_max(
            pos2(well.min.x, well.min.y - CHANNEL_CLIP_GAP - spec::METER_CLIP_CELL),
            pos2(well.max.x, well.min.y - CHANNEL_CLIP_GAP),
        );
        p.rect_filled(cell, 0.0, out.clip_color);
        out.wells.push(r4(well));
        out.peaks.push(peak.map(r4));
        out.rms.push(rms.map(r4));
        out.over.push(over.map(r4));
        out.holds.push(hold.map(r4));
        out.clip_cells.push(r4(cell));
    }
    out
}

/// The width of a channel meter of `n` bars `bar` wide.
pub fn channel_width(n: usize, bar: f32) -> f32 {
    n as f32 * bar + n.saturating_sub(1) as f32 * spec::METER_GAP
}
