//! The application chrome (#326, the maintainer's application chrome document; `chrome` in
//! tokens.json, `docs/design/language` "Application chrome"): egui's own widgets outside the
//! device panels.
//!
//! [`crate::theme`] maps the global rules onto egui's style: borderless buttons, popups, scroll
//! bars, tooltips, selection. egui gives buttons, text fields, combo boxes and checkboxes one
//! shared stroke and fill per state, so what differs from a button is set here, in a scope around
//! the widget: [`field`] (the inset well with a 1 px `line`), [`checkbox`] and [`radio_value`]
//! (the 14 px box, `signal` when on). The default button, the modal dialog, the tool-window
//! footer, the menu styles and the keyboard focus ring are drawn here too. Each returns or stores
//! a layout struct for deterministic tests.

use crate::theme::{TOKENS, Tokens};
use crate::tokens::chrome as c;
use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, vec2};

/// A rect as `[min x, min y, max x, max y]`, rounded as the layout structs carry it.
fn r4(r: Rect) -> [f32; 4] {
    crate::widgets::r4(r)
}

/// Field visuals on `v`: the inset well (`surface.inset`) with a 1 px `line`, a `line.strong`
/// edge on hover or while open. A focused text field takes egui's selection stroke (1.5 px
/// `signal`); a combo box's face is the same well (the spec's dropdown).
pub fn field_visuals(v: &mut egui::Visuals, t: &Tokens) {
    for (w, edge) in [
        (&mut v.widgets.inactive, t.line),
        (&mut v.widgets.hovered, t.line_strong),
        (&mut v.widgets.active, t.line_strong),
        (&mut v.widgets.open, t.line_strong),
    ] {
        w.weak_bg_fill = t.surface_inset;
        w.bg_stroke = Stroke::new(1.0, edge);
    }
}

/// Runs `add` with [`field_visuals`]: wrap a text field, combo box or drag value in it.
pub fn field<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    with_style(ui, |st| field_visuals(&mut st.visuals, &TOKENS), add)
}

/// Runs `add` with the ui's style changed by `set`, then puts the style back. No child ui: the
/// widget lays out and takes its id exactly as it would without the chrome.
fn with_style<R>(ui: &mut Ui, set: impl FnOnce(&mut egui::Style), add: impl FnOnce(&mut Ui) -> R) -> R {
    let saved = ui.style().clone();
    let mut st = (*saved).clone();
    set(&mut st);
    ui.set_style(st);
    let out = add(ui);
    ui.set_style(saved);
    out
}

/// A text field in the chrome: the [`field`] well, 26 high, its text centered vertically.
pub fn text_field(ui: &mut Ui, edit: egui::TextEdit<'_>) -> Response {
    text_field_enabled(ui, true, edit)
}

/// [`text_field`], enabled or not.
pub fn text_field_enabled(ui: &mut Ui, enabled: bool, edit: egui::TextEdit<'_>) -> Response {
    let edit = edit.min_size(vec2(0.0, c::CONTROL_HEIGHT)).vertical_align(egui::Align::Center);
    field(ui, |ui| ui.add_enabled(enabled, edit))
}

/// A combo box's chevron (`ComboBox::icon`): 12 px, `text.faint` (`text.muted` on hover or open).
pub fn chevron(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, open: bool) {
    let hot = open || visuals.bg_stroke.color == TOKENS.line_strong;
    let ink = if hot { TOKENS.text_muted } else { TOKENS.text_faint };
    crate::icons::paint(ui.painter(), rect.center(), crate::icons::Icon::ChevronDown, c::CHEVRON, ink);
}

/// Checkbox or radio visuals on `st` for a control that is `on`: a 14 px box (radius 3; a radio is
/// round) in an 18 px hit; off `surface.inset` with a 1 px `line.strong`, on `signal` with the mark
/// in `surface.inset`. The label stays `text`.
pub fn check_visuals(st: &mut egui::Style, t: &Tokens, on: bool, radio: bool) {
    st.spacing.icon_width = c::CHECKBOX_BOX;
    // egui draws a radio's dot a third of `icon_width_inner` in radius.
    st.spacing.icon_width_inner = if radio { c::RADIO_DOT * 1.5 } else { c::CHECKBOX_MARK };
    st.spacing.interact_size.y = c::CHECKBOX_HIT;
    st.visuals.override_text_color = Some(t.text);
    let v = &mut st.visuals.widgets;
    for w in [&mut v.inactive, &mut v.hovered, &mut v.active] {
        w.bg_fill = if on { t.signal } else { t.surface_inset };
        w.bg_stroke = if on { Stroke::NONE } else { Stroke::new(1.0, t.line_strong) };
        w.fg_stroke = Stroke::new(2.0, t.surface_inset);
        w.corner_radius = CornerRadius::same(c::RADIUS_CHECKBOX as u8);
    }
}

