//! The Compressor's floating window: the shared device window rule, every parameter reachable,
//! the content-sized window re-fitting when the Detector section opens or closes, the remembered
//! position, Escape, Window > Close All Editors, follow selection, and both token sets.

use super::test_rig::{SLOT, T, info, mid};
use super::window::window_size;
use egui::{Rect, pos2};
use serde_json::json;

fn size(w: [f32; 4]) -> (f32, f32) {
    (w[2] - w[0], w[3] - w[1])
}

fn screen(t: &T) -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, t.screen)
}

#[test]
fn the_window_is_the_device_window_at_the_boards_width_with_the_shared_bar_and_a_footer() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let w = p.window;
    // The A bar is the window's top, inside its 1 px edge: no egui title bar above it.
    assert_eq!((p.title.bar[0], p.title.bar[1], p.title.bar[2]), (w[0] + 1.0, w[1] + 1.0, w[2] - 1.0));
    assert_eq!(p.title.device, "Compressor/Limiter");
    assert_eq!(size(w).0, super::DESIGN_WIDTH, "the board's width on a screen wider than it");
    // The footer: the load readout, no credit (the Compressor is transit's own).
    assert_eq!(p.footer[3], w[3] - 1.0);
    assert_eq!(p.footer[3] - p.footer[1], super::FOOTER_H);
    assert!(p.load.starts_with("Audio load "));
    assert!(t.window(tr).credit.is_empty());
    // The frame: the device window's.
    let f = crate::theme::device_panel_frame();
    assert_eq!(f.fill, crate::theme::TOKENS.bg_window);
    // Negative control: a gain insert still opens the generic window, without this panel.
    let mut g = T::new();
    let gt = g.compressor_track();
    g.app.add_device(gt, "gain").unwrap();
    g.frames(4);
    g.open_card(1);
    let gw = g.app.builtin_editor.windows.iter().find(|w| w.key == (gt, 1)).cloned().unwrap();
    assert!(gw.compressor.is_none() && gw.macro_osc.is_none());
}

#[test]
fn every_parameter_is_reachable_from_the_panel_once_the_detector_section_is_open() {
    let (mut t, tr) = T::open();
    let rows = |t: &T| {
        let mut v: Vec<String> = t.window(tr).rows.iter().map(|r| r.param.clone()).collect();
        v.sort();
        v
    };
    // At rest the Detector section is collapsed: its three parameters are not drawn.
    let closed = rows(&t);
    for id in ["lookahead", "sc_hpf_on", "sc_hpf_freq"] {
        assert!(!closed.contains(&id.to_string()), "negative control: {id} is behind the section bar");
    }
    t.expand(tr);
    let mut want: Vec<String> = info().params.iter().map(|p| p.id.to_string()).collect();
    want.sort();
    assert_eq!(rows(&t), want);
}

#[test]
fn the_detector_section_opens_while_in_use_and_the_window_refits_to_it() {
    let (mut t, tr) = T::open();
    t.screen = egui::vec2(1600.0, 1400.0);
    t.frames(3);
    let before = t.panel(tr).window;
    let button = t.panel(tr).detector_bar.unwrap().button;
    t.click(mid(button), 1);
    let p = t.panel(tr);
    let body = p.detector_body.expect("the Detector section opened");
    let grown = p.window;
    assert_eq!((grown[0], grown[1]), (before[0], before[1]), "anchored at the top left");
    assert_eq!(grown[3] - grown[1], before[3] - before[1] + crate::tokens::layout::PANEL_GAP + (body[3] - body[1]), "the body and its gap");
    // The window holds its content exactly: the footer sits right under the body.
    assert_eq!(p.window[3] - 1.0 - super::FOOTER_H, body[3] + crate::tokens::layout::WINDOW_PADDING, "no scroll, no gap");
    assert_ne!(grown, before, "negative control: the window did change");
    t.click(mid(button), 1);
    assert!(t.panel(tr).detector_body.is_none());
    assert_eq!(t.panel(tr).window, before, "collapsed: back to the same rect");
    assert_eq!(t.app.ui.device_sections.get("compressor/detector"), Some(&true), "the user's choice is kept");
    // A device whose lookahead is in use opens with the section open.
    let mut u = T::new();
    let ut = u.compressor_track();
    u.set(ut, "lookahead", 2.0);
    u.open_card(0);
    assert!(u.panel(ut).detector_body.is_some(), "in use: open before the user chooses");
}

