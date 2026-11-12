//! The bench (README, Sampler views, Slices view and its round 2 decisions; the boards
//! `SamplerSlices` and `SamplerRound2`): the header (`SLICES`, the mode, the count, Zoom and
//! `SNAP 0×`), the display well with its flag strip, the slices' cells and tints, the markers and
//! the end boundary, the waveform from the source's peaks, the scroll bar past 1×, the time ruler,
//! the mode's hint and the selection line; the gestures' previews (the ghost, a drag's line and
//! tooltip, a pending detection's dashed markers), the status tag, and the states the bench has
//! without a loop to show (empty, missing media, a refused read).
//!
//! Every position is the source's own frame at its native rate (`mix.sampler_slice_state`); the
//! pad and its note are the engine's. Zoom, scroll and selection change here (UI state); every
//! document change is an [`Intent`] the view sends once its gesture completes
//! ([`super::bench_gestures`]).

use super::bench_gestures::{self as g, Act, Intent, Target};
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
        "meter.hot" => t.meter_hot,
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
pub const STALE_TAG: &str = "keys out of order";
pub const DROP: &str = "+ Drop a loop";
pub const LOAD: &str = "Load…";
pub const RELINK: &str = "Relink…";
pub const NOT_YET: &str = "Not available yet";
pub const HINT_AUTO: &str = "Detected markers are read-only. Any edit copies the set to Manual and switches there.";
pub const HINT_MANUAL: &str = "Click plays · double-click adds · drag a handle moves · click a flag removes · snaps to zero crossings";
/// The bench's hover tooltip after the pointer's milliseconds from S.
pub const HOVER_ADD: &str = "double-click adds a marker";
pub const SNAP: &str = "SNAP 0×";
pub const MODES: [&str; 2] = ["Auto", "Manual"];

/// What the bench shows beside the state: a pending detection's markers (Auto), a stale mapping,
/// a missing source (its file name), a refused state read, and the tooltips naming the keys.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub candidate: Option<Vec<u64>>,
    pub stale: bool,
    pub missing: Option<String>,
    pub error: Option<String>,
    pub cell_tip: String,
    pub flag_tip: String,
    pub handle_tip: String,
    /// A menu, a modal or a dialog is open, or a field has the keyboard: a press on the bench
    /// plays nothing and a double-click adds nothing (as the view's keys are held).
    pub held_off: bool,
}

/// One slice start as drawn: its line, its flag (not drawn where crowded) and (Manual, past S)
/// its handle, with the inks.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagRecord {
    pub index: usize,
    pub frame: u64,
    pub line: [f32; 4],
    pub line_color: Color32,
    pub flag: [f32; 4],
    /// The flag was left out: it would run into the next boundary.
    pub crowded: bool,
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

/// The add preview as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostRecord {
    pub frame: u64,
    pub line: [f32; 4],
    pub color: Color32,
    pub label: String,
    pub flag: [f32; 4],
    /// The short tick at the raw pointer when the snap moved the ghost.
    pub raw_tick: Option<[f32; 4]>,
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
    /// The header's controls' opacity (the unused-control alpha while there is no loop).
    pub header_opacity: f32,
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
    /// Each visible looping slice's band, wash, glyph and crossfade hatch (what plays).
    pub loops: Vec<super::bench_loop::LoopMark>,
    /// Waveform columns drawn, how many in the selected slice, and their inks.
    pub columns: usize,
    pub selected_columns: usize,
    pub wave_ink: Color32,
    pub selected_wave_ink: Color32,
    /// The status tag in the well's corner (`Analyzing…`, `keys out of order`, `64 of 64 slices`).
    pub tag: Option<(String, [f32; 4])>,
    /// Each sounding voice's playhead line in view (#470), its ink, and whether the view asked
    /// for a repaint for them (only while a voice of the loop sounds).
    pub playheads: Vec<[f32; 4]>,
    pub playhead_ink: Color32,
    pub playhead_repaint: bool,
    pub analyzing: Option<[f32; 4]>,
    pub ghost: Option<GhostRecord>,
    /// A pending detection's dashed markers (the x of each, Auto only).
    pub candidate: Vec<f32>,
    /// The dragged marker's line, its tooltip and the tooltip's rect.
    pub drag: Option<([f32; 4], String, [f32; 4])>,
    /// The rename field over a cell.
    pub rename: Option<[f32; 4]>,
    /// The empty bench's drop target and its Load… button.
    pub empty: Option<([f32; 4], [f32; 4])>,
    /// Missing media: the card, its text and its Relink… button.
    pub missing: Option<([f32; 4], String, [f32; 4])>,
    /// A refused state read, in the engine's words.
    pub error: Option<(String, [f32; 4])>,
    /// The scroll bar's track and handle past 1×.
    pub scroll: Option<([f32; 4], [f32; 4])>,
    pub ruler: [f32; 4],
    pub ticks: Vec<(String, [f32; 4])>,
    /// Ruler steps visited this frame (only the visible range's).
    pub ruler_steps: usize,
    pub hint: String,
    pub hint_rect: [f32; 4],
    /// `Slice 5 · 1.380–1.725 s · pad B5 · E3`.
    pub selection: String,
    pub selection_rect: [f32; 4],
    /// The frame under the pointer and its readout in milliseconds from S.
    pub pointer: Option<(u64, f64)>,
    /// The view's text field that has the keyboard (it keeps Escape from the device window).
    pub typing: Option<egui::Id>,
}

