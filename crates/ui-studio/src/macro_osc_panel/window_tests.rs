//! The floating window: the device window rule, every parameter reachable, the content-sized
//! window (326-X), the remembered position, Escape, Window > Close All Editors, follow selection,
//! and both token sets. The shared policy's own tests are in `crate::device_window`.

use super::test_rig::{T, info, mid};
use super::window::window_size;
use egui::{Rect, pos2, vec2};
use serde_json::json;

fn size(w: [f32; 4]) -> (f32, f32) {
    (w[2] - w[0], w[3] - w[1])
}

fn screen(t: &T) -> Rect {
    Rect::from_min_size(egui::Pos2::ZERO, t.screen)
}

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
fn the_remembered_position_round_trips_through_ui_json_and_an_old_rect_opens_content_sized_there() {
    let mut t = T::new();
    // An entry written before 326-X is a rect: its position is read, its size read past.
    let old: crate::UiState = serde_json::from_value(json!({"device_windows": {super::MACRO_OSC: [20.0, 80.0, 1120.0, 700.0]}})).unwrap();
    assert_eq!(old.device_windows.get(super::MACRO_OSC), Some(&[20.0, 80.0]));
    t.app.ui.device_windows = old.device_windows;
    let json = serde_json::to_string(&t.app.ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.device_windows.get(super::MACRO_OSC), Some(&[20.0, 80.0]), "the new shape: position only");
    // An older ui.json without the field reads as empty.
    let none: crate::UiState = serde_json::from_value(json!({})).unwrap();
    assert!(none.device_windows.is_empty() && none.device_sections.is_empty() && none.editor_follow.is_empty());
    let tr = t.instrument_track();
    t.open_card(0);
    let w = t.panel(tr).window;
    assert_eq!((w[0], w[1]), (20.0, 80.0), "the device opens where it was last closed");
    let content = super::content_height(&t.app, &t.ctx, (tr, INST), window_size(screen(&t), 0.0).x - crate::device_window::edge().x);
    let want = window_size(screen(&t), content);
    assert_eq!(size(w), (want.x, want.y), "content-sized");
    assert_ne!(size(w), (1100.0, 620.0), "negative control: not the old entry's size");
    // Moved by its bar, closed, opened again: where it was left.
    let bar = t.panel(tr).title.bar;
    let grip = (bar[0] + 300.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 + 40.0, grip.1 + 30.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0], moved[1]), (60.0, 110.0), "the bar moves the window");
    let close = t.panel(tr).title.close;
    t.click(mid(close), 1);
    assert!(t.app.ui.plugin_windows.is_empty());
    assert_eq!(t.app.ui.device_windows.get(super::MACRO_OSC), Some(&[60.0, 110.0]), "closing writes the position");
    t.open_card(0);
    assert_eq!(t.panel(tr).window, moved);
}

#[test]
fn opening_a_section_grows_the_window_by_exactly_the_body_and_closing_it_shrinks_it_back() {
    let (mut t, tr) = T::open();
    t.screen = vec2(1600.0, 1600.0);
    t.frames(3);
    let before = t.panel(tr).window;
    let button = t.panel(tr).shaper_bar.unwrap().button;
    t.click(mid(button), 1);
    let p = t.panel(tr);
    let body = p.shaper_body.expect("the Shaper opened");
    let grown = p.window;
    assert_eq!((grown[0], grown[1]), (before[0], before[1]), "anchored at the top left");
    assert_eq!(grown[3] - grown[1], before[3] - before[1] + crate::tokens::layout::PANEL_GAP + (body[3] - body[1]), "the body's height and its gap");
    assert_ne!(grown, before, "negative control: the window did change");
    t.click(mid(button), 1);
    assert!(t.panel(tr).shaper_body.is_none());
    assert_eq!(t.panel(tr).window, before, "collapsed: back to the same rect");
    // The window holds its content exactly: the footer sits right under the body's last panel.
    let button = t.panel(tr).modulation_bar.unwrap().button;
    t.click(mid(button), 1);
    let p = t.panel(tr);
    let m = p.modulation_body.expect("the Modulation opened");
    assert_eq!(p.window[3] - 1.0 - super::FOOTER_H, m[3] + crate::tokens::layout::WINDOW_PADDING, "no scroll, no gap");
}

