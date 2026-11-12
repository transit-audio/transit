//! The display well (`docs/design/language`, Components > Display well): an inset `surface.inset`
//! with a 1 px `line.inset`, radius 6 and 8 px padding; readouts in `readout.display` across the
//! top; the curve in `signal`, stroke 2, over a 12 percent fill; draggable points as hollow
//! circles (r 4.5, stroke 1.5); a modulation range as a `mod` band at 10 percent: across x, or
//! between two curves (a reach: the curve at the modulation's two extremes, under the curve).
//!
//! Positions are normalized: x 0..1 left to right, y 0..1 bottom to top. The caller maps them to
//! its units and owns the values; a dragged point comes back as a new normalized position.
//!
//! A curve whose values run off the bottom (a filter's slope past the well's floor) can fade into
//! the floor (`floor_fade`, #428): the stroke's alpha falls from full at that height to nothing at
//! the floor, so the clamped part never draws a flat line along the floor with a hard corner.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{self, alpha, radius};
use egui::{Align2, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

pub const PADDING: f32 = 8.0;
pub const READOUT_H: f32 = 14.0;
pub const POINT_R: f32 = 4.5;
pub const POINT_STROKE: f32 = 1.5;
pub const CURVE_STROKE: f32 = 2.0;
/// The hit box around a point is this much larger than the circle.
const POINT_HIT: f32 = 4.0;

/// The inputs of one well.
#[derive(Default)]
pub struct Well<'a> {
    pub readouts: &'a [&'a str],
    pub curve: &'a [[f32; 2]],
    pub points: &'a [[f32; 2]],
    /// A modulation range across x (normalized).
    pub mod_band: Option<[f32; 2]>,
    /// A modulation reach: the region between two curves sampled at the same x positions
    /// (normalized), filled in the band color under the curve (326-X).
    pub between: Option<(&'a [[f32; 2]], &'a [[f32; 2]])>,
    /// The normalized height over which the curve's stroke fades out toward the floor (0: no fade).
    pub floor_fade: f32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WellLayout {
    pub well: [f32; 4],
    /// Where the curve plots (inside the padding, under the readout row).
    pub plot: [f32; 4],
    pub readouts: Vec<[f32; 4]>,
    pub points: Vec<[f32; 4]>,
    pub band: Option<[f32; 4]>,
    /// The reach's bounds when one is drawn.
    pub between: Option<[f32; 4]>,
    #[serde(skip)]
    pub curve_color: Color32,
    #[serde(skip)]
    pub fill_color: Color32,
    #[serde(skip)]
    pub band_color: Color32,
    /// The curve as drawn (normalized), and the height its stroke fades over toward the floor.
    #[serde(skip)]
    pub curve: Vec<[f32; 2]>,
    pub floor_fade: f32,
}

pub struct WellOut {
    pub response: egui::Response,
    pub layout: WellLayout,
    /// A point being dragged: its index and new normalized position.
    pub dragged: Option<(usize, [f32; 2])>,
}

/// The screen position of a normalized position in `plot`.
pub fn to_screen(plot: Rect, n: [f32; 2]) -> Pos2 {
    pos2(plot.min.x + n[0] * plot.width(), plot.max.y - n[1] * plot.height())
}

/// The normalized position of a screen position in `plot`, clamped to the plot.
pub fn to_norm(plot: Rect, p: Pos2) -> [f32; 2] {
    [((p.x - plot.min.x) / plot.width()).clamp(0.0, 1.0), ((plot.max.y - p.y) / plot.height()).clamp(0.0, 1.0)]
}

/// The curve stroke's alpha (0..1) at normalized height `y` with a floor fade over `fade`: 0 at
/// the floor, 1 from `fade` up; no fade (1 everywhere) when `fade` is 0.
pub fn floor_alpha(y: f32, fade: f32) -> f32 {
    if fade <= 0.0 { 1.0 } else { (y / fade).clamp(0.0, 1.0) }
}

/// The plot area of a well at `well`, under a readout row when it has readouts.
pub fn plot_rect(well: Rect, readouts: bool) -> Rect {
    let inner = well.shrink(PADDING);
    if readouts { Rect::from_min_max(pos2(inner.min.x, inner.min.y + READOUT_H), inner.max) } else { inner }
}

