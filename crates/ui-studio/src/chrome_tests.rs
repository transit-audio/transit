//! The application chrome (#326): egui's own widgets by the chrome rules. Deterministic: styles,
//! rects and tokens, no pixels, no wall clock (the tooltip delay is a constant the test reads).

use crate::chrome::{self, Ask, Choice};
use crate::theme::{DARK, LIGHT, Tokens, contrast};
use crate::tokens::chrome as c;
use crate::widgets::test_support::{Rig, mid};
use egui::{Stroke, pos2};

fn style_of(theme: egui::Theme) -> std::sync::Arc<egui::Style> {
    let ctx = egui::Context::default();
    crate::theme::apply_pinned(&ctx, theme);
    let st = ctx.style_of(theme);
    crate::theme::set_current(egui::Theme::Dark);
    st
}

fn tokens(theme: egui::Theme) -> &'static Tokens {
    if theme == egui::Theme::Light { &LIGHT } else { &DARK }
}

const THEMES: [egui::Theme; 2] = [egui::Theme::Dark, egui::Theme::Light];

#[test]
fn buttons_are_borderless_raised_and_press_without_a_signal_edge() {
    for theme in THEMES {
        let (st, t) = (style_of(theme), tokens(theme));
        let w = &st.visuals.widgets;
        assert_eq!((w.inactive.weak_bg_fill, w.inactive.bg_stroke), (t.surface_raised, Stroke::NONE), "{theme:?}: rest");
        assert_eq!((w.hovered.weak_bg_fill, w.hovered.bg_stroke), (t.button_hover, Stroke::NONE), "{theme:?}: hover");
        assert_eq!((w.active.weak_bg_fill, w.active.bg_stroke), (t.surface_selected, Stroke::NONE), "{theme:?}: pressed, no signal edge");
        assert_eq!(w.open.weak_bg_fill, t.button_hover, "{theme:?}: an open menu bar item");
        // 12 px a side; egui keeps its 18 px interact height (it also sets every row's height).
        assert_eq!(st.spacing.button_padding, egui::vec2(c::BUTTON_PADDING[1], crate::theme::BUTTON_PAD_Y));
        assert_eq!(st.spacing.interact_size.y, egui::Spacing::default().interact_size.y);
        assert_eq!(w.inactive.corner_radius, egui::CornerRadius::same(c::RADIUS_CONTROL as u8));
        // Negative control: the field scope is what draws a border, so a button never looks like
        // a field or an off toggle.
        let mut v = st.visuals.clone();
        chrome::field_visuals(&mut v, t);
        assert_ne!(v.widgets.inactive.bg_stroke, w.inactive.bg_stroke, "{theme:?}: the field scope differs from a button");
    }
}

#[test]
fn a_button_is_26_high_with_12_a_side() {
    let mut rig = Rig::new();
    let (rect, text_w) = rig.idle(|ui| {
        // egui 0.36 sets a button's label in the Body style.
        let w = ui.painter().layout_no_wrap("Export…".into(), egui::TextStyle::Body.resolve(ui.style()), DARK.text).size().x;
        (chrome::button(ui, "Export…").0.rect, w)
    });
    assert_eq!(rect.height(), c::CONTROL_HEIGHT);
    assert!((rect.width() - (text_w + 2.0 * c::BUTTON_PADDING[1])).abs() < 0.01, "{} vs {}", rect.width(), text_w);
    // Negative control: a small button is not held to the control height.
    let small = rig.idle(|ui| ui.small_button("Export…").rect);
    assert!(small.height() < c::CONTROL_HEIGHT);
}

