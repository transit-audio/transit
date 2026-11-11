//! The bench (README, Sampler views, Slices view; the board `SamplerSlices`), read-only: the header
//! (`SLICES`, the mode, the count, Zoom and `SNAP 0×`), the display well with its flag strip, the
//! slices' cells and tints, the markers and the end boundary, the waveform from the source's peaks,
//! the scroll bar past 1×, the time ruler, the mode's hint and the selection line.
//!
//! Every position is the source's own frame at its native rate (`mix.sampler_slice_state`); the
//! pad and its note are the engine's. Only the zoom, the scroll and the selection change here
//! (UI state); nothing sends a command.

use super::state::{Loop, Mode, Slice};
use super::{SlicesView, viewport, wave};
use crate::mixer::scrollbar;
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, layout, radius, sampler as tk};
use crate::widgets::{self, r4, segmented, toggle};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// A color the bench names by its token.
pub(crate) fn named(name: &str) -> Color32 {
    let t = &TOKENS;
    match name {
        "surface.panel" => t.surface_panel,
        "surface.selected" => t.surface_selected,
        "surface.inset" => t.surface_inset,
        "line" => t.line,
        "line.inset" => t.line_inset,
        "line.strong" => t.line_strong,
        "text" => t.text,
        "text.muted" => t.text_muted,
        "text.faint" => t.text_faint,
        "ink.arrow" => t.ink_arrow,
        "signal" => t.signal,
        _ => Color32::MAGENTA,
    }
}

/// The gap between the bench's rows and between its header's items (the `space` steps 8 and 12).
const ROW_GAP: f32 = SPACE[1];
const ITEM_GAP: f32 = SPACE[2];

const FLAG_FONT: tokens::TypeRole = tokens::TypeRole {
    family: tokens::Family::Mono,
    weight: tk::BENCH_FLAG_FONT_WEIGHT as u16,
    size: tk::BENCH_FLAG_FONT_SIZE,
    tracking: 0.0,
    uppercase: false,
    ink: None,
};
const AXIS_FONT: tokens::TypeRole =
    tokens::TypeRole { family: tokens::Family::Mono, weight: tk::TYPE[3].2 as u16, size: tk::TYPE[3].3, tracking: 0.0, uppercase: false, ink: None };
const TAG_FONT: tokens::TypeRole = tokens::TypeRole {
    family: tokens::Family::Mono,
    weight: tk::TAG_FONT_WEIGHT as u16,
    size: tk::TAG_FONT_SIZE,
    tracking: 0.0,
    uppercase: false,
    ink: None,
};

pub const ANALYZING: &str = "Analyzing…";
pub const HINT_AUTO: &str = "Detected markers are read-only. Any edit copies the set to Manual and switches there.";
pub const HINT_MANUAL: &str = "Click adds · drag a handle moves · click a flag removes · snaps to the nearest zero crossing";
pub const SNAP: &str = "SNAP 0×";
pub const MODES: [&str; 2] = ["Auto", "Manual"];

/// One slice start as drawn: its line, its flag and (Manual, past S) its handle, with the inks.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagRecord {
    pub index: usize,
    pub frame: u64,
    pub line: [f32; 4],
    pub line_color: Color32,
    pub flag: [f32; 4],
    pub text: String,
    pub fill: Color32,
    pub ink: Color32,
    pub edge: Color32,
    pub handle: Option<[f32; 4]>,
}

/// The end boundary as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct EndRecord {
    pub line: [f32; 4],
    pub flag: [f32; 4],
    pub fill: Color32,
    pub ink: Color32,
}

