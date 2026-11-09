//! The check gutter is per menu, and a menu-bar menu opens below the bar (#418). A menu, submenu,
//! context menu or popup reserves the 16 px gutter only when one of its rows can carry a check
//! (the menu builder declares it, so the first row paints right); without it the text starts 12
//! inside the popup's edge. The menu-bar popup's top edge sits `chrome.comboPopupGap` below the bar.
use crate::context_menu_tests::{Popup, T, find, rect};
use crate::menu_padding_tests::{EDGE, TEXT_IN, TEXT_IN_PLAIN, near, popup_at, texts};
use crate::tokens::chrome as c;
use egui::Rect;
use serde_json::json;

fn bar_item(t: &T, name: &str) -> [f32; 4] {
    t.app.arrange.bar_menu.iter().find(|m| m.label == name).unwrap_or_else(|| panic!("no menu {name:?}")).rect
}

fn open(t: &mut T, menu: &str) {
    let r = bar_item(t, menu);
    t.click(r, "primary");
}

#[test]
fn the_audio_menu_has_no_checks_so_every_row_and_the_note_start_12_inside_the_edge() {
    let mut t = T::new();
    open(&mut t, "Audio");
    let devices = find(&t.app.arrange.bar_menu.clone(), "Audio Devices…");
    let p = Popup::of(&t, &devices);
    p.insets_plain("Audio Devices…", None);
    for row in ["Reconnect Device", "Scan Plug-ins", "No audio device (--no-audio)"] {
        assert!(near(p.text_in(row), TEXT_IN_PLAIN), "{row} starts {} inside the edge, want 12", p.text_in(row));
    }
    // Negative control in the same popup: the old gutter would put the text at 28.
    assert!(!near(p.text_in("Audio Devices…"), TEXT_IN));
    p.row(&devices);
    p.row(&find(&t.app.arrange.bar_menu.clone(), "Reconnect Device"));
    // The File menu (a submenu, Open Recent, and rows with no check) is plain too.
    open(&mut t, "File");
    let bounce = find(&t.app.arrange.bar_menu.clone(), "Bounce…");
    let p = Popup::of(&t, &bounce);
    p.insets_plain("Bounce…", Some(&crate::shortcuts::label(crate::shortcuts::Action::Bounce)));
    for row in ["New", "Save", "Open Recent", "Quit"] {
        assert!(near(p.text_in(row), TEXT_IN_PLAIN), "{row} starts at 12");
    }
}

#[test]
fn a_menu_with_checks_keeps_the_gutter_on_every_row_and_a_submenu_decides_for_itself() {
    let mut t = T::new();
    t.app.ui.browser.show = true;
    t.frames(3);
    open(&mut t, "View");
    let items = t.app.arrange.bar_menu.clone();
    let grid = find(&items, "Grid Over Clips");
    let p = Popup::of(&t, &grid);
    // A plain row, a check row and the submenu buttons all start at 28.
    p.insets("Grid Over Clips", None);
    for row in ["Browser", "Track Height", "Appearance"] {
        assert!(near(p.text_in(row), TEXT_IN), "{row} starts 28 inside the edge");
    }
    assert_eq!(p.checks_in(&find(&items, "Browser")), 1, "Browser is on: its check sits centered in the gutter");
    assert_eq!(p.checks_in(&grid), 0, "an unchecked row's gutter stays empty");
    // Track Height has no checkable row: its rows start at 12 under a parent that has a gutter.
    let height = find(&items, "Track Height");
    t.input("ui.move", json!({"x": (height.rect[0] + height.rect[2]) / 2.0, "y": (height.rect[1] + height.rect[3]) / 2.0}));
    t.frames(10);
    let items = t.app.arrange.bar_menu.clone();
    let inc = find(&items, "Increase All");
    let sub = Popup::of(&t, &inc);
    assert!(!sub.frame.contains(rect(height.rect).center()), "the rows are in the submenu's own popup");
    sub.insets_plain("Increase All", None);
    sub.row(&inc);
    // Appearance has checks of its own: its rows start at 28.
    let appearance = find(&items, "Appearance");
    t.input("ui.move", json!({"x": (appearance.rect[0] + appearance.rect[2]) / 2.0, "y": (appearance.rect[1] + appearance.rect[3]) / 2.0}));
    t.frames(10);
    let items = t.app.arrange.bar_menu.clone();
    let dark = find(&items, "Dark");
    let sub = Popup::of(&t, &dark);
    sub.insets("Dark", None);
    assert_eq!(sub.checks_in(&dark), 1, "the current appearance is checked");
}

