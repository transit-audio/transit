//! The hatch (#326, the maintainer's Hatch artboard): a region that cannot take input right now.
//!
//! `text` at 7 percent (6 percent in the light set), 1.5 px lines at −45 degrees (rising left to
//! right), 8 px apart, anchored to the window: every line is `x + y = k · pitch · √2` in window
//! coordinates, so the stripes of a track header and its lane line up across the seam.
//!
//! Use it for inactive tracks, suspended or missing plug-ins, frozen lanes, unavailable insert
//! slots and the space beyond a clip's end in an editor. Never for a disabled-but-editable section
//! (that dims to 40 percent), a muted track, a single small control (ghost it instead), or a
//! sliver under 16 px ([`applies`]): track headers and lanes hatch at every height. The kit stores nothing: [`paint`] takes the rect and returns what it
//! drew.

use crate::theme::TOKENS;
use crate::tokens::hatch as h;
use egui::{Color32, Painter, Rect, Stroke, pos2};

/// What [`paint`] drew.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct HatchLayout {
    pub rect: [f32; 4],
    #[serde(skip)]
    pub color: Color32,
    pub stroke: f32,
    /// Each line's `x + y` in window coordinates, in order (a multiple of [`step`]).
    pub lines: Vec<f32>,
}

/// The distance between two lines' `x + y`: the 8 px pitch measured across the −45 degree lines.
pub fn step() -> f32 {
    h::PITCH * std::f32::consts::SQRT_2
}

/// The hatch's ink in the theme being drawn: `text` at 7 percent (6 in the light set).
pub fn color() -> Color32 {
    let light = crate::theme::current() == egui::Theme::Light;
    TOKENS.text.gamma_multiply(if light { h::ALPHA_LIGHT } else { h::ALPHA })
}

/// Whether a region is large enough to hatch: `hatch.minSize` (16 px) or more in both directions,
/// so only slivers skip it.
pub fn applies(rect: Rect) -> bool {
    rect.width() >= h::MIN_SIZE && rect.height() >= h::MIN_SIZE
}

/// The segment of the line `x + y = c` inside `rect`, if it crosses it.
fn segment(rect: Rect, c: f32) -> Option<[egui::Pos2; 2]> {
    let x0 = rect.min.x.max(c - rect.max.y);
    let x1 = rect.max.x.min(c - rect.min.y);
    (x1 > x0).then(|| [pos2(x0, c - x0), pos2(x1, c - x1)])
}

/// Hatches `rect` when it is large enough ([`applies`]); `None` when it is not.
pub fn paint(painter: &Painter, rect: Rect) -> Option<HatchLayout> {
    if !applies(rect) {
        return None;
    }
    let (step, color) = (step(), color());
    let p = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let stroke = Stroke::new(h::STROKE, color);
    let mut lines = Vec::new();
    let mut k = ((rect.min.x + rect.min.y) / step).ceil();
    while k * step < rect.max.x + rect.max.y {
        let c = k * step;
        if let Some(seg) = segment(rect, c) {
            p.line_segment(seg, stroke);
            lines.push(c);
        }
        k += 1.0;
    }
    Some(HatchLayout { rect: super::r4(rect), color, stroke: h::STROKE, lines })
}
