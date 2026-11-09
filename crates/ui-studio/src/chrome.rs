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

/// Menu rows (`MenuConfig::style`) in a menu that has checks: 24 high, radius 4, no border, hover
/// `button.hover`; a row's left inset is `button_padding.x` ([`MenuRow`] adds the rest of the right
/// inset) and every row keeps the 16 px check gutter ([`row_has_gutter`]).
pub fn menu_style(st: &mut egui::Style) {
    egui::containers::menu::menu_style(st);
    st.spacing.interact_size.y = c::ROW_HEIGHT;
    st.spacing.button_padding = vec2(c::ROW_PADDING[0], 0.0);
    row_corners(st);
}

/// [`menu_style`] for a menu with no checkable row: no gutter, the text 12 inside the popup's edge.
/// The declaration rides in the style (`spacing.menu_spacing`, which egui lays nothing out with):
/// zero means no gutter, so every row knows it before the first one paints.
pub fn menu_style_plain(st: &mut egui::Style) {
    menu_style(st);
    st.spacing.menu_spacing = 0.0;
}

/// The row style of a menu that has checks (`true`) or none (`false`).
pub fn menu_style_for(checks: bool) -> fn(&mut egui::Style) {
    if checks { menu_style } else { menu_style_plain }
}

/// Whether the menu `ui` is in reserves the check gutter (it has a checkable row).
pub fn row_has_gutter(ui: &Ui) -> bool {
    ui.style().spacing.menu_spacing != 0.0
}

/// A context menu on `response` (it opens at the pointer on a secondary click) in the chrome's
/// popup with the row style ([`menu_style_for`]); its rows are [`MenuRow`]s and `checks` says
/// whether any of them can carry a check (the gutter is reserved only then). Its submenus decide
/// for themselves ([`submenu`]). Chain `.id(..)` for a menu that shares a response with another popup.
pub fn context_menu(response: &Response, checks: bool) -> egui::Popup<'static> {
    egui::Popup::context_menu(response).style(menu_style_for(checks))
}

/// A popup menu that a click on `response` opens and closes, flush under it, in the chrome's
/// popup with the row style ([`menu_style_for`]); its rows are [`MenuRow`]s and `checks` is as for
/// [`context_menu`].
pub fn popup_menu(response: &Response, checks: bool) -> egui::Popup<'static> {
    egui::Popup::menu(response).style(menu_style_for(checks))
}

/// A menu-bar popup's gap under the bar's bottom line (the design's `chrome.popup.menuBarGap`, the
/// value of `chrome.comboPopupGap`; a local constant until the token mirror lands).
pub const POPUP_MENU_BAR_GAP: f32 = 4.0;

/// A menu-bar menu: the item's button and its popup, whose top edge sits [`POPUP_MENU_BAR_GAP`]
/// below the bar's bottom line, its left edge on the item's (the item is centered in the bar, so the gap is measured from the
/// item's bottom edge plus the bar's padding). `checks` is as for [`context_menu`].
pub fn bar_menu<R>(ui: &mut Ui, text: &str, checks: bool, content: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    let response = ui.add(egui::Button::new(text));
    let config = submenu_config(ui, checks);
    let below_item = (c::MENU_BAR_HEIGHT - c::MENU_BAR_ITEM_HEIGHT) / 2.0;
    let inner = egui::Popup::menu(&response)
        .close_behavior(config.close_behavior)
        .style(config.style.clone())
        .info(egui::UiStackInfo::new(egui::UiKind::Menu).with_tag_value(egui::containers::menu::MenuConfig::MENU_CONFIG_TAG, config))
        .gap(below_item + POPUP_MENU_BAR_GAP)
        .show(content);
    egui::InnerResponse::new(inner.map(|i| i.inner), response)
}

/// A submenu button in the menu `ui` and its popup, which decides for itself whether it has
/// `checks` (a parent with checks does not force a gutter on a checkless submenu, nor the reverse).
/// The button's own text lines up with its siblings' (the parent's gutter state).
pub fn submenu<R>(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    checks: bool,
    content: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let config = submenu_config(ui, checks);
    let (response, inner) = egui::containers::menu::SubMenuButton::new(submenu_atoms(ui, text)).config(config).ui(ui, content);
    egui::InnerResponse::new(inner.map(|i| i.inner), response)
}

/// The config of a menu or submenu with or without `checks`, keeping the enclosing menu's (or the
/// bar's) close behavior.
pub fn submenu_config(ui: &Ui, checks: bool) -> egui::containers::menu::MenuConfig {
    egui::containers::menu::MenuConfig::new()
        .close_behavior(egui::containers::menu::MenuConfig::find(ui).close_behavior)
        .style(menu_style_for(checks))
}

