//! The History well (README, Compressor displays): the last 4 s of the published detector input,
//! output peak and gain reduction, newest at the right edge, scrolling left.
//!
//! The ring ([`History`]) lives in the editor's state per instance and takes one sample each frame
//! while the instance has a scope tap. It is bounded ([`HISTORY_CAP`] samples, preallocated when
//! the window opens), so a fast frame rate shortens the window it holds instead of growing it. The
//! well draws from it with no allocation of its own beyond egui's shapes.
//!
//! Drawn per the `compressor.history` tokens: vertical grid lines each second and horizontal ones
//! at the quarters in `grid`; the input as an area from the bottom in `ink.ghost` at 35 percent;
//! the output as a 1.75 px `signal` line; the gain reduction as a band hanging from the top in
//! `mod` at 22 percent with a 1.75 px `mod` line at its lower edge, on a 0 to 24 dB scale; the
//! threshold as a 1 px dashed (4 4) line in `text` at 45 percent on the level scale (−60 to 0 dB);
//! `−4 s` and `now` in the bottom corners; the `IN · OUT · GR` legend in the label row.

use super::hero::scale;
use super::{Dev, named};
use crate::StudioApp;
use crate::theme::TOKENS;
use crate::tokens::{self, compressor as tok};
use crate::widgets::{self, r4};
use egui::{Align2, Color32, CornerRadius, Mesh, Pos2, Rect, Shape, Stroke, Ui, pos2, vec2};

/// Samples a ring holds: 4 s at up to 256 frames a second.
pub const HISTORY_CAP: usize = 1024;
/// The time labels' role: Mono 400 10 in `text.faint` (README; the design names no role for it).
pub const TIME_LABEL: tokens::TypeRole = tokens::TypeRole { size: 10.0, ..tokens::READOUT_DISPLAY };
/// The time labels' inset from the well's inside edge (the board's 12 across, 10 up).
pub const TIME_LABEL_INSET: [f32; 2] = [12.0, 10.0];
/// The legend (the board's): 8 px swatches with a 2 px radius, the `OUT` stroke 10 by 2 with a
/// 1 px radius, 4 between a swatch and its label, 10 between items.
pub const SWATCH: f32 = 8.0;
pub const SWATCH_RADIUS: f32 = 2.0;
pub const STROKE_SWATCH: [f32; 2] = [10.0, 2.0];
pub const LEGEND_INNER_GAP: f32 = 4.0;
pub const LEGEND_GAP: f32 = 10.0;

/// One frame's reading: when (the UI's clock, seconds) and the three levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub time: f64,
    /// The detector's input level in dB (the fader's floor while none is published).
    pub input_db: f32,
    /// The output's newest peak in dB.
    pub output_db: f32,
    /// The gain reduction in dB (at least 0).
    pub gr_db: f32,
}

/// The ring of one instance: at most [`HISTORY_CAP`] samples, oldest first.
#[derive(Debug, Clone)]
pub struct History {
    buf: Vec<Sample>,
    /// Where the oldest sample sits once the ring is full.
    head: usize,
}

impl Default for History {
    fn default() -> Self {
        History { buf: Vec::with_capacity(HISTORY_CAP), head: 0 }
    }
}