#[test]
fn fields_take_the_inset_well_with_a_line_and_buttons_beside_them_do_not() {
    let mut rig = Rig::new();
    let mut text = String::from("Lead vox");
    let (inside, outside, rect) = rig.idle(|ui| {
        let inside = chrome::field(ui, |ui| ui.visuals().widgets.clone());
        let rect = chrome::text_field(ui, egui::TextEdit::singleline(&mut text)).rect;
        (inside, ui.visuals().widgets.clone(), rect)
    });
    assert_eq!(inside.inactive.bg_stroke, Stroke::new(1.0, DARK.line), "1 px line");
    assert_eq!(inside.hovered.bg_stroke, Stroke::new(1.0, DARK.line_strong), "line.strong on hover");
    assert_eq!(inside.inactive.weak_bg_fill, DARK.surface_inset, "the combo box face is the inset well");
    assert_eq!(rect.height(), c::CONTROL_HEIGHT, "26 high");
    // Negative control: the scope ends with the field.
    assert_eq!(outside.inactive.bg_stroke, Stroke::NONE);
    assert_eq!(outside.inactive.weak_bg_fill, DARK.surface_raised);
    let st = style_of(egui::Theme::Dark);
    assert_eq!(st.visuals.text_edit_bg_color, Some(DARK.surface_inset));
    assert_eq!(st.visuals.selection.stroke, Stroke::new(c::FOCUS_RING_WIDTH, DARK.signal), "focus 1.5 signal");
}

#[test]
fn checkboxes_are_14_px_in_an_18_px_hit_and_signal_when_on() {
    let t = &DARK;
    let base = style_of(egui::Theme::Dark);
    let (mut off, mut on, mut radio) = ((*base).clone(), (*base).clone(), (*base).clone());
    chrome::check_visuals(&mut off, t, false, false);
    chrome::check_visuals(&mut on, t, true, false);
    chrome::check_visuals(&mut radio, t, true, true);
    for st in [&off, &on, &radio] {
        assert_eq!((st.spacing.icon_width, st.spacing.interact_size.y), (c::CHECKBOX_BOX, c::CHECKBOX_HIT));
        assert_eq!(st.visuals.override_text_color, Some(t.text), "the label stays text");
        assert_eq!(st.visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(c::RADIUS_CHECKBOX as u8));
    }
    assert_eq!((off.visuals.widgets.inactive.bg_fill, off.visuals.widgets.inactive.bg_stroke), (t.surface_inset, Stroke::new(1.0, t.line_strong)));
    assert_eq!((on.visuals.widgets.inactive.bg_fill, on.visuals.widgets.inactive.bg_stroke), (t.signal, Stroke::NONE));
    assert_eq!(on.visuals.widgets.inactive.fg_stroke.color, t.surface_inset, "the mark");
    // egui draws the radio dot a third of the inner width in radius: 6 px across.
    assert_eq!(radio.spacing.icon_width_inner * 2.0 / 3.0, c::RADIO_DOT);
    // Negative control: off and on differ in the fill.
    assert_ne!(off.visuals.widgets.inactive.bg_fill, on.visuals.widgets.inactive.bg_fill);
    let mut rig = Rig::new();
    let mut checked = true;
    let rect = rig.idle(|ui| chrome::checkbox(ui, &mut checked, "Grid over clips").rect);
    assert_eq!(rect.height(), c::CHECKBOX_HIT, "the hit is 18 high");
    let clicked = rig.click(mid(crate::widgets::r4(rect)), |ui| chrome::checkbox(ui, &mut checked, "Grid over clips").clicked());
    assert!(clicked && !checked, "it still toggles");
}