/// The bench as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BenchLayout {
    pub panel: [f32; 4],
    pub label: [f32; 4],
    pub mode: Option<segmented::SegmentedLayout>,
    /// `8 of 64 slices`.
    pub count: String,
    pub count_rect: [f32; 4],
    pub zoom_label: [f32; 4],
    pub zoom: Option<segmented::SegmentedLayout>,
    pub snap: Option<toggle::ToggleLayout>,
    pub well: [f32; 4],
    pub strip: [f32; 4],
    /// The frames the well shows (first, last).
    pub view: (f64, f64),
    pub flags: Vec<FlagRecord>,
    pub end: Option<EndRecord>,
    /// Each visible slice's cell in the flag strip (a click selects it).
    pub cells: Vec<(usize, [f32; 4])>,
    /// Each visible slice's tint under the strip and its color (none: no tint).
    pub bands: Vec<(usize, [f32; 4], Option<Color32>)>,
    /// Waveform columns drawn, how many in the selected slice, and their inks.
    pub columns: usize,
    pub selected_columns: usize,
    pub wave_ink: Color32,
    pub selected_wave_ink: Color32,
    pub analyzing: Option<[f32; 4]>,
    /// The scroll bar's track and handle past 1×.
    pub scroll: Option<([f32; 4], [f32; 4])>,
    pub ruler: [f32; 4],
    pub ticks: Vec<(String, [f32; 4])>,
    pub hint: String,
    pub hint_rect: [f32; 4],
    /// `Slice 5 · 1.380–1.725 s · pad B5 · E3`.
    pub selection: String,
    pub selection_rect: [f32; 4],
    /// The frame under the pointer and its readout in milliseconds from S.
    pub pointer: Option<(u64, f64)>,
}

fn line_h(p: &egui::Painter, role: &tokens::TypeRole) -> f32 {
    widgets::role_size(p, role, "Ag").y
}

/// The hint row's height: the taller of its two texts.
fn hint_h(p: &egui::Painter) -> f32 {
    line_h(p, &tokens::LABEL_CONTROL).max(line_h(p, &tokens::VALUE))
}

/// The bench panel's height.
pub fn height(p: &egui::Painter) -> f32 {
    2.0 * layout::PANEL_PADDING[0]
        + layout::DENSE_CONTROL_HEIGHT
        + ROW_GAP
        + tk::BENCH_WELL_HEIGHT
        + ROW_GAP
        + tk::BENCH_RULER_HEIGHT
        + ROW_GAP
        + hint_h(p)
}

/// A segmented control's width with each segment at its text plus the segment padding.
fn segmented_w(p: &egui::Painter, options: &[&str]) -> f32 {
    options.iter().map(|o| widgets::role_size(p, &tokens::VALUE, o).x + 2.0 * segmented::SEGMENT_PAD).sum::<f32>() + 2.0 * segmented::PADDING
}

fn segmented_at(ui: &mut Ui, id: egui::Id, at: Pos2, options: &[&str], selected: usize) -> segmented::SegmentedOut {
    let r = Rect::from_min_size(at, vec2(segmented_w(ui.painter(), options), segmented::HEIGHT));
    ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| segmented::show(ui, id, options, selected, None)).inner
}

/// The selection line: id, times from S, pad and its note (the one namer, in the Middle C setting).
pub fn selection_line(l: &Loop, index: usize, s: &Slice, mc: transit_midi::MiddleC) -> String {
    let mut text = format!("Slice {} · {:.3}–{:.3} s", index + 1, l.seconds(s.frame as f64), l.seconds(s.end as f64));
    if let Some(pad) = s.pad {
        text.push_str(&format!(" · pad {}", super::super::pad_id(pad.saturating_sub(1))));
    }
    if let Some(key) = s.key {
        text.push_str(&format!(" · {}", transit_midi::note_name(key, mc)));
    }
    text
}

