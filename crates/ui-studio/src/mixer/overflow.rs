//! A full-height section past its rows (`docs/design/language`, Components > Channel strip >
//! Overflow; the chrome canvas, "Strip details" (12)): more than three inserts or two sends. The
//! section keeps its fixed box so every fader still lines up; its rows scroll inside a view the
//! height of the rows it holds, the add row stays pinned under the view with the hidden count, a
//! 12 px fade into the panel fill marks each clipped edge, and the scroll bar is the chrome's
//! floating one (4 px `ink.ghost` at rest, 8 px `ink.arrow` on a `surface.panel` track on hover,
//! `text.faint` while dragged; inset 2, handle at least 24). The wheel scrolls the box only while
//! it overflows (it is taken from the mixer then, and the rows' controls nudge only on Ctrl+wheel);
//! a box that fits never builds one.

use super::*;
use crate::tokens::{channel_strip as cs, chrome};

/// Which section's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Section {
    Inserts,
    Sends,
}

/// An overflowing section as last drawn (`ui.inspect mixer`, tests).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct OverflowLayout {
    /// The rows' view, inside the section's fixed box.
    pub view: R4,
    /// How far the rows are scrolled (points from the top) and the most they scroll.
    pub offset: f32,
    pub max: f32,
    /// The rows the view cannot show at once (the add row's count).
    pub hidden: usize,
    /// The count after the add row's label ("2 more") and where it sits.
    pub count_text: String,
    pub count_rect: R4,
    /// The fades at the clipped edges: top while scrolled down, bottom while rows lie below.
    pub fade_top: Option<R4>,
    pub fade_bottom: Option<R4>,
    /// The scroll bar's handle, its width and ink.
    pub thumb: R4,
    pub thumb_width: f32,
    #[serde(skip)]
    pub thumb_color: Color32,
    /// The right gutter the rows leave for the bar (the board's 6 at rest, 10 on hover).
    pub gutter: f32,
}

/// The rows' gutter beside the bar at rest and on hover (the Strip details board).
const GUTTER: [f32; 2] = [6.0, 10.0];

/// One section's scrolling box: built before its rows are drawn, finished after.
pub(super) struct Overflow {
    view: Rect,
    clip: Rect,
    offset: f32,
    max: f32,
    hidden: usize,
    hovered: bool,
    dragging: bool,
    thumb: Rect,
}