#[test]
fn the_default_button_is_signal_with_inset_ink_and_16_a_side() {
    let mut rig = Rig::new();
    let ((_, d), (_, s), text_w) = rig.idle(|ui| {
        let w = ui.painter().layout_no_wrap("Save".into(), c::DEFAULT_BUTTON_FONT.font_id(ui.ctx()), DARK.text).size().x;
        ui.horizontal(|ui| (chrome::default_button(ui, "Save"), chrome::button(ui, "Cancel"), w)).inner
    });
    assert!(d.default && !s.default);
    assert_eq!((d.fill, d.ink), (DARK.signal, DARK.surface_inset));
    assert_eq!(d.rect[3] - d.rect[1], c::CONTROL_HEIGHT);
    assert_eq!(d.rect[2] - d.rect[0], (text_w + 2.0 * c::DEFAULT_BUTTON_PADDING[1]).ceil());
    // Negative control: the standard button is the raised face.
    assert_eq!(s.fill, DARK.surface_raised);
    // Hover steps the fill.
    let center = pos2((d.rect[0] + d.rect[2]) / 2.0, (d.rect[1] + d.rect[3]) / 2.0);
    rig.frame(vec![egui::Event::PointerMoved(center)], |ui| ui.horizontal(|ui| chrome::default_button(ui, "Save")).inner);
    let (_, hovered) = rig.frame(vec![egui::Event::PointerMoved(center)], |ui| ui.horizontal(|ui| chrome::default_button(ui, "Save")).inner);
    assert_eq!(hovered.fill, DARK.default_button_hover);
    // Disabled: the unavailable fill and ink.arrow.
    let (_, off) = rig.idle(|ui| ui.add_enabled_ui(false, |ui| chrome::default_button(ui, "Save")).inner);
    assert_eq!((off.fill, off.ink), (DARK.toggle_unavailable_fill, DARK.ink_arrow));
    // The default ink reads on its fill.
    assert!(contrast(DARK.surface_inset, DARK.signal) >= 4.5);
}

#[test]
fn a_modal_is_380_wide_on_the_panel_over_the_overlay_with_the_default_rightmost() {
    let mut rig = Rig::new();
    let id = egui::Id::new("test_modal");
    let ask = Ask {
        question: "Save changes to “Night drive”?".into(),
        detail: Some("Your changes are lost if you don't save them.".into()),
        default: "Save",
        cancel: "Cancel",
        third: Some("Don't Save"),
    };
    for _ in 0..3 {
        rig.idle(|ui| chrome::modal(ui.ctx(), id, &ask));
    }
    let l = chrome::modal_layout(&rig.ctx, id).expect("drawn");
    assert_eq!(l.rect[2] - l.rect[0], c::MODAL_WIDTH, "380 wide");
    assert_eq!((l.fill, l.edge, l.scrim), (DARK.surface_panel, DARK.line_strong, DARK.overlay));
    let texts: Vec<&str> = l.buttons.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Don't Save", "Cancel", "Save"], "third, Cancel, default left to right");
    let [third, cancel, default] = [&l.buttons[0], &l.buttons[1], &l.buttons[2]];
    assert!(default.default && !cancel.default && !third.default, "one default");
    let inner_right = l.rect[2] - c::MODAL_PADDING[1] - 1.0;
    assert!((default.rect[2] - inner_right).abs() < 0.5, "the default is rightmost: {} vs {inner_right}", default.rect[2]);
    assert!(cancel.rect[2] <= default.rect[0], "Cancel sits left of the default");
    let inner_left = l.rect[0] + c::MODAL_PADDING[1] + 1.0;
    assert!((third.rect[0] - inner_left).abs() < 0.5, "the third choice is alone at the far left");
    assert!(third.rect[2] < cancel.rect[0] - 20.0, "and apart from Cancel");
    assert!((l.question[1] - (l.rect[1] + c::MODAL_PADDING[0] + 1.0)).abs() < 0.5, "the question opens the dialog: no title bar");
    // Negative control: without a third choice there are two buttons.
    let two = Ask { third: None, ..ask };
    let id2 = egui::Id::new("test_modal_two");
    rig.idle(|ui| chrome::modal(ui.ctx(), id2, &two));
    assert_eq!(chrome::modal_layout(&rig.ctx, id2).unwrap().buttons.len(), 2);
    // A click on the default reports it (once the modal is the top one again).
    let three = || Ask { third: Some("Don't Save"), ..two_ask() };
    for _ in 0..3 {
        rig.idle(|ui| chrome::modal(ui.ctx(), id, &three()));
    }
    let now = chrome::modal_layout(&rig.ctx, id).unwrap();
    let d = &now.buttons[2];
    let center = pos2((d.rect[0] + d.rect[2]) / 2.0, (d.rect[1] + d.rect[3]) / 2.0);
    let got = rig.click(center, |ui| chrome::modal(ui.ctx(), id, &three()));
    assert_eq!(got, Some(Choice::Default));
}