#[test]
fn the_window_never_leaves_the_screen_and_the_body_scrolls_when_it_cannot_fit() {
    let (mut t, tr) = T::open();
    t.screen = vec2(1400.0, 700.0);
    t.frames(3);
    t.expand(tr);
    for _ in 0..6 {
        let add = t.in_view(tr, |p| p.add_route.unwrap());
        t.click(mid(add), 1);
    }
    let w = t.panel(tr).window;
    assert!(screen(&t).contains_rect(Rect::from_min_max(pos2(w[0], w[1]), pos2(w[2], w[3]))), "{w:?}");
    assert_eq!(size(w).1, 700.0, "as tall as the screen");
    let content = super::content_height(&t.app, &t.ctx, (tr, INST), size(w).0 - crate::device_window::edge().x);
    assert!(content + crate::tokens::layout::TITLE_BAR_HEIGHT + super::FOOTER_H > 700.0, "negative control: the content is taller than the screen");
    // The bottom is reachable: the body scrolls.
    let add = t.in_view(tr, |p| p.add_route.unwrap());
    assert!(add[3] <= w[3] - super::FOOTER_H, "{add:?} {w:?}");
}

#[test]
fn dragging_the_window_edge_does_not_resize_it() {
    let (mut t, tr) = T::open();
    let w = t.panel(tr).window;
    for (x, y) in [(w[2] - 2.0, w[3] - 2.0), (w[2] - 1.0, (w[1] + w[3]) / 2.0), ((w[0] + w[2]) / 2.0, w[3] - 1.0)] {
        t.drag((x, y), (x + 120.0, y + 90.0), false);
        assert_eq!(t.panel(tr).window, w, "no resize handle at ({x}, {y})");
    }
    // Negative control: the same drag on the bar moves the window (the gesture arrives).
    let bar = t.panel(tr).title.bar;
    let grip = (bar[0] + 300.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 + 20.0, grip.1 + 10.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0] - w[0], moved[1] - w[1], size(moved)), (20.0, 10.0, size(w)));
}

#[test]
fn two_instances_keep_their_own_rects_and_the_second_opens_cascaded() {
    let mut t = T::new();
    let a = t.instrument_track();
    let b = t.instrument_track();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    let ra = t.panel(a).window;
    t.app.focus(b);
    t.frames(3);
    t.open_card(0);
    let rb = t.panel(b).window;
    assert_eq!((rb[0] - ra[0], rb[1] - ra[1]), (24.0, 24.0), "B cascades off A instead of covering it: {ra:?} {rb:?}");
    // Drag B by its bar, close it, open it again: B's own place; A never moves.
    let bar = t.panel(b).title.bar;
    let grip = (bar[0] + 300.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 - 40.0, grip.1 + 30.0), false);
    let moved = t.panel(b).window;
    assert_eq!((moved[0], moved[1]), (rb[0] - 40.0, rb[1] + 30.0));
    let close = t.panel(b).title.close;
    t.click(mid(close), 1);
    assert_eq!(t.app.ui.plugin_windows, vec![(a, INST)]);
    assert_eq!(t.app.ui.device_windows.get(super::MACRO_OSC), Some(&[moved[0], moved[1]]), "the closed window left its position as the device's");
    // A closes after it: the device's rect is now A's, and B still reopens at its own.
    let close_a = t.panel(a).title.close;
    t.click(mid(close_a), 1);
    assert_eq!(t.app.ui.device_windows.get(super::MACRO_OSC), Some(&[ra[0], ra[1]]));
    t.open_card(0);
    assert_eq!(t.panel(b).window, moved, "B reopens where B was left, not at the device's rect");
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    assert_eq!(t.panel(a).window, ra, "A unchanged");
}

