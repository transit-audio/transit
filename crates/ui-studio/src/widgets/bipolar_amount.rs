//! The bipolar amount bar (`docs/design/language`, Components > Bipolar amount bar), the amount of
//! a modulation route: a 4 px `line` track, a 7 px `mod` fill from the center tick (`text.faint`)
//! toward the value. Drag sideways to change it (Shift for fine), double-click for zero. The amount
//! runs from −1 to +1; the caller prints it in mono beside the bar.

use super::r4;
use crate::theme::TOKENS;
use egui::{Color32, Rect, Sense, Ui, pos2, vec2};

pub const TRACK_H: f32 = 4.0;
pub const FILL_H: f32 = 7.0;
pub const HEIGHT: f32 = 16.0;
pub const TICK_W: f32 = 1.0;
pub const FINE: f32 = 10.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AmountLayout {
    pub rect: [f32; 4],
    pub track: [f32; 4],
    /// Empty (zero width) at zero.
    pub fill: [f32; 4],
    pub tick: [f32; 4],
    #[serde(skip)]
    pub track_color: Color32,
    #[serde(skip)]
    pub fill_color: Color32,
    #[serde(skip)]
    pub tick_color: Color32,
}

pub struct AmountOut {
    pub response: egui::Response,
    pub layout: AmountLayout,
    pub amount: Option<f32>,
}

/// The amount after dragging `dx` points across a bar `width` wide (the full width is −1 to +1).
pub fn dragged(amount: f32, dx: f32, width: f32, fine: bool) -> f32 {
    (amount + 2.0 * dx / width / if fine { FINE } else { 1.0 }).clamp(-1.0, 1.0)
}

/// Draws the bar `width` wide at `amount`.
pub fn show(ui: &mut Ui, id: egui::Id, width: f32, amount: f32) -> AmountOut {
    let t = &TOKENS;
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let mut new = None;
    if response.dragged() {
        let (dx, fine) = ui.input(|i| (i.pointer.delta().x, i.modifiers.shift));
        if dx != 0.0 {
            new = Some(dragged(amount, dx, width, fine));
        }
    }
    if super::reset_click(&response) {
        new = Some(0.0);
    }
    let shown = new.unwrap_or(amount).clamp(-1.0, 1.0);
    let cy = rect.center().y;
    let cx = rect.center().x;
    let track = Rect::from_center_size(rect.center(), vec2(width, TRACK_H));
    let x = cx + shown * width / 2.0;
    let fill = Rect::from_x_y_ranges(cx.min(x)..=cx.max(x), (cy - FILL_H / 2.0)..=(cy + FILL_H / 2.0));
    let tick = Rect::from_center_size(pos2(cx, cy), vec2(TICK_W, FILL_H + 4.0));
    let p = ui.painter();
    p.rect_filled(track, 0.0, t.line);
    p.rect_filled(fill, 0.0, t.modulation);
    p.rect_filled(tick, 0.0, t.text_faint);
    let layout = AmountLayout {
        rect: r4(rect),
        track: r4(track),
        fill: r4(fill),
        tick: r4(tick),
        track_color: t.line,
        fill_color: t.modulation,
        tick_color: t.text_faint,
    };
    AmountOut { response, layout, amount: new }
}
