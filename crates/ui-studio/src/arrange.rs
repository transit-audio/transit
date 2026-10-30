//! The arrangement window: a track list on the left, a bar/beat ruler, and the timeline with
//! clips as colored regions. Clicking a clip selects it; dragging moves it (Ctrl-drag, Cmd on a
//! Mac, or Alt-drag copies; clip drags snap to the visible grid, Shift bypasses it); dragging an edge trims it (the dragged edge snaps to the visible grid, Shift bypasses it); clicking empty timeline sets the
//! cursor, and dragging across empty lane space draws a marquee that selects the clips it touches (`marquee`, Logic's convention; Shift adds, Ctrl/Cmd toggles). A drag of a selected clip previews and moves the whole selection (`drag_ghosts`). Every edit goes through engine commands (see `actions`).

mod clips;
mod copy_drag;
mod drag_ghosts;
pub(crate) mod marquee;
mod tracks;
pub(crate) use clips::*;
pub(crate) use tracks::*;

use crate::StudioApp;
use crate::theme::{self, CLIP_NAME_H, RULER_H, TOKENS};
use egui::{Align2, Color32, CursorIcon, FontId, Id, LayerId, Order, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use transit_model::{Clip, ClipContent, ClipId, ClipLoop, Session, Track, TrackId, TrackKind};
use transit_time::{BarBeat, Samples};

/// How close (points) to a clip edge a press trims instead of moving.
pub(crate) const EDGE: f32 = 6.0;

/// Opacity of a loop-drag ghost cycle and the notes drawn in it.
pub(crate) const GHOST_ALPHA: f32 = 0.45;
/// Width (points) of the loop handle's hot zone, measured back from a clip's right edge.
const LOOP_ZONE_W: f32 = 20.0;
/// Room under the rows for the Add Track buttons: the gap, three rows of buttons, a margin.
const ADD_STACK_H: f32 = 8.0 + 60.0 + 22.0 + 8.0;
/// Quick-add buttons: height, the gap between them, and the reserved right-hand slot.
const ADD_BTN_H: f32 = 22.0;
const ADD_GAP: f32 = 8.0;
pub const ADD_SLOT_W: f32 = 18.0;
/// Quick-add buttons: left padding before the plus glyph, its size, and the gap to the label.
const ADD_PAD: f32 = 8.0;
const ADD_PLUS: f32 = 12.0;
const ADD_LABEL_GAP: f32 = 4.0;

/// The x where every quick-add label starts inside button `r`: same offset on all of them.
fn quick_add_label_x(r: Rect) -> f32 {
    r.min.x + ADD_PAD + ADD_PLUS + ADD_LABEL_GAP
}

/// Narrow columns (under this width) use the next font size down.
const ADD_NARROW_W: f32 = 90.0;

/// Paint one quick-add button as `[plus][label]` left-aligned. The right slot stays clear only
/// when the button draws a chevron there; otherwise the label may use it. The label is clipped
/// at that edge and the plus is never clipped. Returns the label origin x and the label's
/// (full width, visible width).
fn paint_quick_add(p: &egui::Painter, r: Rect, label: &str, has_chevron: bool) -> (f32, [f32; 2]) {
    let color = TOKENS.text;
    crate::icons::paint(p, pos2(r.min.x + ADD_PAD + ADD_PLUS / 2.0, r.center().y), crate::icons::Icon::Plus, ADD_PLUS, color);
    let x = quick_add_label_x(r);
    let size = if r.width() < ADD_NARROW_W { 11.0 } else { 12.0 };
    let galley = p.layout_no_wrap(label.to_string(), FontId::proportional(size), color);
    let right = if has_chevron { r.max.x - ADD_SLOT_W } else { r.max.x - 4.0 };
    let clip = Rect::from_min_max(pos2(x, r.min.y), pos2(right.max(x), r.max.y));
    let full = galley.size().x;
    p.with_clip_rect(clip).galley(pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
    (x, [full, full.min(clip.width())])
}

/// Most repeats one loop drag shows or lays down (the engine caps `clip.loop_to` at the same).
const MAX_LOOP_REPEATS: usize = 1000;

/// What a drag on a clip does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DragKind {
    Move,
    TrimStart,
    TrimEnd,
    /// The loop handle: the upper right corner of the header strip (Logic's loop corner).
    Loop,
    /// Audio clips: the fade corners of the body, under the header strip (`fades::corner`), and
    /// the area of an existing fade.
    FadeIn,
    FadeOut,
    /// The curve line of an existing fade: bend its curvature (decided at the press).
    FadeCurveIn,
    FadeCurveOut,
}

/// What a press at `pos` on a clip drawn in `rect` (`[x0, y0, x1, y1]`) would do. The edges trim
/// (`EDGE` wide, as before); the loop handle is the header strip's part of the last
/// `LOOP_ZONE_W` points before the right-edge trim handle; on an `audio` clip the top corners of
/// the body are the fade zones, whole triangles the height of the body (`fades::corner`);
/// everything else moves. Narrow clips shrink the edges and the loop zone, and the fade zones
/// share what is left, so the middle moves last. The area and curve of an existing fade are
/// found by `fades::grab_at`, which needs the clip's fades.
pub fn zone(rect: R4, pos: Pos2, audio: bool) -> DragKind {
    let w = rect[2] - rect[0];
    let edge = EDGE.min(w / 4.0);
    if pos.x - rect[0] <= edge {
        DragKind::TrimStart
    } else if rect[2] - pos.x <= edge {
        DragKind::TrimEnd
    } else if pos.y - rect[1] <= CLIP_NAME_H && rect[2] - pos.x <= LOOP_ZONE_W.min(w / 3.0) {
        DragKind::Loop
    } else if audio && let Some(side) = crate::fades::corner(rect, pos) {
        match side {
            crate::fades::Side::In => DragKind::FadeIn,
            crate::fades::Side::Out => DragKind::FadeOut,
        }
    } else {
        DragKind::Move
    }
}

/// The repeats a loop drag lays down: back-to-back copies of length `len` from `from` up to `to`,
/// the last one trimmed at `to`. Empty when `to` is not past `from`.
pub fn loop_ghosts(from: Samples, len: Samples, to: Samples) -> Vec<(Samples, Samples)> {
    let len = len.max(1);
    let mut out = Vec::new();
    let mut a = from;
    while a < to && out.len() < MAX_LOOP_REPEATS {
        out.push((a, a.saturating_add(len).min(to)));
        a = a.saturating_add(len);
    }
    out
}

/// A mouse gesture in progress.
#[derive(Debug, Clone)]
pub enum Gesture {
    Clip { clip: ClipId, track: TrackId, kind: DragKind, press: Pos2, start: Samples, end: Samples },
    Volume { track: TrackId, start_db: f32 },
}

/// Rects as `[x0, y0, x1, y1]`, for `ui.inspect` (scripts drive the UI with them).
pub type R4 = [f32; 4];

pub fn r4(r: Rect) -> R4 {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TrackRow {
    pub id: u64,
    pub name: String,
    pub kind: TrackKind,
    /// The line under the name (an instrument track: the instrument).
    pub subtitle: String,
    /// A rack's "Rack" badge (ADR 0034); zero elsewhere.
    pub rack_badge: R4,
    pub header: R4,
    /// The name in the header (double-click renames, slice 6).
    pub name_rect: R4,
    pub lane: R4,
    pub mute: R4,
    pub solo: R4,
    pub arm: R4,
    pub monitor: R4,
    pub volume: R4,
    /// The group this track sits in (slice 2c), its depth, and for a group: collapsed or not,
    /// its disclosure triangle, and how many member clips its collapsed lane summarizes.
    pub folder: Option<u64>,
    pub depth: usize,
    pub collapsed: bool,
    pub disclosure: R4,
    pub summary_bars: usize,
    /// Slice 7: the A toggle (show the track's automation lanes) and whether they are shown.
    pub automation: R4,
    pub automation_shown: bool,
    /// The track is frozen: its lane is tinted cold; the snowflake button unfreezes it.
    pub frozen: bool,
    pub freeze_button: R4,
    /// The ancestor groups' color bars beside this row, outermost first (#60).
    pub group_bars: Vec<R4>,
    /// The unresponsive-plug-in marker beside the subtitle (empty: none, #213), and its rect.
    pub unresponsive: String,
    pub unresponsive_rect: R4,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ClipRect {
    pub id: u64,
    pub audio: bool,
    pub track: u64,
    pub rect: R4,
    pub start: Samples,
    pub end: Samples,
}

/// A MIDI clip being recorded on a track's lane.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RecordingLane {
    pub track: u64,
    pub rect: R4,
    /// The notes played so far, as rectangles in the lane.
    pub notes: Vec<R4>,
}

/// Last frame's arrangement geometry.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Layout {
    pub track_list: R4,
    pub ruler: R4,
    /// The loop strip along the top of the ruler.
    pub loop_strip: R4,
    pub timeline: R4,
    pub samples_per_px: f64,
    /// The grid as the ruler corner reads it ("Grid: 1/16", "Adaptive 1/32", "Snap off").
    pub grid_text: String,
    pub scroll: Samples,
    pub tracks: Vec<TrackRow>,
    pub clips: Vec<ClipRect>,
    /// MIDI passes being recorded: the growing clip outline per track with its provisional notes.
    pub recording: Vec<RecordingLane>,
    pub add_audio: R4,
    pub add_instrument: R4,
    pub add_midi: R4,
    pub add_group: R4,
    pub add_more: R4,
    /// Where each quick-add label starts (x), in the order Audio, Instrument, MIDI, Group, "+ ...".
    pub add_label_x: [f32; 5],
    /// The reserved right-hand slot of each quick-add button (only Audio draws its chevron there).
    pub add_slot: [R4; 5],
    /// Each quick-add label's (full width, visible width) as painted.
    pub add_label_w: [[f32; 2]; 5],
    /// Transport bar buttons (`play`, `stop`, `record`, `rtz`, `cycle`).
    pub transport: std::collections::BTreeMap<String, R4>,
    /// The icon each transport button shows (its Lucide name, e.g. `play`).
    pub transport_icons: std::collections::BTreeMap<String, String>,
    /// Where a dragged track header would land: a line between rows or a group header's outline.
    pub drop: Option<R4>,
    /// The clips a move/copy drag in progress would land, as painted (#218).
    pub drag_ghosts: Vec<drag_ghosts::DragGhost>,
    /// Every fade and crossfade as painted: area, curve, dimming, hover dot (#182).
    pub fades: Vec<crate::fades::Paint>,
}

#[derive(Debug, Default)]
pub struct ArrangeState {
    /// Grid mode, triplets and snap (view state; not saved in the session).
    pub grid: crate::grid::GridView,
    pub gesture: Option<Gesture>,
    /// The loop strip gesture in progress (the loop region itself lives in the session).
    pub loop_drag: Option<crate::loopbar::LoopDrag>,
    /// Vertical scroll of the track rows (points).
    pub vscroll: f32,
    pub layout: Layout,
    /// The track menu's open "Instrument" submenu entries (`value` is the plug-in id).
    pub menu: Vec<crate::mixer::MenuItem>,
    /// A track header being dragged (slice 2c).
    pub header_drag: Option<crate::groups::HeaderDrag>,
    /// A marquee selection drag in progress (#217).
    pub marquee: Option<marquee::Marquee>,
    /// The track rows in display order (Shift-click ranges over them).
    pub order: Vec<TrackId>,
    /// The track a plain or Ctrl click on a header last picked (Shift-click ranges from it).
    pub anchor: Option<TrackId>,
    /// Composite gestures started, to give each its own merge key.
    pub serial: u64,
    /// The menu bar's Track menu and its group entries as last drawn (slice 2c).
    pub bar_menu: Vec<crate::mixer::MenuItem>,
    /// The shape menu a click on a fade opened (#63).
    pub fade_menu: Option<crate::fades::Menu>,
    /// The clip (and fade side under the pointer) a right click picked, for the clip context menu.
    pub menu_target: Option<(ClipId, Option<crate::fades::Side>)>,
}

/// Timeline mapping: x ↔ samples, from the session's shared zoom (`edit.zoom`).
#[derive(Clone, Copy)]
pub(crate) struct Map {
    pub(crate) x0: f32,
    pub(crate) scroll: Samples,
    pub(crate) spp: f64,
    /// The grid settings, so every consumer of a map snaps to the lines drawn.
    pub(crate) grid: crate::grid::GridView,
}

impl Map {
    pub(crate) fn x(&self, at: Samples) -> f32 {
        self.x0 + ((at - self.scroll) as f64 / self.spp) as f32
    }
    pub(crate) fn at(&self, x: f32) -> Samples {
        self.scroll + (f64::from(x - self.x0) * self.spp).round() as Samples
    }
}

/// Height of one take lane (slice 6).
pub const TAKE_H: f32 = 22.0;

/// Row geometry: each row's top (content coordinates, before scrolling) and full height. A row is
/// the track's lane (`rh`) plus its take lanes when they are shown, then its automation lanes.
pub(crate) struct RowGeo {
    pub tops: Vec<f32>,
    pub heights: Vec<f32>,
    pub rh: f32,
}

impl RowGeo {
    fn new(app: &StudioApp, rows: &[&Track], rh: f32) -> RowGeo {
        let mut tops = Vec::with_capacity(rows.len());
        let mut heights = Vec::with_capacity(rows.len());
        let mut acc = 0.0;
        for t in rows {
            let h = rh + takes_shown(app, t) as f32 * TAKE_H + crate::automation::lanes_height(app, t);
            tops.push(acc);
            heights.push(h);
            acc += h;
        }
        RowGeo { tops, heights, rh }
    }
    fn total(&self) -> f32 {
        self.tops.last().zip(self.heights.last()).map_or(0.0, |(t, h)| t + h)
    }
    /// The row at content height `y`.
    fn index_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        (0..self.tops.len()).find(|i| y >= self.tops[*i] && y < self.tops[*i] + self.heights[*i])
    }
}