/// Whether Escape was pressed this frame, even when the device window's Escape took the key first
/// (a text field gives its focus up on Escape before the window's keys run; the field keeps
/// Escape, and the window's handler consumes it so that it does not also close the window).
pub(crate) fn escaped(ui: &Ui) -> bool {
    ui.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Escape, pressed: true, .. })))
}

/// The rename field's id on the insert `uid`.
pub fn rename_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_slice_rename", uid))
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
pub(crate) fn segmented_w(p: &egui::Painter, options: &[&str]) -> f32 {
    options.iter().map(|o| widgets::role_size(p, &tokens::VALUE, o).x + 2.0 * segmented::SEGMENT_PAD).sum::<f32>() + 2.0 * segmented::PADDING
}

/// A segmented control at `at`; `enabled: false` draws it at the unused-control alpha, inert.
pub(crate) fn segmented_at(ui: &mut Ui, id: egui::Id, at: Pos2, options: &[&str], selected: usize, enabled: bool) -> segmented::SegmentedOut {
    let r = Rect::from_min_size(at, vec2(segmented_w(ui.painter(), options), segmented::HEIGHT));
    ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
        if !enabled {
            ui.visuals_mut().disabled_alpha = tk::UNUSED_CONTROL_ALPHA;
            ui.disable();
        }
        segmented::show(ui, id, options, selected, None)
    })
    .inner
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

/// A standard button at the dense height (`surface.raised`, `button.hover` on hover), its text in
/// the button font; `enabled: false` draws it at the unused-control alpha with no input and the
/// `Not available yet` tooltip.
pub(crate) fn dense_button(ui: &mut Ui, id: egui::Id, rect: Rect, text: &str, enabled: bool) -> egui::Response {
    dense_button_why(ui, id, rect, text, (!enabled).then_some(NOT_YET))
}

/// [`dense_button`], unavailable with `why` as its tooltip when given.
pub(crate) fn dense_button_why(ui: &mut Ui, id: egui::Id, rect: Rect, text: &str, why: Option<&str>) -> egui::Response {
    let enabled = why.is_none();
    let t = &TOKENS;
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && resp.is_pointer_button_down_on() {
        t.surface_selected
    } else if enabled && resp.hovered() {
        t.button_hover
    } else {
        t.surface_raised
    };
    let a = if enabled { 1.0 } else { tk::UNUSED_CONTROL_ALPHA };
    let p = ui.painter();
    p.rect_filled(rect, chrome::RADIUS_CONTROL, fill.gamma_multiply(a));
    widgets::role_text(p, &chrome::BUTTON_FONT, text, t.text.gamma_multiply(a), Align2::CENTER_CENTER, rect.center());
    match why {
        None => resp,
        Some(w) => resp.on_hover_text(w),
    }
}

/// A dense button's width for `text` (its text plus the button padding a side).
pub(crate) fn button_w(p: &egui::Painter, text: &str) -> f32 {
    (widgets::role_size(p, &chrome::BUTTON_FONT, text).x + 2.0 * chrome::BUTTON_PADDING[1]).ceil()
}