#[test]
fn the_window_is_content_sized_and_its_position_round_trips_through_ui_json() {
    let mut t = T::new();
    let old: crate::UiState = serde_json::from_value(json!({"device_windows": {super::COMPRESSOR: [20.0, 80.0, 1120.0, 700.0]}})).unwrap();
    t.app.ui.device_windows = old.device_windows;
    let tr = t.compressor_track();
    t.open_card(0);
    let w = t.panel(tr).window;
    assert_eq!((w[0], w[1]), (20.0, 80.0), "the device opens where it was last closed");
    let content = super::content_height(&t.app, &t.ctx, (tr, SLOT), window_size(screen(&t), 0.0).x - crate::device_window::edge().x);
    let want = window_size(screen(&t), content);
    assert_eq!(size(w), (want.x, want.y), "content-sized");
    assert_ne!(size(w), (1100.0, 620.0), "negative control: not the old entry's size");
    let bar = t.panel(tr).title.bar;
    let grip = (bar[0] + 200.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 + 40.0, grip.1 + 30.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0], moved[1]), (60.0, 110.0), "the bar moves the window");
    let close = t.panel(tr).title.close;
    t.click(mid(close), 1);
    assert!(t.app.ui.plugin_windows.is_empty());
    assert_eq!(t.app.ui.device_windows.get(super::COMPRESSOR), Some(&[60.0, 110.0]), "closing writes the position");
    t.open_card(0);
    assert_eq!(t.panel(tr).window, moved);
}

#[test]
fn dragging_the_window_edge_does_not_resize_it() {
    let (mut t, tr) = T::open();
    let w = t.panel(tr).window;
    for (x, y) in [(w[2] - 2.0, w[3] - 2.0), (w[2] - 1.0, (w[1] + w[3]) / 2.0), ((w[0] + w[2]) / 2.0, w[3] - 1.0)] {
        t.drag((x, y), (x + 120.0, y + 90.0), false);
        assert_eq!(t.panel(tr).window, w, "no resize handle at ({x}, {y})");
    }
    // Negative control: the same drag on the bar moves the window.
    let bar = t.panel(tr).title.bar;
    let grip = (bar[0] + 200.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 + 20.0, grip.1 + 10.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0] - w[0], moved[1] - w[1], size(moved)), (20.0, 10.0, size(w)));
}

#[test]
fn escape_closes_the_focused_editor_and_a_typed_value_keeps_it() {
    let (mut t, tr) = T::open();
    assert_eq!(t.app.builtin_editor.devices.focused, Some(t.window(tr).uid), "a window that opens takes the focus");
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.is_empty(), "Escape closed it");
    // Negative control: a click outside every editor hands the keyboard back.
    t.open_card(0);
    t.click((10.0, 450.0), 1);
    assert_eq!(t.app.builtin_editor.devices.focused, None);
    t.key("Escape");
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, SLOT)], "Escape elsewhere leaves the editor open");
    let bar = t.panel(tr).title.bar;
    t.click((bar[0] + 200.0, (bar[1] + bar[3]) / 2.0), 1);
    assert_eq!(t.app.builtin_editor.devices.focused, Some(t.window(tr).uid));
    let value = t.panel(tr).knob("ratio").unwrap().knob.value;
    t.click(mid(value), 1);
    assert!(t.panel(tr).knob("ratio").unwrap().knob.editing);
    t.key("Escape");
    assert!(!t.panel(tr).knob("ratio").unwrap().knob.editing, "the field took Escape");
    assert_eq!(t.app.ui.plugin_windows.len(), 1, "and the window stayed");
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.is_empty());
}

#[test]
fn close_all_editors_includes_the_compressor() {
    let mut t = T::new();
    let tr = t.compressor_track();
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    t.open_card(0);
    t.open_card(1);
    assert_eq!(t.app.ui.plugin_windows.len(), 2);
    assert!(t.app.builtin_editor.windows.iter().any(|w| w.compressor.is_some()), "the Compressor's panel is one of them");
    let menu = t.app.arrange.bar_menu.iter().find(|m| m.label == "Window").cloned().unwrap();
    t.click(mid(menu.rect), 1);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Close All Editors").cloned().expect("Window > Close All Editors");
    t.click(mid(item.rect), 1);
    assert!(t.app.ui.plugin_windows.is_empty() && t.app.builtin_editor.windows.is_empty(), "both closed");
    // Negative control: the editor button still opens it (only the command closed it).
    t.open_card(0);
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, SLOT)]);
    // The device menu offers the same.
    let m = t.panel(tr).title.menu;
    t.click(mid(m), 1);
    let at = t.painted_all("Close All Editors").into_iter().next().expect("the device menu lists it");
    t.click(at, 1);
    assert!(t.app.ui.plugin_windows.is_empty());
}