impl History {
    /// Appends a sample; a full ring drops its oldest. A sample no newer than the newest (a second
    /// pass of the same frame) is ignored.
    pub fn push(&mut self, s: Sample) {
        if self.newest().is_some_and(|n| s.time <= n.time) {
            return;
        }
        if self.buf.len() < HISTORY_CAP {
            self.buf.push(s);
        } else if let Some(slot) = self.buf.get_mut(self.head) {
            *slot = s;
            self.head = (self.head + 1) % HISTORY_CAP;
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The samples, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        let (a, b) = self.buf.split_at(self.head.min(self.buf.len()));
        b.iter().chain(a)
    }

    fn newest(&self) -> Option<&Sample> {
        if self.buf.len() < HISTORY_CAP { self.buf.last() } else { self.buf.get((self.head + HISTORY_CAP - 1) % HISTORY_CAP) }
    }
}

/// A legend item as drawn: its label, the swatch's rect and color, and the label's rect.
#[derive(Debug, Clone, Default)]
pub struct LegendItem {
    pub label: String,
    pub swatch: [f32; 4],
    pub color: Color32,
    pub text: [f32; 4],
}

/// The history well's traces as last drawn.
#[derive(Debug, Clone, Default)]
pub struct HistoryRecord {
    /// Samples inside the window that were drawn.
    pub samples: usize,
    /// The bounds of the input area, the output line and the gain-reduction band (`None` with
    /// fewer than two samples).
    pub input_area: Option<[f32; 4]>,
    pub output_line: Option<[f32; 4]>,
    pub gr_band: Option<[f32; 4]>,
    pub input_fill: Color32,
    pub output_stroke: Stroke,
    pub gr_fill: Color32,
    pub gr_stroke: Stroke,
    /// The threshold line (from x, y, to x, y), its level in dB, its stroke and dash.
    pub threshold: [f32; 4],
    pub threshold_db: f32,
    pub threshold_stroke: Stroke,
    pub threshold_dash: [f32; 2],
    /// `−4 s` and `now` and where they sit.
    pub time_labels: Vec<(String, [f32; 4])>,
    pub legend: Vec<LegendItem>,
}

/// The vertical grid lines, one each second inside the window, in `grid`; returns them.
pub(crate) fn second_lines(ui: &Ui, plot: Rect) -> Vec<[f32; 4]> {
    let stroke = Stroke::new(1.0, named(tok::HISTORY_SECOND_LINES));
    let secs = tok::HISTORY_WINDOW_SECONDS as usize;
    (1..secs)
        .map(|k| {
            let x = plot.min.x + k as f32 / secs as f32 * plot.width();
            ui.painter().line_segment([pos2(x, plot.min.y), pos2(x, plot.max.y)], stroke);
            [x, plot.min.y, x, plot.max.y]
        })
        .collect()
}

/// Takes this frame's sample into the instance's ring while it has a scope tap.
pub(crate) fn record(app: &mut StudioApp, ui: &Ui, dev: Dev, gr_db: f32) {
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let Some(hub) = app.scope_hub() else { return };
    if hub.get(dev.key.0, uid).is_none() {
        return;
    }
    let floor = crate::widgets::fader::FLOOR_DB;
    let input_db = hub.detector_level_db(dev.key.0, uid).unwrap_or(floor).max(floor);
    let mut frames = std::mem::take(&mut app.builtin_editor.frames);
    frames.resize(transit_mix::SCOPE_FRAMES, 0.0);
    hub.frames(dev.key.0, uid, &mut frames);
    let output_db = crate::device_editor::title_bar::newest_peak_db(&frames);
    app.builtin_editor.frames = frames;
    let time = ui.input(|i| i.time);
    app.builtin_editor.devices.history.entry(dev.uid()).or_default().push(Sample { time, input_db, output_db, gr_db });
}

/// Draws the traces, the threshold and the time labels into the well `well` (plot `plot`) from
/// the instance's ring, and records them.
pub(crate) fn show(app: &StudioApp, ui: &Ui, dev: Dev, well: Rect, plot: Rect, out: &mut HistoryRecord) {
    let t = &TOKENS;
    let p = ui.painter();
    let now = ui.input(|i| i.time);
    let window = f64::from(tok::HISTORY_WINDOW_SECONDS);
    let x_of = |time: f64| plot.max.x - ((now - time) / window) as f32 * plot.width();
    let level_y = |db: f32| plot.max.y - scale(db) * plot.height();
    let gr_y = |gr: f32| plot.min.y + (gr / tok::HISTORY_GR_RANGE_DB).clamp(0.0, 1.0) * plot.height();
    out.input_fill = named(tok::HISTORY_INPUT_FILL).gamma_multiply(tok::HISTORY_INPUT_ALPHA);
    out.output_stroke = Stroke::new(tok::HISTORY_OUTPUT_WIDTH, named(tok::HISTORY_OUTPUT_STROKE));
    out.gr_fill = named(tok::HISTORY_GR_FILL).gamma_multiply(tok::HISTORY_GR_ALPHA);
    out.gr_stroke = Stroke::new(tok::HISTORY_GR_WIDTH, named(tok::HISTORY_GR_STROKE));
    if let Some(h) = app.builtin_editor.devices.history.get(&dev.uid()) {
        let shown = || h.iter().filter(|s| now - s.time <= window && s.time <= now);
        let n = shown().count();
        out.samples = n;
        if n >= 2 {
            let mut input = Mesh::default();
            let mut gr = Mesh::default();
            let mut output = Vec::with_capacity(n);
            let mut gr_line = Vec::with_capacity(n);
            let mut prev: Option<(f32, f32, f32)> = None;
            let (mut ib, mut ob, mut gb) = (Bounds::default(), Bounds::default(), Bounds::default());
            for s in shown() {
                let x = x_of(s.time).clamp(plot.min.x, plot.max.x);
                let (yi, yo, yg) = (level_y(s.input_db), level_y(s.output_db), gr_y(s.gr_db));
                if let Some((px, pyi, pyg)) = prev {
                    quad(&mut input, [pos2(px, pyi), pos2(x, yi), pos2(x, plot.max.y), pos2(px, plot.max.y)], out.input_fill);
                    quad(&mut gr, [pos2(px, plot.min.y), pos2(x, plot.min.y), pos2(x, yg), pos2(px, pyg)], out.gr_fill);
                }
                ib.add(pos2(x, yi));
                ib.add(pos2(x, plot.max.y));
                ob.add(pos2(x, yo));
                gb.add(pos2(x, yg));
                gb.add(pos2(x, plot.min.y));
                output.push(pos2(x, yo));
                gr_line.push(pos2(x, yg));
                prev = Some((x, yi, yg));
            }
            // The board's layers: the input area, the output line, the band and its edge.
            p.add(Shape::mesh(input));
            p.add(Shape::line(output, out.output_stroke));
            p.add(Shape::mesh(gr));
            p.add(Shape::line(gr_line, out.gr_stroke));
            (out.input_area, out.output_line, out.gr_band) = (ib.rect(), ob.rect(), gb.rect());
        }
    }
    // The threshold, dashed, across the well at the current threshold on the level scale.
    let th = dev.value(app, "threshold");
    let y = level_y(th);
    let stroke = Stroke::new(tok::HISTORY_THRESHOLD_WIDTH, named(tok::HISTORY_THRESHOLD_COLOR).gamma_multiply(tok::HISTORY_THRESHOLD_ALPHA));
    let [dash, gap] = tok::HISTORY_THRESHOLD_DASH;
    p.extend(Shape::dashed_line(&[pos2(plot.min.x, y), pos2(plot.max.x, y)], stroke, dash, gap));
    (out.threshold, out.threshold_db, out.threshold_stroke, out.threshold_dash) =
        ([plot.min.x, y, plot.max.x, y], th, stroke, tok::HISTORY_THRESHOLD_DASH);
    // The time window's labels in the bottom corners.
    let inside = well.shrink(1.0);
    let [ix, iy] = TIME_LABEL_INSET;
    let [old, new] = tok::HISTORY_TIME_LABELS;
    let l = widgets::role_text(p, &TIME_LABEL, old, t.text_faint, Align2::LEFT_BOTTOM, pos2(inside.min.x + ix, inside.max.y - iy));
    let r = widgets::role_text(p, &TIME_LABEL, new, t.text_faint, Align2::RIGHT_BOTTOM, pos2(inside.max.x - ix, inside.max.y - iy));
    out.time_labels = vec![(old.to_string(), r4(l)), (new.to_string(), r4(r))];
}

/// The legend, right-aligned to `right` and centered on `cy` in the label row: an `ink.ghost`
/// swatch `IN`, a `signal` stroke `OUT`, a `mod` swatch `GR`, labels Mono 400 11 `text.faint`.
pub(crate) fn legend(ui: &Ui, right: f32, cy: f32, out: &mut HistoryRecord) {
    let t = &TOKENS;
    let p = ui.painter();
    let role = &tokens::READOUT_DISPLAY;
    let items = [
        ("GR", named(tok::HISTORY_GR_FILL), vec2(SWATCH, SWATCH), SWATCH_RADIUS),
        ("OUT", named(tok::HISTORY_OUTPUT_STROKE), vec2(STROKE_SWATCH[0], STROKE_SWATCH[1]), STROKE_SWATCH[1] / 2.0),
        ("IN", named(tok::HISTORY_INPUT_FILL), vec2(SWATCH, SWATCH), SWATCH_RADIUS),
    ];
    let mut x = right;
    let mut drawn = Vec::with_capacity(items.len());
    for (label, color, size, radius) in items {
        let text = widgets::role_text(p, role, label, t.text_faint, Align2::RIGHT_CENTER, pos2(x, cy));
        let swatch = Rect::from_center_size(pos2(text.min.x - LEGEND_INNER_GAP - size.x / 2.0, cy), size);
        p.rect_filled(swatch, CornerRadius::same(radius as u8), color);
        drawn.push(LegendItem { label: label.to_string(), swatch: r4(swatch), color, text: r4(text) });
        x = swatch.min.x - LEGEND_GAP;
    }
    drawn.reverse();
    out.legend = drawn;
}

/// Adds the quad `v` (in order around it) to `mesh` in `color`.
fn quad(mesh: &mut Mesh, v: [Pos2; 4], color: Color32) {
    let base = mesh.vertices.len() as u32;
    for p in v {
        mesh.colored_vertex(p, color);
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

/// The running bounds of a trace.
#[derive(Default)]
struct Bounds(Option<Rect>);

impl Bounds {
    fn add(&mut self, p: Pos2) {
        self.0 = Some(self.0.map_or(Rect::from_min_max(p, p), |r| r.union(Rect::from_min_max(p, p))));
    }

    fn rect(&self) -> Option<[f32; 4]> {
        self.0.map(r4)
    }
}
