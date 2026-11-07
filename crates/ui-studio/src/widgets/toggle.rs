//! Latching toggles (`docs/design/language`, Components > Toggle): M, S, R, I and option toggles
//! (SYNC, RETRIG, HZ), 24 high with radius 4 and Sans 600 11. Off, every role is `surface.raised`
//! with `text.muted` and a 1 px `line`; on, the role decides: mute inverts (`text` fill,
//! `bg.window` letter), solo is `signal`, record arm `meter.hot`, monitor and option a tinted fill
//! with `signal` letter and border. An unavailable toggle stays visible, ghosted, so strips keep
//! their layout. The switch ([`super::switch`]) is for enabling a whole section; this is for
//! everything else.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::toggle as spec;
use egui::{Align2, Color32, Sense, Stroke, StrokeKind, Ui, vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Role {
    Mute,
    Solo,
    Arm,
    Monitor,
    Option,
}

/// (fill, text, border) of a toggle in `role`, `on` or off, available or not.
pub fn colors(role: Role, on: bool, available: bool) -> (Color32, Color32, Color32) {
    let t = &TOKENS;
    if !available {
        return (t.toggle_unavailable_fill, t.ink_arrow, t.line);
    }
    if !on {
        return (t.surface_raised, t.text_muted, t.line);
    }
    match role {
        Role::Mute => (t.text, t.bg_window, t.text),
        Role::Solo => (t.signal, t.surface_inset, t.signal),
        Role::Arm => (t.meter_hot, t.bg_window, t.meter_hot),
        Role::Monitor | Role::Option => (t.toggle_option_fill, t.signal, t.signal),
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToggleLayout {
    pub rect: [f32; 4],
    pub text: String,
    pub on: bool,
    pub available: bool,
    #[serde(skip)]
    pub fill: Color32,
    #[serde(skip)]
    pub ink: Color32,
    #[serde(skip)]
    pub border: Color32,
}

pub struct ToggleOut {
    pub response: egui::Response,
    pub layout: ToggleLayout,
    /// The user clicked it: the caller flips its state.
    pub toggled: bool,
}

/// The width a toggle labeled `text` needs (at least square).
pub fn width(ui: &Ui, text: &str) -> f32 {
    (super::role_size(ui.painter(), &spec::FONT, text).x + 12.0).max(spec::HEIGHT)
}

/// Draws a toggle `width` wide (`None`: fit the label).
pub fn show(ui: &mut Ui, id: egui::Id, text: &str, role: Role, on: bool, available: bool, width: Option<f32>) -> ToggleOut {
    let w = width.unwrap_or_else(|| self::width(ui, text));
    let (rect, _) = ui.allocate_exact_size(vec2(w, spec::HEIGHT), Sense::hover());
    let response = ui.interact(rect, id, if available { Sense::click() } else { Sense::hover() });
    let toggled = available && response.clicked();
    let layout = paint(ui.painter(), rect, text, role, on, available);
    ToggleOut { response, layout, toggled }
}

/// Paints a toggle in `rect` without taking input (the caller owns the click, as the mixer's
/// strips do); [`show`] is this plus the click.
pub fn paint(p: &egui::Painter, rect: egui::Rect, text: &str, role: Role, on: bool, available: bool) -> ToggleLayout {
    let (fill, ink, border) = colors(role, on, available);
    p.rect_filled(rect, spec::RADIUS, fill);
    p.rect_stroke(rect, spec::RADIUS, Stroke::new(1.0, border), StrokeKind::Inside);
    super::role_text(p, &spec::FONT, text, ink, Align2::CENTER_CENTER, rect.center());
    ToggleLayout { rect: r4(rect), text: text.to_string(), on, available, fill, ink, border }
}