/// How many take lanes a track shows: all its playlists when it has several and they are open.
pub(crate) fn takes_shown(app: &StudioApp, t: &Track) -> usize {
    if t.playlists.len() > 1 && app.ui.takes_open.contains(&t.id) { t.playlists.len() } else { 0 }
}

/// A row in the track list: not hidden, and not in a collapsed group (view state, slice 2c).
fn shown(app: &StudioApp, s: &Session, t: &Track) -> bool {
    !t.hidden && !app.tucked(s, t)
}

pub fn show(app: &mut StudioApp, ui: &mut Ui) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| arrangement(app, ui));
}

fn arrangement(app: &mut StudioApp, ui: &mut Ui) {
    let full = ui.max_rect();
    let hw = app.ui.header_width.clamp(160.0, 480.0);
    let rh = app.ui.track_height.clamp(24.0, 200.0);
    let list = Rect::from_min_max(pos2(full.min.x, full.min.y + RULER_H), pos2(full.min.x + hw, full.max.y));
    let ruler = Rect::from_min_max(pos2(full.min.x + hw, full.min.y), pos2(full.max.x, full.min.y + RULER_H));
    let tl = Rect::from_min_max(pos2(full.min.x + hw, full.min.y + RULER_H), full.max);
    let corner = Rect::from_min_max(full.min, pos2(full.min.x + hw, full.min.y + RULER_H));
    let s = app.engine.session_arc();
    let map = Map { x0: tl.min.x, scroll: s.edit.zoom.scroll, spp: s.edit.zoom.samples_per_px.max(0.01), grid: app.arrange.grid };
    let p = ui.painter();
    p.rect_filled(corner, 0.0, TOKENS.ruler);
    p.with_clip_rect(corner).text(
        pos2(corner.max.x - 8.0, corner.max.y - 9.0),
        Align2::RIGHT_CENTER,
        crate::grid::label(&s, map.spp, &map.grid),
        FontId::proportional(11.0),
        TOKENS.text_dim,
    );
    p.rect_filled(list, 0.0, TOKENS.panel);

    // Rows.
    let rows: Vec<&Track> = s.tracks.iter().filter(|t| shown(app, &s, t)).collect();
    app.arrange.order = rows.iter().map(|t| t.id).collect();
    let geo = RowGeo::new(app, &rows, rh);
    let content_h = geo.total() + ADD_STACK_H;
    app.finish.layout.takes.clear();
    app.finish.layout.chips.clear();
    app.arrange.vscroll = app.arrange.vscroll.clamp(0.0, (content_h - tl.height()).max(0.0));
    let mut layout = Layout {
        track_list: r4(list),
        ruler: r4(ruler),
        timeline: r4(tl),
        samples_per_px: map.spp,
        grid_text: crate::grid::label(&s, map.spp, &map.grid),
        scroll: map.scroll,
        transport: std::mem::take(&mut app.arrange.layout.transport),
        transport_icons: std::mem::take(&mut app.arrange.layout.transport_icons),
        ..Layout::default()
    };
    let selected_tracks = s.edit.selected_tracks.clone();
    let focus = app.chosen_track();
    app.arrange.menu.clear();
    // One status read per frame, and only when some track sends to an external port (#69).
    let outputs = s.tracks.iter().any(|t| t.midi_output.is_external()).then(|| crate::midi_output::OutputView::read(app.player.as_ref()));
    for (i, t) in rows.iter().enumerate() {
        let y0 = tl.min.y + geo.tops[i] - app.arrange.vscroll;
        let header = Rect::from_min_max(pos2(list.min.x, y0), pos2(list.max.x, y0 + geo.heights[i]));
        let lane = Rect::from_min_max(pos2(tl.min.x, y0), pos2(tl.max.x, y0 + rh));
        let mut row = TrackRow {
            id: t.id.0,
            name: t.name.clone(),
            kind: t.kind,
            header: r4(header),
            lane: r4(lane),
            folder: crate::groups::parent_of(&s, t).map(|f| f.0),
            depth: crate::groups::depth(&s, t),
            collapsed: t.is_folder() && app.collapsed(t.id),
            frozen: t.freeze.is_some(),
            ..TrackRow::default()
        };
        if header.max.y >= list.min.y && header.min.y <= list.max.y {
            let selected = selected_tracks.contains(&t.id) || focus == Some(t.id);
            track_header(app, ui, t, header, list, selected, outputs.as_ref(), &mut row);
        }
        let lane_fill = if i % 2 == 0 { TOKENS.lane } else { TOKENS.lane_alt };
        let row_rect = Rect::from_min_max(lane.min, pos2(lane.max.x, y0 + geo.heights[i]));
        ui.painter().with_clip_rect(tl).rect_filled(row_rect, 0.0, lane_fill);
        if row.frozen {
            // A frozen lane reads cold: the accent, faint, over the lane.
            ui.painter().with_clip_rect(tl).rect_filled(row_rect, 0.0, TOKENS.accent.gamma_multiply(0.12));
        }
        layout.tracks.push(row);
    }
    // Add Track buttons below the rows: two equal columns, then "+ ..." across both.
    let below = tl.min.y + geo.total() - app.arrange.vscroll + ADD_GAP;
    let col_w = ((hw - 3.0 * ADD_GAP) / 2.0).max(1.0);
    let add_a = Rect::from_min_size(pos2(list.min.x + ADD_GAP, below), vec2(col_w, ADD_BTN_H));
    let add_i = Rect::from_min_size(pos2(add_a.max.x + ADD_GAP, below), add_a.size());
    let step = ADD_BTN_H + ADD_GAP;
    let add_m = Rect::from_min_size(pos2(add_a.min.x, below + step), add_a.size());
    let add_g = Rect::from_min_size(pos2(add_i.min.x, below + step), add_a.size());
    let add_more = Rect::from_min_max(pos2(add_a.min.x, below + 2.0 * step), pos2(add_i.max.x, below + 2.0 * step + ADD_BTN_H));
    let slot_of = |r: Rect| Rect::from_min_max(pos2(r.max.x - ADD_SLOT_W, r.min.y), r.max);
    let mut label_x = [0.0f32; 5];
    let mut label_w = [[0.0f32; 2]; 5];
    if add_a.min.y < list.max.y {
        let clip = ui.painter().with_clip_rect(list);
        // Audio adds a mono track (the common case: one guitar or mic); its chevron picks mono or stereo.
        let chev = slot_of(add_a);
        let main = Rect::from_min_max(add_a.min, pos2(chev.min.x, add_a.max.y));
        let main_resp = ui.interact(main.intersect(list), ui.id().with("add_audio"), Sense::click());
        let chev_resp = ui.interact(chev.intersect(list), ui.id().with("add_audio_format"), Sense::click());
        let hot = main_resp.hovered() || chev_resp.hovered();
        clip.rect_filled(add_a, crate::theme::RADIUS, if hot { TOKENS.button_hover } else { TOKENS.button });
        (label_x[0], label_w[0]) = paint_quick_add(&clip, add_a, "Audio", true);
        // A dim 1 px divider marks where the dropdown's hot area starts, hovered or not.
        let inset = 4.0;
        clip.vline(chev.min.x, (chev.min.y + inset)..=(chev.max.y - inset), egui::Stroke::new(1.0, TOKENS.line));
        crate::icons::paint(&clip, chev.center(), crate::icons::Icon::ChevronDown, 10.0, TOKENS.text_dim);
        if main_resp.clicked() {
            let _ = app.add_audio_track("mono");
        }
        egui::Popup::menu(&chev_resp).width(110.0).show(|ui| {
            if ui.button("Mono").clicked() {
                let _ = app.add_audio_track("mono");
                ui.close();
            }
            if ui.button("Stereo").clicked() {
                let _ = app.add_audio_track("stereo");
                ui.close();
            }
        });
        let resp = ui.interact(add_i.intersect(list), ui.id().with("add_instrument"), Sense::click());
        clip.rect_filled(add_i, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        (label_x[1], label_w[1]) = paint_quick_add(&clip, add_i, "Instrument", false);
        if resp.clicked() {
            let _ = app.add_track("instrument");
        }
        let resp = ui.interact(add_m.intersect(list), ui.id().with("add_midi"), Sense::click());
        clip.rect_filled(add_m, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        (label_x[2], label_w[2]) = paint_quick_add(&clip, add_m, "MIDI", false);
        if resp.clicked() {
            let _ = app.add_track("midi");
        }
        let resp = ui.interact(add_g.intersect(list), ui.id().with("add_group"), Sense::click());
        clip.rect_filled(add_g, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        (label_x[3], label_w[3]) = paint_quick_add(&clip, add_g, "Group", false);
        if resp.clicked() {
            let _ = app.new_group_track();
        }
        // "+ ..." lists the kinds the quick buttons leave out (the studio has no New Track dialog).
        let resp = ui.interact(add_more.intersect(list), ui.id().with("add_more"), Sense::click());
        clip.rect_filled(add_more, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        (label_x[4], label_w[4]) = paint_quick_add(&clip, add_more, "…", false);
        egui::Popup::menu(&resp).width(130.0).show(|ui| {
            if ui.button("Stereo Audio Track").clicked() {
                let _ = app.add_audio_track("stereo");
                ui.close();
            }
            if ui.button("FX Channel").clicked() {
                let _ = app.add_fx_channel();
                ui.close();
            }
        });
    }
    layout.add_label_x = label_x;
    layout.add_label_w = label_w;
    layout.add_slot = [add_a, add_i, add_m, add_g, add_more].map(|r| r4(slot_of(r)));
    layout.add_audio = r4(add_a);
    layout.add_instrument = r4(add_i);
    layout.add_midi = r4(add_m);
    layout.add_group = r4(add_g);
    layout.add_more = r4(add_more);
    ui.painter().line_segment([pos2(list.max.x, full.min.y), pos2(list.max.x, full.max.y)], Stroke::new(1.0, TOKENS.line));

    grid_and_ruler(app, ui, &s, map, ruler, tl);
    clips(app, ui, &s, map, tl, &rows, &geo, &mut layout);
    group_summaries(app, ui, &s, map, tl, &mut layout);
    header_drag(app, ui, &s, &mut layout);
    crate::keys::lane_hints(app, ui, tl, &layout.tracks);
    timeline_input(app, ui, &s, map, tl, &rows, &geo, &layout);
    takes(app, ui, &s, map, tl, list, &rows, &geo, &layout);
    let vscroll = app.arrange.vscroll;
    crate::automation::lanes(app, ui, &s, map, tl, list, &rows, &geo, vscroll);
    let strip = Rect::from_min_max(ruler.min, pos2(ruler.max.x, ruler.min.y + crate::loopbar::STRIP_H));
    crate::loopbar::strip(app, ui, &s, map, strip);
    layout.loop_strip = r4(strip);
    ruler_input(app, ui, map, Rect::from_min_max(pos2(ruler.min.x, strip.max.y), ruler.max));
    crate::loopbar::block(app, ui, &s, map, ruler);
    overlays(app, ui, &s, map, ruler, tl, &rows, &geo, &mut layout);
    scroll_and_zoom(app, ui, map, full, tl, content_h);
    follow_playhead(app, map, tl);
    app.arrange.layout = layout;
}

fn bar_at(s: &Session, bar: i64) -> Samples {
    s.tempo.samples_at_bar_beat(BarBeat { bar, beat: 1, tick: 0 }, s.sample_rate)
}

/// Bar and beat lines across the timeline, and the ruler with bar numbers.
fn grid_and_ruler(app: &StudioApp, ui: &Ui, s: &Session, map: Map, ruler: Rect, tl: Rect) {
    let p = ui.painter();
    p.rect_filled(ruler, 0.0, TOKENS.ruler);
    p.line_segment([pos2(ruler.min.x, ruler.max.y), pos2(ruler.max.x, ruler.max.y)], Stroke::new(1.0, TOKENS.line));
    let first = s.tempo.bar_beat_at(map.at(tl.min.x).max(0), s.sample_rate).bar.max(1);
    let bar_px = (map.x(bar_at(s, first + 1)) - map.x(bar_at(s, first))).max(0.01);
    // Label every n bars so labels keep about 40 points apart.
    let every = [1, 2, 4, 8, 16, 32, 64, 128].into_iter().find(|n| bar_px * *n as f32 >= 40.0).unwrap_or(256);
    let gp = p.with_clip_rect(tl);
    let rp = p.with_clip_rect(ruler);
    let ppt = crate::grid::px_per_tick(s, map.spp);
    let step = crate::grid::ticks(ppt, crate::grid::bar_ticks(s), &map.grid);
    let sub_step = crate::grid::sub_line_step(step, ppt, transit_time::TICKS_PER_QUARTER);
    for bar in (first..).take(4000) {
        let x = map.x(bar_at(s, bar));
        if x > tl.max.x {
            break;
        }
        let beats = s.tempo.meter_at_tick(s.tempo.bar_start_tick(bar)).numerator.max(1);
        let next = map.x(bar_at(s, bar + 1));
        if (next - x) / beats as f32 >= 10.0 {
            for b in 1..beats {
                let bx = map.x(s.tempo.samples_at_bar_beat(BarBeat { bar, beat: i64::from(b) + 1, tick: 0 }, s.sample_rate));
                gp.line_segment([pos2(bx, tl.min.y), pos2(bx, tl.max.y)], Stroke::new(1.0, TOKENS.grid_beat));
                rp.line_segment([pos2(bx, ruler.max.y - 5.0), pos2(bx, ruler.max.y)], Stroke::new(1.0, TOKENS.text_dim.gamma_multiply(0.5)));
            }
        }
        // Sub-beat lines at the grid step, fainter than the beats; skipped where they would be
        // denser than `MIN_DRAW_PX` (the snap still uses the step).
        if let Some(step) = sub_step {
            let (t0, t1) = (s.tempo.bar_start_tick(bar), s.tempo.bar_start_tick(bar + 1));
            let beat = s.tempo.meter_at_tick(t0).ticks_per_beat().max(1);
            // Multiples of the step from tick 0, as `snap` rounds to them.
            for tick in ((t0 + step - 1).div_euclid(step) * step..t1).step_by(step as usize) {
                if (tick - t0) % beat == 0 {
                    continue;
                }
                let sx = map.x(s.tempo.tick_to_samples(tick, s.sample_rate));
                if sx >= tl.min.x - 1.0 && sx <= tl.max.x {
                    gp.line_segment([pos2(sx, tl.min.y), pos2(sx, tl.max.y)], Stroke::new(1.0, TOKENS.grid_sub));
                }
            }
        }
        if (bar - 1) % every == 0 {
            gp.line_segment([pos2(x, tl.min.y), pos2(x, tl.max.y)], Stroke::new(1.0, TOKENS.grid_bar));
            rp.line_segment([pos2(x, ruler.min.y + crate::loopbar::STRIP_H + 10.0), pos2(x, ruler.max.y)], Stroke::new(1.0, TOKENS.text_dim));
            rp.text(
                pos2(x + 4.0, ruler.min.y + crate::loopbar::STRIP_H + 9.0),
                Align2::LEFT_CENTER,
                bar.to_string(),
                FontId::proportional(11.0),
                TOKENS.text,
            );
        } else if bar_px >= 6.0 {
            gp.line_segment([pos2(x, tl.min.y), pos2(x, tl.max.y)], Stroke::new(1.0, TOKENS.grid_beat));
        }
    }
    let _ = app;
}

/// Does a clip drag with these modifiers copy instead of move? Ctrl (Cmd on a Mac; egui's
/// `command`) is Ableton's copy-drag; Alt is Logic's. The shortcut table binds neither to a drag.
pub fn drag_copies(m: egui::Modifiers) -> bool {
    m.command || m.alt
}

/// Where a dragged clip's start lands: `start` moved by `dx` (through `snap`, the session grid's
/// snapped delta, unless Shift is held), never before zero. Moves and copies share this.
pub fn drop_start(start: Samples, dx: Samples, shift: bool, snap: impl Fn(Samples) -> Samples) -> Samples {
    start.saturating_add(if shift { dx } else { snap(dx) }).max(0)
}

/// Where a trimmed edge lands: `edge` moved by `dx` and snapped as a position to the visible grid
/// (like the loop handle; a move snaps its delta instead), unless Shift is held. The caller clamps
/// against the other edge; the engine command clamps against neighbors and the source.
fn trim_edge(app: &StudioApp, map: Map, edge: Samples, dx: Samples, shift: bool) -> Samples {
    let to = edge.saturating_add(dx).max(0);
    if shift { to } else { crate::automation::snap(app.engine.session(), map.spp, &map.grid, to) }
}

/// The clip being dragged, as it would land: (track, start, end).
fn drag_preview(app: &StudioApp, ui: &Ui, map: Map, rows: &[&Track], tl: Rect, geo: &RowGeo) -> Option<(ClipId, TrackId, Samples, Samples, bool)> {
    let Some(Gesture::Clip { clip, track, kind, press, start, end }) = app.arrange.gesture.clone() else { return None };
    let (pos, m) = ui.input(|i| (i.pointer.latest_pos(), i.modifiers));
    let pos = pos?;
    let dx = f64::from(pos.x - press.x) * map.spp;
    let dx = dx.round() as Samples;
    Some(match kind {
        DragKind::Move => {
            let s = app.engine.session();
            let new_start = drop_start(start, dx, m.shift, |d| crate::automation::snap_delta(s, map.spp, &map.grid, start, d));
            let row = geo.index_at(pos.y - tl.min.y + app.arrange.vscroll);
            let dest = row.and_then(|i| rows.get(i)).map(|t| t.id).filter(|t| app.holds_clips(*t)).unwrap_or(track);
            (clip, dest, new_start, new_start + (end - start), drag_copies(m))
        }
        DragKind::TrimStart => (clip, track, trim_edge(app, map, start, dx, m.shift).clamp(0, end - 1), end, false),
        DragKind::TrimEnd => (clip, track, start, trim_edge(app, map, end, dx, m.shift).max(start + 1), false),
        DragKind::Loop | DragKind::FadeIn | DragKind::FadeOut | DragKind::FadeCurveIn | DragKind::FadeCurveOut => return None,
    })
}

/// What is under a point: a clip (and whether near its start or end edge) and the row's track.
fn hit(layout: &Layout, pos: Pos2) -> (Option<(ClipId, TrackId, DragKind, Samples, Samples)>, Option<TrackId>) {
    let track = layout.tracks.iter().find(|r| pos.y >= r.lane[1] && pos.y < r.lane[3]).map(|r| TrackId(r.id));
    // Topmost (last drawn) first.
    let clip = layout
        .clips
        .iter()
        .rev()
        .find(|c| pos.x >= c.rect[0] && pos.x <= c.rect[2] && pos.y >= c.rect[1] && pos.y <= c.rect[3])
        .map(|c| (ClipId(c.id), TrackId(c.track), zone(c.rect, pos, c.audio), c.start, c.end));
    (clip, track)
}

#[allow(clippy::too_many_arguments)]
fn timeline_input(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, tl: Rect, rows: &[&Track], geo: &RowGeo, layout: &Layout) {
    let resp = ui.interact(tl, ui.id().with("timeline"), Sense::click_and_drag());
    if let Some(pos) = resp.hover_pos()
        && app.arrange.gesture.is_none()
    {
        let over = hit(layout, pos).0;
        // The fade cursor over a fade corner, a drawn fade or its curve (#182).
        if let Some(t) = over.and_then(|(_, _, kind, _, _)| crate::fades::grab_at(s, map, layout, pos, kind)) {
            ui.ctx().set_cursor_icon(t.grab.cursor());
        } else {
            match over {
                Some((_, _, DragKind::TrimStart | DragKind::TrimEnd, _, _)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Some((_, _, DragKind::Loop, _, _)) => loop_cursor(ui.ctx(), pos),
                Some(_) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                None => {}
            }
        }
    }
    if app.arrange.gesture.is_none()
        && let Some(pos) = resp.hover_pos()
        && let Some((_, _, DragKind::Loop, _, _)) = hit(layout, pos).0
    {
        resp.clone().on_hover_text("Drag right to loop this clip (Shift: no snap, Alt: independent copies, Ctrl/Cmd: overwrite the clips in the way instead of stopping at them). Drag back to shorten or unloop.");
    }
    if resp.drag_started()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
        && let (Some((clip, track, kind, start, end)), _) = hit(layout, press)
    {
        if !s.edit.selected_clips.contains(&clip) {
            app.click_clip(clip, false);
        }
        // A press on a fade corner, a drawn fade or its curve line takes the fade, whichever clip
        // of an overlap draws it (#182); Alt only slows the drag.
        let (clip, track, kind, start, end) = match crate::fades::grab_at(s, map, layout, press, kind) {
            Some(t) => {
                if !s.edit.selected_clips.contains(&t.clip) {
                    app.click_clip(t.clip, false);
                }
                (t.clip, t.track, t.grab.kind(), t.start, t.end)
            }
            None => (clip, track, kind, start, end),
        };
        app.arrange.gesture = Some(Gesture::Clip { clip, track, kind, press, start, end });
    }
    if resp.dragged()
        && let Some(Gesture::Clip { kind, .. }) = &app.arrange.gesture
    {
        match kind {
            DragKind::FadeIn => ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe),
            DragKind::FadeOut => ui.ctx().set_cursor_icon(CursorIcon::ResizeNeSw),
            DragKind::FadeCurveIn | DragKind::FadeCurveOut => ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical),
            _ => {}
        }
    }
    if resp.dragged() && matches!(app.arrange.gesture, Some(Gesture::Clip { kind: DragKind::Move, .. })) {
        let copy = ui.input(|i| drag_copies(i.modifiers));
        ui.ctx().set_cursor_icon(if copy { CursorIcon::Copy } else { CursorIcon::Grabbing });
    }
    if resp.dragged()
        && matches!(app.arrange.gesture, Some(Gesture::Clip { kind: DragKind::Loop, .. }))
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        loop_cursor(ui.ctx(), pos);
    }
    if resp.drag_stopped() {
        let preview = drag_preview(app, ui, map, rows, tl, geo);
        let looped = loop_drag(app, ui, map);
        let fade = crate::fades::drag(app, ui, map);
        let gesture = app.arrange.gesture.take();
        if let Some(f) = fade {
            crate::fades::commit(app, &f);
        }
        if let Some(l) = looped {
            commit_loop(app, &l);
        }
        if let (Some(Gesture::Clip { kind, track, start, end, .. }), Some((clip, dest, a, b, copy))) = (gesture, preview) {
            commit_drag(app, kind, clip, track, dest, start, end, a, b, copy);
        }
    }
    marquee::input(app, ui, &resp, map, tl, layout, (geo.total() + ADD_STACK_H - tl.height()).max(0.0));
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let additive = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        match hit(layout, pos) {
            (Some((clip, ..)), _) => {
                app.click_clip(clip, additive);
                // A click on a fade opens its shape menu.
                app.arrange.fade_menu = match crate::fades::hit(s, map, layout, pos) {
                    Some((c, side)) if c == clip && !additive => Some(crate::fades::Menu { clip, side, at: pos + vec2(8.0, 8.0) }),
                    _ => None,
                };
            }
            (None, track) => app.click_empty(track, map.at(pos.x)),
        }
    }
    // Double-click a fade: back to the default shape and no bend.
    let on_fade = if resp.double_clicked() { resp.interact_pointer_pos().and_then(|pos| crate::fades::hit(s, map, layout, pos)) } else { None };
    if let Some((clip, side)) = on_fade {
        app.arrange.fade_menu = None;
        crate::fades::reset(app, clip, side);
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let (None, Some(track)) = hit(layout, pos)
        && s.track(track).is_some_and(|t| t.kind.is_midi())
    {
        let _ = app.new_midi_clip(Some(track));
    }
    // Slice 3: double-click a MIDI clip to edit it in the piano roll; slice 6: double-click its
    // name strip to rename it.
    if resp.double_clicked()
        && on_fade.is_none()
        && let Some(pos) = resp.interact_pointer_pos()
        && let (Some((clip, ..)), _) = hit(layout, pos)
    {
        let strip = layout.clips.iter().find(|c| c.id == clip.0).map(|c| c.rect).filter(|r| pos.y <= r[1] + CLIP_NAME_H);
        match strip {
            Some(r) => {
                let x0 = r[0].max(tl.min.x);
                let field = [x0, r[1], r[2].min(tl.max.x).max(x0 + 160.0), r[1] + CLIP_NAME_H + 6.0];
                app.start_rename(crate::finish::RenameTarget::Clip(clip), field);
            }
            None => {
                app.open_keys(clip);
            }
        }
    }
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        app.arrange.menu_target = match hit(layout, pos).0 {
            Some((clip, ..)) => {
                if !s.edit.selected_clips.contains(&clip) {
                    app.click_clip(clip, false);
                }
                Some((clip, crate::fades::hit(s, map, layout, pos).filter(|h| h.0 == clip).map(|h| h.1)))
            }
            None => None,
        };
    }
    crate::fades::click_menu(app, ui);
    resp.context_menu(|ui| {
        if let Some((clip, side)) = app.arrange.menu_target.filter(|(c, _)| s.find_clip(*c).is_some()) {
            if let Some(side) = side
                && crate::fades::shape_items(app, ui, clip, side)
            {
                ui.close();
            }
            if crate::fades::overlap(s, clip).is_some() {
                let b = ui.button("Crossfade");
                app.arrange.menu.retain(|m| m.label != "Crossfade");
                app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Crossfade".into(), String::new(), b.rect));
                if b.clicked() {
                    crate::fades::crossfade(app, clip);
                    ui.close();
                }
            }
            ui.separator();
        }
        if ui.button("Add Mono Audio Track").clicked() {
            let _ = app.add_audio_track("mono");
            ui.close();
        }
        if ui.button("Add Stereo Audio Track").clicked() {
            let _ = app.add_audio_track("stereo");
            ui.close();
        }
        if ui.button("Add Instrument Track").clicked() {
            let _ = app.add_track("instrument");
            ui.close();
        }
        if ui.button("Add MIDI Track").clicked() {
            let _ = app.add_track("midi");
            ui.close();
        }
        ui.separator();
        if ui.button("Paste at Cursor").clicked() {
            let _ = app.paste_at_cursor();
            ui.close();
        }
        if ui.button("Split at Cursor").clicked() {
            let _ = app.split_at_cursor();
            ui.close();
        }
        ui.separator();
        crate::edit_tools::context_menu_items(app, ui);
    });
}

/// A loop-handle drag as it would land.
struct LoopDrag {
    clip: ClipId,
    /// The clip's end now, and the drop point (snapped unless Shift is held): the new end.
    from: Samples,
    to: Samples,
    /// A cycle's length, and the clip-time end of the first pass (where unlooping cuts to).
    cycle: Samples,
    first_end: Samples,
    /// The clip starts here.
    start: Samples,
    looped: bool,
    /// Alt: plain copies instead of cycles.
    independent: bool,
    /// Cmd/Ctrl: cut into the clips after this one instead of stopping at the next.
    overwrite: bool,
}

impl LoopDrag {
    /// The ghost spans, in timeline samples: beyond the clip's end the cycles (or copies) it
    /// would gain, the last one cut at the drop point.
    fn ghosts(&self) -> Vec<(Samples, Samples)> {
        if self.to <= self.from {
            return Vec::new();
        }
        if self.independent {
            return loop_ghosts(self.from, self.from - self.start, self.to);
        }
        // Cycle boundaries sit at first_end + k * cycle; the first ghost starts where the clip ends.
        let seam = self.start + self.first_end;
        let mut out = Vec::new();
        let mut a = self.from;
        while a < self.to && out.len() < MAX_LOOP_REPEATS {
            let next = seam + ((a - seam).div_euclid(self.cycle.max(1)) + 1) * self.cycle.max(1);
            out.push((a, next.min(self.to)));
            a = next;
        }
        out
    }
    /// The part of the clip the drop would cut off (dragging left).
    fn cut(&self) -> Option<(Samples, Samples)> {
        let to = if self.looped && !self.independent && self.to <= self.start + self.first_end { self.start + self.first_end } else { self.to };
        (!self.independent && self.looped && to < self.from).then_some((to, self.from))
    }
    /// Does dropping here change anything?
    fn changes(&self) -> bool {
        if self.independent {
            self.to > self.from
        } else if self.looped {
            self.to != self.from
        } else {
            self.to > self.from
        }
    }
}

/// The loop handle drag in progress, from the pointer: its travel moves the clip's end.
fn loop_drag(app: &StudioApp, ui: &Ui, map: Map) -> Option<LoopDrag> {
    let Some(Gesture::Clip { clip, kind: DragKind::Loop, press, end, .. }) = app.arrange.gesture.clone() else { return None };
    let (pos, m) = ui.input(|i| (i.pointer.latest_pos(), i.modifiers));
    let s = app.engine.session();
    let c = s.find_clip(clip)?.1;
    let dx = (f64::from(pos?.x - press.x) * map.spp).round() as Samples;
    let to = end.saturating_add(dx).max(0);
    let to = if m.shift { to } else { crate::automation::snap_delta(s, map.spp, &map.grid, 0, to) };
    let overwrite = m.command && !m.alt;
    // Without the modifier a longer loop stops at the next clip, as the command does.
    let to = match (overwrite, next_clip_start(s, clip)) {
        (false, Some(next)) if to > c.end() && !m.alt => to.min(next.max(c.end())),
        _ => to,
    };
    let l = c.looping.filter(|l| l.is_valid());
    let (cycle, first_end) = l.map_or((c.length, c.length), |l| (l.cycle(), l.end));
    Some(LoopDrag { clip, from: c.end(), to, cycle, first_end, start: c.start, looped: l.is_some(), independent: m.alt, overwrite })
}

/// Where the next clip on the clip's track starts, if any.
fn next_clip_start(s: &Session, clip: ClipId) -> Option<Samples> {
    let (track, c) = s.find_clip(clip)?;
    s.track(track)?.clips().iter().filter(|o| o.id != clip && o.start > c.start).map(|o| o.start).min()
}

/// Where an overwriting loop drag stops: the drop point, or the start of the edit-locked clip the
/// commit will not run into (the same rule as `clip.loop_to`).
fn overwrite_stop(s: &Session, l: &LoopDrag) -> Samples {
    let Some((track, _)) = s.find_clip(l.clip) else { return l.to };
    let lock = s
        .track(track)
        .map(|t| t.clips())
        .unwrap_or_default()
        .iter()
        .filter(|o| o.edit_locked && o.id != l.clip && o.end() > l.from)
        .map(|o| o.start.max(l.from))
        .min();
    lock.map_or(l.to, |x| l.to.min(x))
}

/// The spans of other clips on the track that an overwriting loop drag would cut into. An
/// edit-locked clip stops the drag, so it and anything past it is not shaded.
fn overwritten(s: &Session, l: &LoopDrag) -> Vec<(Samples, Samples)> {
    if !l.overwrite || l.independent || l.to <= l.from {
        return Vec::new();
    }
    let Some((track, _)) = s.find_clip(l.clip) else { return Vec::new() };
    let Some(t) = s.track(track) else { return Vec::new() };
    let to = overwrite_stop(s, l);
    t.clips()
        .iter()
        .filter(|o| o.id != l.clip && !o.edit_locked)
        .filter(|o| o.start < to && o.end() > l.from)
        .map(|o| (o.start.max(l.from), o.end().min(to)))
        .collect()
}

/// Commit a loop drag: one `clip.loop_to`, so one undo step.
fn commit_loop(app: &mut StudioApp, l: &LoopDrag) {
    if !l.changes() {
        return;
    }
    let _ = app.run("clip.loop_to", json!({"clip": l.clip.0, "to": l.to, "independent": l.independent, "overwrite": l.overwrite}));
}

/// Turn a finished drag into commands.
#[allow(clippy::too_many_arguments)]
fn commit_drag(
    app: &mut StudioApp,
    kind: DragKind,
    clip: ClipId,
    from: TrackId,
    dest: TrackId,
    start: Samples,
    end: Samples,
    a: Samples,
    b: Samples,
    copy: bool,
) {
    match kind {
        DragKind::Move => {
            let by = a - start;
            if by == 0 && dest == from {
                if copy {
                    app.status = "Ctrl-drag (Alt-drag) the clip somewhere else to copy it.".into();
                }
                return;
            }
            let set = copy_drag::drag_set(app, clip);
            if copy {
                copy_drag::copy(app, &set, a, by, dest);
                return;
            }
            // Several clips move in time only; one clip may change track.
            let to = (set.len() == 1 && dest != from).then_some(dest);
            let _ = app.move_clips(&set, by, to);
        }
        DragKind::TrimStart if a != start => {
            let _ = app.trim_clip(clip, true, a);
        }
        DragKind::TrimEnd if b != end => {
            let _ = app.trim_clip(clip, false, b);
        }
        _ => {}
    }
}

fn ruler_input(app: &mut StudioApp, ui: &mut Ui, map: Map, ruler: Rect) {
    let resp = ui.interact(ruler, ui.id().with("ruler"), Sense::click_and_drag());
    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        app.click_ruler(map.at(pos.x.clamp(ruler.min.x, ruler.max.x)));
    }
}

