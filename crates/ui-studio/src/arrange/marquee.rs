//! Marquee (rubber-band) clip selection (#217).
//!
//! Convention: Logic. With the pointer tool, a press on empty lane space and a drag draws a
//! rectangle across tracks; on release every clip it touches is selected, Shift adds to the
//! selection and Ctrl (Cmd on a Mac) toggles. Ableton drags in empty arrangement space select a
//! time range instead; for clips transit follows Logic (decided in #217). A plain click without a
//! drag keeps today's behavior: it sets the cursor (`StudioApp::click_empty`) and selects nothing.
//!
//! The rectangle is anchored in content coordinates (a timeline sample and a height among the
//! rows), so it follows the vertical scroll, the horizontal scroll and the zoom while the drag
//! autoscrolls at the lane edges. The result goes through the session's selection set
//! (`edit.select`), the same one the move and copy paths read, so the multi-clip copy (#214) and
//! the whole-selection drag preview (#218) work on it. Clips are the rectangles the arrangement
//! drew (`Layout::clips`), so the children of a collapsed folder, which its row shows only as
//! read-only summary bars, are not selected. Escape cancels the drag and leaves the selection as
//! it was (`menus.rs` offers the key to [`cancel`] before Select None).

use super::{ClipId, Layout, Map, R4, StudioApp, r4};
use crate::theme::TOKENS;
use egui::{Rect, Response, Stroke, StrokeKind, Ui, pos2};
use serde_json::json;
use transit_time::Samples;

/// How far (points) the pointer must travel from the press before the marquee is live; a shakier
/// click stays a click.
pub(super) const THRESHOLD: f32 = 4.0;
/// Within this many points of a lane edge the drag scrolls the view; farther out scrolls faster.
const EDGE: f32 = 24.0;
/// Most points one frame scrolls.
const MAX_STEP: f32 = 24.0;

/// A marquee drag in progress.
#[derive(Debug, Clone)]
pub struct Marquee {
    /// Where the press landed: a timeline sample and a height among the rows (content points).
    anchor_at: Samples,
    anchor_y: f32,
    press: egui::Pos2,
    /// The selection when the press happened (Shift and Ctrl/Cmd combine with it).
    base: Vec<ClipId>,
    /// The pointer has gone past [`THRESHOLD`].
    pub live: bool,
    /// The rectangle as last drawn, `[x0, y0, x1, y1]` (screen points).
    pub rect: R4,
}

/// How a marquee combines with the selection at the press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Replace,
    Add,
    Toggle,
}

/// The mode the modifiers select: Shift adds, Ctrl/Cmd toggles, neither replaces.
pub fn mode(m: egui::Modifiers) -> Mode {
    if m.shift {
        Mode::Add
    } else if m.command {
        Mode::Toggle
    } else {
        Mode::Replace
    }
}

/// The selection after a marquee that touched `hit` (in track order) over `base`.
pub fn combine(mode: Mode, base: &[ClipId], hit: &[ClipId]) -> Vec<ClipId> {
    match mode {
        Mode::Replace => hit.to_vec(),
        Mode::Add => base.iter().chain(hit.iter().filter(|c| !base.contains(c))).copied().collect(),
        Mode::Toggle => base.iter().filter(|c| !hit.contains(c)).chain(hit.iter().filter(|c| !base.contains(c))).copied().collect(),
    }
}

/// Escape: drop the marquee, keeping the selection. True when there was one to cancel.
pub(crate) fn cancel(app: &mut StudioApp) -> bool {
    app.arrange.marquee.take().is_some()
}

