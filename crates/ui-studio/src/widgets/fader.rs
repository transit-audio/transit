//! The fader (`docs/design/language`, Components > Fader): one dB taper everywhere, shared by the
//! fader, its scale and any meter beside it ([`taper`], [`db_at`]) so positions line up. Three
//! variants: Channel (fader, labeled scale and a meter slot, 116 wide), Compact (fader and ticks,
//! 40 wide) and Horizontal (sends, crossfades; a 12 by 18 cap). A 4 px `line` track inset 10 at
//! each end, the fill in `signal` from the 0 dB line to the cap (Fill from unity: down for a cut,
//! up for a boost, none at unity; `ink.ghost` while muted), a 12 by 1 `ink.arrow` notch across the
//! track at 0 dB over the fill, and a `knob.capActive` cap with a 1 px `ink.arrow` rim and a 2 px
//! `text` center line.
//!
//! Interaction: drag (Shift for fine), Alt-click or double-click for unity, scroll to nudge 0.5 dB.
//! The fader reports the new gain; the caller writes it.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{self, fader as spec};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The bottom of the taper: −60 to −90 dB fill the last 6 percent; below is minus infinity.
pub const FLOOR_DB: f32 = -90.0;
/// One wheel notch (40 px of smooth scroll) nudges this many dB.
pub const NUDGE_DB: f32 = 0.5;
pub const FINE: f32 = 10.0;
/// The Channel variant's columns (`fader.channelColumns`, x from the level block's left edge):
/// the fader column, the labels' right edge, the tick, the meter's left edge.
pub const CHANNEL_FADER_W: f32 = spec::COLUMN_FADER[1];
pub const CHANNEL_LABEL_RIGHT: f32 = spec::COLUMN_LABEL_RIGHT;
pub const CHANNEL_METER_X: f32 = spec::COLUMN_METER_X;
/// The cap's span in the fader column (x 2 to 34: a 32 wide cap centered on the track at 18).
pub const COLUMN_FADER_CAP: [f32; 2] = [CHANNEL_FADER_W / 2.0 - spec::CAP[0] / 2.0, CHANNEL_FADER_W / 2.0 + spec::CAP[0] / 2.0];
const TICK: f32 = 4.0;
/// The Channel variant's tick with each placed label (the Fader construction board): 4 px, 3 px
/// after the label's right edge, toward the meter (x 63 to 67), never on the cap side where a
/// dash before `0` reads as a minus; `line.strong`, `text.muted` at 0 dB.
pub const CHANNEL_TICK: f32 = spec::TICK_LENGTH;
pub const CHANNEL_TICK_GAP: f32 = spec::TICK_GAP_AFTER_LABEL;

/// Fraction of travel from the top (0 at +6 dB, 1 at minus infinity) for `db`.
pub fn taper(db: f32) -> f32 {
    if db.is_nan() || db <= FLOOR_DB {
        return 1.0;
    }
    let marks = spec::TAPER;
    if db >= marks[0].0.unwrap_or(0.0) {
        return 0.0;
    }
    for pair in marks.windows(2) {
        let ((Some(hi), fa), (lo, fb)) = (pair[0], pair[1]) else { continue };
        let lo = lo.unwrap_or(FLOOR_DB);
        if db >= lo {
            return fa + (hi - db) / (hi - lo) * (fb - fa);
        }
    }
    1.0
}

/// The gain at a fraction of travel from the top: the inverse of [`taper`] (minus infinity at 1).
pub fn db_at(frac: f32) -> f32 {
    if frac >= 1.0 {
        return f32::NEG_INFINITY;
    }
    let marks = spec::TAPER;
    if frac <= 0.0 {
        return marks[0].0.unwrap_or(0.0);
    }
    for pair in marks.windows(2) {
        let ((Some(hi), fa), (lo, fb)) = (pair[0], pair[1]) else { continue };
        if frac <= fb {
            let lo = lo.unwrap_or(FLOOR_DB);
            return hi - (frac - fa) / (fb - fa) * (hi - lo);
        }
    }
    f32::NEG_INFINITY
}

/// The scale label for a mark (`None` is minus infinity), with a typographic minus.
pub fn label(db: Option<f32>) -> String {
    match db {
        None => "−∞".into(),
        Some(d) if d > 0.0 => format!("+{d:.0}"),
        Some(d) if d < 0.0 => format!("−{:.0}", -d),
        Some(_) => "0".into(),
    }
}