impl Overflow {
    /// The box for `total` rows in a `view` that shows `visible` of them at once (a row is
    /// [`cs::INSERT_SLOT_HEIGHT`] with [`cs::INSERT_SLOT_GAP`] between). Takes the wheel while the
    /// pointer is over the view and handles the scroll bar's drag.
    pub(super) fn begin(
        app: &mut StudioApp,
        ui: &mut Ui,
        track: TrackId,
        section: Section,
        view: Rect,
        clip: Rect,
        total: usize,
        visible: usize,
    ) -> Self {
        let key = (track.0, section);
        let pitch = cs::INSERT_SLOT_HEIGHT + cs::INSERT_SLOT_GAP;
        let content = total as f32 * pitch - cs::INSERT_SLOT_GAP;
        let max = (content - view.height()).max(0.0);
        let mut offset = app.mixer.overflow.get(&key).copied().unwrap_or(0.0).clamp(0.0, max);
        let seen = view.intersect(clip);
        let hovered = seen.is_positive() && ui.rect_contains_pointer(seen);
        if hovered && app.mixer.gesture.is_none() {
            // One owner per wheel event: the plain wheel scrolls the box and is consumed here,
            // before the rows run, so a row's `control()` never nudges with it; Ctrl+wheel (Cmd
            // on macOS, the kit's nudge modifier, `widgets::command_wheel`) is left for the
            // hovered row's control and does not scroll the box.
            let command = ui.input_mut(|i| {
                let command = i.modifiers.ctrl
                    || i.modifiers.command
                    || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { modifiers, .. } if modifiers.ctrl || modifiers.command));
                i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { modifiers, .. } if !(modifiers.ctrl || modifiers.command)));
                command
            });
            let d = ui.input(|i| i.smooth_scroll_delta);
            let d = if d.x.abs() > d.y.abs() { d.x } else { d.y };
            if !command && d.abs() > 0.1 {
                offset = (offset - d).clamp(0.0, max);
            }
            // The box overflows: the wheel is its, never the mixer's, while over it.
            app.mixer.wheel_taken = true;
        }
        // The handle: the view's share of the rows, at least the chrome's minimum.
        let inset = chrome::SCROLL_INSET;
        let track_h = view.height() - 2.0 * inset;
        let handle = (track_h * view.height() / content.max(1.0)).clamp(chrome::SCROLL_MIN_HANDLE.min(track_h), track_h);
        let bar = Rect::from_min_max(pos2(view.max.x - chrome::SCROLL_HOVER, view.min.y), view.max);
        let drag = ui.interact(bar.intersect(clip), ui.id().with(("overflow_bar", key.0, section as u8)), Sense::drag());
        if drag.dragged() && max > 0.0 {
            let dy = drag.drag_delta().y;
            offset = (offset + dy * max / (track_h - handle).max(1.0)).clamp(0.0, max);
        }
        let dragging = drag.dragged();
        app.mixer.overflow.insert(key, offset);
        let width = if hovered || dragging { chrome::SCROLL_HOVER } else { chrome::SCROLL_REST };
        let top = view.min.y + inset + if max > 0.0 { (track_h - handle) * offset / max } else { 0.0 };
        let thumb = Rect::from_min_max(pos2(view.max.x - width, top), pos2(view.max.x, top + handle));
        Overflow { view, clip, offset, max, hidden: total.saturating_sub(visible), hovered: hovered || dragging, dragging, thumb }
    }

    /// Row `i` of the rows (it may lie outside the view; draw it clipped to [`Self::clip`]).
    pub(super) fn row(&self, i: usize) -> Rect {
        let y = self.view.min.y + i as f32 * (cs::INSERT_SLOT_HEIGHT + cs::INSERT_SLOT_GAP) - self.offset;
        let right = self.view.max.x - self.gutter();
        Rect::from_min_max(pos2(self.view.min.x, y), pos2(right, y + cs::INSERT_SLOT_HEIGHT))
    }

    /// The clip the rows are drawn and hit in: the view inside the strip's own clip.
    pub(super) fn clip(&self) -> Rect {
        self.view.intersect(self.clip)
    }

    fn gutter(&self) -> f32 {
        if self.hovered { GUTTER[1] } else { GUTTER[0] }
    }

    /// The fades and the scroll bar over the drawn rows; returns the layout.
    pub(super) fn finish(&self, p: &egui::Painter) -> OverflowLayout {
        let p = p.with_clip_rect(self.clip());
        let fade = cs::OVERFLOW_FADE;
        let panel = TOKENS.surface_panel;
        let fade_top = (self.offset > 0.0).then(|| Rect::from_min_size(self.view.min, vec2(self.view.width(), fade)));
        let fade_bottom = (self.offset < self.max).then(|| Rect::from_min_max(pos2(self.view.min.x, self.view.max.y - fade), self.view.max));
        if let Some(r) = fade_top {
            gradient(&p, r, panel, Color32::TRANSPARENT);
        }
        if let Some(r) = fade_bottom {
            gradient(&p, r, Color32::TRANSPARENT, panel);
        }
        let (rad, ink) = (chrome::SCROLL_HOVER / 2.0, self.thumb_color());
        if self.hovered {
            let track = Rect::from_min_max(pos2(self.view.max.x - chrome::SCROLL_HOVER, self.view.min.y), self.view.max);
            p.rect_filled(track, rad, TOKENS.surface_panel);
        }
        p.rect_filled(self.thumb, rad, ink);
        OverflowLayout {
            view: r4(self.view),
            offset: self.offset,
            max: self.max,
            hidden: self.hidden,
            fade_top: fade_top.map(r4),
            fade_bottom: fade_bottom.map(r4),
            thumb: r4(self.thumb),
            thumb_width: self.thumb.width(),
            thumb_color: ink,
            gutter: self.gutter(),
            ..OverflowLayout::default()
        }
    }

    fn thumb_color(&self) -> Color32 {
        if self.dragging {
            TOKENS.text_faint
        } else if self.hovered {
            TOKENS.ink_arrow
        } else {
            TOKENS.ink_ghost
        }
    }
}

/// A vertical gradient from `top` to `bottom` over `r`.
fn gradient(p: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    for (pos, c) in [(r.left_top(), top), (r.right_top(), top), (r.right_bottom(), bottom), (r.left_bottom(), bottom)] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
}

/// The hidden count after an add row's label ("2 more"): Mono 400 10 `text.faint`, right-aligned
/// in the row less its 6 px padding. Returns the text and where it sits.
pub(super) fn paint_count(p: &egui::Painter, row: Rect, hidden: usize) -> (String, R4) {
    let role = crate::tokens::TypeRole { size: cs::OVERFLOW_COUNT_SIZE, ..crate::tokens::READOUT_DISPLAY };
    let text = format!("{hidden} more");
    let at = super::channel::text_in(p, &role, &text, TOKENS.text_faint, row.shrink2(vec2(6.0, 0.0)), Align2::RIGHT_CENTER);
    (text, r4(at))
}