#[test]
fn follow_selection_moves_to_the_focused_tracks_compressor_and_never_to_another_device() {
    let mut t = T::new();
    let a = t.compressor_track();
    let b = t.compressor_track();
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let g = transit_model::TrackId(r["tracks"][0].as_u64().unwrap());
    t.app.add_device(g, "gain").unwrap();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    // Negative control: with follow off, focusing another track keeps the window on its insert.
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(a, SLOT)]);
    let link = t.panel(a).title.follow;
    t.click(mid(link), 1);
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(b, SLOT)], "following: the focused track's Compressor");
    assert!(t.panel(b).title.follow_on);
    // A track whose slot holds another device: the window stays.
    t.app.focus(g);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(b, SLOT)], "a gain is not the same device");
}

#[test]
fn both_token_sets_draw_the_panel() {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        let (mut t, tr) = T::open();
        crate::theme::apply_pinned(&t.ctx, theme);
        t.frames(3);
        let tokens = if theme == egui::Theme::Light { &crate::theme::LIGHT } else { &crate::theme::DARK };
        let p = t.panel(tr);
        let k = &p.knob("threshold").unwrap().knob;
        assert_eq!(k.arc, tokens.signal, "{theme:?}: the value arc");
        assert_eq!(k.cap, tokens.knob_cap, "{theme:?}: the cap");
        assert_eq!(p.hero.gr_meter.as_ref().unwrap().fill_color, tokens.modulation, "{theme:?}: gain reduction is `mod`");
        assert_eq!(p.mode.as_ref().unwrap().selected_fill, tokens.surface_selected);
    }
    assert_ne!(crate::theme::DARK.signal, crate::theme::LIGHT.signal, "negative control: the sets differ");
}

#[test]
fn the_window_never_leaves_the_screen() {
    let mut t = T::new();
    t.screen = egui::vec2(1400.0, 600.0);
    t.app.ui.device_windows.insert(super::COMPRESSOR.into(), [20.0, 500.0]);
    let tr = t.compressor_track();
    t.open_card(0);
    let w = t.panel(tr).window;
    assert!(screen(&t).contains_rect(Rect::from_min_max(pos2(w[0], w[1]), pos2(w[2], w[3]))), "{w:?}");
    assert!(w[1] < 500.0, "negative control: moved up from where it was remembered: {w:?}");
    let want = crate::device_window::anchored(pos2(20.0, 500.0), egui::vec2(size(w).0, size(w).1), screen(&t));
    assert_eq!((w[0], w[1]), (want.x, want.y));
}

#[test]
fn the_shared_title_bar_browses_the_compressors_presets_and_bypasses_it() {
    let (mut t, tr) = T::open();
    t.app.services.config_dir = None;
    let factory = crate::presets::factory_for(super::COMPRESSOR);
    assert!(factory.len() >= 2);
    let preset = |t: &T| t.app.builtin_at((tr, SLOT)).unwrap().preset;
    let before = t.undo_len();
    let next = t.panel(tr).title.next;
    t.click(mid(next), 1);
    assert_eq!(preset(&t), factory[0].0, "from no preset, next is the first");
    assert_eq!(t.undo_len(), before + 1, "a recall is one undo step");
    assert_eq!((t.panel(tr).title.preset_group.as_str(), t.panel(tr).title.preset_name.clone()), ("Factory", factory[0].0.to_string()));
    let ratio = factory[0].1.params.get("ratio").copied().unwrap_or(info().param("ratio").unwrap().default);
    assert_eq!(t.param(tr, "ratio"), ratio, "the preset's values applied");
    let prev = t.panel(tr).title.prev;
    t.click(mid(prev), 1);
    assert_eq!(preset(&t), factory.last().unwrap().0, "previous from the first wraps to the last");
    // Negative control: a click on the field opens the list and recalls nothing.
    let n = t.undo_len();
    let field = t.panel(tr).title.field;
    t.click(mid(field), 1);
    assert_eq!(t.undo_len(), n);
    assert!(!t.painted_all("Save As\u{2026}").is_empty(), "the list opens with its Save As field");
    t.key("Escape");
    // An insert has a bypass: the glyph is live and toggles the insert slot's bypass.
    let b = t.panel(tr).title;
    assert!(b.bypass_available && b.bypass_on);
    t.click(mid(b.bypass), 1);
    assert!(t.app.builtin_at((tr, SLOT)).unwrap().bypass);
    assert!(!t.panel(tr).title.bypass_on);
}