/// Labels by travel: the scale marks whose labels the Channel variant draws on a travel of
/// `travel` points, top to bottom. Each mark is placed greedily in `fader.labelPriority` order
/// (0, minus infinity, −12, −24, +6, −48, −6, −36) and skipped when its center would fall closer
/// than `fader.labelMinGap` (13) to a mark already placed, so labels never collide on a short
/// strip. A tick goes with each placed label only; the meter's grid lines stay at every mark.
pub fn placed_marks(travel: f32) -> Vec<Option<f32>> {
    let y = |m: Option<f32>| taper(m.unwrap_or(f32::NEG_INFINITY)) * travel;
    let mut placed: Vec<Option<f32>> = Vec::new();
    for m in spec::LABEL_PRIORITY {
        if placed.iter().all(|p| (y(*p) - y(m)).abs() >= spec::LABEL_MIN_GAP) {
            placed.push(m);
        }
    }
    spec::SCALE_LABELS.into_iter().filter(|m| placed.contains(m)).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Variant {
    Channel,
    Compact,
    Horizontal,
}

impl Variant {
    pub fn cap(self) -> [f32; 2] {
        match self {
            Variant::Channel => spec::CAP,
            Variant::Compact => spec::CAP_COMPACT,
            Variant::Horizontal => spec::CAP_HORIZONTAL,
        }
    }

    /// The fader's size for a travel of `length` (height, or width when horizontal).
    pub fn size(self, length: f32) -> egui::Vec2 {
        match self {
            Variant::Channel => vec2(spec::CHANNEL_WIDTH, length),
            Variant::Compact => vec2(spec::COMPACT_WIDTH, length),
            Variant::Horizontal => vec2(length, spec::HORIZONTAL_HEIGHT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FaderLayout {
    pub rect: [f32; 4],
    pub variant: Variant,
    pub track: [f32; 4],
    /// The signal fill, from the 0 dB line to the cap's center (Fill from unity); zero height (and
    /// not painted) at unity.
    pub fill: [f32; 4],
    /// The unity notch across the track on the 0 dB row (`fader.unityNotch`), drawn over the fill.
    pub notch: [f32; 4],
    pub cap: [f32; 4],
    pub center_line: [f32; 4],
    /// Scale labels (Channel: the placed ones, [`placed_marks`]) or ticks (Compact): text and where
    /// it sits.
    pub marks: Vec<(String, [f32; 4])>,
    /// The Channel variant's ticks, one per placed label, after it toward the meter (empty otherwise).
    pub ticks: Vec<[f32; 4]>,
    /// Where the caller draws the meter (Channel only).
    pub meter: Option<[f32; 4]>,
    pub muted: bool,
    #[serde(skip)]
    pub fill_color: Color32,
    #[serde(skip)]
    pub notch_color: Color32,
    #[serde(skip)]
    pub cap_fill: Color32,
    #[serde(skip)]
    pub cap_rim: Color32,
    #[serde(skip)]
    pub mark_inks: Vec<Color32>,
    #[serde(skip)]
    pub tick_inks: Vec<Color32>,
}

pub struct FaderOut {
    pub response: egui::Response,
    pub layout: FaderLayout,
    /// The new gain in dB, when the user moved it.
    pub db: Option<f32>,
}

/// The track's travel: top to bottom (vertical) or left to right (horizontal).
fn travel(variant: Variant, rect: Rect) -> (Rect, bool) {
    let horizontal = variant == Variant::Horizontal;
    let track = if horizontal {
        Rect::from_center_size(rect.center(), vec2(rect.width() - 2.0 * spec::INSET, spec::TRACK_WIDTH))
    } else {
        let cx = if variant == Variant::Channel { rect.min.x + CHANNEL_FADER_W / 2.0 } else { rect.center().x };
        Rect::from_center_size(pos2(cx, rect.center().y), vec2(spec::TRACK_WIDTH, rect.height() - 2.0 * spec::INSET))
    };
    (track, horizontal)
}

/// The cap center for `db` on `track`.
fn at(track: Rect, horizontal: bool, db: f32) -> Pos2 {
    let f = taper(db);
    if horizontal { pos2(track.max.x - f * track.width(), track.center().y) } else { pos2(track.center().x, track.min.y + f * track.height()) }
}

/// Draws a fader with a travel of `length` at `db`.
pub fn show(ui: &mut Ui, id: egui::Id, variant: Variant, length: f32, db: f32, muted: bool) -> FaderOut {
    let (rect, _) = ui.allocate_exact_size(variant.size(length), Sense::hover());
    let (track, horizontal) = travel(variant, rect);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut new = None;
    let fine = ui.input(|i| i.modifiers.shift);
    if response.dragged() {
        let d = ui.input(|i| i.pointer.delta());
        let (moved, len) = if horizontal { (-d.x, track.width()) } else { (d.y, track.height()) };
        if moved != 0.0 {
            let f = (taper(db) + moved / len / if fine { FINE } else { 1.0 }).clamp(0.0, 1.0);
            new = Some(db_at(f));
        }
    }
    let alt_click = response.clicked() && ui.input(|i| i.modifiers.alt);
    if alt_click || super::reset_click(&response) {
        new = Some(0.0);
    }
    if response.hovered() {
        let notches = super::wheel_notches(ui);
        if notches != 0.0 {
            let base = if db.is_finite() { db } else { FLOOR_DB };
            let step = notches * NUDGE_DB / if fine { FINE } else { 1.0 };
            new = Some((base + step).min(spec::TAPER[0].0.unwrap_or(0.0)));
        }
    }
    let layout = paint(ui.painter(), rect, variant, new.unwrap_or(db), muted);
    FaderOut { response, layout, db: new }
}

/// The travel's span for a fader laid out in `rect` (the track's rect: its top and bottom are the
/// taper's +6 dB and minus infinity).
pub fn track_rect(variant: Variant, rect: Rect) -> Rect {
    travel(variant, rect).0
}

/// A line `thickness` points thick centered on `y`, as whole physical pixel rows (crisp at any
/// scale): the rect it covers.
pub fn snap_rows(p: &egui::Painter, y: f32, thickness: f32) -> (f32, f32) {
    let ppp = p.pixels_per_point();
    let n = (thickness * ppp).round().max(1.0);
    let top = (y * ppp - n / 2.0).round();
    (top / ppp, (top + n) / ppp)
}

/// The Channel variant's scale as painted: the placed labels and a tick with each.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ChannelScale {
    pub marks: Vec<(String, [f32; 4])>,
    #[serde(skip)]
    pub mark_inks: Vec<Color32>,
    pub ticks: Vec<[f32; 4]>,
    #[serde(skip)]
    pub tick_inks: Vec<Color32>,
}

/// Paints the Channel variant's scale for a level block in `rect` (the Frame: the travel runs
/// from the block's top plus the inset to its bottom less it): the labels [`placed_marks`] places,
/// right-aligned at x 60, each with its 4 px tick at x 63 to 67, the label's center and the tick
/// on the mark's whole physical pixel row (the row the meter's grid line takes). A fader paints
/// it under its cap; the main bus without a master fader paints it alone, so a level can be read.
pub fn paint_channel_scale(p: &egui::Painter, rect: Rect) -> ChannelScale {
    let t = &TOKENS;
    let track = travel(Variant::Channel, rect).0;
    let mut out = ChannelScale::default();
    for m in placed_marks(track.height()) {
        let y = track.min.y + taper(m.unwrap_or(f32::NEG_INFINITY)) * track.height();
        let ink = if m == Some(0.0) { t.text } else { t.text_faint };
        let (top, bottom) = snap_rows(p, y, 1.0);
        let row = (top + bottom) / 2.0;
        let x = rect.min.x + CHANNEL_LABEL_RIGHT + CHANNEL_TICK_GAP;
        let tick = Rect::from_min_max(pos2(x, top), pos2(x + CHANNEL_TICK, bottom));
        let tick_ink = if m == Some(0.0) { t.text_muted } else { t.line_strong };
        p.rect_filled(tick, 0.0, tick_ink);
        out.ticks.push(r4(tick));
        out.tick_inks.push(tick_ink);
        let role = tokens::TypeRole { size: 10.0, ..tokens::READOUT_DISPLAY };
        let r = super::role_text(p, &role, &label(m), ink, Align2::RIGHT_CENTER, pos2(rect.min.x + CHANNEL_LABEL_RIGHT, row));
        out.marks.push((label(m), r4(r)));
        out.mark_inks.push(ink);
    }
    out
}

/// Paints a fader in `rect` at `db` without taking any input (the caller owns the gestures, as
/// the mixer's strips do); [`show`] is this plus the kit's gestures. The fill runs from the 0 dB
/// line to the cap (Fill from unity), under the unity notch on the 0 dB physical pixel row (the
/// row the Channel scale's label, tick and the meter's grid line take). The cap's center line is
/// snapped to whole physical pixels.
pub fn paint(p: &egui::Painter, rect: Rect, variant: Variant, db: f32, muted: bool) -> FaderLayout {
    let t = &TOKENS;
    let (track, horizontal) = travel(variant, rect);
    let c = at(track, horizontal, db);
    let [cw, ch] = variant.cap();
    let cap = Rect::from_center_size(c, vec2(cw, ch));
    // Fill from unity: gain is bipolar around 0 dB (`fader.fill.fromDb`), so the fill spans the
    // 0 dB line and the cap's center, whichever is first; none at unity.
    let zero = at(track, horizontal, spec::FILL_FROM_DB);
    let fill = if horizontal {
        Rect::from_min_max(pos2(zero.x.min(c.x), track.min.y), pos2(zero.x.max(c.x), track.max.y))
    } else {
        Rect::from_min_max(pos2(track.min.x, zero.y.min(c.y)), pos2(track.max.x, zero.y.max(c.y)))
    };
    let fill_color = if muted { t.ink_ghost } else { t.signal };
    let [nw, nh] = spec::UNITY_NOTCH;
    let notch = if horizontal {
        let (left, right) = snap_rows(p, zero.x, nh);
        Rect::from_min_max(pos2(left, zero.y - nw / 2.0), pos2(right, zero.y + nw / 2.0))
    } else {
        let (top, bottom) = snap_rows(p, zero.y, nh);
        Rect::from_min_max(pos2(zero.x - nw / 2.0, top), pos2(zero.x + nw / 2.0, bottom))
    };
    let center_line = if horizontal {
        Rect::from_center_size(c, vec2(spec::CAP_CENTER_LINE, ch - 6.0))
    } else {
        let (top, bottom) = snap_rows(p, c.y, spec::CAP_CENTER_LINE);
        Rect::from_min_max(pos2(c.x - (cw - 8.0) / 2.0, top), pos2(c.x + (cw - 8.0) / 2.0, bottom))
    };
    p.rect_filled(track, spec::TRACK_RADIUS, t.line);
    if fill.width() > 0.0 && fill.height() > 0.0 {
        p.rect_filled(fill, spec::TRACK_RADIUS, fill_color);
    }
    p.rect_filled(notch, 0.0, t.ink_arrow);
    // Scale labels (Channel) or ticks (Compact) at the taper's marks.
    let (mut marks, mut mark_inks, mut ticks, mut tick_inks) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if variant == Variant::Channel {
        ChannelScale { marks, mark_inks, ticks, tick_inks } = paint_channel_scale(p, rect);
    } else if !horizontal {
        for m in spec::SCALE_LABELS {
            let y = track.min.y + taper(m.unwrap_or(f32::NEG_INFINITY)) * track.height();
            let r = Rect::from_min_max(pos2(track.max.x + 6.0, y - 0.5), pos2(track.max.x + 6.0 + TICK, y + 0.5));
            p.rect_filled(r, 0.0, t.line_strong);
            marks.push((label(m), r4(r)));
            mark_inks.push(if m == Some(0.0) { t.text } else { t.text_faint });
        }
    }
    p.rect_filled(cap, spec::CAP_RADIUS, t.knob_cap_active);
    p.rect_stroke(cap, spec::CAP_RADIUS, Stroke::new(1.0, t.ink_arrow), StrokeKind::Inside);
    p.rect_filled(center_line, 0.0, t.text);
    let meter =
        (variant == Variant::Channel).then(|| r4(Rect::from_min_max(pos2(rect.min.x + CHANNEL_METER_X, track.min.y), pos2(rect.max.x, track.max.y))));
    FaderLayout {
        rect: r4(rect),
        variant,
        track: r4(track),
        fill: r4(fill),
        notch: r4(notch),
        cap: r4(cap),
        center_line: r4(center_line),
        marks,
        ticks,
        meter,
        muted,
        fill_color,
        notch_color: t.ink_arrow,
        cap_fill: t.knob_cap_active,
        cap_rim: t.ink_arrow,
        mark_inks,
        tick_inks,
    }
}