/// Run the marquee for this frame: start on a press in empty space, grow with the pointer
/// (autoscrolling at the edges), draw, and select on release.
#[allow(clippy::too_many_arguments)]
pub(super) fn input(app: &mut StudioApp, ui: &Ui, resp: &Response, map: Map, tl: Rect, layout: &Layout, max_vscroll: f32) {
    if resp.drag_started()
        && app.arrange.gesture.is_none()
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
        && tl.contains(press)
        && super::hit(layout, press).0.is_none()
    {
        app.arrange.marquee = Some(Marquee {
            anchor_at: map.at(press.x),
            anchor_y: press.y - tl.min.y + app.arrange.vscroll,
            press,
            base: app.engine.session().edit.selected_clips.to_vec(),
            live: false,
            rect: [press.x, press.y, press.x, press.y],
        });
    }
    let Some(mut m) = app.arrange.marquee.clone() else { return };
    if resp.dragged()
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
    {
        if !m.live && (pos - m.press).length() >= THRESHOLD {
            m.live = true;
        }
        if m.live {
            autoscroll(app, pos, tl, max_vscroll);
            let corner = pos2(pos.x.clamp(tl.min.x, tl.max.x), pos.y.clamp(tl.min.y, tl.max.y));
            let anchor = pos2(map.x(m.anchor_at), tl.min.y + m.anchor_y - app.arrange.vscroll);
            m.rect = r4(Rect::from_two_pos(anchor, corner));
            paint(ui, tl, m.rect);
        }
        app.arrange.marquee = Some(m.clone());
    }
    if resp.drag_stopped() {
        app.arrange.marquee = None;
        if !m.live {
            // Never went past the threshold: it was a click.
            let track = layout.tracks.iter().find(|r| m.press.y >= r.lane[1] && m.press.y < r.lane[3]).map(|r| crate::TrackId(r.id));
            app.click_empty(track, map.at(m.press.x));
            return;
        }
        let rect = Rect::from_min_max(pos2(m.rect[0], m.rect[1]), pos2(m.rect[2], m.rect[3]));
        let hit: Vec<ClipId> = layout
            .clips
            .iter()
            .filter(|c| rect.intersects(Rect::from_min_max(pos2(c.rect[0], c.rect[1]), pos2(c.rect[2], c.rect[3]))))
            .map(|c| ClipId(c.id))
            .collect();
        let selected = combine(mode(ui.input(|i| i.modifiers)), &m.base, &hit);
        select(app, &selected, hit.first().copied());
    }
}

/// Put `clips` in the selection set; the first touched clip's track gets the focus.
fn select(app: &mut StudioApp, clips: &[ClipId], first: Option<ClipId>) {
    app.auto.selection.clear();
    if clips.is_empty() {
        let _ = app.run("edit.select_none", json!({}));
        return;
    }
    if let Some((t, _)) = first.and_then(|c| app.engine.session().find_clip(c)) {
        app.ui.focus_track = Some(t);
    }
    let _ = app.run("edit.select", json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}));
}

/// Scroll the view while the pointer is at or past a lane edge: rows vertically, time sideways.
fn autoscroll(app: &mut StudioApp, pos: egui::Pos2, tl: Rect, max_vscroll: f32) {
    let step = |over: f32| (over / EDGE * MAX_STEP).clamp(1.0, MAX_STEP);
    if pos.y < tl.min.y + EDGE {
        app.arrange.vscroll = (app.arrange.vscroll - step(tl.min.y + EDGE - pos.y)).max(0.0);
    } else if pos.y > tl.max.y - EDGE {
        app.arrange.vscroll = (app.arrange.vscroll + step(pos.y - (tl.max.y - EDGE))).min(max_vscroll);
    }
    if pos.x < tl.min.x + EDGE {
        let _ = app.engine.execute("view.scroll", &json!({"by_px": -f64::from(step(tl.min.x + EDGE - pos.x))}));
    } else if pos.x > tl.max.x - EDGE {
        let _ = app.engine.execute("view.scroll", &json!({"by_px": f64::from(step(pos.x - (tl.max.x - EDGE)))}));
    }
}

fn paint(ui: &Ui, tl: Rect, rect: R4) {
    let r = Rect::from_min_max(pos2(rect[0], rect[1]), pos2(rect[2], rect[3]));
    let p = ui.painter().with_clip_rect(tl);
    p.rect_filled(r, 0.0, TOKENS.selection_outline.gamma_multiply(0.12));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, TOKENS.selection_outline), StrokeKind::Inside);
}