/// Draws the bench into `rect` (the panel card) for instance `uid`.
pub(crate) fn show(
    ui: &mut Ui,
    uid: u64,
    view: &mut SlicesView,
    l: Option<&Loop>,
    audio: Option<&transit_model::SourceAudio>,
    mc: transit_midi::MiddleC,
    rect: Rect,
) -> BenchLayout {
    let t = &TOKENS;
    let mut out = BenchLayout { panel: r4(rect), ..BenchLayout::default() };
    let p = ui.painter().clone();
    p.rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]));
    let row = Rect::from_min_size(inner.min, vec2(inner.width(), layout::DENSE_CONTROL_HEIGHT));
    let cy = row.center().y;
    // Header, left: SLICES, the mode (display only: switching sends a document change), the count.
    out.label = r4(widgets::role_text(&p, &tokens::LABEL_SECTION, "Slices", t.text_muted, Align2::LEFT_CENTER, pos2(row.min.x, cy)));
    let mode = l.map_or(Mode::Auto, |l| l.mode);
    let seg_top = cy - segmented::HEIGHT / 2.0;
    let m = segmented_at(ui, egui::Id::new(("sampler_slice_mode", uid)), pos2(out.label[2] + ITEM_GAP, seg_top), &MODES, mode.index());
    let count = l.map_or(0, |l| l.showing().len());
    out.count = format!("{count} of {} slices", transit_model::sampler_patch::MAX_SLICES);
    out.count_rect = r4(widgets::role_text(&p, &tokens::VALUE, &out.count, t.text, Align2::LEFT_CENTER, pos2(m.layout.track[2] + ITEM_GAP, cy)));
    out.mode = Some(m.layout);
    // Header, right: Zoom and its levels (UI state), then SNAP 0× (display only; unavailable in
    // Auto, where it shapes nothing).
    let snap_w = toggle::width(ui, SNAP);
    let snap_rect = Rect::from_min_size(pos2(row.max.x - snap_w, cy - tokens::toggle::HEIGHT / 2.0), vec2(snap_w, tokens::toggle::HEIGHT));
    out.snap = Some(toggle::paint(&p, snap_rect, SNAP, toggle::Role::Option, l.is_some_and(|l| l.snap), mode == Mode::Manual));
    let zooms: Vec<String> = tk::BENCH_ZOOMS.iter().map(|z| format!("{z}×")).collect();
    let zoom_opts: Vec<&str> = zooms.iter().map(String::as_str).collect();
    let zoom_x = snap_rect.min.x - ITEM_GAP - segmented_w(&p, &zoom_opts);
    let z = segmented_at(ui, egui::Id::new(("sampler_slice_zoom", uid)), pos2(zoom_x, seg_top), &zoom_opts, view.viewport.zoom);
    if let Some(i) = z.picked {
        view.viewport.zoom_to(i);
    }
    out.zoom_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Zoom", t.text_muted, Align2::RIGHT_CENTER, pos2(zoom_x - ITEM_GAP, cy)));
    out.zoom = Some(z.layout);
    // The well.
    let well = Rect::from_min_size(pos2(inner.min.x, row.max.y + ROW_GAP), vec2(inner.width(), tk::BENCH_WELL_HEIGHT));
    out.well = r4(well);
    p.rect(well, radius::DISPLAY, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_inset), StrokeKind::Inside);
    let area = well.shrink(chrome::FIELD_EDGE_WIDTH);
    let strip = Rect::from_min_size(area.min, vec2(area.width(), tk::BENCH_FLAG_STRIP));
    out.strip = r4(strip);
    let inner_radius = (radius::DISPLAY - chrome::FIELD_EDGE_WIDTH) as u8;
    p.rect_filled(strip, CornerRadius { nw: inner_radius, ne: inner_radius, sw: 0, se: 0 }, t.bg_titlebar);
    let wp = p.with_clip_rect(area);
    let body = Rect::from_min_max(pos2(area.min.x, strip.max.y), area.max);
    if let Some(l) = l {
        draw_loop(ui, &wp, uid, view, l, audio, area, strip, body, &mut out);
    }
    wp.line_segment([pos2(strip.min.x, strip.max.y - 0.5), pos2(strip.max.x, strip.max.y - 0.5)], Stroke::new(1.0, named(tk::BENCH_FLAG_STRIP_LINE)));
    // The ruler, the hint and the selection line.
    let ruler = Rect::from_min_size(pos2(inner.min.x, well.max.y + ROW_GAP), vec2(inner.width(), tk::BENCH_RULER_HEIGHT));
    out.ruler = r4(ruler);
    if let Some(l) = l {
        ticks(&p, view, l, area, ruler, &mut out);
    }
    let hint_row = Rect::from_min_size(pos2(inner.min.x, ruler.max.y + ROW_GAP), vec2(inner.width(), hint_h(&p)));
    out.hint = if mode == Mode::Auto { HINT_AUTO } else { HINT_MANUAL }.to_string();
    out.hint_rect = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, &out.hint, t.text_muted, Align2::LEFT_CENTER, hint_row.left_center()));
    if let Some((l, (i, s))) = l.and_then(|l| l.showing().get(view.selected).map(|s| (l, (view.selected, s)))) {
        out.selection = selection_line(l, i, s, mc);
        out.selection_rect = r4(widgets::role_text(&p, &tokens::VALUE, &out.selection, t.text, Align2::RIGHT_CENTER, hint_row.right_center()));
    }
    out
}

/// The view's x of `frame` across the well's inside.
fn x(view: &SlicesView, l: &Loop, area: Rect, frame: f64) -> f32 {
    view.viewport.x_of(l, frame, area.min.x, area.width())
}