/// The atom id of a menu row's check gutter (its rect is where the check is painted).
pub const MENU_CHECK_ATOM: &str = "menu_check";

/// The atom that stands where the gutter would in a menu without one: no width, so the row's gap
/// after it puts the text 12 inside the popup's edge.
const MENU_INSET_ATOM: &str = "menu_inset";

/// A menu row as the chrome's Menus artboard draws it (#386; README "Menus and popups", Row):
/// 4 px inset, the 16 px check gutter (only in a menu that has a checkable row, [`row_has_gutter`]),
/// the 12 px check centered in it when checked, 4 px gaps, the text, then the shortcut
/// right-aligned with an 8 px inset. The height, the corners and the hover fill come from the
/// menu's style ([`menu_style_for`]), so in the menu bar's menus, the context menus
/// ([`context_menu`]) and the popups ([`popup_menu`]) the text starts 28 pt inside the popup's
/// edge (12 without the gutter) and the shortcut ends 12 pt inside it. A row built with
/// [`MenuRow::checked`] in a menu declared without checks is a bug (debug assertion).
#[must_use]
pub struct MenuRow {
    text: egui::WidgetText,
    shortcut: Option<String>,
    right: Option<egui::WidgetText>,
    checked: Option<bool>,
}

impl MenuRow {
    pub fn new(text: impl Into<egui::WidgetText>) -> Self {
        Self { text: text.into(), shortcut: None, right: None, checked: None }
    }

    /// The row's keyboard shortcut, in the shortcut role ([`shortcut`]).
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    /// Other right-aligned text in the shortcut's place (Open Recent's folder).
    pub fn right_text(mut self, text: impl Into<egui::WidgetText>) -> Self {
        self.right = Some(text.into());
        self
    }

    /// The row can carry a check: show the mark in the gutter when `on`. The menu must have been
    /// declared with checks.
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = Some(on);
        self
    }
}

impl egui::Widget for MenuRow {
    fn ui(self, ui: &mut Ui) -> Response {
        let right: Option<egui::WidgetText> = match (self.shortcut, self.right) {
            (Some(keys), _) => Some(shortcut(ui.ctx(), keys).into()),
            (None, other) => other,
        };
        let has_gutter = row_has_gutter(ui);
        debug_assert!(has_gutter || self.checked.is_none(), "a checkable row ({:?}) in a menu declared without checks", self.text.text());
        let gutter = egui::Id::new(MENU_CHECK_ATOM);
        let atoms = (gutter_atom(has_gutter), self.text);
        // The right inset is the style's left one, a gap and this spacer.
        let end = egui::Atom::custom(egui::Id::new("menu_row_end"), vec2(c::ROW_PADDING[1] - c::ROW_PADDING[0] - c::ROW_GAP, 0.0));
        let b = egui::Button::new(atoms).gap(c::ROW_GAP);
        let b = match right {
            Some(r) => b.shortcut_text((r, end)),
            None => b.shortcut_text(end),
        };
        let resp = b.atom_ui(ui);
        if self.checked == Some(true)
            && let Some(r) = resp.rect(gutter)
        {
            crate::icons::paint(ui.painter(), r.center(), crate::icons::Icon::Check, c::CHECK_GLYPH, TOKENS.text);
        }
        resp.response
    }
}

/// The first atom of a row: the check gutter, or the empty inset in a menu without one.
fn gutter_atom(has_gutter: bool) -> egui::Atom<'static> {
    if has_gutter {
        egui::Atom::custom(egui::Id::new(MENU_CHECK_ATOM), vec2(c::CHECK_GUTTER, c::CHECK_GLYPH))
    } else {
        egui::Atom::custom(egui::Id::new(MENU_INSET_ATOM), vec2(0.0, c::CHECK_GLYPH))
    }
}

/// A line of text in a menu (Audio's device name) set where the rows' text starts.
pub fn menu_note(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    let gutter = if row_has_gutter(ui) { c::CHECK_GUTTER } else { 0.0 };
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().button_padding.x + gutter + c::ROW_GAP);
        ui.label(text)
    })
    .inner
}

/// The atoms of a submenu's button: the gutter (empty) when the parent menu has one, then the
/// text, so its text lines up with the rows' ([`MenuRow`]).
pub fn submenu_atoms(ui: &Ui, text: impl Into<egui::WidgetText>) -> (egui::Atom<'static>, egui::WidgetText) {
    (gutter_atom(row_has_gutter(ui)), text.into())
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
