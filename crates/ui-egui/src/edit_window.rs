//! The Edit window: toolbar, rulers, track headers and playlists.

mod rulers;
use rulers::draw_rulers;
pub use rulers::ruler_height;
mod tracks;
pub(crate) use tracks::track_context_menu;
use tracks::{column_titles, track_row, views_button};
mod painting;
use painting::{clip_name_height, draw_automation, draw_clip, draw_live_take, fade_control_position, grid_lines, plugin_range};
mod interaction;
pub use interaction::{CLIP_FADER_TRAVEL, clip_gain_icon};
use interaction::{drag_plan, drive_drag, fade_drag_preview, lane_interaction, midi_clip_span, near_fade_midpoint, snap};
mod overlay;
use overlay::overlay;

#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "edit_window/tests/frame.rs"]
mod tests;

use crate::dialogs::Dialog;
use crate::spot::Spot;
use crate::theme::{Tokens, bold, clip_colors, regular, rgb};
use crate::widgets::{groove, level_pos, meter_pos, raised_key, shade};
use crate::{Gesture, SoundApp, toolbar};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use transit_engine::edit as ops;
use transit_model::{AutoParam, Clip, ClipContent, ClipId, EditMode, Session, Tool, Track, TrackHeight, TrackId, TrackKind};
use transit_time::{GridValue, NoteValue, Range, Samples, TimeFormat, format_position};

pub const HEADER_W: f32 = 136.0;
pub const COLUMN_W: f32 = 112.0;
/// Insert, send and instrument columns: Pro Tools' narrow stacks of slots.
const PLUGIN_COLUMN_W: f32 = 75.0;
/// The track colour strip at the left of each track header.
const STRIP_W: f32 = 14.0;
/// The row under the rulers that holds the column titles, when any column is shown.
const COLUMN_TITLES_H: f32 = 16.0;

/// An optional Edit-window column: view id, menu label, title and width.
type Column = (&'static str, &'static str, &'static str, f32);

const COLUMNS: [Column; 7] = [
    ("instruments", "Instruments", "INSTRUMENT", PLUGIN_COLUMN_W),
    ("inserts_ae", "Inserts A-E", "INSERTS A-E", PLUGIN_COLUMN_W),
    ("inserts_fj", "Inserts F-J", "INSERTS F-J", PLUGIN_COLUMN_W),
    ("sends_ae", "Sends A-E", "SENDS A-E", PLUGIN_COLUMN_W),
    ("sends_fj", "Sends F-J", "SENDS F-J", PLUGIN_COLUMN_W),
    ("io", "I/O", "I/O", 94.0),
    ("comments", "Comments", "COMMENTS", COLUMN_W),
];

/// Optional Edit-window columns (View › Edit Window Views) that are switched on, left to right.
pub fn edit_columns(s: &Session) -> Vec<Column> {
    let all = s.edit.flag("edit_view.all");
    COLUMNS.into_iter().filter(|c| all || s.edit.flag(&format!("edit_view.{}", c.0))).collect()
}

pub fn header_width(cols: &[Column]) -> f32 {
    HEADER_W + cols.iter().map(|c| c.3).sum::<f32>()
}
pub const RULER_H: f32 = 17.0;

/// Geometry of the last frame, for hit testing and agents (`ui.inspect`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EditLayout {
    pub timeline: [f32; 4],
    pub rows: Vec<(u64, [f32; 4])>,
    pub scroll_y: f32,
    pub content_h: f32,
    /// While Zoom Toggle is on: the vertical scroll to go back to.
    zoom_back: Option<f32>,
    /// Follow-hold: the view was moved by something other than the playhead
    /// follow (a manual pan, scrollbar, universe jump, or programmatic
    /// scroll) during playback, so timeline auto-scroll stays paused until
    /// the playhead is scrolled back into view (or the transport
    /// stops/starts, or a scrolling mode is re-selected). Lets the timeline
    /// be inspected anywhere while playing.
    pub follow_hold: bool,
    /// Session `zoom.scroll` seen by the last overlay frame, for telling our
    /// own follow jumps apart from outside view moves.
    pub last_scroll: Samples,
    /// Follow target issued by the last overlay frame (`None` when none).
    pub last_follow_to: Option<Samples>,
    #[serde(skip)]
    pub scroll_carry: f64,
    /// The current swipe's movement per axis until either passes 6 points; it scrolls only along the larger.
    #[serde(skip)]
    swipe: egui::Vec2,
}