#[allow(clippy::too_many_arguments)]
fn draw_loop(
    ui: &mut Ui,
    wp: &egui::Painter,
    uid: u64,
    view: &mut SlicesView,
    l: &Loop,
    audio: Option<&transit_model::SourceAudio>,
    area: Rect,
    strip: Rect,
    body: Rect,
    out: &mut BenchLayout,
) {
    let t = &TOKENS;
    let set = l.showing();
    view.selected = view.selected.min(set.len().saturating_sub(1));
    let manual = l.mode == Mode::Manual;
    out.view = view.viewport.range(l);
    let visible = |a: f32, b: f32| b > area.min.x && a < area.max.x;
    // The slices' tints: alternate slices `text` at 3 percent, the selected one `signal` at 12.
    for (i, s) in set.iter().enumerate() {
        let (a, b) = (x(view, l, area, s.frame as f64), x(view, l, area, s.end as f64));
        if !visible(a, b) {
            continue;
        }
        let r = Rect::from_min_max(pos2(a, body.min.y), pos2(b, body.max.y));
        let tint = if i == view.selected {
            Some(t.signal.gamma_multiply(tk::BENCH_SELECTED_ALPHA))
        } else if i % 2 == 1 {
            Some(t.text.gamma_multiply(tk::BENCH_ALT_SLICE_ALPHA))
        } else {
            None
        };
        if let Some(c) = tint {
            wp.rect_filled(r, 0.0, c);
        }
        out.bands.push((i, r4(r), tint));
    }
    // The waveform: one column per physical pixel, the selected slice `signal` at 60 percent,
    // the rest `text.faint` at 45 percent; the midline over it.
    let ppp = ui.ctx().pixels_per_point();
    let cols = (body.width() * ppp).ceil().max(1.0) as usize;
    let (a, b) = out.view;
    out.wave_ink = named(tk::BENCH_WAVE).gamma_multiply(tk::BENCH_WAVE_ALPHA);
    out.selected_wave_ink = t.signal.gamma_multiply(tk::BENCH_SELECTED_WAVE_ALPHA);
    let sel = set.get(view.selected).map(|s| (s.frame as f64, s.end as f64));
    match wave::columns(audio, a, b, cols) {
        Some(spans) => {
            let (cw, half, cy) = (1.0 / ppp, body.height() / 2.0, body.center().y);
            let step = (b - a) / cols as f64;
            for (c, span) in spans.into_iter().enumerate() {
                let Some((lo, hi)) = span else { continue };
                let mid = a + step * (c as f64 + 0.5);
                let selected = sel.is_some_and(|(s, e)| mid >= s && mid < e);
                let x0 = body.min.x + c as f32 * cw;
                let (top, bot) = (cy - hi.clamp(-1.0, 1.0) * half, cy - lo.clamp(-1.0, 1.0) * half);
                let r = Rect::from_min_max(pos2(x0, top.min(bot - cw)), pos2(x0 + cw, bot.max(top + cw)));
                wp.rect_filled(r, 0.0, if selected { out.selected_wave_ink } else { out.wave_ink });
                out.columns += 1;
                out.selected_columns += usize::from(selected);
            }
        }
        None if l.source.is_some() => {
            let size = widgets::role_size(wp, &TAG_FONT, ANALYZING) + vec2(2.0 * tk::TAG_PADDING_X, 0.0);
            let tag = Rect::from_min_size(pos2(body.max.x - tk::TAG_INSET - size.x, body.min.y + tk::TAG_INSET), size);
            wp.rect(tag, tk::TAG_RADIUS, Color32::TRANSPARENT, Stroke::new(chrome::FIELD_EDGE_WIDTH, named(tk::TAG_EDGE)), StrokeKind::Inside);
            widgets::role_text(wp, &TAG_FONT, ANALYZING, named(tk::TAG_INK), Align2::CENTER_CENTER, tag.center());
            out.analyzing = Some(r4(tag));
        }
        None => {}
    }
    wp.line_segment([pos2(body.min.x, body.center().y), pos2(body.max.x, body.center().y)], Stroke::new(1.0, named(tk::BENCH_MIDLINE)));
    // The end boundary: a 2 px line with its own flag, right-aligned to the line.
    let ex = x(view, l, area, l.end as f64);
    let end_line = Rect::from_min_max(pos2(ex - tk::BENCH_END_WIDTH, area.min.y), pos2(ex, area.max.y));
    let mut end_left = area.max.x;
    if visible(end_line.min.x, end_line.max.x + 1.0) {
        wp.rect_filled(end_line, 0.0, named(tk::BENCH_END));
        let w = widgets::role_size(wp, &FLAG_FONT, "E").x + 2.0 * tk::BENCH_FLAG_PADDING_X;
        let flag = Rect::from_min_size(pos2(end_line.min.x - w, strip.min.y + tk::BENCH_FLAG_INSET), vec2(w, tk::BENCH_FLAG_HEIGHT));
        let r = tk::BENCH_END_FLAG_RADIUS.map(|v| v as u8);
        let (fill, ink) = (named(tk::BENCH_END_FLAG[0]), named(tk::BENCH_END_FLAG[1]));
        wp.rect_filled(flag, CornerRadius { nw: r[0], ne: r[1], se: r[2], sw: r[3] }, fill);
        widgets::role_text(wp, &FLAG_FONT, "E", ink, Align2::CENTER_CENTER, flag.center());
        out.end = Some(EndRecord { line: r4(end_line), flag: r4(flag), fill, ink });
        end_left = flag.min.x;
    }
    // The markers: lines, then flags over them, then the Manual handles.
    let (auto, manual_ink, selected) = (tk::BENCH_AUTO_FLAG, tk::BENCH_MANUAL_FLAG, tk::BENCH_SELECTED_FLAG);
    let mut flag_rights = Vec::with_capacity(set.len());
    for (i, s) in set.iter().enumerate() {
        let lx = x(view, l, area, s.frame as f64);
        let is_sel = i == view.selected;
        let line_color = named(if is_sel {
            tk::BENCH_SELECTED_LINE
        } else if manual {
            tk::BENCH_MANUAL_LINE
        } else {
            tk::BENCH_AUTO_LINE
        });
        let text = if i == 0 { "S 1".to_string() } else { (i + 1).to_string() };
        let w = (widgets::role_size(wp, &FLAG_FONT, &text).x + 2.0 * (tk::BENCH_FLAG_PADDING_X + tk::BENCH_FLAG_EDGE_WIDTH))
            .max(tk::BENCH_FLAG_MIN_WIDTH);
        let flag = Rect::from_min_size(pos2(lx, strip.min.y + tk::BENCH_FLAG_INSET), vec2(w, tk::BENCH_FLAG_HEIGHT));
        flag_rights.push(flag.max.x);
        if !visible(lx, flag.max.x) {
            continue;
        }
        let line = Rect::from_min_max(pos2(lx, area.min.y), pos2(lx + 1.0, area.max.y));
        wp.rect_filled(line, 0.0, line_color);
        let names = if is_sel {
            selected
        } else if manual {
            manual_ink
        } else {
            auto
        };
        let (fill, ink, edge) = (named(names[0]), named(names[1]), named(names[2]));
        let r = tk::BENCH_FLAG_RADIUS.map(|v| v as u8);
        wp.rect(
            flag,
            CornerRadius { nw: r[0], ne: r[1], se: r[2], sw: r[3] },
            fill,
            Stroke::new(tk::BENCH_FLAG_EDGE_WIDTH, edge),
            StrokeKind::Inside,
        );
        let text_at = pos2(flag.min.x + tk::BENCH_FLAG_EDGE_WIDTH + tk::BENCH_FLAG_PADDING_X, flag.center().y);
        widgets::role_text(wp, &FLAG_FONT, &text, ink, Align2::LEFT_CENTER, text_at);
        let handle = (manual && i > 0).then(|| {
            let d = tk::BENCH_HANDLE;
            let h = Rect::from_center_size(pos2(lx + 0.5, area.max.y - tk::BENCH_HANDLE_INSET - d / 2.0), vec2(d, d));
            let stroke = Stroke::new(tk::BENCH_HANDLE_STROKE, named(tk::BENCH_HANDLE_LINE));
            wp.circle(h.center(), (d - tk::BENCH_HANDLE_STROKE) / 2.0, named(tk::BENCH_HANDLE_FILL), stroke);
            r4(h)
        });
        out.flags.push(FlagRecord { index: i, frame: s.frame, line: r4(line), line_color, flag: r4(flag), text, fill, ink, edge, handle });
    }
    // The cells: from a slice's flag to the next boundary; a click selects the slice.
    for (i, s) in set.iter().enumerate() {
        let next = set.get(i + 1).map_or(end_left, |n| x(view, l, area, n.frame as f64));
        let left = flag_rights[i].max(area.min.x);
        let right = next.min(area.max.x);
        if right <= left {
            continue;
        }
        let cell = Rect::from_min_max(pos2(left, strip.min.y), pos2(right, strip.max.y));
        let resp = ui.interact(cell, egui::Id::new(("sampler_slice_cell", uid, i)), Sense::click());
        if resp.clicked() {
            view.selected = i;
            view.viewport.select(s.frame, s.end);
        }
        out.cells.push((i, r4(cell)));
    }
    // The pointer readout: milliseconds from S at the frame under the pointer.
    let hover = ui.interact(area, egui::Id::new(("sampler_slice_well", uid)), Sense::hover());
    if let Some(at) = hover.hover_pos() {
        let frame = view.viewport.frame_at(l, at.x, area.min.x, area.width());
        let ms = viewport::ms_from_start(l, frame);
        out.pointer = Some((frame, ms));
        hover.on_hover_text(format!("{ms:.1} ms"));
    }
    scroll(ui, wp, uid, view, l, area, out);
}