fn two_ask() -> Ask<'static> {
    Ask {
        question: "Save changes to “Night drive”?".into(),
        detail: Some("Your changes are lost if you don't save them.".into()),
        default: "Save",
        cancel: "Cancel",
        third: None,
    }
}

#[test]
fn popups_are_raised_with_a_strong_edge_and_nothing_casts_a_shadow() {
    for theme in THEMES {
        let (st, t) = (style_of(theme), tokens(theme));
        let v = &st.visuals;
        assert_eq!((v.window_fill, v.window_stroke), (t.surface_raised, Stroke::new(1.0, t.line_strong)), "{theme:?}");
        assert_eq!((v.window_shadow, v.popup_shadow), (egui::Shadow::NONE, egui::Shadow::NONE));
        assert_eq!(v.menu_corner_radius, egui::CornerRadius::same(c::RADIUS_POPUP as u8));
        assert_eq!(v.window_corner_radius, egui::CornerRadius::same(c::RADIUS_WINDOW as u8));
        assert_eq!(st.spacing.menu_margin, egui::Margin::same(c::POPUP_PADDING as i8));
        assert!(!v.window_highlight_topmost, "every title bar stays bg.titlebar");
        assert_eq!(v.weak_text_color, Some(t.text_faint), "shortcuts and placeholders");
        assert_eq!(v.hyperlink_color, t.signal);
        assert_eq!(v.selection.bg_fill, t.toggle_option_fill, "{theme:?}: a selected widget takes the option toggle fill (#326)");
        assert_eq!(st.interaction.tooltip_delay, c::TOOLTIP_DELAY_MS as f32 / 1000.0, "500 ms");
    }
}

#[test]
fn scroll_bars_float_thin_at_rest_and_widen_on_hover() {
    for theme in THEMES {
        let (st, t) = (style_of(theme), tokens(theme));
        let s = &st.spacing.scroll;
        assert!(s.floating && !s.foreground_color);
        assert_eq!((s.floating_width, s.bar_width, s.handle_min_length), (c::SCROLL_REST, c::SCROLL_HOVER, c::SCROLL_MIN_HANDLE));
        assert_eq!((s.bar_inner_margin, s.bar_outer_margin), (c::SCROLL_INSET, c::SCROLL_INSET));
        assert_eq!(
            (s.dormant_handle_opacity, s.dormant_background_opacity, s.interact_background_opacity),
            (1.0, 0.0, 1.0),
            "a handle at rest, a track on hover"
        );
        // egui paints the handle from the states' bg_fill and the track from extreme_bg_color.
        let w = &st.visuals.widgets;
        assert_eq!((w.inactive.bg_fill, w.hovered.bg_fill, w.active.bg_fill), (t.ink_ghost, t.ink_arrow, t.text_faint), "{theme:?}");
        assert_eq!(st.visuals.extreme_bg_color, t.surface_panel);
        // Negative control: the handle colors are not the button faces.
        assert_ne!(w.inactive.bg_fill, w.inactive.weak_bg_fill);
    }
}

#[test]
fn menu_rows_are_24_and_menu_bar_items_22_with_row_corners() {
    let base = style_of(egui::Theme::Dark);
    let (mut rows, mut bar) = ((*base).clone(), (*base).clone());
    chrome::menu_style(&mut rows);
    chrome::menu_bar_style(&mut bar);
    assert_eq!(rows.spacing.interact_size.y, c::ROW_HEIGHT);
    assert_eq!(bar.spacing.interact_size.y, c::MENU_BAR_ITEM_HEIGHT);
    assert_eq!(bar.spacing.button_padding.x, c::MENU_BAR_ITEM_PADDING[1]);
    for st in [&rows, &bar] {
        assert_eq!(st.visuals.widgets.hovered.corner_radius, egui::CornerRadius::same(c::RADIUS_ROW as u8));
        assert_eq!(st.visuals.widgets.inactive.weak_bg_fill, egui::Color32::TRANSPARENT, "rows are flat until hovered");
        assert_eq!(st.visuals.widgets.hovered.bg_stroke, Stroke::NONE);
    }
    // Negative control: the rows are not the 26 px control height.
    assert_ne!(rows.spacing.interact_size.y, base.spacing.interact_size.y);
    let shortcut = chrome::shortcut(&egui::Context::default(), "Ctrl+D");
    assert_eq!(shortcut.text(), "Ctrl+D");
}