/// Draws a well of `size`.
pub fn show(ui: &mut Ui, id: egui::Id, size: egui::Vec2, w: &Well) -> WellOut {
    let t = &TOKENS;
    let (well, response) = ui.allocate_exact_size(size, Sense::hover());
    let plot = plot_rect(well, !w.readouts.is_empty());
    let p = ui.painter().clone();
    p.rect_filled(well, radius::DISPLAY, t.surface_inset);
    p.rect_stroke(well, radius::DISPLAY, Stroke::new(1.0, t.line_inset), StrokeKind::Inside);
    // Readouts spread across the top row.
    let mut readouts = Vec::new();
    let n = w.readouts.len().max(1) as f32;
    for (i, text) in w.readouts.iter().enumerate() {
        let slot = Rect::from_min_size(pos2(plot.min.x + plot.width() * i as f32 / n, well.min.y + PADDING), vec2(plot.width() / n, READOUT_H));
        let r = super::role_text(&p, &tokens::READOUT_DISPLAY, text, tokens::READOUT_DISPLAY.color(), Align2::LEFT_CENTER, slot.left_center());
        readouts.push(r4(r));
    }
    let band_color = t.modulation.gamma_multiply(alpha::MOD_BAND);
    let band = w.mod_band.map(|[a, b]| {
        let r = Rect::from_x_y_ranges(to_screen(plot, [a.min(b), 0.0]).x..=to_screen(plot, [a.max(b), 0.0]).x, plot.y_range());
        p.rect_filled(r, 0.0, band_color);
        r4(r)
    });
    let between = w.between.filter(|(a, b)| a.len() >= 2 && a.len() == b.len()).map(|(a, b)| {
        let (a, b): (Vec<Pos2>, Vec<Pos2>) = (a.iter().map(|n| to_screen(plot, *n)).collect(), b.iter().map(|n| to_screen(plot, *n)).collect());
        let mut mesh = Mesh::default();
        for i in 0..a.len() - 1 {
            let base = mesh.vertices.len() as u32;
            for v in [a[i], a[i + 1], b[i + 1], b[i]] {
                mesh.colored_vertex(v, band_color);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        let bounds = mesh.calc_bounds();
        p.add(Shape::mesh(mesh));
        r4(bounds)
    });
    let fill_color = t.signal.gamma_multiply(alpha::CURVE_FILL);
    if w.curve.len() >= 2 {
        let pts: Vec<Pos2> = w.curve.iter().map(|n| to_screen(plot, *n)).collect();
        let mut mesh = Mesh::default();
        for pair in pts.windows(2) {
            let base = mesh.vertices.len() as u32;
            for v in [pair[0], pair[1], pos2(pair[1].x, plot.max.y), pos2(pair[0].x, plot.max.y)] {
                mesh.colored_vertex(v, fill_color);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(Shape::mesh(mesh));
        if w.floor_fade > 0.0 {
            // The stroke's ink by height: full from the fade's top, nothing at the floor.
            let (signal, fade) = (t.signal, w.floor_fade);
            let stroke = egui::epaint::PathStroke::new_uv(CURVE_STROKE, move |_, at| {
                let y = (plot.max.y - at.y) / plot.height().max(1.0);
                signal.gamma_multiply(floor_alpha(y, fade))
            });
            p.add(Shape::line(pts, stroke));
        } else {
            p.add(Shape::line(pts, Stroke::new(CURVE_STROKE, t.signal)));
        }
    }
    let mut points = Vec::new();
    let mut dragged = None;
    for (i, n) in w.points.iter().enumerate() {
        let c = to_screen(plot, *n);
        let hit = Rect::from_center_size(c, vec2(2.0 * (POINT_R + POINT_HIT), 2.0 * (POINT_R + POINT_HIT)));
        let r = ui.interact(hit, id.with(i), Sense::drag());
        if r.dragged()
            && let Some(at) = r.interact_pointer_pos()
        {
            dragged = Some((i, to_norm(plot, at)));
        }
        p.circle(c, POINT_R, t.surface_inset, Stroke::new(POINT_STROKE, t.text));
        points.push(r4(Rect::from_center_size(c, vec2(2.0 * POINT_R, 2.0 * POINT_R))));
    }
    let layout = WellLayout {
        well: r4(well),
        plot: r4(plot),
        readouts,
        points,
        band,
        between,
        curve_color: t.signal,
        fill_color,
        band_color,
        curve: w.curve.to_vec(),
        floor_fade: w.floor_fade,
    };
    WellOut { response, layout, dragged }
}