/// Past 1×, the chrome's horizontal scroll bar inside the well's foot (Scroll bars: inset 2).
fn scroll(ui: &mut Ui, wp: &egui::Painter, uid: u64, view: &mut SlicesView, l: &Loop, area: Rect, out: &mut BenchLayout) {
    let factor = view.viewport.factor();
    if factor <= 1.0 {
        return;
    }
    let bottom = area.max.y - chrome::SCROLL_INSET;
    let bar = Rect::from_min_max(pos2(area.min.x, bottom - chrome::SCROLL_HOVER), pos2(area.max.x, bottom));
    let resp = ui.interact(bar, egui::Id::new(("sampler_slice_scroll", uid)), Sense::drag());
    let content = area.width() * factor;
    let max = content - area.width();
    let (track, handle) = scrollbar::handle_len(area.width(), content);
    let (a, _) = view.viewport.range(l);
    let mut offset = ((a - l.start as f64) / l.frames().max(1.0) * f64::from(content)) as f32;
    if resp.dragged() {
        offset = scrollbar::dragged_offset(offset, resp.drag_delta().x, max, track, handle);
        let start = l.start as f64 + f64::from(offset / content) * l.frames();
        view.viewport.scroll_to(l, start);
    }
    let (hovered, dragging) = (resp.hovered() || resp.dragged(), resp.dragged());
    let left = area.min.x + scrollbar::handle_start(track, handle, offset, max);
    let w = scrollbar::thickness(hovered, dragging);
    let thumb = Rect::from_min_max(pos2(left, bottom - w), pos2(left + handle, bottom));
    scrollbar::paint(wp, hovered.then_some(bar), thumb, scrollbar::ink(hovered, dragging));
    out.scroll = Some((r4(bar), r4(thumb)));
}