/// `ui.checkbox` in the chrome's checkbox.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
    checkbox_enabled(ui, true, checked, text)
}

/// `ui.add_enabled(enabled, Checkbox::new(..))` in the chrome's checkbox.
pub fn checkbox_enabled(ui: &mut Ui, enabled: bool, checked: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
    let on = *checked;
    with_style(ui, |st| check_visuals(st, &TOKENS, on, false), |ui| ui.add_enabled(enabled, egui::Checkbox::new(checked, text)))
}

/// `ui.radio_value` in the chrome's radio button.
pub fn radio_value<V: PartialEq>(ui: &mut Ui, current: &mut V, alternative: V, text: impl Into<egui::WidgetText>) -> Response {
    let on = *current == alternative;
    with_style(ui, |st| check_visuals(st, &TOKENS, on, true), |ui| ui.radio_value(current, alternative, text))
}

/// `ui.add_enabled(enabled, RadioButton::new(..))` in the chrome's radio button.
pub fn radio_enabled(ui: &mut Ui, enabled: bool, selected: bool, text: impl Into<egui::WidgetText>) -> Response {
    with_style(ui, |st| check_visuals(st, &TOKENS, selected, true), |ui| ui.add_enabled(enabled, egui::RadioButton::new(selected, text)))
}

/// A button as last drawn: where, its fill and ink, its label.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ButtonLayout {
    pub rect: [f32; 4],
    #[serde(skip)]
    pub fill: Color32,
    #[serde(skip)]
    pub ink: Color32,
    pub text: String,
    /// The default style (`signal` fill).
    pub default: bool,
}

/// The default button: `signal` fill, ink `surface.inset`, Sans 600 12, 26 high with 16 px a
/// side; hover and pressed step its fill; disabled takes the unavailable fill and `ink.arrow`. At
/// most one per dialog, the action Enter performs.
pub fn default_button(ui: &mut Ui, text: &str) -> (Response, ButtonLayout) {
    let t = &TOKENS;
    let enabled = ui.is_enabled();
    let font = c::DEFAULT_BUTTON_FONT.font_id(ui.ctx());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, t.surface_inset);
    let size = vec2((galley.size().x + 2.0 * c::DEFAULT_BUTTON_PADDING[1]).ceil(), c::CONTROL_HEIGHT);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    let (fill, ink) = if !enabled {
        (t.toggle_unavailable_fill, t.ink_arrow)
    } else if resp.is_pointer_button_down_on() {
        (t.default_button_pressed, t.surface_inset)
    } else if resp.hovered() {
        (t.default_button_hover, t.surface_inset)
    } else {
        (t.signal, t.surface_inset)
    };
    let p = ui.painter();
    p.rect_filled(rect, c::RADIUS_CONTROL, fill);
    p.text(rect.center(), Align2::CENTER_CENTER, text, c::DEFAULT_BUTTON_FONT.font_id(ui.ctx()), ink);
    (resp, ButtonLayout { rect: r4(rect), fill, ink, text: text.to_string(), default: true })
}

/// A standard button, recorded: egui's button (borderless `surface.raised`), exactly 26 high with
/// 12 px a side, and its layout.
pub fn button(ui: &mut Ui, text: &str) -> (Response, ButtonLayout) {
    let resp = with_style(
        ui,
        |st| st.spacing.button_padding = vec2(c::BUTTON_PADDING[1], c::BUTTON_PADDING[0]),
        |ui| ui.add(egui::Button::new(text).min_size(vec2(0.0, c::CONTROL_HEIGHT))),
    );
    let t = &TOKENS;
    let fill = if resp.is_pointer_button_down_on() {
        t.surface_selected
    } else if resp.hovered() {
        t.button_hover
    } else {
        t.surface_raised
    };
    let layout = ButtonLayout { rect: r4(resp.rect), fill, ink: t.text, text: text.to_string(), default: false };
    (resp, layout)
}

/// What a modal asks: the question (its title), an optional detail line, and its buttons: the
/// default (rightmost; the action Enter performs, its label naming what happens), Cancel left of
/// it, and an optional third choice alone at the far left.
pub struct Ask<'a> {
    pub question: String,
    pub detail: Option<String>,
    pub default: &'a str,
    pub cancel: &'a str,
    pub third: Option<&'a str>,
}

/// The choice a modal's buttons made this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Default,
    Cancel,
    Third,
}