impl EditLayout {
    pub fn scroll_by(&mut self, dy: f32, view_h: f32) {
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, (self.content_h - view_h + 40.0).max(0.0));
    }
}

pub fn rect([x0, y0, x1, y1]: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

fn lane_rect(track: &Track, row: Rect, tl: Rect) -> Rect {
    Rect::from_min_max(pos2(tl.min.x, row.min.y), pos2(tl.max.x, row.min.y + track.height.points()))
}

pub fn x_of(s: &Session, tl: Rect, at: Samples) -> f32 {
    tl.min.x + ((at - s.edit.zoom.scroll) as f64 / s.edit.zoom.samples_per_px.max(0.01)) as f32
}

pub fn sample_at(s: &Session, tl: Rect, x: f32) -> Samples {
    s.edit.zoom.scroll + transit_time::to_samples(f64::from(x - tl.min.x) * s.edit.zoom.samples_per_px)
}

pub const LANE_H: f32 = 46.0;

pub fn track_height(t: &Track) -> f32 {
    let alt = if t.view == "playlists" { t.playlists.len().saturating_sub(1) as f32 * LANE_H } else { 0.0 };
    t.height.points() + alt
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    drive_drag(app, ui.ctx());
    egui::Panel::top("edit_toolbar").exact_size(toolbar::HEIGHT).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| toolbar::show(app, ui));
    egui::Panel::bottom("edit_status").exact_size(22.0).frame(egui::Frame::NONE.fill(t.toolbar_bg)).show(ui, |ui| status_bar(app, ui));
    egui::Panel::bottom("lower_dock_tabs").exact_size(22.0).frame(egui::Frame::NONE.fill(t.panel_bg2)).show(ui, |ui| dock_tabs(app, ui));
    if app.ui.show_universe {
        egui::Panel::top("universe").exact_size(64.0).frame(egui::Frame::NONE.fill(Tokens::current().universe_bg)).show(ui, |ui| universe(app, ui));
    }
    crate::docking::show(app, ui, crate::MainWindow::Edit);
}

/// Universe: the whole session in miniature; the box is the visible area (drag or click to move).
fn universe(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.max_rect().shrink2(vec2(8.0, 4.0));
    let s = app.engine.session();
    let total = (s.content_end() + s.sample_rate.samples(10.0)).max(1) as f32;
    let tracks: Vec<&Track> = s.tracks.iter().filter(|t| !t.hidden).collect();
    let n = tracks.len().max(1) as f32;
    let rh = (r.height() / n).max(1.0);
    for (i, t) in tracks.iter().enumerate() {
        let y = r.min.y + i as f32 * rh;
        for c in t.clips() {
            let x0 = r.min.x + c.start as f32 / total * r.width();
            let x1 = r.min.x + c.end() as f32 / total * r.width();
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(x0, y + 0.5), pos2(x1.max(x0 + 1.0), y + rh - 0.5)),
                0.0,
                rgb(c.color.unwrap_or(t.color)),
            );
        }
    }
    let vis0 = s.edit.zoom.scroll as f32 / total;
    let tlw = app.edit_layout.timeline[2] - app.edit_layout.timeline[0];
    let vis1 = vis0 + (f64::from(tlw.max(100.0)) * s.edit.zoom.samples_per_px) as f32 / total;
    let vr = Rect::from_min_max(pos2(r.min.x + vis0 * r.width(), r.min.y - 2.0), pos2(r.min.x + vis1.min(1.0) * r.width(), r.max.y + 2.0));
    ui.painter().rect_stroke(vr, 2.0, Stroke::new(1.5, Tokens::current().universe_view), StrokeKind::Outside);
    if app.is_playing() {
        let x = r.min.x + app.position() as f32 / total * r.width();
        ui.painter().line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.0, Tokens::current().playhead));
    }
    let resp = ui.interact(r, ui.id().with("universe"), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let half = (vis1 - vis0) * 0.5;
        let k = ((p.x - r.min.x) / r.width() - half).clamp(0.0, 1.0);
        let _ = app.engine.execute("view.scroll", &json!({"to": (k * total) as i64}));
    }
}