/// Draws the bench into `rect` (the panel card) for instance `uid`; returns what it drew and what
/// the gestures that completed this frame ask for, in order: a name being typed commits before
/// what the click that ended it asks for (a mode switch, an audition, an add, a remove).
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut Ui,
    uid: u64,
    view: &mut SlicesView,
    l: Option<&Loop>,
    audio: Option<&transit_model::SourceAudio>,
    inputs: &Inputs,
    mc: transit_midi::MiddleC,
    rect: Rect,
) -> (BenchLayout, Vec<Intent>) {
    let t = &TOKENS;
    let mut out = BenchLayout { panel: r4(rect), header_opacity: 1.0, ..BenchLayout::default() };
    let mut intent = None;
    let (mut label, mut gestures) = (None, Vec::new());
    let p = ui.painter().clone();
    p.rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]));
    let row = Rect::from_min_size(inner.min, vec2(inner.width(), layout::DENSE_CONTROL_HEIGHT));
    let cy = row.center().y;
    let live = l.is_some();
    if !live {
        out.header_opacity = tk::UNUSED_CONTROL_ALPHA;
    }
    let faded = |c: Color32| if live { c } else { c.gamma_multiply(tk::UNUSED_CONTROL_ALPHA) };
    // Header, left: SLICES, the mode (one step per switch), the count.
    out.label = r4(widgets::role_text(&p, &tokens::LABEL_SECTION, "Slices", t.text_muted, Align2::LEFT_CENTER, pos2(row.min.x, cy)));
    let mode = l.map_or(Mode::Auto, |l| l.mode);
    let seg_top = cy - segmented::HEIGHT / 2.0;
    let m = segmented_at(ui, egui::Id::new(("sampler_slice_mode", uid)), pos2(out.label[2] + ITEM_GAP, seg_top), &MODES, mode.index(), live);
    if let Some(i) = m.picked.filter(|_| live) {
        intent = Some(Intent::Mode(if i == 0 { Mode::Auto } else { Mode::Manual }));
    }
    let count = l.map_or(0, |l| l.showing().len());
    out.count = out_count(count);
    out.count_rect =
        r4(widgets::role_text(&p, &tokens::VALUE, &out.count, faded(t.text), Align2::LEFT_CENTER, pos2(m.layout.track[2] + ITEM_GAP, cy)));
    out.mode = Some(m.layout);
    // Header, right: Zoom and its levels (UI state), then SNAP 0× (one step; unavailable in Auto,
    // where it shapes nothing).
    let snap_w = toggle::width(ui, SNAP);
    let snap_rect = Rect::from_min_size(pos2(row.max.x - snap_w, cy - tokens::toggle::HEIGHT / 2.0), vec2(snap_w, tokens::toggle::HEIGHT));
    let snap_on = l.is_some_and(|l| l.snap);
    let snap_ok = live && mode == Mode::Manual;
    let snap_resp = ui.interact(snap_rect, egui::Id::new(("sampler_slice_snap", uid)), if snap_ok { Sense::click() } else { Sense::hover() });
    if snap_resp.clicked() {
        intent = Some(Intent::Snap(!snap_on));
    }
    out.snap = Some(toggle::paint(&p, snap_rect, SNAP, toggle::Role::Option, snap_on, snap_ok));
    let zooms: Vec<String> = tk::BENCH_ZOOMS.iter().map(|z| format!("{z}×")).collect();
    let zoom_opts: Vec<&str> = zooms.iter().map(String::as_str).collect();
    let zoom_x = snap_rect.min.x - ITEM_GAP - segmented_w(&p, &zoom_opts);
    let z = segmented_at(ui, egui::Id::new(("sampler_slice_zoom", uid)), pos2(zoom_x, seg_top), &zoom_opts, view.viewport.zoom, live);
    if let Some(i) = z.picked {
        view.viewport.zoom_to(i);
    }
    out.zoom_label =
        r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Zoom", faded(t.text_muted), Align2::RIGHT_CENTER, pos2(zoom_x - ITEM_GAP, cy)));
    out.zoom = Some(z.layout);
    // The well.
    let well = Rect::from_min_size(pos2(inner.min.x, row.max.y + ROW_GAP), vec2(inner.width(), tk::BENCH_WELL_HEIGHT));
    out.well = r4(well);
    let area = well.shrink(chrome::FIELD_EDGE_WIDTH);
    let strip = Rect::from_min_size(area.min, vec2(area.width(), tk::BENCH_FLAG_STRIP));
    out.strip = r4(strip);
    match l {
        Some(l) => {
            p.rect(well, radius::DISPLAY, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_inset), StrokeKind::Inside);
            let inner_radius = (radius::DISPLAY - chrome::FIELD_EDGE_WIDTH) as u8;
            p.rect_filled(strip, CornerRadius { nw: inner_radius, ne: inner_radius, sw: 0, se: 0 }, t.bg_titlebar);
            let wp = p.with_clip_rect(area);
            let body = Rect::from_min_max(pos2(area.min.x, strip.max.y), area.max);
            if let Some(name) = &inputs.missing {
                missing(ui, &wp, uid, body, name, &mut out);
            }
            (label, gestures) = draw_loop(ui, &wp, uid, view, l, audio, inputs, area, strip, body, &mut out);
            wp.line_segment(
                [pos2(strip.min.x, strip.max.y - 0.5), pos2(strip.max.x, strip.max.y - 0.5)],
                Stroke::new(1.0, named(tk::BENCH_FLAG_STRIP_LINE)),
            );
        }
        None => {
            if empty(ui, &p, uid, well, inputs, &mut out) {
                intent = Some(Intent::Load);
            }
        }
    }
    // The ruler, the hint and the selection line.
    let ruler = Rect::from_min_size(pos2(inner.min.x, well.max.y + ROW_GAP), vec2(inner.width(), tk::BENCH_RULER_HEIGHT));
    out.ruler = r4(ruler);
    if let Some(l) = l {
        ticks(&p, view, l, area, ruler, &mut out);
    }
    let hint_row = Rect::from_min_size(pos2(inner.min.x, ruler.max.y + ROW_GAP), vec2(inner.width(), hint_h(&p)));
    out.hint = if mode == Mode::Auto { HINT_AUTO } else { HINT_MANUAL }.to_string();
    if live {
        out.hint_rect = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, &out.hint, t.text_muted, Align2::LEFT_CENTER, hint_row.left_center()));
    }
    if let Some((l, (i, s))) = l.and_then(|l| l.showing().get(view.selected).map(|s| (l, (view.selected, s)))) {
        out.selection = selection_line(l, i, s, mc);
        out.selection_rect = r4(widgets::role_text(&p, &tokens::VALUE, &out.selection, t.text, Align2::RIGHT_CENTER, hint_row.right_center()));
    }
    (out, [label, intent].into_iter().flatten().chain(gestures).collect())
}

/// No loop: the well is a dashed `line.strong` drop target with `+ Drop a loop` and Load…, or,
/// when the state read was refused, the engine's words in the error pattern. Returns whether
/// Load… was clicked.
fn empty(ui: &mut Ui, p: &egui::Painter, uid: u64, well: Rect, inputs: &Inputs, out: &mut BenchLayout) -> bool {
    if let Some(e) = &inputs.error {
        dashed_well(p, well);
        let font = tokens::LABEL_CONTROL.font_id(ui.ctx());
        let g = crate::chrome::glyph_size(font.size);
        let w = widgets::role_size(p, &tokens::LABEL_CONTROL, e).x + g + 3.0;
        let r = crate::chrome::status_text(p, pos2(well.center().x - w / 2.0, well.center().y), e, font, crate::chrome::Severity::Error);
        out.error = Some((e.clone(), r4(r)));
        return false;
    }
    let (b, clicked) = drop_target(ui, p, egui::Id::new(("sampler_slice_load", uid)), well, DROP);
    out.empty = Some((r4(well), r4(b)));
    clicked
}