/// Recording takes, cursor and playhead (the loop block is `loopbar::block`).
#[allow(clippy::too_many_arguments)]
fn overlays(app: &StudioApp, ui: &Ui, s: &Session, map: Map, ruler: Rect, tl: Rect, rows: &[&Track], geo: &RowGeo, layout: &mut Layout) {
    let p = ui.painter();
    // The take being recorded grows on each armed track where it will land (slice 6: inside the
    // punch range, after a count-in), with a "Recording" name strip.
    let span = app.engine.transport.recording.then(|| {
        let pass = app.take_plan.as_ref().and_then(|p| p.pass);
        transit_engine::io::live_span(app.record_start, pass, app.punch, (app.position() - app.record_start).max(0))
    });
    if let Some(Some((_, span))) = span {
        let rp = p.with_clip_rect(tl);
        for (i, t) in rows.iter().enumerate() {
            if t.mixer.record_arm && !app.midi_live.as_ref().is_some_and(|live| live.tracks.contains(&t.id)) {
                let y0 = tl.min.y + geo.tops[i] - app.arrange.vscroll;
                let r = Rect::from_min_max(pos2(map.x(span.start), y0 + 1.0), pos2(map.x(span.end).max(map.x(span.start) + 2.0), y0 + geo.rh - 1.0));
                rp.rect_filled(r, crate::theme::RADIUS, TOKENS.record.gamma_multiply(0.45));
                let strip = Rect::from_min_max(r.min, pos2(r.max.x, (r.min.y + CLIP_NAME_H).min(r.max.y)));
                rp.rect_filled(strip, crate::theme::top_corners(), TOKENS.record);
                let tp = rp.with_clip_rect(strip.intersect(tl));
                tp.text(
                    pos2(strip.min.x + 4.0, strip.center().y),
                    Align2::LEFT_CENTER,
                    "Recording",
                    FontId::proportional(11.0),
                    theme::text_on(TOKENS.record),
                );
            }
        }
    }
    midi_recording(app, ui, s, map, tl, rows, geo, layout);
    let full = Rect::from_min_max(ruler.min, tl.max);
    let lp = p.with_clip_rect(full);
    let cx = map.x(app.cursor());
    lp.line_segment([pos2(cx, ruler.min.y), pos2(cx, tl.max.y)], Stroke::new(1.0, TOKENS.cursor.gamma_multiply(0.8)));
    let tri = vec![pos2(cx - 5.0, ruler.max.y - 8.0), pos2(cx + 5.0, ruler.max.y - 8.0), pos2(cx, ruler.max.y - 1.0)];
    lp.add(egui::Shape::convex_polygon(tri, TOKENS.cursor, Stroke::NONE));
    if app.is_playing() {
        let px = map.x(app.engine.transport.position);
        lp.line_segment([pos2(px, ruler.min.y), pos2(px, tl.max.y)], Stroke::new(1.5, TOKENS.accent));
    }
}