#[test]
fn a_context_menu_with_checks_keeps_the_gutter_and_one_without_does_not() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    t.app.ui.show_mixer = true;
    t.frames(3);
    // With checks: the pan knob's MIDI Learn menu (its rows carry the takeover mode's check).
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == gtr.0).cloned().unwrap();
    t.click(strip.knob, "secondary");
    let menu = t.app.midi_learn.menu.clone();
    let learn = find(&menu, "MIDI Learn");
    let p = Popup::of(&t, &learn);
    p.insets("MIDI Learn", None);
    p.row(&learn);
    assert_eq!(p.checks_in(&learn), 0, "an unchecked row's gutter stays empty");
    // Without: the track header's menu (the context test measures its rows at 12 too).
    t.app.ui.show_mixer = false;
    t.frames(3);
    let h = t.header(gtr);
    t.click([h[0] + 50.0, h[1], h[0] + 70.0, h[3]], "secondary");
    let freeze = find(&t.app.arrange.menu.clone(), "Freeze");
    Popup::of(&t, &freeze).insets_plain("Freeze", None);
}

/// A scalar of where the popup's `text` row is each frame after the click, and the popup's frame.
fn first_frames(t: &mut T, menu: &str, text: &str) -> Vec<(Rect, Rect)> {
    let r = bar_item(t, menu);
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0, "button": "primary"}));
    let _ = crate::control::handle_for_test(&mut t.app, &t.ctx, &req);
    let mut seen = Vec::new();
    for _ in 0..8 {
        t.frames(1);
        if let Some(at) = texts(&t.shapes, text).first().copied() {
            seen.push((popup_at(&t.ctx, at.center()), at));
        }
    }
    seen
}

#[test]
fn a_menu_bar_popup_opens_4_below_the_bar_and_does_not_jump_between_frames() {
    for zoom in [1.0, 1.5] {
        let mut t = T::new();
        let ctx = t.ctx.clone();
        t.app.set_interface_zoom(&ctx, zoom);
        t.frames(3);
        for menu in ["Audio", "View"] {
            let text = if menu == "Audio" { "Reconnect Device" } else { "Grid Over Clips" };
            let seen = first_frames(&mut t, menu, text);
            assert!(seen.len() >= 2, "{menu} at {zoom}: the popup painted");
            let bar_bottom = c::MENU_BAR_HEIGHT;
            for (frame, text_rect) in &seen {
                assert!(
                    near(frame.min.y, bar_bottom + crate::chrome::POPUP_MENU_BAR_GAP),
                    "{menu} at {zoom}: the popup's top {} want {}",
                    frame.min.y,
                    bar_bottom + 4.0
                );
                assert!(
                    near(text_rect.min.x - (frame.min.x + EDGE), if menu == "Audio" { TEXT_IN_PLAIN } else { TEXT_IN }),
                    "{menu}: the gutter is right on the first frame"
                );
            }
            let item = bar_item(&t, menu);
            assert!(near(seen[0].0.min.x, item[0]), "{menu} at {zoom}: the popup's left edge {} is the item's {}", seen[0].0.min.x, item[0]);
            assert_eq!(seen[0], seen[1], "{menu} at {zoom}: the first frame equals the second");
            // Close it again for the next menu.
            t.input("ui.key", json!({"key": "Escape"}));
            t.frames(4);
        }
    }
}