/// The time ruler under the well: seconds from S each step (finer from zoom 4), a label left at
/// its tick, skipped where it would run past the ruler or into the label before it.
fn ticks(p: &egui::Painter, view: &SlicesView, l: &Loop, area: Rect, ruler: Rect, out: &mut BenchLayout) {
    let step = f64::from(if view.viewport.factor() >= tk::BENCH_RULER_FINE_FROM_ZOOM { tk::BENCH_RULER_FINE_STEP } else { tk::BENCH_RULER_STEP });
    let length = l.seconds(l.end as f64);
    let mut last_right = f32::NEG_INFINITY;
    let mut k = 0u32;
    loop {
        let s = f64::from(k) * step;
        if s > length + 1e-9 {
            break;
        }
        k += 1;
        let tx = x(view, l, area, l.start as f64 + s * f64::from(l.rate));
        let label = format!("{s:.1} s");
        let w = widgets::role_size(p, &AXIS_FONT, &label).x;
        if tx < ruler.min.x || tx + w > ruler.max.x || tx < last_right {
            continue;
        }
        let r = widgets::role_text(p, &AXIS_FONT, &label, TOKENS.text_faint, Align2::LEFT_TOP, pos2(tx, ruler.min.y));
        last_right = r.max.x;
        out.ticks.push((label, r4(r)));
    }
}
