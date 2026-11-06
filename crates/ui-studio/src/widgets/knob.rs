//! The design language's knob (`docs/design/language`, Components > Knob): a 270 degree sweep in
//! a 100-unit box scaled to its size, a `line` track, the value arc in `signal` (from 7 o'clock,
//! or from 12 o'clock when bipolar), the modulation ring in `mod` from the base value to base plus
//! depth, a `knob.cap` face with a `line.strong` rim and a `text` pointer; the label below in
//! `label.control` (`label.hero` at size L) and the value below that in mono.
//!
//! Interaction: vertical drag (Shift for fine), double-click to reset, click the value to type,
//! Ctrl+scroll (Cmd on macOS) to nudge. A plain wheel is never the knob's: it scrolls the panel
//! around it and leaves the value alone (326-M: a device body scrolls). The knob works on a normalized position (0..1); the caller maps it to the
//! parameter, formats the value text and parses what was typed.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{self, knob as geo};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// Pixels of vertical drag for the full travel, and the divisor Shift applies.
pub const DRAG_PX: f32 = 200.0;
pub const FINE: f32 = 10.0;
/// One Ctrl+wheel notch (or 40 px of smooth scroll) moves this much; Shift a tenth of it.
pub const NUDGE: f32 = 0.01;
/// Rows under the dial: the label and the value.
pub const LABEL_H: f32 = 16.0;
pub const VALUE_H: f32 = 15.0;

/// The three sizes of the design (diameters from tokens.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Size {
    /// A device's 2 to 4 hero controls.
    L,
    /// The default.
    M,
    /// Dense sections.
    S,
}

impl Size {
    pub fn diameter(self) -> f32 {
        match self {
            Size::L => geo::L,
            Size::M => geo::M,
            Size::S => geo::S,
        }
    }
}

/// What the knob shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum State {
    Default,
    /// Held or hovered: the cap lifts to `knob.capActive`.
    Active,
    /// A modulation ring is drawn.
    Modulated,
    /// Arc in `ink.ghost`, pointer in `ink.arrow`; no interaction.
    Disabled,
}

/// The inputs of one knob.
pub struct Knob<'a> {
    pub size: Size,
    pub label: &'a str,
    /// The value as the caller formats it, in real units (`1.20 kHz`).
    pub value_text: &'a str,
    /// Normalized position, 0..1.
    pub value: f32,
    /// Where double-click returns (normalized).
    pub default: f32,
    /// The arc grows from 12 o'clock instead of 7.
    pub bipolar: bool,
    /// Signed modulation depth (normalized); `None` hides the ring (no route, or the modulation
    /// section is disabled).
    pub modulation: Option<f32>,
    pub disabled: bool,
}

impl<'a> Knob<'a> {
    pub fn new(label: &'a str, value_text: &'a str, value: f32) -> Knob<'a> {
        Knob { size: Size::M, label, value_text, value, default: 0.0, bipolar: false, modulation: None, disabled: false }
    }
}

/// Where everything was drawn, and with which tokens (for tests and `ui.inspect`).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct KnobLayout {
    /// The whole cell (dial, label, value).
    pub rect: [f32; 4],
    pub dial: [f32; 4],
    pub label: [f32; 4],
    pub value: [f32; 4],
    pub size: Size,
    pub state: State,
    /// The pointer's angle, degrees clockwise from 12 o'clock.
    pub pointer_deg: f32,
    /// The value arc, degrees clockwise from 12 o'clock (equal ends: no arc).
    pub arc_deg: [f32; 2],
    /// The modulation ring, when drawn.
    pub mod_deg: Option<[f32; 2]>,
    /// Radii in points: track and value arc, ring, cap.
    pub radii: [f32; 3],
    #[serde(skip)]
    pub cap: Color32,
    #[serde(skip)]
    pub arc: Color32,
    #[serde(skip)]
    pub pointer: Color32,
    /// Whether the value field is open for typing.
    pub editing: bool,
}