/// A modal as last drawn (tests read it with [`modal_layout`]).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ModalLayout {
    /// The dialog's outer rect (380 wide).
    pub rect: [f32; 4],
    #[serde(skip)]
    pub fill: Color32,
    #[serde(skip)]
    pub edge: Color32,
    #[serde(skip)]
    pub scrim: Color32,
    pub question: [f32; 4],
    pub detail: Option<[f32; 4]>,
    /// The detail line's text.
    pub detail_text: Option<String>,
    /// Left to right.
    pub buttons: Vec<ButtonLayout>,
}

fn modal_key(id: egui::Id) -> egui::Id {
    id.with("chrome-modal-layout")
}

/// The layout the modal `id` drew last frame.
pub fn modal_layout(ctx: &egui::Context, id: egui::Id) -> Option<ModalLayout> {
    ctx.data(|d| d.get_temp::<ModalLayout>(modal_key(id)))
}

/// The modal's frame: `surface.panel`, 1 px `line.strong`, radius 8, padding 18 18 16, no shadow.
pub fn modal_frame(t: &Tokens) -> egui::Frame {
    let [top, side, bottom] = c::MODAL_PADDING;
    egui::Frame::new()
        .fill(t.surface_panel)
        .stroke(Stroke::new(1.0, t.line_strong))
        .corner_radius(CornerRadius::same(c::RADIUS_WINDOW as u8))
        .inner_margin(egui::Margin { left: side as i8, right: side as i8, top: top as i8, bottom: bottom as i8 })
}

/// A modal dialog over the `overlay` scrim: no title bar (the question is the title, Sans 600 13;
/// the detail Sans 12 `text.muted`), 380 wide, the footer as [`Ask`] says. Returns the choice made
/// this frame: a click, or the keys. Enter is the default button and Escape is Cancel, as in Live
/// and Logic (the chrome document: "Enter = default, Escape = Cancel").
pub fn modal(ctx: &egui::Context, id: egui::Id, ask: &Ask<'_>) -> Option<Choice> {
    let t = &TOKENS;
    let frame = modal_frame(t);
    // The content width that makes the outer width 380: the padding and the 1 px edge each side.
    let inner_w = c::MODAL_WIDTH - 2.0 * c::MODAL_PADDING[1] - 2.0 * frame.stroke.width;
    let mut layout = ModalLayout { fill: t.surface_panel, edge: t.line_strong, scrim: t.overlay, ..ModalLayout::default() };
    let mut choice = None;
    let shown = egui::Modal::new(id).frame(frame).backdrop_color(t.overlay).show(ctx, |ui| {
        ui.set_width(inner_w);
        ui.spacing_mut().item_spacing.y = 6.0;
        let q = ui.label(egui::RichText::new(&ask.question).font(c::QUESTION_FONT.font_id(ui.ctx())).color(t.text));
        layout.question = r4(q.rect);
        if let Some(detail) = &ask.detail {
            let d = ui.label(egui::RichText::new(detail).font(c::DETAIL_FONT.font_id(ui.ctx())).color(t.text_muted));
            layout.detail = Some(r4(d.rect));
            layout.detail_text = Some(detail.clone());
        }
        ui.add_space(c::MODAL_FOOTER_GAP - ui.spacing().item_spacing.y);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(third) = ask.third {
                let (b, l) = button(ui, third);
                layout.buttons.push(l);
                if b.clicked() {
                    choice = Some(Choice::Third);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (d, dl) = default_button(ui, ask.default);
                let (cb, cl) = button(ui, ask.cancel);
                layout.buttons.push(cl);
                layout.buttons.push(dl);
                if d.clicked() {
                    choice = Some(Choice::Default);
                }
                if cb.clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
    });
    layout.rect = r4(shown.response.rect);
    ctx.data_mut(|d| d.insert_temp(modal_key(id), layout));
    choice.or_else(|| modal_keys(ctx))
}

/// The modal's keys this frame: Enter is the default button, Escape is Cancel.
fn modal_keys(ctx: &egui::Context) -> Option<Choice> {
    use crate::shortcuts::{Action, hit};
    let (enter, escape) = ctx.input(|i| (hit(i, Action::DialogConfirm), hit(i, Action::DialogCancel)));
    if enter {
        Some(Choice::Default)
    } else if escape {
        Some(Choice::Cancel)
    } else {
        None
    }
}

/// A tool window's footer: a 1 px `line` across the window above it, 10 px under the line, the
/// buttons right-aligned (the first one `add` adds is rightmost). Call it last in the body.
pub fn tool_footer<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let body_x = c::TOOL_BODY_PADDING[1];
    let y = ui.cursor().min.y;
    let r = ui.max_rect();
    ui.painter().line_segment([egui::pos2(r.min.x - body_x, y), egui::pos2(r.max.x + body_x, y)], Stroke::new(1.0, TOKENS.line));
    ui.add_space(c::TOOL_FOOTER_PADDING[0]);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add).inner
}

