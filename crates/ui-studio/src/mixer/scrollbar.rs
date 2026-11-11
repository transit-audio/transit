//! The chrome's floating scroll bar, shared by every scrolling region of the mixer: the overflow
//! sections' rows (vertical, `overflow.rs`) and the strips lane (horizontal, `strip.rs`). The
//! rule (`docs/design/language`, Application chrome, Scroll bars): at rest a 4 px `ink.ghost`
//! handle, on hover 8 px `ink.arrow` on a `surface.panel` track, `text.faint` while dragged;
//! inset 2, handle at least 24, round ends. This file holds the axis-independent parts: the
//! handle's length, its place along the track, the drag's conversion back to a scroll offset, the
//! ink and the paint.

use super::*;
use crate::tokens::chrome;

/// The track length along the scroll axis and the handle's length in it, for a view of
/// `view_len` showing that share of `content` (at least the chrome's minimum handle).
pub(crate) fn handle_len(view_len: f32, content: f32) -> (f32, f32) {
    let track = view_len - 2.0 * chrome::SCROLL_INSET;
    let handle = (track * view_len / content.max(1.0)).clamp(chrome::SCROLL_MIN_HANDLE.min(track), track);
    (track, handle)
}

/// Where the handle starts along the scroll axis, from the view's start, for `offset` of `max`.
pub(crate) fn handle_start(track: f32, handle: f32, offset: f32, max: f32) -> f32 {
    chrome::SCROLL_INSET + if max > 0.0 { (track - handle) * offset / max } else { 0.0 }
}

/// The scroll offset after the handle is dragged `delta` along the axis.
pub(crate) fn dragged_offset(offset: f32, delta: f32, max: f32, track: f32, handle: f32) -> f32 {
    (offset + delta * max / (track - handle).max(1.0)).clamp(0.0, max)
}

/// The handle's thickness: wide on hover or drag, thin at rest.
pub(crate) fn thickness(hovered: bool, dragging: bool) -> f32 {
    if hovered || dragging { chrome::SCROLL_HOVER } else { chrome::SCROLL_REST }
}

/// The handle's ink.
pub(crate) fn ink(hovered: bool, dragging: bool) -> Color32 {
    if dragging {
        TOKENS.text_faint
    } else if hovered {
        TOKENS.ink_arrow
    } else {
        TOKENS.ink_ghost
    }
}

/// Paints the hover track (when `track` is given) and the handle, round ended.
pub(crate) fn paint(p: &egui::Painter, track: Option<Rect>, thumb: Rect, color: Color32) {
    let rad = chrome::SCROLL_HOVER / 2.0;
    if let Some(track) = track {
        p.rect_filled(track, rad, TOKENS.surface_panel);
    }
    p.rect_filled(thumb, rad, color);
}

/// The strips lane's horizontal bar as last drawn (`ui.inspect mixer`, tests).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct LaneBar {
    /// The strip along the lane's bottom edge the handle travels in and is dragged from.
    pub track: R4,
    pub thumb: R4,
    pub thumb_width: f32,
    #[serde(skip)]
    pub thumb_color: Color32,
}

/// The lane's bar, hit and painted; moves `hscroll` while the handle is dragged. `None` while the
/// strips fit (`max` is 0). `lane` is the scrolling lane alone: the pinned main bus and
/// "+ FX Channel" lie outside it, so they are outside the track.
pub(super) fn lane_bar(app: &mut StudioApp, ui: &mut Ui, lane: Rect, content_w: f32, max: f32) -> Option<LaneBar> {
    if max <= 0.0 {
        return None;
    }
    let bar = Rect::from_min_max(pos2(lane.min.x, lane.max.y - chrome::SCROLL_HOVER), lane.max);
    let resp = ui.interact(bar, ui.id().with("lane_bar"), Sense::drag());
    let (track, handle) = handle_len(lane.width(), content_w);
    if resp.dragged() {
        app.mixer.hscroll = dragged_offset(app.mixer.hscroll, resp.drag_delta().x, max, track, handle);
    }
    let (hovered, dragging) = (resp.hovered() || resp.dragged(), resp.dragged());
    let left = lane.min.x + handle_start(track, handle, app.mixer.hscroll, max);
    let width = thickness(hovered, dragging);
    let thumb = Rect::from_min_max(pos2(left, lane.max.y - width), pos2(left + handle, lane.max.y));
    let color = ink(hovered, dragging);
    paint(&ui.painter().with_clip_rect(lane), hovered.then_some(bar), thumb, color);
    Some(LaneBar { track: r4(bar), thumb: r4(thumb), thumb_width: width, thumb_color: color })
}