/// What a knob did this frame.
pub struct KnobOut {
    pub response: egui::Response,
    pub layout: KnobLayout,
    /// The new normalized value, when the user turned, nudged or reset it.
    pub value: Option<f32>,
    /// Text the user typed into the value field and confirmed with Enter.
    pub typed: Option<String>,
    /// The new value came from a Ctrl+wheel nudge (the caller merges a wheel gesture's nudges into
    /// one undo step while the wheel continues).
    pub wheeled: bool,
}

/// The pointer angle of a normalized position: degrees clockwise from 12 o'clock.
pub fn angle(norm: f32) -> f32 {
    geo::START_DEGREES + geo::SWEEP_DEGREES * norm.clamp(0.0, 1.0)
}

/// The value arc's ends: from 7 o'clock (unipolar) or 12 o'clock (bipolar) to the pointer.
pub fn arc_span(norm: f32, bipolar: bool) -> [f32; 2] {
    let a = angle(norm);
    let origin = if bipolar { angle(0.5) } else { angle(0.0) };
    [origin.min(a), origin.max(a)]
}

/// The modulation ring's ends: from the base value to base plus depth, clamped to the sweep.
pub fn mod_span(norm: f32, depth: f32) -> [f32; 2] {
    let (a, b) = (angle(norm), angle(norm + depth));
    [a.min(b), a.max(b)]
}

/// The normalized position after dragging `dy` points (up is negative) from `norm`.
pub fn dragged(norm: f32, dy: f32, fine: bool) -> f32 {
    (norm - dy / DRAG_PX / if fine { FINE } else { 1.0 }).clamp(0.0, 1.0)
}

/// The normalized position after scrolling `notches` wheel notches (up is positive).
pub fn nudged(norm: f32, notches: f32, fine: bool) -> f32 {
    (norm + notches * NUDGE / if fine { FINE } else { 1.0 }).clamp(0.0, 1.0)
}

/// A point at `deg` (clockwise from 12) and `r` from `c`.
fn polar(c: Pos2, r: f32, deg: f32) -> Pos2 {
    let a = (deg - 90.0).to_radians();
    c + vec2(a.cos(), a.sin()) * r
}

fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = (((to - from).abs() / geo::SWEEP_DEGREES * 48.0).ceil() as usize).max(2);
    (0..=n).map(|i| polar(c, r, from + (to - from) * i as f32 / n as f32)).collect()
}

/// The cell size of a knob of `size`: the dial, the label row and the value row.
pub fn cell(size: Size) -> egui::Vec2 {
    let d = size.diameter();
    vec2(d.max(56.0), d + LABEL_H + VALUE_H)
}

