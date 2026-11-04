//! Automation lane sizing and its view-state drag handle.

use super::*;

/// A new lane's height, and the range a drag on its bottom edge keeps it in (points).
pub const LANE_H: f32 = 56.0;
pub const LANE_MIN: f32 = 40.0;
pub const LANE_MAX: f32 = 240.0;
/// Valid bases preserve the full range of proportions at either parent-height endpoint.
pub(crate) fn base(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 { value.clamp(40.0 * 46.0 / 440.0, 240.0 * 46.0 / 16.0) } else { LANE_H }
}

pub(crate) fn display(value: f32, parent: f32) -> f32 {
    (base(value) * parent / 46.0).clamp(LANE_MIN, LANE_MAX)
}

pub(super) fn handle(app: &mut StudioApp, ui: &mut Ui, t: &Track, lane: &Lane, list: Rect, r: Rect, row: &mut LaneRow) {
    let p = ui.painter().with_clip_rect(list);
    let lid = lane.id();
    let salt = (t.id.0, lid.clone());
    // The bottom edge: drag to change the lane's height (view state).
    let handle = Rect::from_min_max(pos2(r.min.x, r.max.y - 5.0), r.max);
    row.height_handle = r4(handle);
    let resp = ui.interact(handle.intersect(list), ui.id().with(("lane_height", &salt)), Sense::drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        p.line_segment([pos2(r.min.x, r.max.y - 1.0), pos2(r.max.x, r.max.y - 1.0)], Stroke::new(2.0, TOKENS.accent));
    }
    if resp.drag_started() {
        let y = ui.input(|i| i.pointer.press_origin()).map_or(r.max.y, |p| p.y);
        app.auto.gesture = Some(AutoGesture::Height { track: t.id, lane: lid.clone(), start_h: row.height, press_y: y });
    }
    if resp.dragged()
        && let Some(AutoGesture::Height { track, lane: gl, start_h, press_y }) = app.auto.gesture.clone()
        && track == t.id
        && gl == lid
        && let Some(now) = ui.input(|i| i.pointer.latest_pos())
        && let Some(v) = app.ui.auto_lanes.iter_mut().find(|v| v.track == t.id && v.lane == lid)
    {
        v.height = (start_h + now.y - press_y).clamp(LANE_MIN, LANE_MAX);
    }
    if resp.drag_stopped() && matches!(app.auto.gesture, Some(AutoGesture::Height { .. })) {
        app.auto.gesture = None;
    }
}