fn dock_tabs(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let r = ui.max_rect();
    let mut x = r.min.x + 10.0;
    for (label, on, id) in [
        ("MIDI EDITOR", app.ui.show_midi_editor, "window.midi_editor"),
        ("MEMORY LOCATIONS", app.ui.show_memory_locations, "window.memory_locations"),
        ("UNDO HISTORY", app.ui.show_undo_history, "window.undo_history"),
    ] {
        let g = ui.painter().layout_no_wrap(label.to_string(), bold(10.5), if on { Color32::WHITE } else { t.text_dim });
        let tr = Rect::from_min_size(pos2(x, r.min.y + 2.0), vec2(g.size().x + 16.0, r.height() - 4.0));
        if on {
            ui.painter().rect_filled(tr, 3.0, t.accent_dark);
        }
        ui.painter().galley(pos2(tr.min.x + 8.0, tr.center().y - g.size().y * 0.5), g, Color32::WHITE);
        if ui.interact(tr, ui.id().with(("dock", label)), Sense::click()).clicked() {
            let _ = app.run(id, json!({}));
        }
        x = tr.max.x + 6.0;
    }
}

fn status_bar(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let r = ui.max_rect();
    let s = app.engine.session();
    let mut left = format!(
        "{} · {} Hz · {} · {} tracks · {}",
        s.name,
        s.sample_rate.hz(),
        match s.bit_depth {
            transit_model::BitDepthSetting::Int16 => "16-bit",
            transit_model::BitDepthSetting::Int24 => "24-bit",
            transit_model::BitDepthSetting::Float32 => "32-bit float",
        },
        s.tracks.len(),
        app.ui.status
    );
    transit_model::truncate_chars(&mut left, 4 * transit_model::MAX_NAME_CHARS);
    ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, left, regular(11.0), t.text_dim);
    let undo = app.engine.undo_label().map_or(String::new(), |l| format!("Undo {l}"));
    ui.painter().text(
        pos2(r.max.x - 10.0, r.center().y),
        Align2::RIGHT_CENTER,
        if app.frame_ms < 50.0 { format!("{undo}   {:.1} ms/frame", app.frame_ms) } else { undo },
        regular(11.0),
        t.text_dim,
    );
}

pub(crate) fn main_area(app: &mut SoundApp, ui: &mut Ui) {
    let session = app.engine.session();
    let mut ruler_height_total = session.edit.rulers.iter().map(|ruler| ruler_height(ruler)).sum::<f32>();
    if session.edit.flag("view.ruler.tempo_editor") && session.edit.rulers.iter().any(|ruler| ruler == "tempo") {
        ruler_height_total += ruler_height("tempo_editor");
    }
    let minimum = vec2(header_width(&edit_columns(session)) + 64.0, ruler_height_total + 100.0);
    crate::docking::minimum_body(ui, minimum, |ui| main_area_content(app, ui));
}