/// Draws the knob and handles its gestures.
pub fn show(ui: &mut Ui, id: egui::Id, k: &Knob) -> KnobOut {
    let t = &TOKENS;
    let d = k.size.diameter();
    let (rect, _) = ui.allocate_exact_size(cell(k.size), Sense::hover());
    let dial = Rect::from_center_size(pos2(rect.center().x, rect.min.y + d / 2.0), vec2(d, d));
    let label = Rect::from_min_size(pos2(rect.min.x, dial.max.y), vec2(rect.width(), LABEL_H));
    let value_r = Rect::from_min_size(pos2(rect.min.x, label.max.y), vec2(rect.width(), VALUE_H));
    let sense = if k.disabled { Sense::hover() } else { Sense::click_and_drag() };
    let response = ui.interact(dial, id, sense);
    let value_resp = ui.interact(value_r, id.with("value"), if k.disabled { Sense::hover() } else { Sense::click() });
    let edit_id = id.with("editing");
    let mut editing = ui.data(|d| d.get_temp::<String>(edit_id));
    let mut new = None;
    let mut typed = None;
    let mut wheeled = false;
    if !k.disabled {
        let fine = ui.input(|i| i.modifiers.shift);
        if response.dragged() {
            let dy = ui.input(|i| i.pointer.delta().y);
            let n = dragged(k.value, dy, fine);
            if (n - k.value).abs() > f32::EPSILON {
                new = Some(n);
            }
        }
        if response.double_clicked() {
            new = Some(k.default);
        }
        if response.hovered() {
            // Only a wheel with Ctrl (Cmd) is the knob's, and the knob takes it; a plain wheel
            // stays with the scroll area around it.
            let (notches, fine) = super::command_wheel(ui);
            if notches != 0.0 {
                let n = nudged(k.value, notches, fine);
                if (n - k.value).abs() > f32::EPSILON {
                    new = Some(n);
                    wheeled = true;
                }
            }
        }
        if value_resp.clicked() && editing.is_none() {
            editing = Some(k.value_text.to_string());
        }
    }
    if let Some(text) = editing.as_mut() {
        let edit = egui::TextEdit::singleline(text).font(tokens::VALUE.font_id(ui.ctx())).horizontal_align(egui::Align::Center);
        let r = ui.put(value_r, edit);
        r.request_focus();
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter {
            typed = Some(text.clone());
        }
        if enter || escape {
            editing = None;
        }
    }
    ui.data_mut(|d| match &editing {
        Some(text) => {
            d.insert_temp(edit_id, text.clone());
        }
        None => {
            d.remove::<String>(edit_id);
        }
    });
    let shown = new.unwrap_or(k.value);
    let state = if k.disabled {
        State::Disabled
    } else if response.dragged() || response.hovered() {
        State::Active
    } else if k.modulation.is_some() {
        State::Modulated
    } else {
        State::Default
    };
    let s = d / geo::BOX;
    let c = dial.center();
    let (cap, arc_c, pointer_c) = match state {
        State::Disabled => (t.knob_cap, t.ink_ghost, t.ink_arrow),
        State::Active => (t.knob_cap_active, t.signal, t.text),
        _ => (t.knob_cap, t.signal, t.text),
    };
    let p = ui.painter();
    let (track_r, ring_r, cap_r) = (geo::TRACK_RADIUS * s, geo::MOD_RING_RADIUS * s, geo::CAP_RADIUS * s);
    p.add(Shape::line(arc(c, track_r, angle(0.0), angle(1.0)), Stroke::new(geo::TRACK_STROKE * s, t.line)));
    let span = arc_span(shown, k.bipolar);
    if span[1] - span[0] > 0.1 {
        p.add(Shape::line(arc(c, track_r, span[0], span[1]), Stroke::new(geo::TRACK_STROKE * s, arc_c)));
    }
    let mod_deg = k.modulation.filter(|_| !k.disabled).map(|depth| mod_span(shown, depth));
    if let Some([a, b]) = mod_deg
        && b - a > 0.1
    {
        p.add(Shape::line(arc(c, ring_r, a, b), Stroke::new(geo::MOD_RING_STROKE * s, t.modulation)));
    }
    p.circle(c, cap_r, cap, Stroke::new(geo::CAP_RIM_STROKE * s, t.line_strong));
    let a = angle(shown);
    p.line_segment([polar(c, geo::POINTER_FROM * s, a), polar(c, geo::POINTER_TO * s, a)], Stroke::new(geo::POINTER_STROKE * s, pointer_c));
    let label_role = if k.size == Size::L { tokens::LABEL_HERO } else { tokens::LABEL_CONTROL };
    let label_ink = if k.disabled { t.text_faint } else { label_role.color() };
    super::role_text(p, &label_role, k.label, label_ink, Align2::CENTER_CENTER, label.center());
    if editing.is_none() {
        let ink = if k.disabled { t.text_faint } else { tokens::VALUE.color() };
        super::role_text(p, &tokens::VALUE, k.value_text, ink, Align2::CENTER_CENTER, value_r.center());
    }
    let layout = KnobLayout {
        rect: r4(rect),
        dial: r4(dial),
        label: r4(label),
        value: r4(value_r),
        size: k.size,
        state,
        pointer_deg: a,
        arc_deg: span,
        mod_deg,
        radii: [track_r, ring_r, cap_r],
        cap,
        arc: arc_c,
        pointer: pointer_c,
        editing: editing.is_some(),
    };
    KnobOut { response, layout, value: new, typed, wheeled }
}
