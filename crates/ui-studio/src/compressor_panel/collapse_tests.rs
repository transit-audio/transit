//! Collapsing a device window to its title bar (326-V): a double click on an empty part of the
//! bar, the bar's controls while collapsed, per-instance memory, follow selection, Window >
//! Collapse All Editors and Expand All Editors, and the flag never being saved.

use super::test_rig::{SLOT, T, mid};
use crate::tokens::layout::TITLE_BAR_HEIGHT;
use serde_json::json;
use transit_model::TrackId;

fn height(w: [f32; 4]) -> f32 {
    w[3] - w[1]
}

/// An empty part of the bar: left of the preset browser, right of the device title.
fn grip(t: &T, tr: TrackId) -> (f32, f32) {
    let title = t.panel(tr).title;
    assert!(title.prev[0] - title.device_rect[2] > 8.0, "room between the device title and the browser");
    ((title.device_rect[2] + title.prev[0]) / 2.0, mid(title.bar).1)
}

/// The height of a collapsed window: the bar and the frame's 1 px edge a side.
fn collapsed_height() -> f32 {
    TITLE_BAR_HEIGHT + crate::device_window::edge().y
}

/// Two Compressors on two tracks, both open (the second cascaded off the first).
fn two() -> (T, TrackId, TrackId) {
    let mut t = T::new();
    let a = t.compressor_track();
    t.open_card(0);
    let b = t.compressor_track();
    t.open_card(0);
    assert_eq!(t.app.ui.plugin_windows.len(), 2);
    (t, a, b)
}

#[test]
fn a_double_click_on_the_bar_collapses_the_window_and_a_second_expands_it() {
    let (mut t, tr) = T::open();
    let open = t.panel(tr).window;
    assert!(height(open) > collapsed_height() + 100.0 && !t.panel(tr).panels.is_empty());
    let at = grip(&t, tr);
    t.click(at, 2);
    let p = t.panel(tr);
    assert_eq!(height(p.window), collapsed_height(), "the bar and the edge");
    assert_eq!((p.window[0], p.window[1], p.window[2]), (open[0], open[1], open[2]), "anchored at the top left, same width");
    assert!(p.title.collapsed);
    assert!(p.panels.is_empty() && p.knobs.is_empty() && p.detector_bar.is_none() && p.load.is_empty(), "no body, no footer");
    // The bar's controls respond: the bypass click toggles while collapsed.
    let b = p.title;
    assert!(b.bypass_available && b.bypass_on);
    t.click(mid(b.bypass), 1);
    assert!(t.app.builtin_at((tr, SLOT)).unwrap().bypass, "bypass works collapsed");
    assert_eq!(height(t.panel(tr).window), collapsed_height(), "and the click did not expand");
    // The second double click expands to the content height again.
    let at = grip(&t, tr);
    t.click(at, 2);
    let p = t.panel(tr);
    assert_eq!(p.window, open, "back to the content-sized rect");
    assert!(!p.title.collapsed && !p.panels.is_empty());
}

#[test]
fn a_single_click_does_not_collapse_and_a_triple_click_counts_as_the_toggle() {
    let (mut t, tr) = T::open();
    let open = t.panel(tr).window;
    let at = grip(&t, tr);
    t.click(at, 1);
    assert_eq!(t.panel(tr).window, open, "negative control: one click is not the gesture");
    t.click(at, 3);
    assert_eq!(height(t.panel(tr).window), collapsed_height(), "a triple click is the toggle (the kit's reset rule)");
}

#[test]
fn a_double_click_on_a_control_does_what_the_control_does() {
    let (mut t, tr) = T::open();
    let open = t.panel(tr).window;
    let field = t.panel(tr).title.field;
    t.click(mid(field), 2);
    assert_eq!(t.panel(tr).window, open, "the preset field did not collapse the window");
    // The meter's group is no empty bar either (a fresh window: the field's list may be open).
    let (mut t, tr) = T::open();
    let meter = t.panel(tr).title.meter;
    t.click(mid(meter), 2);
    assert_eq!(t.panel(tr).window, open, "the meter did not collapse the window");
    // Negative control: the empty part of the same bar does.
    let at = grip(&t, tr);
    t.click(at, 2);
    assert_eq!(height(t.panel(tr).window), collapsed_height());
}

#[test]
fn a_collapsed_window_keeps_dragging_by_its_bar_and_escape() {
    let (mut t, tr) = T::open();
    let at = grip(&t, tr);
    t.click(at, 2);
    let w = t.panel(tr).window;
    t.drag(at, (at.0 + 40.0, at.1 + 30.0), false);
    let moved = t.panel(tr).window;
    assert_eq!((moved[0] - w[0], moved[1] - w[1], height(moved)), (40.0, 30.0, collapsed_height()), "the bar still moves it");
    t.click((moved[0] + 300.0, moved[1] + 20.0), 1);
    t.key("Escape");
    assert!(t.app.ui.plugin_windows.is_empty(), "Escape closes a collapsed window");
}