/// The MIDI take being recorded grows on each armed instrument lane from the latch to the playhead,
/// with the notes played so far drawn inside it.
#[allow(clippy::too_many_arguments)]
fn midi_recording(app: &StudioApp, ui: &Ui, s: &Session, map: Map, tl: Rect, rows: &[&Track], geo: &RowGeo, layout: &mut Layout) {
    let Some(live) = &app.midi_live else { return };
    let rp = ui.painter().with_clip_rect(tl);
    for (i, t) in rows.iter().enumerate() {
        if !live.tracks.contains(&t.id) {
            continue;
        }
        let y0 = tl.min.y + geo.tops[i] - app.arrange.vscroll;
        let (x0, x1) = (map.x(live.start), map.x(live.end).max(map.x(live.start) + 2.0));
        let r = Rect::from_min_max(pos2(x0, y0 + 1.0), pos2(x1, y0 + geo.rh - 1.0));
        rp.rect_filled(r, crate::theme::RADIUS, TOKENS.record.gamma_multiply(0.45));
        let strip = Rect::from_min_max(r.min, pos2(r.max.x, (r.min.y + CLIP_NAME_H).min(r.max.y)));
        rp.rect_filled(strip, crate::theme::top_corners(), TOKENS.record);
        rp.with_clip_rect(strip.intersect(tl)).text(
            pos2(strip.min.x + 4.0, strip.center().y),
            Align2::LEFT_CENTER,
            "Recording",
            FontId::proportional(11.0),
            theme::text_on(TOKENS.record),
        );
        // Provisional notes: pitch maps onto the lane under the name strip.
        let mine: Vec<_> = live.built.iter().filter(|b| b.track == t.id).collect();
        let (lo, hi) = mine.iter().flat_map(|b| b.sequence.notes.iter().map(|n| n.pitch)).fold((127u8, 0u8), |(lo, hi), p| (lo.min(p), hi.max(p)));
        let body = Rect::from_min_max(pos2(r.min.x, strip.max.y + 2.0), pos2(r.max.x, r.max.y - 2.0));
        let span = f32::from(hi.saturating_sub(lo).max(11)) + 1.0;
        let mut notes = Vec::new();
        for b in mine {
            let base = s.tempo.samples_to_ticks(b.range.start, s.sample_rate);
            for n in &b.sequence.notes {
                let (a, e) = (s.tempo.tick_to_samples(base + n.start, s.sample_rate), s.tempo.tick_to_samples(base + n.end(), s.sample_rate));
                let h = (body.height() / span).clamp(1.5, 6.0);
                let y = body.max.y - (f32::from(n.pitch.saturating_sub(lo)) + 1.0) * body.height() / span;
                let nr = Rect::from_min_max(pos2(map.x(a), y), pos2(map.x(e).max(map.x(a) + 1.5), y + h)).intersect(body);
                if nr.is_positive() {
                    rp.rect_filled(nr, crate::theme::RADIUS_SM, theme::text_on(TOKENS.record));
                }
                notes.push(r4(nr));
            }
        }
        layout.recording.push(RecordingLane { track: t.id.0, rect: r4(r), notes });
    }
}

