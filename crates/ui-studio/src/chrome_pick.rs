//! Choosing in the chrome (#326): the option label (a latching choice in the toggle's option role),
//! the check row (a choice in a list) and the combo (the spec's dropdown, whose list opens a gap
//! below its field). The option and check helpers take the shape of egui's `selectable_label`,
//! `selectable_value` and `toggle_value` and return the same `Response`, so a call site changes one
//! identifier.

use crate::theme::TOKENS;
use crate::tokens::chrome as c;
use crate::widgets::toggle::{self, Role};
use egui::{InnerResponse, Response, Sense, Stroke, StrokeKind, Ui, vec2};
use std::fmt::Display;
use std::hash::Hash;

/// A latching choice drawn as the kit's option toggle: on is `toggle.on.option.fill` with `signal`
/// text and a 1 px `signal` border; off is `surface.raised`, `text.muted`, `line`; 24 high, radius
/// 4; ghosted while the ui is disabled. Stands in for `selectable_label`.
pub fn option_label(ui: &mut Ui, selected: bool, text: impl Display) -> Response {
    let id = ui.next_auto_id();
    toggle::show(ui, id, &text.to_string(), Role::Option, selected, ui.is_enabled(), None).response
}

/// [`option_label`] for one value of a set: a click sets `current` to `alternative`. Stands in for
/// `selectable_value`.
pub fn option_value<T: PartialEq>(ui: &mut Ui, current: &mut T, alternative: T, text: impl Display) -> Response {
    let mut response = option_label(ui, *current == alternative, text);
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}

/// [`option_label`] for a flag: a click flips it. Stands in for `toggle_value`.
pub fn option_toggle(ui: &mut Ui, on: &mut bool, text: impl Display) -> Response {
    let mut response = option_label(ui, *on, text);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
}

/// A choice in a list (a combo's popup, a menu): the menu row with the current value checked in
/// `text`. Stands in for `selectable_label` inside a list.
pub fn choice_label(ui: &mut Ui, selected: bool, text: impl Display) -> Response {
    crate::menus::check_button(ui, selected, text.to_string())
}

/// [`choice_label`] for one value of a set. Stands in for `selectable_value` inside a list.
pub fn choice_value<T: PartialEq>(ui: &mut Ui, current: &mut T, alternative: T, text: impl Display) -> Response {
    let mut response = choice_label(ui, *current == alternative, text);
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}

/// The popup of a list that opens from a field (`response`): the chrome's popup, its top
/// `chrome.comboPopupGap` below the field. Menu-bar menus do not use this; they stay flush.
pub fn dropdown(response: &Response) -> egui::Popup<'static> {
    egui::Popup::menu(response).gap(c::COMBO_POPUP_GAP).style(crate::chrome::menu_style)
}

/// [`dropdown`] for a list that holds an editable field (a Save As name): it closes on a click
/// outside only, since egui's default closes on any click, the click into the field included. What
/// ends the choice (a pick, Save, Delete) calls `ui.close()` itself; Escape closes it as always.
pub fn dropdown_with_fields(response: &Response) -> egui::Popup<'static> {
    dropdown(response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
}

/// The spec's dropdown: a `surface.inset` face with a 1 px `line` (`line.strong` on hover or while
/// open), radius 5, 26 high, the value in `text` and a `text.faint` chevron (`text.muted` hot);
/// its list is a [`dropdown`] popup as wide as the face.
pub struct Combo {
    salt: egui::Id,
    text: String,
    width: Option<f32>,
    keep_open: bool,
    dense: bool,
    row: Option<f32>,
}

impl Combo {
    pub fn new(salt: impl Hash + std::fmt::Debug, text: impl Display) -> Self {
        Self { salt: egui::Id::NULL.with(salt), text: text.to_string(), width: None, keep_open: false, dense: false, row: None }
    }

    /// A fixed face width (default: the value's text and the chevron).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The face is the dense control height (24, `layout.denseControlHeight`) instead of the chrome's 26, for device panels and strips.
    pub fn dense(mut self) -> Self {
        self.dense = true;
        self
    }

    /// The face is `height` high: a cell in a compact grid of rows (the generic panel's modulation
    /// matrix), which sets its own line height, not a control standing alone.
    pub fn row(mut self, height: f32) -> Self {
        self.row = Some(height);
        self
    }

    /// The list stays open after a click inside it (checklists); a click outside closes it.
    pub fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }

    /// Draws the face and, while open, the list built by `add`.
    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
        let font = egui::FontId::proportional(12.0);
        let galley = ui.painter().layout_no_wrap(self.text.clone(), font.clone(), TOKENS.text);
        let pad = 8.0;
        let w = self.width.unwrap_or_else(|| (galley.size().x + 2.0 * pad + c::CHEVRON + pad).max(60.0));
        let h = if let Some(h) = self.row {
            h
        } else if self.dense {
            crate::tokens::layout::DENSE_CONTROL_HEIGHT
        } else {
            c::CONTROL_HEIGHT
        };
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let id = ui.make_persistent_id(self.salt);
        let sense = if ui.is_enabled() { Sense::click() } else { Sense::hover() };
        let response = ui.interact(rect, id, sense);
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        let hot = open || response.hovered();
        let p = ui.painter();
        p.rect_filled(rect, c::RADIUS_CONTROL, TOKENS.surface_inset);
        p.rect_stroke(rect, c::RADIUS_CONTROL, Stroke::new(1.0, if hot { TOKENS.line_strong } else { TOKENS.line }), StrokeKind::Inside);
        let text_at = egui::pos2(rect.min.x + pad, rect.center().y);
        let clipped = p.with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(rect.max.x - pad - c::CHEVRON, rect.max.y)));
        clipped.text(text_at, egui::Align2::LEFT_CENTER, &self.text, font, TOKENS.text);
        let chevron = egui::Rect::from_center_size(egui::pos2(rect.max.x - pad - c::CHEVRON / 2.0, rect.center().y), vec2(c::CHEVRON, c::CHEVRON));
        crate::icons::paint(
            p,
            chevron.center(),
            crate::icons::Icon::ChevronDown,
            c::CHEVRON,
            if hot { TOKENS.text_muted } else { TOKENS.text_faint },
        );
        let behavior = if self.keep_open { egui::PopupCloseBehavior::CloseOnClickOutside } else { egui::PopupCloseBehavior::CloseOnClick };
        let inner = dropdown(&response).width(rect.width()).close_behavior(behavior).show(|ui| {
            ui.set_min_width(ui.available_width());
            // The Menus board's row: padding 4 on the left, a 16 px check gutter, the text straight after it.
            ui.spacing_mut().icon_spacing = 0.0;
            ui.spacing_mut().button_padding = vec2(c::POPUP_PADDING, 0.0);
            egui::ScrollArea::vertical()
                .max_height(ui.spacing().combo_height)
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    add(ui)
                })
                .inner
        });
        InnerResponse::new(inner.map(|r| r.inner), response)
    }
}