#[test]
fn after_a_restart_an_instance_opens_at_the_device_rect_never_at_another_instances() {
    // The first launch: A is left at one place, B at another; A closes first, so the device's
    // rect (the one ui.json keeps) is B's.
    let mut t = T::new();
    let a = t.instrument_track();
    let b = t.instrument_track();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    let ra = t.panel(a).window;
    t.app.focus(b);
    t.frames(3);
    t.open_card(0);
    let rb = t.panel(b).window;
    // (B overlaps A's close button, so A closes from its card.)
    t.app.close_editor((a, INST));
    t.frames(2);
    t.click(mid(t.panel(b).title.close), 1);
    assert!(t.app.ui.plugin_windows.is_empty());
    let mut saved = serde_json::to_value(&t.app.ui).unwrap();
    // The next launch numbers its inserts from 1 again: its first instance may get A's old uid.
    // Model that by filing A's rect under the new instance's uid, as a uid-keyed ui.json would.
    let mut next = T::new();
    let x = next.instrument_track();
    let uid_x = next.app.builtin_at((x, INST)).unwrap().uid.get();
    saved["editor_rects"] = json!({ uid_x.to_string(): ra });
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    next.app.ui = restored;
    next.app.toggle_editor((x, INST));
    next.frames(3);
    let rx = next.panel(x).window;
    assert_eq!(rx, rb, "the device's rect from ui.json, not the instance that had this uid before");
    assert_ne!(rx, ra);
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
fn a_followed_window_leaves_the_escape_focus_with_the_click_that_changed_the_track() {
    let mut t = T::new();
    let a = t.instrument_track();
    let b = t.instrument_track();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    let link = t.panel(a).title.follow;
    t.click(mid(link), 1);
    // A click outside the editor (where a track header sits), then the focused track changes.
    t.click((10.0, 450.0), 1);
    assert_eq!(t.app.builtin_editor.macro_osc.focused, None);
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(b, INST)], "the window followed");
    assert_eq!(t.app.builtin_editor.macro_osc.focused, None, "and did not take the Escape focus");
    t.key("Escape");
    assert_eq!(t.app.ui.plugin_windows, vec![(b, INST)], "Escape went to the views, not the editor");
}

#[test]
fn a_following_window_stays_put_and_leaves_its_rect_to_the_shown_instance_when_follow_ends() {
    let mut t = T::new();
    let a = t.instrument_track();
    let b = t.instrument_track();
    t.app.focus(a);
    t.frames(3);
    t.open_card(0);
    let ra = t.panel(a).window;
    // B has a place of its own: opened (cascaded off A), dragged down, closed.
    t.app.focus(b);
    t.frames(3);
    t.open_card(0);
    let bar = t.panel(b).title.bar;
    let grip = (bar[0] + 300.0, (bar[1] + bar[3]) / 2.0);
    t.drag(grip, (grip.0 - 40.0, grip.1 + 60.0), false);
    let rb = t.panel(b).window;
    t.app.close_editor((b, INST));
    t.frames(2);
    let uid_b = t.app.builtin_at((b, INST)).unwrap().uid.get();
    assert_eq!(t.app.builtin_editor.macro_osc.rects.get(&uid_b), Some(&pos2(rb[0], rb[1])));
    // A follows; focusing B swaps the content and the window stays where A's was.
    t.app.focus(a);
    t.frames(3);
    let link = t.panel(a).title.follow;
    t.click(mid(link), 1);
    t.app.focus(b);
    t.frames(3);
    assert_eq!(t.app.ui.plugin_windows, vec![(b, INST)]);
    assert_eq!(t.panel(b).window, ra, "the window stays put, not B's own rect");
    assert_eq!(t.app.builtin_editor.macro_osc.rects.get(&uid_b), Some(&pos2(rb[0], rb[1])), "following writes nothing to B's position");
    // Follow off: where the window is becomes B's place.
    let link = t.panel(b).title.follow;
    t.click(mid(link), 1);
    assert_eq!(t.app.builtin_editor.macro_osc.rects.get(&uid_b), Some(&pos2(ra[0], ra[1])));
    t.app.close_editor((b, INST));
    t.frames(2);
    t.app.toggle_editor((b, INST));
    t.frames(3);
    assert_eq!(t.panel(b).window, ra, "B now opens where follow left it");
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