/// A well's dashed `line.strong` edge (the Sampler views' empty drop target).
pub(crate) fn dashed_well(p: &egui::Painter, well: Rect) {
    let pts = [well.left_top(), well.right_top(), well.right_bottom(), well.left_bottom(), well.left_top()];
    let [dash, gap] = tk::CANDIDATE_DASH;
    p.extend(egui::Shape::dashed_line(&pts, Stroke::new(chrome::FIELD_EDGE_WIDTH, TOKENS.line_strong), dash, gap));
}

/// A Sampler view's empty well (README, Sampler views, States, "empty"): a dashed `line.strong`
/// drop target with `drop` (`+ Drop a loop`, `+ Drop a sample`) and Load… centered in it. Returns
/// the button's rect and whether it was clicked.
pub(crate) fn drop_target(ui: &mut Ui, p: &egui::Painter, id: egui::Id, well: Rect, drop: &str) -> (Rect, bool) {
    dashed_well(p, well);
    let text_w = widgets::role_size(p, &tokens::LABEL_CONTROL, drop).x;
    let bw = button_w(p, LOAD);
    let x = well.center().x - (text_w + ITEM_GAP + bw) / 2.0;
    widgets::role_text(p, &tokens::LABEL_CONTROL, drop, TOKENS.text_faint, Align2::LEFT_CENTER, pos2(x, well.center().y));
    let b = Rect::from_min_size(
        pos2(x + text_w + ITEM_GAP, well.center().y - layout::DENSE_CONTROL_HEIGHT / 2.0),
        vec2(bw, layout::DENSE_CONTROL_HEIGHT),
    );
    let clicked = dense_button(ui, id, b, LOAD, true).clicked();
    (b, clicked)
}

/// Missing media: the hatch over the well's body (it cannot take input) with a card naming the
/// file and Relink…, which waits for its command (drawn at the unused-control alpha).
fn missing(ui: &mut Ui, wp: &egui::Painter, uid: u64, body: Rect, name: &str, out: &mut BenchLayout) {
    out.missing = Some(missing_card(ui, wp, egui::Id::new(("sampler_slice_relink", uid)), body, name));
}

/// A Sampler view's missing source (README, Sampler views, States, "missing media"): the hatch
/// over `body` with a card naming the file (`Loop.wav is missing`) and Relink…, unavailable until
/// a relink command exists. Returns the card, its text and the button.
pub(crate) fn missing_card(ui: &mut Ui, wp: &egui::Painter, id: egui::Id, body: Rect, name: &str) -> ([f32; 4], String, [f32; 4]) {
    let t = &TOKENS;
    widgets::hatch::paint(wp, body);
    let text = format!("{name} is missing");
    let font = tokens::LABEL_CONTROL.font_id(ui.ctx());
    let g = crate::chrome::glyph_size(font.size);
    let bw = button_w(wp, RELINK);
    let [pt, pr, pb, pl] = chrome::NOTICE_PADDING;
    let tw = widgets::role_size(wp, &tokens::LABEL_CONTROL, &text).x;
    let size = vec2(pl + g + 3.0 + tw + SPACE[1] + bw + pr, pt + layout::DENSE_CONTROL_HEIGHT + pb);
    let card = Rect::from_center_size(body.center(), size);
    wp.rect(card, chrome::NOTICE_RADIUS, t.surface_panel, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line_strong), StrokeKind::Inside);
    crate::chrome::status_text(wp, pos2(card.min.x + pl, card.center().y), &text, font, crate::chrome::Severity::Error);
    let b = Rect::from_min_size(pos2(card.max.x - pr - bw, card.min.y + pt), vec2(bw, layout::DENSE_CONTROL_HEIGHT));
    dense_button(ui, id, b, RELINK, false);
    (r4(card), text, r4(b))
}

/// The view's x of `frame` across the well's inside.
fn x(view: &SlicesView, l: &Loop, area: Rect, frame: f64) -> f32 {
    view.viewport.x_of(l, frame, area.min.x, area.width())
}

fn out_count(n: usize) -> String {
    format!("{n} of {} slices", transit_model::sampler_patch::MAX_SLICES)
}

/// A status tag (README, Sampler views, "Status tag") in the top-right corner of `body`, the well's
/// area under anything along its top (the bench's flag strip, the zone waveform's brace row).
pub(crate) fn tag(wp: &egui::Painter, body: Rect, text: &str) -> Rect {
    let size = widgets::role_size(wp, &TAG_FONT, text) + vec2(2.0 * tk::TAG_PADDING_X, 0.0);
    let r = Rect::from_min_size(pos2(body.max.x - tk::TAG_INSET - size.x, body.min.y + tk::TAG_INSET), size);
    wp.rect(r, tk::TAG_RADIUS, Color32::TRANSPARENT, Stroke::new(chrome::FIELD_EDGE_WIDTH, named(tk::TAG_EDGE)), StrokeKind::Inside);
    widgets::role_text(wp, &TAG_FONT, text, named(tk::TAG_INK), Align2::CENTER_CENTER, r.center());
    r
}