#[test]
fn the_focus_ring_surrounds_a_keyboard_focused_button_and_never_a_text_field() {
    let mut rig = Rig::new();
    let mut text = String::new();
    let draw = |ui: &mut egui::Ui, text: &mut String| (ui.button("Export…"), ui.add(egui::TextEdit::singleline(text)));
    let (button, field) = rig.idle(|ui| draw(ui, &mut text));
    assert_eq!(rig.idle(|ui| chrome::focus_ring(ui.ctx())), None, "nothing focused, no ring");
    rig.ctx.memory_mut(|m| m.request_focus(button.id));
    let ring = rig.idle(|ui| {
        draw(ui, &mut text);
        chrome::focus_ring(ui.ctx())
    });
    let want = button.rect.expand(c::FOCUS_RING_OFFSET + c::FOCUS_RING_WIDTH / 2.0);
    assert_eq!(ring, Some(want), "1.5 px, 2 px outside the button");
    // Negative control: a focused text field shows focus on its own edge, no ring.
    rig.ctx.memory_mut(|m| m.request_focus(field.id));
    let ring = rig.idle(|ui| {
        draw(ui, &mut text);
        chrome::focus_ring(ui.ctx())
    });
    assert_eq!(ring, None);
}

/// egui dims a disabled widget by one alpha; at [`crate::theme::DISABLED_ALPHA`] `text` over
/// `surface.raised` reads as `ink.arrow` (nearer it than the other inks).
#[test]
fn disabled_text_lands_on_ink_arrow() {
    let blend = |a: f32| {
        let t = &DARK;
        let mix = |x: u8, y: u8| (f32::from(x) * a + f32::from(y) * (1.0 - a)).round() as u8;
        egui::Color32::from_rgb(mix(t.text.r(), t.surface_raised.r()), mix(t.text.g(), t.surface_raised.g()), mix(t.text.b(), t.surface_raised.b()))
    };
    let dist = |x: egui::Color32, y: egui::Color32| {
        let d = |p: u8, q: u8| (f32::from(p) - f32::from(q)).powi(2);
        d(x.r(), y.r()) + d(x.g(), y.g()) + d(x.b(), y.b())
    };
    let nearest_is_arrow = |a: f32| {
        let c = blend(a);
        let t = &DARK;
        dist(c, t.ink_arrow) < dist(c, t.text_faint) && dist(c, t.ink_arrow) < dist(c, t.text_muted) && dist(c, t.ink_arrow) < dist(c, t.ink_ghost)
    };
    assert!(nearest_is_arrow(crate::theme::DISABLED_ALPHA));
    assert_eq!(style_of(egui::Theme::Dark).visuals.disabled_alpha, crate::theme::DISABLED_ALPHA);
    // Negative control: egui's default (0.5) reads as text.faint, not ink.arrow.
    assert!(!nearest_is_arrow(0.5));
}

/// The ink of every textured mesh (the icons) and every text in `shapes` inside `area`.
fn inks(shapes: &[egui::epaint::ClippedShape], area: egui::Rect) -> (Vec<egui::Color32>, Vec<egui::Color32>) {
    let (mut icons, mut texts) = (Vec::new(), Vec::new());
    for c in shapes {
        match &c.shape {
            egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() && area.contains(m.vertices[0].pos) => icons.push(m.vertices[0].color),
            egui::Shape::Text(t) if area.contains(t.pos) => texts.extend(t.galley.job.sections.iter().map(|s| s.format.color)),
            _ => {}
        }
    }
    (icons, texts)
}

