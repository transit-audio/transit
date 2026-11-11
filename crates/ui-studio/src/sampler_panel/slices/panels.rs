//! The Slices view's panels row under the bench (the board `SamplerSlices`): two equal columns a
//! panel gap apart, the left one Detect (Auto, [`super::detect_panel`]) or Manual (this file),
//! the right one Use slices ([`super::use_panel`]); both cards as tall as the taller content, the
//! left one at least `sampler.slicePanels.minHeight`. Rows are `slicePanels.rowGap` apart inside
//! the panel padding. Without a loop the whole row is drawn at the unused-control alpha and takes
//! no input (README, States: empty).

use super::detect_panel::{self, DetectLayout};
use super::edit;
use super::feedback::{self, CardRecord, LineRecord};
use super::state::{Loop, Mode};
use super::use_panel::{self, UseLayout};
use super::{SlicesView, bench};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, layout, radius, sampler as tk};
use crate::widgets::{self, r4};
use egui::{Align2, Rect, Stroke, StrokeKind, Ui, pos2, vec2};

pub const MANUAL_TEXT: &str = "Your own markers, kept apart from Auto's. Click the bench to add, drag to move, click a marker to remove. The first marker is the loop start and stays; E is a boundary, not a slice.";
pub const MANUAL_RESTING: &str = "Each edit is one undo step; Manual is never overwritten by detection.";
pub const FROM_AUTO: &str = "Start from Auto's markers";
pub const CLEAR: &str = "Clear";
/// The four gestures, each as its verb (in `text`) and the rest (in `text.faint`).
pub const GESTURES: [(&str, &str); 4] =
    [("click", " the bench · add"), ("drag", " a handle · move"), ("click", " a flag · remove"), ("click", " the strip · select")];

/// A panel's rows: a cursor down its inside.
pub struct Rows {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    first: bool,
    pub error: Option<CardRecord>,
    pub line: Option<LineRecord>,
}

impl Rows {
    pub fn new(inner: Rect) -> Rows {
        Rows { x: inner.min.x, y: inner.min.y, w: inner.width(), first: true, error: None, line: None }
    }

    /// The next row, `h` high, one row gap under the last.
    pub fn row(&mut self, h: f32) -> Rect {
        if !self.first {
            self.y += tk::PANEL_ROW_GAP;
        }
        self.first = false;
        let r = Rect::from_min_size(pos2(self.x, self.y), vec2(self.w, h));
        self.y += h;
        r
    }

    /// A row label (`label.control`, `text.muted`) at `r`'s left.
    pub fn label(&self, p: &egui::Painter, r: Rect, text: &str) -> Rect {
        widgets::role_text(p, &tokens::LABEL_CONTROL, text, TOKENS.text_muted, Align2::LEFT_CENTER, r.left_center())
    }

    /// A `line.inset` rule a row gap under the last row, the next row a row gap under it (the
    /// board's top border with its padding).
    pub fn separator(&mut self, p: &egui::Painter) {
        self.y += tk::PANEL_ROW_GAP;
        p.line_segment([pos2(self.x, self.y), pos2(self.x + self.w, self.y)], Stroke::new(1.0, TOKENS.line_inset));
        self.y += tk::PANEL_ROW_GAP;
        self.first = true;
    }

    /// The panel's error banner (the engine's words), else its Success line, else its resting
    /// line.
    pub fn feedback(&mut self, ui: &mut Ui, id: egui::Id, error: Option<&str>, success: Option<&str>, resting: Option<&str>) {
        if let Some(e) = error {
            let h = feedback::card_height(ui, e, self.w, true, None);
            let r = self.row(h);
            self.error = Some(feedback::card(ui, id, r, e, true, None).0);
        } else if let Some(text) = success.map(|s| (s, true)).or(resting.map(|s| (s, false))) {
            let r = self.row(feedback::line_height(ui));
            self.line = Some(feedback::last_edit(ui, r.min, text.0, text.1));
        }
    }
}

/// The Manual panel as drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManualLayout {
    pub notice: Option<CardRecord>,
    pub text: [f32; 4],
    pub from_auto: [f32; 4],
    pub clear: [f32; 4],
    pub gestures: [f32; 4],
}

/// The panels row as drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelsLayout {
    pub left: [f32; 4],
    pub right: [f32; 4],
    /// `detect` or `manual`.
    pub left_name: String,
    /// The row's opacity (the unused-control alpha without a loop).
    pub opacity: f32,
    pub left_label: [f32; 4],
    pub right_label: [f32; 4],
    pub detect: Option<DetectLayout>,
    pub manual: Option<ManualLayout>,
    pub uses: UseLayout,
    pub left_error: Option<CardRecord>,
    pub left_line: Option<LineRecord>,
    pub use_error: Option<CardRecord>,
    pub use_line: Option<LineRecord>,
    /// The sections row's loop row under the panels (Playback).
    pub loop_row: super::loop_row::LoopRowLayout,
}