fn dashed(wp: &egui::Painter, x: f32, top: f32, bottom: f32, color: Color32) {
    let [dash, gap] = tk::CANDIDATE_DASH;
    wp.extend(egui::Shape::dashed_line(&[pos2(x + 0.5, top), pos2(x + 0.5, bottom)], Stroke::new(1.0, color), dash, gap));
}

#[allow(clippy::too_many_arguments)]
fn draw_loop(
    ui: &mut Ui,
    wp: &egui::Painter,
    uid: u64,
    view: &mut SlicesView,
    l: &Loop,
    audio: Option<&transit_model::SourceAudio>,
    inputs: &Inputs,
    area: Rect,
    strip: Rect,
    body: Rect,
    out: &mut BenchLayout,
) -> (Option<Intent>, Vec<Intent>) {
    let t = &TOKENS;
    let mut intent = None;
    let set = l.showing();
    view.selected = view.selected.min(set.len().saturating_sub(1));
    let manual = l.mode == Mode::Manual;
    out.view = view.viewport.range(l);
    let visible = |a: f32, b: f32| b > area.min.x && a < area.max.x;
    let now = ui.input(|i| i.time);
    let window = ui.ctx().options(|o| o.input_options.max_double_click_delay);
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
        if let Some((mode, xf)) = super::bench_loop::used(l, i) {
            let xf_left = (xf > 0).then(|| x(view, l, area, s.end.saturating_sub(xf) as f64));
            out.loops.push(super::bench_loop::paint(wp, i, mode, a, b, xf_left, strip, body));
        }
    }
    // The waveform: one column per physical pixel, the selected slice `signal` at 60 percent,
    // the rest `text.faint` at 45 percent; the midline over it. A missing source draws none.
    let ppp = ui.ctx().pixels_per_point();
    let cols = (body.width() * ppp).ceil().max(1.0) as usize;
    let (a, b) = out.view;
    out.wave_ink = named(tk::BENCH_WAVE).gamma_multiply(tk::BENCH_WAVE_ALPHA);
    out.selected_wave_ink = t.signal.gamma_multiply(tk::BENCH_SELECTED_WAVE_ALPHA);
    let sel = set.get(view.selected).map(|s| (s.frame as f64, s.end as f64));
    let spans = if inputs.missing.is_some() { None } else { wave::columns(audio, a, b, cols) };
    let waiting = spans.is_none() && inputs.missing.is_none() && l.source.is_some();
    if let Some(spans) = spans {
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
    wp.line_segment([pos2(body.min.x, body.center().y), pos2(body.max.x, body.center().y)], Stroke::new(1.0, named(tk::BENCH_MIDLINE)));
    // A pending detection's markers (Auto only): dashed `text.faint` lines without flags where
    // the candidate differs from the set in use.
    if !manual && let Some(frames) = &inputs.candidate {
        for f in frames.iter().filter(|f| !set.iter().any(|s| s.frame == **f)) {
            let cx = x(view, l, area, *f as f64);
            if visible(cx, cx + 1.0) {
                dashed(wp, cx, body.min.y, body.max.y, named(tk::CANDIDATE_LINE));
                out.candidate.push(cx);
            }
        }
    }
    // The status tag: Analyzing… while the peaks are not there, else the stale mapping, else a
    // full set.
    let tag_text = if waiting {
        Some(ANALYZING.to_string())
    } else if inputs.stale {
        Some(STALE_TAG.to_string())
    } else if set.len() >= transit_model::sampler_patch::MAX_SLICES {
        Some(out_count(set.len()))
    } else {
        None
    };
    if let Some(text) = tag_text {
        let r = tag(wp, body, &text);
        if waiting {
            out.analyzing = Some(r4(r));
        }
        out.tag = Some((text, r4(r)));
    }
    // The end boundary: a 2 px line with its own flag, right-aligned to the line.
    let ex = x(view, l, area, l.end as f64);
    let end_line = Rect::from_min_max(pos2(ex - tk::BENCH_END_WIDTH, area.min.y), pos2(ex, area.max.y));
    if visible(end_line.min.x, end_line.max.x + 1.0) {
        wp.rect_filled(end_line, 0.0, named(tk::BENCH_END));
        let w = widgets::role_size(wp, &FLAG_FONT, "E").x + 2.0 * tk::BENCH_FLAG_PADDING_X;
        let flag = Rect::from_min_size(pos2(end_line.min.x - w, strip.min.y + tk::BENCH_FLAG_INSET), vec2(w, tk::BENCH_FLAG_HEIGHT));
        let r = tk::BENCH_END_FLAG_RADIUS.map(|v| v as u8);
        let (fill, ink) = (named(tk::BENCH_END_FLAG[0]), named(tk::BENCH_END_FLAG[1]));
        wp.rect_filled(flag, CornerRadius { nw: r[0], ne: r[1], se: r[2], sw: r[3] }, fill);
        widgets::role_text(wp, &FLAG_FONT, "E", ink, Align2::CENTER_CENTER, flag.center());
        out.end = Some(EndRecord { line: r4(end_line), flag: r4(flag), fill, ink });
    }
    let end_left = out.end.as_ref().map_or(area.max.x, |e| e.flag[0]);
    // The bench under the flag strip: a press selects the slice under the pointer and plays it
    // while held, a double-click adds; hovering shows the ghost and the pointer's milliseconds
    // from S. Registered before the handles, which sit over it and take their own presses (the
    // toolkit gives a press on a drag-only widget on top to that widget alone).
    let sense = if inputs.missing.is_some() { Sense::hover() } else { Sense::click() };
    let body_resp = ui.interact(body, egui::Id::new(("sampler_slice_well", uid)), sense);
    // Past 1× the scroll bar sits in the well's foot under the handles (the board draws the two
    // overlapping): registered before them, so a handle takes its own presses where they overlap.
    scroll(ui, wp, uid, view, l, area, out);
    // The markers: lines, then flags over them (a flag that would run into the next boundary is
    // left out, so the cell between the two boundaries stays clickable), then the Manual handles.
    // The window losing focus cancels a drag (the system may never deliver its release: the next
    // release in the window would otherwise commit wherever it lands).
    if let Some(d) = view.gesture.drag.as_mut()
        && ui.input(|i| i.raw.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(false))))
    {
        d.cancelled = true;
    }
    let dragging = view.gesture.drag.clone();
    let mut flag_rights = Vec::with_capacity(set.len());
    for (i, s) in set.iter().enumerate() {
        let preview = dragging.as_ref().filter(|d| d.index == i && !d.cancelled).and_then(|d| g::drag_preview(l, audio, i, d.requested));
        let frame = preview.as_ref().map_or(s.frame, |p| p.0);
        let lx = x(view, l, area, frame as f64);
        let next = set.get(i + 1).map_or(end_left, |n| x(view, l, area, n.frame as f64));
        let is_sel = i == view.selected;
        let line_color = if preview.is_some() {
            t.text
        } else {
            named(if is_sel {
                tk::BENCH_SELECTED_LINE
            } else if manual {
                tk::BENCH_MANUAL_LINE
            } else {
                tk::BENCH_AUTO_LINE
            })
        };
        let text = super::bench_loop::flag_text(i, super::bench_loop::used(l, i).map(|u| u.0));
        let w = (widgets::role_size(wp, &FLAG_FONT, &text).x + 2.0 * (tk::BENCH_FLAG_PADDING_X + tk::BENCH_FLAG_EDGE_WIDTH))
            .max(tk::BENCH_FLAG_MIN_WIDTH);
        let flag = Rect::from_min_size(pos2(lx, strip.min.y + tk::BENCH_FLAG_INSET), vec2(w, tk::BENCH_FLAG_HEIGHT));
        let crowded = flag.max.x > next;
        flag_rights.push(if crowded { lx + 1.0 } else { flag.max.x });
        // The dragged marker keeps its handle's interaction while its line is off the view (a drag
        // past the well's edge), so its release is seen; it is drawn only when visible.
        let vis = visible(lx, flag.max.x);
        let held_here = dragging.as_ref().is_some_and(|d| d.index == i);
        if !vis && !held_here {
            continue;
        }
        let line = Rect::from_min_max(pos2(lx, area.min.y), pos2(lx + 1.0, area.max.y));
        if vis {
            wp.rect_filled(line, 0.0, line_color);
        }
        let names = if is_sel {
            tk::BENCH_SELECTED_FLAG
        } else if manual {
            tk::BENCH_MANUAL_FLAG
        } else {
            tk::BENCH_AUTO_FLAG
        };
        let (fill, ink, edge) = (named(names[0]), named(names[1]), named(names[2]));
        if vis && !crowded {
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
            // A click on a flag removes its marker (the engine answers S).
            let resp = ui.interact(flag, egui::Id::new(("sampler_slice_flag", uid, i)), Sense::click()).on_hover_text(&inputs.flag_tip);
            if resp.clicked()
                && view.gesture.click(now, resp.double_clicked() || resp.triple_clicked(), Target::Flag(i), window) == Act::Single
                && let Some(id) = s.id
            {
                intent = Some(Intent::Remove { id, index: i });
            }
        }
        let handle = (manual && i > 0).then(|| {
            let d = tk::BENCH_HANDLE;
            let h = Rect::from_center_size(pos2(lx + 0.5, area.max.y - tk::BENCH_HANDLE_INSET - d / 2.0), vec2(d, d));
            let held = preview.is_some();
            let ring = if held { t.signal } else { named(tk::BENCH_HANDLE_LINE) };
            let fill = if held { t.signal } else { named(tk::BENCH_HANDLE_FILL) };
            if vis {
                wp.circle(h.center(), (d - tk::BENCH_HANDLE_STROKE) / 2.0, fill, Stroke::new(tk::BENCH_HANDLE_STROKE, ring));
            }
            if let Some(id) = s.id {
                let resp = ui.interact(h, egui::Id::new(("sampler_slice_handle", uid, i)), Sense::drag()).on_hover_text(&inputs.handle_tip);
                let pointer = resp.interact_pointer_pos();
                // The toolkit starts a drag-only widget's drag on the press: the marker stays
                // until the pointer has traveled past the drag threshold, then moves by the
                // distance dragged (where the press grabbed the handle stays under the pointer).
                if resp.drag_started() {
                    let grab = ui.input(|i| i.pointer.press_origin()).or(pointer).unwrap_or(h.center());
                    view.gesture.drag = Some(g::Drag { index: i, id, requested: s.frame, cancelled: false, grab, moved: false });
                } else if resp.dragged()
                    && let (Some(d), Some(p)) = (view.gesture.drag.as_mut(), pointer)
                    && d.index == i
                {
                    let threshold = ui.ctx().options(|o| o.input_options.max_click_dist);
                    d.moved |= p.distance(d.grab) > threshold;
                    if d.moved {
                        d.requested = view.viewport.shifted(l, s.frame, p.x - d.grab.x, area.width());
                    }
                }
                if resp.drag_stopped()
                    && let Some(d) = view.gesture.drag.take()
                    && !d.cancelled
                    && d.moved
                    && d.requested != s.frame
                {
                    intent = Some(Intent::Move { id: d.id, frame: d.requested });
                }
            }
            r4(h)
        });
        if let Some((_, tip)) = preview.as_ref().filter(|_| vis) {
            let size = widgets::role_size(wp, &TAG_FONT, tip) + vec2(2.0 * tk::TAG_PADDING_X, 0.0);
            let bottom = area.max.y - tk::BENCH_HANDLE_INSET - tk::BENCH_HANDLE - tk::TAG_INSET;
            let r = Rect::from_min_size(pos2(lx + tk::TAG_INSET, bottom - size.y), size);
            wp.rect(r, tk::TAG_RADIUS, t.surface_raised, Stroke::new(chrome::FIELD_EDGE_WIDTH, named(tk::TAG_EDGE)), StrokeKind::Inside);
            widgets::role_text(wp, &TAG_FONT, tip, t.text, Align2::CENTER_CENTER, r.center());
            out.drag = Some((r4(line), tip.clone(), r4(r)));
        }
        if vis {
            out.flags.push(FlagRecord {
                index: i,
                frame: s.frame,
                line: r4(line),
                line_color,
                flag: r4(flag),
                crowded,
                text,
                fill,
                ink,
                edge,
                handle,
            });
        }
    }
    // A drag whose release was not seen (no button is down any more) is over: nothing is sent.
    if view.gesture.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        view.gesture.drag = None;
    }
    // The cells: from a slice's flag (its line where the flag is left out) to the next boundary;
    // a click selects the slice, a double-click on the same cell renames it in place.
    for (i, s) in set.iter().enumerate() {
        let next = set.get(i + 1).map_or(end_left, |n| x(view, l, area, n.frame as f64));
        let left = flag_rights[i].max(area.min.x);
        let mut right = next.min(area.max.x);
        // The last cell runs under the E flag (not interactive) when the flag would leave it none.
        if i + 1 == set.len() && right <= left {
            right = (ex - tk::BENCH_END_WIDTH).min(area.max.x);
        }
        if right <= left {
            continue;
        }
        let cell = Rect::from_min_max(pos2(left, strip.min.y), pos2(right, strip.max.y));
        let resp = ui.interact(cell, egui::Id::new(("sampler_slice_cell", uid, i)), Sense::click()).on_hover_text(&inputs.cell_tip);
        if resp.clicked() {
            match view.gesture.click(now, resp.double_clicked() || resp.triple_clicked(), Target::Cell(i), window) {
                Act::Single => {
                    view.selected = i;
                    view.viewport.select(s.frame, s.end);
                }
                Act::Rename => intent = Some(Intent::Rename(i)),
                Act::Swallow | Act::Add => {}
            }
        }
        out.cells.push((i, r4(cell)));
    }
    let cells = out.cells.clone();
    let label = rename(ui, uid, view, &cells, strip, out);
    let audition = press(ui, view, l, inputs, area, &body_resp);
    // A double-click on the bench adds (the ghost previews where); a click whose press was held
    // off is not counted, so it never starts a double-click.
    if body_resp.clicked()
        && !view.gesture.press_held_off
        && !inputs.held_off
        && let Some(at) = body_resp.interact_pointer_pos()
        && view.gesture.click(now, body_resp.double_clicked() || body_resp.triple_clicked(), Target::Body, window) == Act::Add
    {
        intent = Some(Intent::Add(view.viewport.frame_at(l, at.x, area.min.x, area.width())));
    }
    if let Some(at) = body_resp.hover_pos() {
        let frame = view.viewport.frame_at(l, at.x, area.min.x, area.width());
        let ms = viewport::ms_from_start(l, frame);
        out.pointer = Some((frame, ms));
        if view.gesture.drag.is_none() && inputs.missing.is_none() {
            ghost(wp, view, l, audio, area, strip, body, frame, out);
        }
        body_resp.on_hover_text(format!("{ms:.1} ms · {HOVER_ADD}"));
    }
    (label, audition.into_iter().chain(intent).collect())
}