#[test]
fn a_warning_is_text_and_a_triangle_and_an_error_adds_meter_hot() {
    use chrome::Severity;
    let mut rig = Rig::new();
    for _ in 0..2 {
        rig.idle(|ui| {
            chrome::status_text(
                ui.painter(),
                pos2(40.0, 40.0),
                "Buffers under 64 samples may drop out",
                egui::FontId::proportional(12.0),
                Severity::Warning,
            )
        });
    }
    let (warn_rect, shapes) = rig.shapes(|ui| {
        chrome::status_text(
            ui.painter(),
            pos2(40.0, 40.0),
            "Buffers under 64 samples may drop out",
            egui::FontId::proportional(12.0),
            Severity::Warning,
        )
    });
    let (icons, texts) = inks(&shapes, warn_rect.expand(1.0));
    assert_eq!((icons.as_slice(), texts.as_slice()), ([DARK.text].as_slice(), [DARK.text].as_slice()), "a warning: no color");
    let (err_rect, shapes) = rig
        .shapes(|ui| chrome::status_text(ui.painter(), pos2(40.0, 140.0), "The device stopped", egui::FontId::proportional(12.0), Severity::Error));
    let (icons, texts) = inks(&shapes, err_rect.expand(1.0));
    assert_eq!(icons, [DARK.meter_hot], "an error's triangle is meter.hot");
    assert_eq!(texts, [DARK.text], "its message stays text");
    // The glyphs are the artboard's: triangle-alert for a warning, circle-alert for an error.
    use crate::icons::Icon;
    assert_eq!((Severity::Warning.icon(), Severity::Error.icon()), (Icon::Warning, Icon::Error));
    assert_eq!((Icon::Warning.name(), Icon::Error.name()), ("triangle-alert", "circle-alert"));
    assert_ne!(Severity::Warning.icon(), Severity::Error.icon(), "negative control: two shapes, not one");
    // Negative control: the warning glyph is not the error's.
    assert_ne!(Severity::Warning.glyph(), Severity::Error.glyph());
    assert_ne!(Severity::Warning.glyph(), DARK.modulation, "no mod amber on a warning");
    assert_eq!(style_of(egui::Theme::Dark).visuals.warn_fg_color, DARK.text, "egui's own warnings follow");
}

/// The fills of every rect a frame painted.
fn rect_fills(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(s: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match s {
            egui::Shape::Rect(r) => out.push(r.fill),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

#[test]
fn a_selected_selectable_label_reads_as_an_option_toggle_not_a_solid_signal_button() {
    for theme in THEMES {
        let t = tokens(theme);
        let mut rig = Rig::themed(theme);
        let (resp, shapes) = rig.shapes(|ui| ui.selectable_label(true, "Notes"));
        let fills = rect_fills(&shapes);
        assert!(fills.contains(&t.toggle_option_fill), "{theme:?}: option fill, got {fills:?}");
        assert!(!fills.iter().any(|f| *f == t.signal), "{theme:?}: never a solid signal fill");
        let st = style_of(theme);
        assert_eq!(st.visuals.selection.stroke.color, t.signal, "{theme:?}: the text and stroke are signal");
        assert!(resp.rect.width() > 0.0);
        // Negative control: with the mapping removed (egui's selection fill as it was) the option fill is gone.
        let mut rig = Rig::themed(theme);
        let alpha = if theme == egui::Theme::Light { c::SELECTION_ALPHA_LIGHT } else { c::SELECTION_ALPHA_DARK };
        let (_, old) = rig.shapes(|ui| {
            ui.visuals_mut().selection.bg_fill = t.signal.gamma_multiply(alpha);
            ui.selectable_label(true, "Notes")
        });
        assert!(!rect_fills(&old).contains(&t.toggle_option_fill), "{theme:?}: negative control");
    }
}
