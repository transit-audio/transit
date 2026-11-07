//! The hero panel (README, Device anatomy B; the board's first section): three columns, the
//! board's widths.
//!
//! - **Transfer** (236 wide): a display well with grid lines at the quarters in `grid` and the
//!   static transfer curve in `signal` over its fill (input dB across, output dB up, both −60 to
//!   0 dB, the board's scale), from the threshold, ratio, knee and makeup the way the plug-in's
//!   gain computer applies them ([`transfer_db`]; Limit is its ratio of 1000).
//!   Over the curve the operating point, a solid 8 px `text` dot with a 2 px `surface.inset` ring
//!   at the published detector level and the curve's output there, stepped with the peak fall-off
//!   ballistics and hidden below −60 dB or until a level is published (README, Compressor
//!   displays).
//! - **History** (the rest): a display well with grid lines at the quarters.
//! - **GR** (64 wide): the kit's gain-reduction bar (14 wide, the board's), filling down from the
//!   top with its `GR` tag, stepped with the peak fall-off ballistics, and the readout under it.

use super::{CompressorLayout, Dev, WellRecord, named, panel, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::{self, alpha, compressor as tok, layout as space};
use crate::widgets::{self, display_well, level_meter, r4};
use egui::{Align2, Color32, Mesh, Pos2, Rect, Shape, Stroke, Ui, pos2, vec2};

/// The operating point as drawn: the displayed input level, the curve's output there, the
/// center, the dot's radius, fill and ring.
#[derive(Debug, Clone, Default)]
pub struct DotRecord {
    pub input_db: f32,
    pub output_db: f32,
    pub center: [f32; 2],
    pub radius: f32,
    pub fill: Color32,
    pub ring: Stroke,
}

/// A column label's row and the room under it (the board's 6).
pub const LABEL_H: f32 = 14.0;
pub const LABEL_GAP: f32 = 6.0;
/// The wells: the board's 218 px plot inside 8 px of padding and a 1 px edge a side.
pub const WELL: f32 = 236.0;
/// The transfer column and the gain-reduction column (the board's 236 and 64).
pub const TRANSFER_W: f32 = WELL;
pub const GR_W: f32 = 64.0;
/// The gain-reduction bar's width (the board's 14).
pub const GR_BAR_W: f32 = 14.0;
/// The panel's height: its padding, a label row and a well.
pub const HEIGHT: f32 = 2.0 * space::PANEL_PADDING[0] + LABEL_H + LABEL_GAP + WELL;
/// The transfer well's scale, in dB, on both axes (the board's).
pub const DB_FLOOR: f32 = -60.0;
pub const DB_TOP: f32 = 0.0;
/// The curve's samples across the well.
pub const CURVE_POINTS: usize = 121;
/// The ratio the plug-in uses while Limit is on.
pub const LIMIT_RATIO: f32 = 1000.0;

/// The gain reduction (dB, at least 0) the plug-in's static soft-knee gain computer applies to a
/// detector level of `level_db`: below the knee none, above it `(1 − 1/ratio)` of the overshoot,
/// and inside it (`|over| ≤ knee / 2`) the quadratic that joins the two.
pub fn transfer_gr_db(level_db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold;
    if knee > 1e-3 && 2.0 * over.abs() <= knee {
        let t = over + knee / 2.0;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// The steady-state output level (dB) for an input level `input_db`: the input less the gain
/// reduction, plus the makeup. Limit sets the ratio to [`LIMIT_RATIO`].
pub fn transfer_db(input_db: f32, threshold: f32, ratio: f32, knee: f32, makeup: f32, limit: bool) -> f32 {
    let ratio = if limit { LIMIT_RATIO } else { ratio };
    input_db - transfer_gr_db(input_db, threshold, ratio, knee) + makeup
}

/// A level on the well's scale (0 at −60 dB, 1 at 0 dB), clamped.
pub fn scale(db: f32) -> f32 {
    ((db - DB_FLOOR) / (DB_TOP - DB_FLOOR)).clamp(0.0, 1.0)
}

/// The transfer curve of the device's current values, sampled across the well (normalized).
pub(crate) fn curve(app: &StudioApp, dev: Dev) -> Vec<[f32; 2]> {
    let v = |id: &str| dev.value(app, id);
    let (t, r, k, m, limit) = (v("threshold"), v("ratio"), v("knee"), v("makeup"), dev.on(app, "limiter"));
    (0..CURVE_POINTS)
        .map(|i| {
            let x = i as f32 / (CURVE_POINTS - 1) as f32;
            let input = DB_FLOOR + x * (DB_TOP - DB_FLOOR);
            [x, scale(transfer_db(input, t, r, k, m, limit))]
        })
        .collect()
}

/// The readout under the gain-reduction bar: the reduction as a negative gain (`-4.2 dB`),
/// `0.0 dB` while nothing is reduced.
pub fn gr_readout(gr_db: f32) -> String {
    if gr_db < 0.05 { "0.0 dB".to_string() } else { format!("-{gr_db:.1} dB") }
}

/// Grid lines at the quarters across a well's plot (`vertical` adds the three upright ones), in
/// `grid`, 1 px.
fn grid(ui: &Ui, plot: Rect, vertical: bool) -> Vec<[f32; 4]> {
    let p = ui.painter();
    let stroke = Stroke::new(1.0, TOKENS.grid);
    let mut lines = Vec::new();
    for q in [0.25, 0.5, 0.75] {
        let y = plot.max.y - q * plot.height();
        lines.push([plot.min.x, y, plot.max.x, y]);
        if vertical {
            let x = plot.min.x + q * plot.width();
            lines.push([x, plot.min.y, x, plot.max.y]);
        }
    }
    for l in &lines {
        p.line_segment([pos2(l[0], l[1]), pos2(l[2], l[3])], stroke);
    }
    lines
}

/// The well's curve (README, Display well): `signal`, 2 px, over its 12 percent fill down to the
/// plot's floor.
fn paint_curve(ui: &Ui, plot: Rect, curve: &[[f32; 2]]) {
    let t = &TOKENS;
    let pts: Vec<Pos2> = curve.iter().map(|n| display_well::to_screen(plot, *n)).collect();
    let fill = t.signal.gamma_multiply(alpha::CURVE_FILL);
    let mut mesh = Mesh::default();
    for w in pts.windows(2) {
        let base = mesh.vertices.len() as u32;
        for v in [w[0], w[1], pos2(w[1].x, plot.max.y), pos2(w[0].x, plot.max.y)] {
            mesh.colored_vertex(v, fill);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    let p = ui.painter();
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(display_well::CURVE_STROKE, t.signal)));
}

/// The gain reduction the meter shows: the tap's newest reading, stepped with the peak fall-off
/// ballistics (a deeper reduction shows within the frame, a recovery falls off over 300 ms).
fn gr_level(app: &mut StudioApp, ui: &Ui, dev: Dev) -> f32 {
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let target = app.scope_hub().map_or(0.0, |h| h.gain_reduction_db(dev.key.0, uid)).max(0.0);
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    let st = &mut app.builtin_editor.devices;
    let prev = st.gr.get(&dev.uid()).copied().unwrap_or(0.0);
    let shown = level_meter::meter_step(prev, target, dt, level_meter::Ballistics::PEAK_FALLOFF).max(0.0);
    st.gr.insert(dev.uid(), shown);
    shown
}

/// The operating point's displayed input level: the tap's newest detector level, stepped with the
/// peak fall-off ballistics; `None` until a level has been published since the editor opened.
fn dot_level(app: &mut StudioApp, ui: &Ui, dev: Dev) -> Option<f32> {
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let target = app.scope_hub().and_then(|h| h.detector_level_db(dev.key.0, uid));
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    let st = &mut app.builtin_editor.devices;
    let prev = st.dots.get(&dev.uid()).copied();
    let target = match (prev, target) {
        (None, None) => return None,
        (_, Some(t)) => t,
        (Some(_), None) => crate::widgets::fader::FLOOR_DB,
    };
    let shown = level_meter::meter_step(prev.unwrap_or(target), target, dt, level_meter::Ballistics::PEAK_FALLOFF);
    st.dots.insert(dev.uid(), shown);
    Some(shown)
}

/// The operating point at (`input_db`, the curve's output there), unless it is below the floor.
fn paint_dot(app: &StudioApp, ui: &Ui, dev: Dev, plot: Rect, input_db: f32) -> Option<DotRecord> {
    if input_db < tok::OPERATING_POINT_HIDE_BELOW_DB {
        return None;
    }
    let v = |id: &str| dev.value(app, id);
    let output_db = transfer_db(input_db, v("threshold"), v("ratio"), v("knee"), v("makeup"), dev.on(app, "limiter"));
    let c = display_well::to_screen(plot, [scale(input_db), scale(output_db)]);
    let radius = tok::OPERATING_POINT_SIZE / 2.0;
    let fill = named(tok::OPERATING_POINT_FILL);
    let ring = Stroke::new(tok::OPERATING_POINT_RING_WIDTH, named(tok::OPERATING_POINT_RING));
    ui.painter().circle(c, radius, fill, ring);
    Some(DotRecord { input_db, output_db, center: [c.x, c.y], radius, fill, ring })
}

/// Draws the hero panel into `rect`.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut CompressorLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "hero", out);
    out.hero.panel = r4(rect);
    let well_y = inner.min.y + LABEL_H + LABEL_GAP;
    // Transfer.
    let tl = section_label(ui, inner.min, "Transfer");
    out.hero.labels.push(("TRANSFER".to_string(), r4(tl)));
    let curve = curve(app, dev);
    let tw = Rect::from_min_size(pos2(inner.min.x, well_y), vec2(TRANSFER_W, WELL));
    let mut c = super::child(ui, tw);
    let well = display_well::show(&mut c, egui::Id::new(("compressor_transfer", dev.uid())), tw.size(), &display_well::Well::default());
    // The grid first, then the curve over its fill (the well's curve rule), as the board layers them.
    let plot = rect_of(well.layout.plot);
    let lines = grid(ui, plot, true);
    paint_curve(ui, plot, &curve);
    out.hero.curve = curve;
    out.hero.dot = dot_level(app, ui, dev).and_then(|db| paint_dot(app, ui, dev, plot, db));
    out.wells.push(WellRecord { name: "transfer".to_string(), well: well.layout, grid: lines });
    // GR, right-aligned.
    let gr_col = Rect::from_min_max(pos2(inner.max.x - GR_W, inner.min.y), inner.max);
    // History, between them.
    let hx = tw.max.x + space::PANEL_GAP;
    let hl = section_label(ui, pos2(hx, inner.min.y), "History");
    out.hero.labels.push(("HISTORY".to_string(), r4(hl)));
    let hw = Rect::from_min_max(pos2(hx, well_y), pos2(gr_col.min.x - space::PANEL_GAP, well_y + WELL));
    let mut c = super::child(ui, hw);
    let hist = display_well::show(&mut c, egui::Id::new(("compressor_history", dev.uid())), hw.size(), &display_well::Well::default());
    let lines = grid(ui, rect_of(hist.layout.plot), false);
    out.wells.push(WellRecord { name: "history".to_string(), well: hist.layout, grid: lines });
    lay.scope = r4(hw);
    // The gain-reduction bar and its readout.
    let gr = gr_level(app, ui, dev);
    let readout = gr_readout(gr);
    let value_h = crate::widgets::knob::VALUE_H;
    let bar = Rect::from_min_max(
        pos2(gr_col.center().x - GR_BAR_W / 2.0, well_y),
        pos2(gr_col.center().x + GR_BAR_W / 2.0, well_y + WELL - LABEL_GAP - value_h),
    );
    let mut c = super::child(ui, bar);
    let meter = level_meter::show(&mut c, bar.width(), bar.height(), &level_meter::Meter::gain_reduction(gr));
    let rr = widgets::role_text(ui.painter(), &tokens::VALUE, &readout, t.text, Align2::CENTER_BOTTOM, pos2(gr_col.center().x, well_y + WELL));
    lay.gr_bar = meter.well;
    lay.gr = format!("GR {readout}");
    lay.level = out.title.readout.clone();
    out.hero.gr_meter = Some(meter);
    out.hero.gr_db = gr;
    out.hero.gr_readout = readout;
    out.hero.gr_readout_rect = r4(rr);
}

/// A recorded rect back as a `Rect`.
pub(crate) fn rect_of(r: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}