/// A press on the bench this frame (a fast click's press and release may share it): the slice
/// under the pointer becomes the selected one and the returned intent auditions it while the
/// button is held. The view does not scroll (only at 1×, where the view shows the whole loop, the
/// selection sets the center the next zoom keeps), so the second click of a double-click lands
/// where the first did. Held off, the press does nothing and is remembered as held off.
fn press(ui: &Ui, view: &mut SlicesView, l: &Loop, inputs: &Inputs, area: Rect, body: &egui::Response) -> Option<Intent> {
    let pressed = ui.input(|i| i.pointer.primary_pressed()) && (body.is_pointer_button_down_on() || body.clicked());
    if !pressed {
        return None;
    }
    view.gesture.press_held_off = inputs.held_off;
    if inputs.held_off {
        return None;
    }
    let at = body.interact_pointer_pos()?;
    let frame = view.viewport.frame_at(l, at.x, area.min.x, area.width());
    let set = l.showing();
    let i = set.iter().rposition(|s| s.frame <= frame)?;
    view.selected = i;
    if view.viewport.factor() <= 1.0 {
        view.viewport.select(set[i].frame, set[i].end);
    }
    Some(Intent::Audition(i))
}

/// The rename field over the renamed slice's cell (the cell's height inside the strip, at least
/// four flags wide): Enter or a click elsewhere commits (the name returned), Escape cancels.
fn rename(ui: &mut Ui, uid: u64, view: &mut SlicesView, cells: &[(usize, [f32; 4])], strip: Rect, out: &mut BenchLayout) -> Option<Intent> {
    let r = view.gesture.renaming.as_mut()?;
    let cell = cells.iter().find(|c| c.0 == r.index).map(|c| c.1)?;
    let (left, right) = (cell[0], cell[2].max(cell[0] + tk::BENCH_FLAG_MIN_WIDTH * 4.0));
    let field = Rect::from_min_max(pos2(left, strip.min.y + tk::BENCH_FLAG_INSET), pos2(right, strip.max.y - tk::BENCH_FLAG_INSET));
    let edit =
        egui::TextEdit::singleline(&mut r.text).font(FLAG_FONT.font_id(ui.ctx())).margin(vec2(tk::BENCH_FLAG_PADDING_X, 0.0)).id(rename_id(uid));
    let resp = ui.put(field, edit);
    if !r.focused {
        resp.request_focus();
        r.focused = true;
    }
    out.rename = Some(r4(field));
    if !resp.lost_focus() {
        return None;
    }
    let cancel = super::escaped(ui);
    let (id, text) = (r.id, r.text.clone());
    view.gesture.renaming = None;
    (!cancel).then_some(Intent::Label { id, text })
}