/// Draws the panels row from `at` across `width`; returns its layout and height.
pub(crate) fn show(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    v: &mut SlicesView,
    l: Option<&Loop>,
    at: egui::Pos2,
    width: f32,
) -> (PanelsLayout, f32) {
    let t = &TOKENS;
    let mut out = PanelsLayout { opacity: 1.0, ..PanelsLayout::default() };
    let col = (width - layout::PANEL_GAP) / 2.0;
    let left = Rect::from_min_size(at, vec2(col, 0.0));
    let right = Rect::from_min_size(pos2(at.x + col + layout::PANEL_GAP, at.y), vec2(width - col - layout::PANEL_GAP, 0.0));
    let pad = vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]);
    let cards = (ui.painter().add(egui::Shape::Noop), ui.painter().add(egui::Shape::Noop));
    let (lh, rh) = ui
        .scope(|ui| {
            if l.is_none() {
                out.opacity = tk::UNUSED_CONTROL_ALPHA;
                ui.visuals_mut().disabled_alpha = tk::UNUSED_CONTROL_ALPHA;
                ui.disable();
            }
            let empty = Loop::empty();
            let lp = l.unwrap_or(&empty);
            let p = ui.painter().clone();
            // Left: Detect in Auto, Manual in Manual.
            let mut rows = Rows::new(left.shrink2(pad).with_max_y(f32::INFINITY));
            let r = rows.row(super::super::line_h(&p, &tokens::LABEL_SECTION));
            let auto = lp.mode == Mode::Auto;
            out.left_name = if auto { "detect" } else { "manual" }.into();
            out.left_label = r4(super::super::section_label(&p, r.min, if auto { "Detect" } else { "Manual" }));
            if auto {
                let mut d = DetectLayout::default();
                detect_panel::show(app, ui, dev, v, lp, &mut rows, &mut d);
                out.detect = Some(d);
            } else {
                out.manual = Some(manual(app, ui, dev, v, &mut rows));
            }
            (out.left_error, out.left_line) = (rows.error.take(), rows.line.take());
            let lh = rows.y - left.min.y + pad.y;
            // Right: Use slices.
            let mut rows = Rows::new(right.shrink2(pad).with_max_y(f32::INFINITY));
            out.right_label = use_panel::show(app, ui, dev, v, lp, &mut rows, &mut out.uses);
            (out.use_error, out.use_line) = (rows.error.take(), rows.line.take());
            (lh, rows.y - right.min.y + pad.y)
        })
        .inner;
    let h = lh.max(rh).max(tk::PANEL_MIN_HEIGHT);
    let (left, right) = (left.with_max_y(left.min.y + h), right.with_max_y(right.min.y + h));
    let fade = |c: egui::Color32| c.gamma_multiply(out.opacity);
    for (idx, r) in [(cards.0, left), (cards.1, right)] {
        ui.painter()
            .set(idx, egui::epaint::RectShape::new(r, radius::PANEL, fade(t.surface_panel), Stroke::new(1.0, fade(t.line)), StrokeKind::Inside));
    }
    (out.left, out.right) = (r4(left), r4(right));
    (out, h)
}

/// The Manual panel's rows: the Notice after an Auto gesture (with Undo), the gesture rules in a
/// sentence, `Start from Auto's markers` and `Clear`, the four gestures in Mono 11, and the last
/// edit or a refusal's error banner.
fn manual(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, rows: &mut Rows) -> ManualLayout {
    let t = &TOKENS;
    let uid = dev.uid();
    let mut out = ManualLayout::default();
    let p = ui.painter().clone();
    if let Some((text, _)) = v.feedback.notice.clone() {
        let r = rows.row(feedback::card_height(ui, &text, rows.w, false, Some("Undo")));
        let (card, undo) = feedback::card(ui, egui::Id::new(("sampler_slice_notice", uid)), r, &text, false, Some("Undo"));
        out.notice = Some(card);
        if undo {
            edit::undo(app, v);
        }
    }
    let mut job = tokens::LABEL_CONTROL.job(ui.ctx(), MANUAL_TEXT, t.text_muted);
    job.wrap.max_width = rows.w;
    let galley = p.layout_job(job);
    let r = rows.row(galley.size().y);
    out.text = r4(Rect::from_min_size(r.min, galley.size()));
    p.galley(r.min, galley, t.text_muted);
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    let fw = bench::button_w(&p, FROM_AUTO);
    let from = Rect::from_min_size(r.min, vec2(fw, r.height()));
    let clear = Rect::from_min_size(pos2(from.max.x + SPACE[1], r.min.y), vec2(bench::button_w(&p, CLEAR), r.height()));
    if bench::dense_button(ui, egui::Id::new(("sampler_slice_from_auto", uid)), from, FROM_AUTO, true).clicked() {
        edit::from_auto(app, dev, v);
    }
    if bench::dense_button(ui, egui::Id::new(("sampler_slice_clear", uid)), clear, CLEAR, true).clicked() {
        edit::clear(app, dev, v);
    }
    (out.from_auto, out.clear) = (r4(from), r4(clear));
    let r = rows.row(super::super::line_h(&p, &tokens::READOUT_DISPLAY));
    let mut x = r.min.x;
    for (verb, rest) in GESTURES {
        let a = widgets::role_text(&p, &tokens::READOUT_DISPLAY, verb, t.text, Align2::LEFT_CENTER, pos2(x, r.center().y));
        let b = widgets::role_text(&p, &tokens::READOUT_DISPLAY, rest, t.text_faint, Align2::LEFT_CENTER, pos2(a.max.x, r.center().y));
        x = b.max.x + tk::PANEL_ITEM_GAP;
    }
    out.gestures = r4(Rect::from_min_max(r.min, pos2(x - tk::PANEL_ITEM_GAP, r.max.y)));
    let error = v.feedback.left_error.clone();
    let success = v.feedback.left_success.clone();
    rows.feedback(ui, egui::Id::new(("sampler_manual_feedback", uid)), error.as_deref(), success.as_deref(), Some(MANUAL_RESTING));
    out
}