#[test]
fn the_flag_is_per_instance_and_survives_follow_selection() {
    let (mut t, a, b) = two();
    let (open_a, open_b) = (t.panel(a).window, t.panel(b).window);
    // A's bar is under B's window below its top strip (B cascades 24 px down).
    let at = (grip(&t, a).0, t.panel(a).title.bar[1] + 8.0);
    t.click(at, 2);
    assert_eq!(height(t.panel(a).window), collapsed_height());
    assert_eq!(t.panel(b).window, open_b, "negative control: the other instance stays expanded");
    assert_ne!(open_a, t.panel(a).window);
    // Follow selection moves a collapsed window as it would an expanded one, and leaves it collapsed.
    let c = t.compressor_track();
    t.app.focus(a);
    t.frames(3);
    let link = t.panel(a).title.follow;
    t.click(mid(link), 1);
    let before = t.panel(a).window;
    t.app.focus(c);
    t.frames(3);
    assert!(t.app.ui.plugin_windows.contains(&(c, SLOT)) && !t.app.ui.plugin_windows.contains(&(a, SLOT)), "the window followed to the new track");
    let after = t.panel(c).window;
    assert_eq!((after[0], after[1], height(after)), (before[0], before[1], collapsed_height()), "same place, still collapsed");
    assert_eq!(t.panel(b).window, open_b);
}

#[test]
fn collapse_all_and_expand_all_act_on_every_open_editor() {
    let (mut t, a, b) = two();
    let (open_a, open_b) = (t.panel(a).window, t.panel(b).window);
    let menu = t.app.arrange.bar_menu.iter().find(|m| m.label == "Window").cloned().unwrap();
    t.click(mid(menu.rect), 1);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Collapse All Editors").cloned().expect("Window > Collapse All Editors");
    assert_eq!(item.value, "window.collapse_editors");
    t.click(mid(item.rect), 1);
    assert_eq!((height(t.panel(a).window), height(t.panel(b).window)), (collapsed_height(), collapsed_height()));
    t.click(mid(menu.rect), 1);
    let item = t.app.arrange.bar_menu.iter().find(|m| m.label == "Expand All Editors").cloned().expect("Window > Expand All Editors");
    assert_eq!(item.value, "window.expand_editors");
    t.click(mid(item.rect), 1);
    assert_eq!((t.panel(a).window, t.panel(b).window), (open_a, open_b), "both back");
    // The commands answer agents too; negative control: expanding what is expanded changes nothing.
    assert_eq!(t.app.run("window.expand_editors", json!({})).unwrap()["expanded"], 2);
    t.frames(3);
    assert_eq!(t.panel(a).window, open_a);
    assert_eq!(t.app.run("window.collapse_editors", json!({})).unwrap()["collapsed"], 2);
    t.frames(3);
    assert_eq!(height(t.panel(b).window), collapsed_height());
}

#[test]
fn the_flag_is_never_saved_and_a_reopened_editor_starts_expanded() {
    let (mut t, tr) = T::open();
    let saved = serde_json::to_value(&t.app.ui).unwrap();
    let at = grip(&t, tr);
    t.click(at, 2);
    assert!(t.panel(tr).title.collapsed);
    assert_eq!(serde_json::to_value(&t.app.ui).unwrap(), saved, "collapsing changes nothing ui.json would hold");
    // Closing writes the window's position (the file's only window entry) and nothing else about it.
    let w = t.panel(tr).window;
    let close = t.panel(tr).title.close;
    t.click(mid(close), 1);
    assert!(t.app.ui.plugin_windows.is_empty());
    let mut after = serde_json::to_value(&t.app.ui).unwrap();
    assert_eq!(after["device_windows"][super::COMPRESSOR], json!([w[0], w[1]]), "only the top left");
    // Nothing else of the file changed: the flag is not in it (negative control: the position is).
    let mut before = saved;
    for v in [&mut before, &mut after] {
        // The two fields closing the window changes: where it was, and that it is open.
        v.as_object_mut().unwrap().remove("device_windows");
        v.as_object_mut().unwrap().remove("plugin_windows");
    }
    assert_eq!(before, after, "no collapsed flag in what the window writes");
    t.open_card(0);
    assert!(!t.panel(tr).title.collapsed && height(t.panel(tr).window) > collapsed_height(), "a reopened editor starts expanded");
}