/// The add preview: a dashed `text` line at the snapped frame with its time on a `text` flag, a
/// short `text.faint` tick at the raw pointer when the snap moved it; `meter.hot` and `too close`
/// where the add would be refused.
#[allow(clippy::too_many_arguments)]
fn ghost(
    wp: &egui::Painter,
    view: &SlicesView,
    l: &Loop,
    audio: Option<&transit_model::SourceAudio>,
    area: Rect,
    strip: Rect,
    body: Rect,
    raw: u64,
    out: &mut BenchLayout,
) {
    let t = &TOKENS;
    let gh = g::ghost(l, audio, raw);
    let color = named(if gh.refused { tk::BENCH_GHOST_REFUSED } else { tk::BENCH_GHOST_LINE });
    let gx = x(view, l, area, gh.frame as f64);
    dashed(wp, gx, body.min.y, body.max.y, color);
    let w = widgets::role_size(wp, &FLAG_FONT, &gh.label).x + 2.0 * tk::BENCH_FLAG_PADDING_X;
    let flag = Rect::from_min_size(pos2(gx, strip.max.y + SPACE[0]), vec2(w, tk::BENCH_FLAG_HEIGHT));
    let r = tk::BENCH_FLAG_RADIUS.map(|v| v as u8);
    wp.rect_filled(flag, CornerRadius { nw: r[0], ne: r[1], se: r[2], sw: r[3] }, color);
    widgets::role_text(wp, &FLAG_FONT, &gh.label, t.surface_inset, Align2::LEFT_CENTER, pos2(flag.min.x + tk::BENCH_FLAG_PADDING_X, flag.center().y));
    let rx = x(view, l, area, gh.raw as f64);
    let raw_tick = ((rx - gx).abs() > 0.5).then(|| {
        let r = Rect::from_center_size(pos2(rx + 0.5, body.center().y), vec2(1.0, tk::BENCH_FLAG_HEIGHT));
        wp.rect_filled(r, 0.0, t.text_faint);
        r4(r)
    });
    let line = Rect::from_min_max(pos2(gx, body.min.y), pos2(gx + 1.0, body.max.y));
    out.ghost = Some(GhostRecord { frame: gh.frame, line: r4(line), color, label: gh.label, flag: r4(flag), raw_tick });
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
/// its tick, skipped where it would run past the ruler or into the label before it. Only the
/// steps inside the visible range are visited and measured.
fn ticks(p: &egui::Painter, view: &SlicesView, l: &Loop, area: Rect, ruler: Rect, out: &mut BenchLayout) {
    let step = f64::from(if view.viewport.factor() >= tk::BENCH_RULER_FINE_FROM_ZOOM { tk::BENCH_RULER_FINE_STEP } else { tk::BENCH_RULER_STEP });
    let length = l.seconds(l.end as f64);
    let (a, b) = view.viewport.range(l);
    let first = (l.seconds(a) / step - 1e-9).ceil().max(0.0) as u64;
    let last = (l.seconds(b).min(length) / step + 1e-9).floor().max(0.0) as u64;
    let mut last_right = f32::NEG_INFINITY;
    for k in first..=last {
        out.ruler_steps += 1;
        let s = k as f64 * step;
        let tx = x(view, l, area, l.start as f64 + s * f64::from(l.rate));
        if tx < ruler.min.x || tx < last_right {
            continue;
        }
        let label = format!("{s:.1} s");
        let w = widgets::role_size(p, &AXIS_FONT, &label).x;
        if tx + w > ruler.max.x {
            break;
        }
        let r = widgets::role_text(p, &AXIS_FONT, &label, TOKENS.text_faint, Align2::LEFT_TOP, pos2(tx, ruler.min.y));
        last_right = r.max.x;
        out.ticks.push((label, r4(r)));
    }
}
