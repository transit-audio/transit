//! The floating window: the device window rule, every parameter reachable, the remembered rect,
//! Escape, Window > Close All Editors, follow selection, and both token sets.

use super::test_rig::{T, info, mid};
use super::window::{on_screen, opening_rect};
use egui::{Rect, pos2, vec2};
use serde_json::json;

const INST: usize = transit_mix::INSTRUMENT_SLOT;

#[test]
fn the_window_is_the_device_window_with_its_own_bar_and_footer() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let w = p.window;
    // The A bar is the window's top, inside its 1 px edge: no egui title bar above it.
    assert_eq!((p.title.bar[0], p.title.bar[1], p.title.bar[2]), (w[0] + 1.0, w[1] + 1.0, w[2] - 1.0));
    assert_eq!(p.credit, super::CREDIT);
    assert!(p.credit.contains("Émilie Gillet") && p.credit.contains("(MIT)"), "ADR 0033's credit");
    assert!(p.credit_rect[3] <= w[3] && p.credit_rect[1] >= w[3] - super::FOOTER_H, "the credit is in the footer");
    assert!(p.load.starts_with("Audio load "));
    assert!(w[2] - w[0] >= super::MIN_WIDTH && w[2] <= 1400.0 && w[3] <= 900.0, "{w:?}");
    // The frame: bg.window ground, line.strong edge, the panel radius, no shadow, no margin.
    let f = crate::theme::device_panel_frame();
    assert_eq!(f.fill, crate::theme::TOKENS.bg_window);
    assert_eq!(f.stroke, egui::Stroke::new(1.0, crate::theme::TOKENS.line_strong));
    assert_eq!(f.corner_radius, egui::CornerRadius::same(8));
    assert_eq!(f.shadow, egui::epaint::Shadow::NONE);
    assert_eq!(f.inner_margin, egui::Margin::ZERO);
}

#[test]
fn every_parameter_is_reachable_from_the_panel() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    for _ in 0..16 {
        let add = t.in_view(tr, |p| p.add_route.unwrap());
        t.click(mid(add), 1);
    }
    let mut got: Vec<String> = t.window(tr).rows.iter().map(|r| r.param.clone()).collect();
    for n in [1, 2] {
        let id = format!("lfo{n}_sync");
        let sync = t.in_view(tr, |p| p.toggles.iter().find(|x| x.param == id).unwrap().toggle.rect);
        t.click(mid(sync), 1);
    }
    got.extend(t.window(tr).rows.iter().map(|r| r.param.clone()));
    got.sort();
    got.dedup();
    let mut want: Vec<String> = info().params.iter().map(|p| p.id.to_string()).collect();
    want.sort();
    assert_eq!(got, want);
    // Every control has a help entry (the ids are the older panel's, moved with their controls).
    for p in info().params {
        assert!(super::help(p.id).is_some(), "{}", p.id);
    }
    assert!(super::help("threshold").is_none(), "negative control: not a Macro Oscillator parameter");
}

#[test]
fn the_remembered_rect_round_trips_through_ui_json_and_opens_on_screen() {
    let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0));
    let mut t = T::new();
    t.app.ui.device_windows.insert(super::MACRO_OSC.into(), [100.0, 80.0, 1200.0, 700.0]);
    let json = serde_json::to_string(&t.app.ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.device_windows.get(super::MACRO_OSC), Some(&[100.0, 80.0, 1200.0, 700.0]));
    // An older ui.json without the field reads as empty.
    let old: crate::UiState = serde_json::from_value(json!({})).unwrap();
    assert!(old.device_windows.is_empty() && old.device_sections.is_empty() && old.editor_follow.is_empty());
    let tr = t.instrument_track();
    t.open_card(0);
    assert_eq!(t.panel(tr).window, [100.0, 80.0, 1200.0, 700.0], "the device opens where it was last closed");
    // Moved by its bar, closed, opened again: where it was left.
    let bar = t.panel(tr).title.bar;
    let grip = (bar[0] + 300.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 + 40.0, grip.1 + 30.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0], moved[1]), (140.0, 110.0), "the bar moves the window");
    let close = t.panel(tr).title.close;
    t.click(mid(close), 1);
    assert!(t.app.ui.plugin_windows.is_empty());
    t.open_card(0);
    assert_eq!(t.panel(tr).window, moved);
    // Negative control: a rect off the screen comes back onto it.
    let r = opening_rect(Some([1300.0, 850.0, 2400.0, 1600.0]), screen, 900.0, 0);
    assert!(screen.contains_rect(r), "{r:?}");
    assert_eq!(on_screen(Rect::from_min_size(pos2(10.0, 10.0), vec2(500.0, 400.0)), screen).min, pos2(10.0, 10.0), "a rect on screen stays");
}

