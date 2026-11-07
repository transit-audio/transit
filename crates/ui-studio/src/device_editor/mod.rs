//! What every device panel in the design language shares (#326): the device a panel shows and
//! how its controls read and write it ([`Dev`], [`value_at`], [`parse`], [`set`]), the panel
//! dropdown ([`dropdown`]), and the A title bar ([`title_bar`], README, Device anatomy A:
//! identical on every device). The Macro Oscillator was the first panel (326-M) and the
//! Compressor the second (326-K); the window policy is [`crate::device_window`].

pub mod title_bar;

use crate::StudioApp;
use crate::theme::TOKENS;
use crate::tokens::{self, radius};
use crate::widgets;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_dsp::{ParamInfo, PluginInfo, Unit};
use transit_model::{Insert, TrackId};

/// The device an editor shows: its key, its insert and its descriptor.
#[derive(Clone, Copy)]
pub(crate) struct Dev<'a> {
    pub key: (TrackId, usize),
    pub ins: &'a Insert,
    pub info: &'static PluginInfo,
}

impl Dev<'_> {
    pub fn param(&self, id: &str) -> Option<&'static ParamInfo> {
        self.info.param(id)
    }

    /// The value a control shows: the automation it follows, else the insert's, else the default.
    pub fn value(&self, app: &StudioApp, id: &str) -> f32 {
        let Some(p) = self.param(id) else { return 0.0 };
        p.clamp(app.followed_param(self.key, id).or_else(|| self.ins.params.get(id).copied()).unwrap_or(p.default))
    }

    /// The value as its control prints it.
    pub fn text(&self, app: &StudioApp, id: &str) -> String {
        self.param(id).map(|p| p.format(self.value(app, id))).unwrap_or_default()
    }

    pub fn on(&self, app: &StudioApp, id: &str) -> bool {
        self.value(app, id) >= 0.5
    }

    pub fn index(&self, app: &StudioApp, id: &str) -> usize {
        self.value(app, id).round().max(0.0) as usize
    }

    pub fn uid(&self) -> u64 {
        self.ins.uid.get()
    }
}

/// The parameter value at a knob position: the taper honored, semitones and choices whole.
pub fn value_at(p: &ParamInfo, norm: f32) -> f32 {
    let v = p.from_normalized(norm);
    if matches!(p.unit, Unit::Semitones | Unit::Cents | Unit::Choice) { p.clamp(v.round()) } else { v }
}

/// A value typed into a knob's field: a number in the parameter's unit (`1.2k` and `1.2 kHz` for
/// kilohertz; for a time, a bare number or `ms` is milliseconds and a trailing `s` seconds), or a
/// choice by name. `None` when it does not read.
pub fn parse(p: &ParamInfo, text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    if !p.choices.is_empty() {
        return p.choices.iter().position(|c| c.to_lowercase() == t).map(|i| i as f32);
    }
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let mut v: f32 = num.parse().ok()?;
    let rest = t[num.len()..].trim_start();
    if p.unit == Unit::Hz && rest.starts_with('k') {
        v *= 1000.0;
    }
    if p.unit == Unit::Ms && rest.starts_with('s') {
        v *= 1000.0;
    }
    v.is_finite().then(|| p.clamp(v))
}

/// Whether a knob's arc grows from 12 o'clock: a linear range that spans zero.
pub fn bipolar(p: &ParamInfo) -> bool {
    p.min < 0.0 && p.max > 0.0 && p.taper == transit_dsp::Taper::Linear
}

/// Writes one parameter (merged into the gesture in progress) when it changed.
pub(crate) fn set(app: &mut StudioApp, dev: Dev, id: &str, value: f32) {
    let Some(p) = dev.param(id) else { return };
    let v = p.clamp(value);
    if (v - dev.value(app, id)).abs() > f32::EPSILON {
        app.set_builtin_param(dev.key, id, v);
    }
}

/// The child `Ui` that draws into `rect` without moving the parent's cursor.
pub(crate) fn child(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Min)))
}

/// What a dropdown shows before its text.
#[derive(Clone, Copy)]
pub(crate) enum Lead<'a> {
    None,
    /// A faint group or operator (`Tone`, `×`).
    Text(&'a str),
    /// An 8 px dot (a routing row's source).
    Dot(Color32),
    /// Room the caller paints into (a curve icon).
    Gap(f32),
}

/// A dropdown (README, Dropdown): `fill` (`surface.inset` on panels, `surface.raised` in routing
/// rows), 1 px `line`, the control radius, the chevron in `text.faint`, `lead` before the text.
/// Its list is a popup of check rows. Returns the response and the choice the user made.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dropdown(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    fill: Color32,
    lead: Lead,
    text: &str,
    mono: bool,
    choices: &[&str],
    selected: usize,
) -> (egui::Response, Option<usize>) {
    let t = &TOKENS;
    let resp = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    p.rect(rect, radius::CONTROL, if resp.hovered() { t.button_hover } else { fill }, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let mut x = rect.min.x + 8.0;
    let cy = rect.center().y;
    match lead {
        Lead::None => {}
        Lead::Text(pre) => {
            let r = widgets::role_text(p, &tokens::LABEL_CONTROL, pre, t.text_faint, Align2::LEFT_CENTER, pos2(x, cy));
            x = r.max.x + 8.0;
        }
        Lead::Dot(c) => {
            p.circle_filled(pos2(x + 4.0, cy), 4.0, c);
            x += 16.0;
        }
        Lead::Gap(w) => x += w + 8.0,
    }
    let role = if mono { tokens::VALUE } else { tokens::chrome::BUTTON_FONT };
    let clip = p.with_clip_rect(Rect::from_x_y_ranges(x..=rect.max.x - 18.0, rect.y_range()).intersect(p.clip_rect()));
    widgets::role_text(&clip, &role, text, t.text, Align2::LEFT_CENTER, pos2(x, cy));
    chevron(p, pos2(rect.max.x - 11.0, cy), 8.0, t.text_faint);
    let mut pick = None;
    crate::chrome_pick::dropdown(&resp).show(|ui| {
        crate::chrome::menu_style(ui.style_mut());
        for (i, c) in choices.iter().enumerate() {
            if crate::menus::check_button(ui, i == selected, *c).clicked() && i != selected {
                pick = Some(i);
            }
        }
    });
    (resp, pick)
}

/// A downward chevron `w` wide centered at `c`.
pub(crate) fn chevron(p: &egui::Painter, c: Pos2, w: f32, color: Color32) {
    let h = w / 2.0;
    p.add(egui::Shape::line(vec![c + vec2(-h, -h / 2.0), c + vec2(0.0, h / 2.0), c + vec2(h, -h / 2.0)], Stroke::new(1.5, color)));
}