fn main_area_content(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let rulers: Vec<String> = app.engine.session().edit.rulers.clone();
    let mut rulers = rulers;
    if app.engine.session().edit.flag("view.ruler.tempo_editor")
        && let Some(i) = rulers.iter().position(|r| r == "tempo")
    {
        rulers.insert(i + 1, "tempo_editor".into());
    }
    let cols = edit_columns(app.engine.session());
    let titles_h = if cols.is_empty() { 0.0 } else { COLUMN_TITLES_H };
    let rulers_h = rulers.iter().map(|r| ruler_height(r)).sum::<f32>() + 6.0 + titles_h;
    let hscroll_h = 14.0;
    let hw = header_width(&cols);
    let header = Rect::from_min_max(full.min, pos2(full.min.x + hw, full.max.y - hscroll_h));
    let tl = Rect::from_min_max(pos2(full.min.x + hw, full.min.y + rulers_h), pos2(full.max.x - 12.0, full.max.y - hscroll_h));
    let rulers_rect = Rect::from_min_max(pos2(full.min.x, full.min.y), pos2(full.max.x, full.min.y + rulers_h));
    app.edit_layout.timeline = [tl.min.x, tl.min.y, tl.max.x, tl.max.y];
    follow_zoom_toggle(app);
    ui.painter().rect_filled(full, 0.0, t.window_bg);
    scroll_and_zoom(app, ui, full, tl);
    draw_rulers(app, ui, rulers_rect, tl, &rulers);
    views_button(app, ui, rulers_rect);
    column_titles(ui, rulers_rect, &cols);
    // Tracks.
    let mut y = tl.min.y - app.edit_layout.scroll_y;
    app.edit_layout.rows.clear();
    let mut content_h = 0.0;
    for t in app.engine.session().tracks.iter().filter(|x| in_edit_window(app.engine.session(), x)) {
        let h = track_height(t);
        app.edit_layout.rows.push((t.id.0, [full.min.x, y, tl.max.x, y + h]));
        y += h + 1.0;
        content_h += h + 1.0;
    }
    app.edit_layout.content_h = content_h;
    let clip_rect = Rect::from_min_max(pos2(full.min.x, tl.min.y), pos2(full.max.x, tl.max.y));
    for (id, r) in app.edit_layout.rows.clone() {
        let row = rect(r);
        if row.max.y >= clip_rect.min.y && row.min.y <= clip_rect.max.y {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row));
            child.set_clip_rect(clip_rect);
            track_row(app, &mut child, TrackId(id), row, tl, &cols);
        }
    }
    // Below the tracks a click clears the selection and a double-click opens New Tracks.
    let empty = Rect::from_min_max(pos2(full.min.x, y.max(tl.min.y)), tl.max);
    if empty.height() > 4.0 {
        let resp = ui.interact(empty, ui.id().with("empty"), Sense::click());
        if resp.double_clicked() {
            let _ = app.dialogs.open_for_command(&app.engine, "track.new");
        } else if resp.clicked() {
            let _ = app.run("edit.select_none", json!({}));
        }
    }
    overlay(app, ui, tl, Rect::from_min_max(pos2(tl.min.x, rulers_rect.min.y), tl.max));
    let _ = header;
    // Horizontal scrollbar.
    let sb = Rect::from_min_max(pos2(tl.min.x, full.max.y - hscroll_h), pos2(tl.max.x, full.max.y));
    hscrollbar(app, ui, sb, tl);
    // Vertical scrollbar gutter.
    let vs = Rect::from_min_max(pos2(tl.max.x, tl.min.y), pos2(full.max.x, tl.max.y));
    ui.painter().rect_filled(vs, 0.0, t.panel_bg);
    if content_h > tl.height() {
        let frac = tl.height() / content_h;
        let off = app.edit_layout.scroll_y / content_h;
        let thumb = Rect::from_min_size(pos2(vs.min.x + 2.0, vs.min.y + vs.height() * off), vec2(vs.width() - 4.0, (vs.height() * frac).max(20.0)));
        ui.painter().rect_filled(thumb, 3.0, t.button_hi);
        let resp = ui.interact(vs, ui.id().with("vscroll"), Sense::drag());
        if resp.dragged() {
            app.edit_layout.scroll_by(resp.drag_delta().y / frac, tl.height());
        }
    }
}

fn in_edit_window(s: &Session, t: &Track) -> bool {
    ops::shown(s, t)
}

/// Pro Tools' rule for track height: Alt reaches every shown track, Alt+Shift the selected ones too.
pub(crate) fn height_targets(s: &Session, m: egui::Modifiers, own: &[TrackId]) -> Vec<u64> {
    let extra = |t: &Track| if m.shift { s.edit.selected_tracks.contains(&t.id) } else { in_edit_window(s, t) };
    s.tracks.iter().filter(|t| own.contains(&t.id) || (m.alt && extra(t))).map(|t| t.id.0).collect()
}

/// How many folders this track sits in, for the name's indent (at most 6 shown).
fn folder_depth(s: &Session, track: &Track) -> usize {
    let mut current = track.folder;
    let mut depth = 0;
    while let Some(folder) = current.and_then(|folder| s.track(folder))
        && depth < 6
    {
        depth += 1;
        current = folder.folder;
    }
    depth
}

/// Pro Tools' track-name click: Shift adds the tracks up to the nearest selected one, Cmd/Ctrl adds or removes one.
pub(crate) fn click_track(app: &mut SoundApp, id: TrackId, mods: egui::Modifiers, shown: impl Fn(&Session, &Track) -> bool) {
    let s = app.engine.session();
    let order: Vec<TrackId> = s.tracks.iter().filter(|t| shown(s, t)).map(|t| t.id).collect();
    let row = |t: &TrackId| order.iter().position(|o| o == t);
    let mut tracks = s.edit.selected_tracks.clone();
    if mods.command {
        if tracks.contains(&id) {
            tracks.retain(|t| *t != id);
        } else {
            tracks.push(id);
        }
    } else if mods.shift
        && let Some(a) = row(&id)
        && let Some(b) = tracks.iter().filter_map(row).min_by_key(|b| b.abs_diff(a))
    {
        let span = order.get(a.min(b)..=a.max(b)).unwrap_or_default();
        tracks.extend(span.iter().filter(|t| !tracks.contains(t)).copied().collect::<Vec<_>>());
    } else {
        tracks = vec![id];
    }
    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
    let _ = app.run("edit.select", json!({"tracks": ids}));
}