#[test]
fn escape_closes_the_focused_editor_only() {
    let (mut t, tr) = T::open();
    assert_eq!(t.app.builtin_editor.macro_osc.focused, Some(t.window(tr).uid), "a window that opens takes the focus");
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.is_empty(), "Escape closed it");
    // Negative control: a click outside every editor hands the keyboard back.
    t.open_card(0);
    t.click((10.0, 450.0), 1);
    assert_eq!(t.app.builtin_editor.macro_osc.focused, None);
    t.key("Escape");
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, INST)], "Escape elsewhere leaves the editor open");
    // A click in the window focuses it again; a value being typed keeps Escape for itself.
    let bar = t.panel(tr).title.bar;
    t.click((bar[0] + 300.0, (bar[1] + bar[3]) / 2.0), 1);
    assert_eq!(t.app.builtin_editor.macro_osc.focused, Some(t.window(tr).uid));
    let value = t.panel(tr).knob("decay").unwrap().knob.value;
    t.click(mid(value), 1);
    assert!(t.panel(tr).knob("decay").unwrap().knob.editing);
    t.key("Escape");
    assert!(!t.panel(tr).knob("decay").unwrap().knob.editing, "the field took Escape");
    assert_eq!(t.app.ui.plugin_windows.len(), 1, "and the window stayed");
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.is_empty());
}

#[test]
fn close_all_editors_closes_every_editor_from_the_window_menu() {
    let mut t = T::new();
    let tr = t.instrument_track();
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    t.open_card(0);
    t.open_card(1);
    assert_eq!(t.app.ui.plugin_windows.len(), 2);
    let menu = t.app.arrange.bar_menu.iter().find(|m| m.label == "Window").cloned().unwrap();
    t.click(mid(menu.rect), 1);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Close All Editors").cloned().expect("Window > Close All Editors");
    assert_eq!(item.value, "window.close_editors");
    t.click(mid(item.rect), 1);
    assert!(t.app.ui.plugin_windows.is_empty() && t.app.builtin_editor.windows.is_empty(), "both closed");
    // The command answers agents too, and the device menu offers the same.
    t.open_card(1);
    assert_eq!(t.app.run("window.close_editors", json!({})).unwrap()["closed"], 1);
    assert!(t.app.ui.plugin_windows.is_empty());
}

#[test]
fn follow_selection_switches_the_shown_device_to_the_focused_track() {
    let mut t = T::new();
    let a = t.instrument_track();
    let b = t.instrument_track();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    assert_eq!(t.app.ui.plugin_windows, vec![(a, INST)]);
    // Negative control: with follow off, focusing another track keeps the window on its insert.
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(a, INST)]);
    let link = t.panel(a).title.follow;
    t.click(mid(link), 1);
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(b, INST)], "following: the focused track's Macro Oscillator");
    assert!(t.panel(b).title.follow_on, "still following");
    assert_eq!(t.panel(b).title.track, t.app.engine.session().track(b).unwrap().name);
    t.app.focus(a);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(a, INST)]);
}

#[test]
fn both_token_sets_draw_the_panel() {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let (mut t, tr) = T::open();
        crate::theme::apply_pinned(&t.ctx, theme);
        t.frames(3);
        let tokens = if theme == egui::Theme::Light { &crate::theme::LIGHT } else { &crate::theme::DARK };
        let p = t.panel(tr);
        let k = &p.knob("harmonics").unwrap().knob;
        assert_eq!(k.arc, tokens.signal, "{theme:?}: the value arc");
        assert_eq!(k.cap, tokens.knob_cap, "{theme:?}: the cap");
        assert_eq!(p.title.follow_fill, tokens.surface_raised, "{theme:?}: the off toggle");
        let bar = p.shaper_bar.unwrap();
        assert_eq!(bar.tag_border, tokens.line_strong);
        let well = &p.wells.iter().find(|w| w.name == "scope").unwrap().well;
        assert_eq!(well.curve_color, tokens.signal);
    }
    // Negative control: the two sets differ where the panel reads them.
    assert_ne!(crate::theme::DARK.signal, crate::theme::LIGHT.signal);
    assert_ne!(crate::theme::DARK.surface_raised, crate::theme::LIGHT.surface_raised);
}
