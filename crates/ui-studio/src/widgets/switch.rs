//! The section switch (`docs/design/language`, Components > Switch): 34 by 20, radius 10; on is a
//! `signal` track with a white knob, off a `line.strong` track with a `text.muted` knob; paired
//! with a mono `ON`/`OFF` readout on its left. Reserved for enabling an entire section; anything
//! else is a toggle.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{self, radius, switch as spec};
use egui::{Align2, Color32, Rect, Sense, Ui, pos2, vec2};

/// Gap between the readout and the track.
pub const GAP: f32 = 6.0;
/// The readout's slot, wide enough for `OFF` so the track never moves.
pub const READOUT_W: f32 = 24.0;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SwitchLayout {
    pub rect: [f32; 4],
    pub track: [f32; 4],
    pub knob_center: [f32; 2],
    pub readout: [f32; 4],
    pub readout_text: &'static str,
    pub on: bool,
    #[serde(skip)]
    pub track_color: Color32,
    #[serde(skip)]
    pub knob_color: Color32,
}

pub struct SwitchOut {
    pub response: egui::Response,
    pub layout: SwitchLayout,
    pub toggled: bool,
}

/// The size the switch and its readout take.
pub fn size() -> egui::Vec2 {
    vec2(READOUT_W + GAP + spec::WIDTH, spec::HEIGHT)
}

/// Draws the switch in `rect`-sized space allocated from `ui`.
pub fn show(ui: &mut Ui, id: egui::Id, on: bool) -> SwitchOut {
    let (rect, _) = ui.allocate_exact_size(size(), Sense::hover());
    paint(ui, id, rect, on)
}

/// Draws the switch at `rect` (from [`size`]) without allocating (the section bar places it).
pub fn paint(ui: &mut Ui, id: egui::Id, rect: Rect, on: bool) -> SwitchOut {
    let t = &TOKENS;
    let readout = Rect::from_min_size(rect.min, vec2(READOUT_W, rect.height()));
    let track = Rect::from_min_size(pos2(rect.max.x - spec::WIDTH, rect.center().y - spec::HEIGHT / 2.0), vec2(spec::WIDTH, spec::HEIGHT));
    let response = ui.interact(rect, id, Sense::click());
    let toggled = response.clicked();
    let inset = (spec::HEIGHT - spec::KNOB) / 2.0;
    let knob_x = if on { track.max.x - inset - spec::KNOB / 2.0 } else { track.min.x + inset + spec::KNOB / 2.0 };
    let knob_center = pos2(knob_x, track.center().y);
    let (track_color, knob_color) = if on { (t.signal, t.switch_on_knob) } else { (t.line_strong, t.text_muted) };
    let p = ui.painter();
    p.rect_filled(track, radius::SWITCH, track_color);
    p.circle_filled(knob_center, spec::KNOB / 2.0, knob_color);
    let readout_text = if on { "ON" } else { "OFF" };
    super::role_text(p, &tokens::VALUE, readout_text, if on { t.text } else { t.text_faint }, Align2::RIGHT_CENTER, readout.right_center());
    let layout = SwitchLayout {
        rect: r4(rect),
        track: r4(track),
        knob_center: [knob_center.x, knob_center.y],
        readout: r4(readout),
        readout_text,
        on,
        track_color,
        knob_color,
    };
    SwitchOut { response, layout, toggled }
}