fn scroll_and_zoom(app: &mut SoundApp, ui: &Ui, area: Rect, tl: Rect) {
    let (delta, zoom, pointer, scrolling) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos(), i.is_scrolling()));
    let mut swipe = if scrolling { app.edit_layout.swipe } else { egui::Vec2::ZERO };
    if swipe.max_elem() < 6.0 {
        swipe += delta.abs();
    }
    app.edit_layout.swipe = swipe;
    let Some(p) = pointer.filter(|_| tl.is_positive() && ui.rect_contains_pointer(area)) else { return };
    if zoom != 1.0 {
        let _ = app.engine.execute("view.zoom_by", &json!({"factor": zoom, "x": p.x.clamp(tl.min.x, tl.max.x) - tl.min.x}));
    }
    let horizontal = swipe.x > swipe.y;
    if horizontal && delta.x != 0.0 {
        pan(app, -f64::from(delta.x));
    }
    if !horizontal && delta.y != 0.0 {
        app.edit_layout.scroll_by(-delta.y, tl.height());
    }
}

fn pan(app: &mut SoundApp, px: f64) {
    let z = app.engine.session().edit.zoom;
    let want = (z.scroll as f64 + app.edit_layout.scroll_carry + px * z.samples_per_px).clamp(0.0, transit_engine::cmd::MAX_POSITION as f64);
    if want.is_nan() {
        return;
    }
    let to = transit_time::to_samples(want);
    app.edit_layout.scroll_carry = want - to as f64;
    if to != z.scroll {
        let _ = app.engine.execute("view.scroll", &json!({"to": to}));
    }
}

fn hscrollbar(app: &mut SoundApp, ui: &mut Ui, sb: Rect, tl: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(sb, 0.0, t.panel_bg);
    let s = app.engine.session();
    let vis = f64::from(tl.width()) * s.edit.zoom.samples_per_px;
    let total = (s.content_end().saturating_add(s.sample_rate.samples(30.0)).max(1) as f64).max(s.edit.zoom.scroll as f64 + vis);
    let frac = (vis / total).clamp(0.02, 1.0) as f32;
    let off = (s.edit.zoom.scroll as f64 / total).clamp(0.0, 1.0) as f32;
    let thumb = Rect::from_min_size(pos2(sb.min.x + sb.width() * off, sb.min.y + 3.0), vec2((sb.width() * frac).max(24.0), sb.height() - 6.0));
    ui.painter().rect_filled(thumb, 3.0, t.button_hi);
    let resp = ui.interact(sb, ui.id().with("hscroll"), Sense::drag());
    if resp.dragged() {
        let dx = resp.drag_delta().x / sb.width();
        let by_px = f64::from(dx) * total / s.edit.zoom.samples_per_px;
        pan(app, by_px);
    }
}

/// Zoom Toggle sized to the track area on screen.
pub fn zoom_toggle(app: &mut SoundApp) {
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let _ = app.run("view.zoom_toggle", json!({"width_px": x1 - x0, "height_px": y1 - y0}));
}

/// Zoom Toggle scrolls the first track it fitted to the top, and back when it turns off.
fn follow_zoom_toggle(app: &mut SoundApp) {
    let l = &mut app.edit_layout;
    match (&app.engine.session().edit.zoom_toggle, l.zoom_back) {
        (Some(z), None) => {
            l.zoom_back = Some(l.scroll_y);
            let content_top = l.rows.first().map_or(0.0, |(_, r)| r[1]);
            if let Some((_, r)) = l.rows.iter().find(|(id, _)| z.heights.iter().any(|(t, _)| t.0 == *id)) {
                l.scroll_y = r[1] - content_top;
            }
        }
        (None, Some(y)) => (l.scroll_y, l.zoom_back) = (y, None),
        _ => {}
    }
}