/// The menu bar's items (`MenuBar::style`): 22 high, 8 px a side, radius 4, no border; the open
/// item takes `button.hover` (egui's `widgets.open`).
pub fn menu_bar_style(st: &mut egui::Style) {
    egui::containers::menu::menu_style(st);
    st.spacing.interact_size.y = c::MENU_BAR_ITEM_HEIGHT;
    st.spacing.button_padding = vec2(c::MENU_BAR_ITEM_PADDING[1], c::MENU_BAR_ITEM_PADDING[0]);
    row_corners(st);
}

/// Menu rows (`MenuConfig::style`): 24 high, radius 4, no border, hover `button.hover`.
pub fn menu_style(st: &mut egui::Style) {
    egui::containers::menu::menu_style(st);
    st.spacing.interact_size.y = c::ROW_HEIGHT;
    st.spacing.button_padding = vec2(c::POPUP_PADDING, 0.0);
    row_corners(st);
}

fn row_corners(st: &mut egui::Style) {
    let v = &mut st.visuals.widgets;
    for w in [&mut v.inactive, &mut v.hovered, &mut v.active, &mut v.open] {
        w.corner_radius = CornerRadius::same(c::RADIUS_ROW as u8);
    }
}

/// A menu row's shortcut: Mono 400 11 in `text.faint`, right-aligned by egui.
pub fn shortcut(ctx: &egui::Context, text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).font(c::SHORTCUT_FONT.font_id(ctx)).color(TOKENS.text_faint)
}

/// Paints the keyboard focus ring: 1.5 px `focus.ring`, 2 px outside the focused widget. Text
/// fields show focus on their own edge (1.5 px `signal`), so they get none; a mouse click never
/// focuses a button in egui, so a ring means the keyboard moved there. Returns the ring's rect.
/// Call once a frame, after everything is drawn.
pub fn focus_ring(ctx: &egui::Context) -> Option<Rect> {
    let id = ctx.memory(|m| m.focused())?;
    if egui::TextEdit::load_state(ctx, id).is_some() {
        return None;
    }
    let resp = ctx.read_response(id)?;
    let ring = resp.rect.expand(c::FOCUS_RING_OFFSET + c::FOCUS_RING_WIDTH / 2.0);
    ctx.layer_painter(resp.layer_id).rect_stroke(
        ring,
        c::RADIUS_CONTROL + c::FOCUS_RING_OFFSET,
        Stroke::new(c::FOCUS_RING_WIDTH, TOKENS.focus_ring),
        StrokeKind::Middle,
    );
    Some(ring)
}

/// How a status line reads (#326): a warning is the message in `text` with a triangle, no color;
/// an error is a `meter.hot` circle-alert (and edge, where there is one). The message
/// itself is always `text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    /// The triangle's ink.
    pub fn glyph(self) -> Color32 {
        match self {
            Severity::Warning => TOKENS.text,
            Severity::Error => TOKENS.meter_hot,
        }
    }

    /// The glyph: triangle-alert for a warning, circle-alert for an error (as the artboard draws them).
    pub fn icon(self) -> crate::icons::Icon {
        match self {
            Severity::Warning => crate::icons::Icon::Warning,
            Severity::Error => crate::icons::Icon::Error,
        }
    }

    /// The message's ink: `text` for both.
    pub fn ink(self) -> Color32 {
        TOKENS.text
    }
}

/// The triangle's box beside a message of `font_size` points.
pub fn glyph_size(font_size: f32) -> f32 {
    (font_size + 3.0).round()
}

/// Paints a status line from `at` (its left center): the triangle in the severity's ink, then the
/// message in `text`. Returns the rect of both.
pub fn status_text(p: &egui::Painter, at: egui::Pos2, text: &str, font: egui::FontId, sev: Severity) -> Rect {
    let g = glyph_size(font.size);
    crate::icons::paint(p, egui::pos2(at.x + g / 2.0, at.y), sev.icon(), g, sev.glyph());
    let r = p.text(egui::pos2(at.x + g + 3.0, at.y), Align2::LEFT_CENTER, text, font, sev.ink());
    Rect::from_min_max(egui::pos2(at.x, r.min.y.min(at.y - g / 2.0)), egui::pos2(r.max.x, r.max.y.max(at.y + g / 2.0)))
}

/// [`status_text`] as a widget: the triangle and a label of `size` points.
pub fn status_label(ui: &mut Ui, text: impl Into<String>, size: f32, sev: Severity) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let g = glyph_size(size);
        let (r, _) = ui.allocate_exact_size(vec2(g, g), Sense::hover());
        crate::icons::paint(ui.painter(), r.center(), sev.icon(), g, sev.glyph());
        ui.label(egui::RichText::new(text.into()).size(size).color(sev.ink()))
    })
    .inner
}
