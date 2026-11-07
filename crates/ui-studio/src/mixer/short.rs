//! The short variant's lists (Channel strip, Short): the `FX n` and `Snd n` buttons open the
//! strip's full insert or send list in a popup under the button (over it when the strip sits at the pane's bottom edge). The list is an area of its own,
//! not an egui popup, so the menus opened from inside it (+ Insert's device menu, a send's target
//! menu, a slot's right-click menu) can open while it stays: egui keeps one popup open at a time.
//! It closes on a second click on its button, Escape, or a press outside it while no menu is open.

use super::*;
use crate::tokens::{self, channel_strip as cs};

/// Which list a short strip has open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum ShortList {
    Inserts,
    Sends,
}

/// `n` rows of a list, one under another at the strip's content width.
fn rows(ui: &mut Ui, n: usize) -> Vec<Rect> {
    let w = cs::WIDTH - 2.0 * cs::PADDING[1];
    ui.spacing_mut().item_spacing.y = cs::INSERT_SLOT_GAP;
    (0..n).map(|_| ui.allocate_exact_size(vec2(w, cs::INSERT_SLOT_HEIGHT), Sense::hover()).0).collect()
}

/// Where a list `h` high opens for a button at `anchor` in `pane`: under the button, or over it
/// (growing upward) when it would leave the pane at the bottom and fits above. The area is also
/// constrained to the pane, so a list taller than either side is moved up to fit.
pub fn list_origin(anchor: Rect, h: f32, pane: Rect) -> (egui::Pos2, egui::Align2) {
    if anchor.max.y + h > pane.max.y && anchor.min.y - h >= pane.min.y {
        (anchor.left_top(), egui::Align2::LEFT_BOTTOM)
    } else {
        (anchor.left_bottom(), egui::Align2::LEFT_TOP)
    }
}

/// The height of a list of `n` rows with its frame.
fn list_height(n: usize) -> f32 {
    n as f32 * cs::INSERT_SLOT_HEIGHT + n.saturating_sub(1) as f32 * cs::INSERT_SLOT_GAP + 2.0 * tokens::chrome::POPUP_PADDING + 2.0
}

/// The list `which` of `track` under `button` while it is open, with `n` rows filled by `fill`
/// (rows, clip). A click on the button opens and closes it. Returns the list's rect while open.
pub(super) fn list(
    app: &mut StudioApp,
    ui: &mut Ui,
    track: TrackId,
    which: ShortList,
    button: &egui::Response,
    n: usize,
    fill: impl FnOnce(&mut StudioApp, &mut Ui, &[Rect], Rect),
) -> Option<Rect> {
    let me = Some((track, which));
    // Whether a menu was open before this frame's clicks were handled: a press in it is not a
    // press outside the list.
    let menu_open = egui::Popup::is_any_open(ui.ctx());
    if button.clicked() {
        app.mixer.short_list = if app.mixer.short_list == me { None } else { me };
    }
    if app.mixer.short_list != me {
        return None;
    }
    let anchor = button.rect;
    let pane = ui.max_rect();
    let (pos, pivot) = list_origin(anchor, list_height(n), pane);
    let area = egui::Area::new(egui::Id::new(("mixer_short_list", track.0, which)))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .constrain_to(pane)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(TOKENS.surface_raised)
                .stroke(Stroke::new(1.0, TOKENS.line_strong))
                .corner_radius(tokens::chrome::RADIUS_POPUP)
                .inner_margin(tokens::chrome::POPUP_PADDING)
                .show(ui, |ui| {
                    let r = rows(ui, n);
                    let clip = ui.clip_rect();
                    fill(app, ui, &r, clip);
                })
        });
    let rect = area.response.rect;
    let (pressed, pos, escape) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(egui::Key::Escape)));
    let outside = pressed && pos.is_some_and(|p| !rect.contains(p) && !anchor.contains(p));
    if escape || (outside && !menu_open) {
        app.mixer.short_list = None;
    }
    Some(rect)
}