/// Fraction of the visible width one wheel notch pans.
const PAN_PER_NOTCH: f64 = 0.10;

/// A horizontal wheel delta in points as a fraction of the visible width (positive: content moves
/// right, so the view moves earlier). A notch is `line_scroll_speed` points, so the pan does not
/// depend on zoom or on `pixels_per_point`. egui's wheel state already folds Shift+vertical into
/// `x` (`egui::input_state::wheel_state::on_wheel_event`: `delta = vec2(delta.x + delta.y, 0.0)`),
/// so callers must not test Shift and read `y` themselves.
pub(crate) fn wheel_pan(ui: &Ui, dx: f32) -> f64 {
    let notch = ui.ctx().options(|o| o.input_options.line_scroll_speed).max(1.0);
    f64::from(dx / notch) * PAN_PER_NOTCH
}

/// Wheel scrolls the rows, Shift+wheel or a sideways swipe scrolls time, Ctrl+wheel or a pinch zooms.
fn scroll_and_zoom(app: &mut StudioApp, ui: &Ui, map: Map, full: Rect, tl: Rect, content_h: f32) {
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else { return };
    if !full.contains(pos) {
        return;
    }
    let (delta, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
    if (zoom - 1.0).abs() > 1e-4 {
        let _ = app.engine.execute("view.zoom_by", &json!({"factor": zoom, "x": (pos.x - tl.min.x).max(0.0)}));
        return;
    }
    // egui has already turned Shift+vertical wheel into a horizontal delta (see `wheel_pan`), so
    // time pans on `delta.x` alone, whether it came from Shift, a tilt wheel or a swipe.
    if delta.x.abs() > 0.1 {
        let by_px = -wheel_pan(ui, delta.x) * f64::from(tl.width());
        let _ = app.engine.execute("view.scroll", &json!({"by_px": by_px}));
    }
    if delta.y.abs() > 0.1 {
        app.arrange.vscroll = (app.arrange.vscroll - delta.y).clamp(0.0, (content_h - tl.height()).max(0.0));
    }
    let _ = map;
}

/// While playing, page the view so the playhead stays on screen.
fn follow_playhead(app: &mut StudioApp, map: Map, tl: Rect) {
    if !app.is_playing() || app.arrange.gesture.is_some() {
        return;
    }
    let x = map.x(app.engine.transport.position);
    if x > tl.max.x - 8.0 || x < tl.min.x {
        let to = app.engine.transport.position - (f64::from(tl.width()) * map.spp * 0.05) as Samples;
        let _ = app.engine.execute("view.scroll", &json!({"to": to.max(0)}));
    }
}

/// Slice 6: a chip on each clip that other takes overlap ("2 takes", click to show or hide the
/// track's take lanes), and the take lanes of tracks whose takes are shown: one lane per playlist
/// with its clips as bars (click: that take plays), and in the track list its label with Promote
/// (keep only this take) and Delete. Drawn after the timeline's input, so these take the clicks.
#[allow(clippy::too_many_arguments)]
fn takes(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, tl: Rect, list: Rect, rows: &[&Track], geo: &RowGeo, layout: &Layout) {
    let _ = s;
    for (i, t) in rows.iter().enumerate() {
        if t.playlists.len() < 2 {
            continue;
        }
        let y0 = tl.min.y + geo.tops[i] - app.arrange.vscroll;
        let open = app.ui.takes_open.contains(&t.id);
        for c in t.clips() {
            let others = t
                .playlists
                .iter()
                .enumerate()
                .filter(|(k, pl)| *k != t.active_playlist && pl.clips.iter().any(|o| o.start < c.end() && o.end() > c.start))
                .count();
            let Some(cr) = layout.clips.iter().find(|r| r.id == c.id.0).map(|r| r.rect) else { continue };
            let wide = cr[2] - cr[0] >= 64.0 + 8.0;
            if others == 0 {
                continue;
            }
            // Wide clips get the "2 takes" chip at their right end; narrower ones a 10 px glyph at
            // their left edge, so the lanes stay reachable at any zoom.
            let chip = if wide {
                Rect::from_min_max(pos2(cr[2] - 64.0 - 2.0, cr[1] + 1.5), pos2(cr[2] - 2.0, cr[1] + CLIP_NAME_H - 0.5))
            } else {
                Rect::from_min_max(pos2(cr[0].max(tl.min.x) + 1.0, cr[1] + 1.5), pos2(cr[0].max(tl.min.x) + 11.0, cr[1] + CLIP_NAME_H - 0.5))
            };
            if !tl.intersects(chip) {
                continue;
            }
            let resp = ui.interact(chip.intersect(tl), ui.id().with(("take_chip", c.id.0)), Sense::click());
            let p = ui.painter().with_clip_rect(tl);
            p.rect_filled(chip, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
            let icon = if open { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::ChevronRight };
            if wide {
                crate::icons::paint(&p, pos2(chip.min.x + 8.0, chip.center().y), icon, 12.0, TOKENS.text);
                p.text(
                    pos2(chip.min.x + 16.0, chip.center().y),
                    Align2::LEFT_CENTER,
                    format!("{} takes", others + 1),
                    FontId::proportional(10.5),
                    TOKENS.text,
                );
            } else {
                crate::icons::paint(&p, chip.center(), icon, 10.0, TOKENS.text);
            }
            if resp.on_hover_text(if open { "Hide the takes" } else { "Show the takes" }).clicked() {
                app.toggle_takes(t.id);
            }
            app.finish.layout.chips.push(crate::finish::TakeChip { track: t.id.0, clip: c.id.0, takes: others + 1, open, rect: r4(chip) });
        }
        if !open {
            continue;
        }
        let n = t.playlists.len();
        for (k, pl) in t.playlists.iter().enumerate() {
            let ly = y0 + geo.rh + k as f32 * TAKE_H;
            let lane = Rect::from_min_max(pos2(tl.min.x, ly), pos2(tl.max.x, ly + TAKE_H));
            let active = k == t.active_playlist;
            let p = ui.painter().with_clip_rect(tl);
            p.rect_filled(lane.shrink2(vec2(0.0, 1.0)), 0.0, if active { TOKENS.accent.gamma_multiply(0.2) } else { TOKENS.bg });
            for c in &pl.clips {
                let r = Rect::from_min_max(pos2(map.x(c.start), ly + 4.0), pos2(map.x(c.end()).max(map.x(c.start) + 2.0), ly + TAKE_H - 4.0));
                let col = theme::rgb(c.color.unwrap_or(t.color));
                p.rect_filled(r, crate::theme::RADIUS_SM, if active { col } else { col.gamma_multiply(0.5) });
            }
            let lresp = ui.interact(lane.intersect(tl), ui.id().with(("take_lane", t.id.0, k)), Sense::click());
            // In the track list: the take's label, Promote and Delete.
            let lp = ui.painter().with_clip_rect(list);
            let hl = Rect::from_min_max(pos2(list.min.x + 20.0, ly + 1.0), pos2(list.max.x - 6.0, ly + TAKE_H - 1.0));
            let del = Rect::from_min_size(pos2(hl.max.x - 20.0, hl.min.y + 1.0), vec2(20.0, hl.height() - 2.0));
            let pro = Rect::from_min_size(pos2(del.min.x - 70.0, hl.min.y + 1.0), vec2(66.0, hl.height() - 2.0));
            let label = Rect::from_min_max(hl.min, pos2(pro.min.x - 4.0, hl.max.y));
            let hresp = ui.interact(label.intersect(list), ui.id().with(("take_label", t.id.0, k)), Sense::click());
            if active {
                lp.rect_filled(label, crate::theme::RADIUS_SM, TOKENS.accent.gamma_multiply(0.2));
            }
            let text = if active { format!("Take {} · playing", k + 1) } else { format!("Take {}", k + 1) };
            lp.text(
                pos2(label.min.x + 6.0, label.center().y),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(11.0),
                if active { TOKENS.text } else { TOKENS.text_dim },
            );
            let presp = ui.interact(pro.intersect(list), ui.id().with(("take_promote", t.id.0, k)), Sense::click());
            lp.rect_filled(pro, crate::theme::RADIUS_SM, if presp.hovered() { TOKENS.button_hover } else { TOKENS.button });
            lp.text(pro.center(), Align2::CENTER_CENTER, "Promote", FontId::proportional(10.5), TOKENS.text);
            let can_delete = n == 2;
            let dresp = ui.interact(del.intersect(list), ui.id().with(("take_delete", t.id.0, k)), Sense::click());
            lp.rect_filled(del, crate::theme::RADIUS_SM, if dresp.hovered() && can_delete { TOKENS.button_hover } else { TOKENS.button });
            let ink = if can_delete { TOKENS.text } else { TOKENS.text_dim.gamma_multiply(0.5) };
            crate::icons::paint(&lp, del.center(), crate::icons::Icon::Remove, 12.0, ink);
            app.finish.layout.takes.push(crate::finish::TakeRow {
                track: t.id.0,
                index: k,
                active,
                clips: pl.clips.len(),
                lane: r4(lane),
                label: r4(label),
                promote: r4(pro),
                delete: r4(del),
                delete_enabled: can_delete,
            });
            if (lresp.clicked() || hresp.clicked()) && !active {
                let _ = app.select_take(t.id, k);
            }
            if presp.on_hover_text("Keep this take and remove the others").clicked() {
                let _ = app.promote_take(t.id, k);
            }
            let tip =
                if can_delete { "Delete this take" } else { "Deleting one of three or more takes needs an engine command; promote a take instead" };
            if dresp.on_hover_text(tip).clicked() && can_delete {
                app.dialog = Some(crate::Dialog::DeleteTake { track: t.id, index: k });
            }
        }
    }
}
