//! The segmented control (`docs/design/language`, Components > Segmented control): a
//! `surface.inset` track with a 1 px `line` and radius 6 and 2 px padding; the selected segment is
//! `surface.selected` with `text`, the others transparent with `text.faint`. Text for modes
//! (LP/BP/HP); icon segments are the caller's (a later addition).

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{self, layout, radius};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

pub const PADDING: f32 = 2.0;
/// Horizontal room around a segment's text.
pub const SEGMENT_PAD: f32 = 10.0;
/// The track is the larger segment height of the design.
pub const HEIGHT: f32 = layout::SEGMENT_HEIGHT[1];

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SegmentedLayout {
    pub track: [f32; 4],
    pub segments: Vec<[f32; 4]>,
    pub selected: usize,
    #[serde(skip)]
    pub inks: Vec<Color32>,
    #[serde(skip)]
    pub selected_fill: Color32,
}

pub struct SegmentedOut {
    pub layout: SegmentedLayout,
    /// The segment the user clicked, when it is not the selected one.
    pub picked: Option<usize>,
}

/// Draws `options` with `selected` highlighted; `width` spreads the segments evenly (`None`: each
/// segment fits its text).
pub fn show(ui: &mut Ui, id: egui::Id, options: &[&str], selected: usize, width: Option<f32>) -> SegmentedOut {
    let t = &TOKENS;
    let role = tokens::VALUE;
    let natural: Vec<f32> = options.iter().map(|o| super::role_size(ui.painter(), &role, o).x + 2.0 * SEGMENT_PAD).collect();
    let total = width.unwrap_or(natural.iter().sum::<f32>() + 2.0 * PADDING);
    let (track, _) = ui.allocate_exact_size(vec2(total, HEIGHT), Sense::hover());
    let inner = track.shrink(PADDING);
    let even = width.is_some();
    let n = options.len().max(1) as f32;
    let mut x = inner.min.x;
    let mut segments = Vec::new();
    let mut inks = Vec::new();
    let mut picked = None;
    let p = ui.painter().clone();
    p.rect_filled(track, radius::DISPLAY, t.surface_inset);
    p.rect_stroke(track, radius::DISPLAY, Stroke::new(1.0, t.line), StrokeKind::Inside);
    for (i, o) in options.iter().enumerate() {
        let w = if even { inner.width() / n } else { natural[i] };
        let seg = Rect::from_min_size(pos2(x, inner.min.y), vec2(w, inner.height()));
        x += w;
        let r = ui.interact(seg, id.with(i), Sense::click());
        if r.clicked() && i != selected {
            picked = Some(i);
        }
        let ink = if i == selected { t.text } else { t.text_faint };
        if i == selected {
            p.rect_filled(seg, radius::SEGMENT, t.surface_selected);
        }
        super::role_text(&p, &role, o, ink, Align2::CENTER_CENTER, seg.center());
        segments.push(r4(seg));
        inks.push(ink);
    }
    SegmentedOut { layout: SegmentedLayout { track: r4(track), segments, selected, inks, selected_fill: t.surface_selected }, picked }
}
