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
                    near(frame.min.y, bar_bottom + c::MENU_BAR_GAP),
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

#[test]
fn the_add_device_popup_has_no_current_device_so_it_has_no_gutter() {
    let mut t = T::new();
    let fx = t.track("audio", "Gtr");
    t.app.ui.show_devices = true;
    t.app.focus(fx);
    t.frames(3);
    let add = t.app.mixer.devices.add;
    t.click(add, "primary");
    let menu = t.app.mixer.devices.menu.clone();
    let hardware = find(&menu, "Hardware Insert…");
    let p = Popup::of(&t, &hardware);
    p.insets_plain("Hardware Insert…", None);
    assert!(near(p.text_in("Built-in"), TEXT_IN_PLAIN), "the heading lines up with the rows");
    for row in menu.iter().filter(|m| m.visible && m.value == "gain") {
        assert!(near(p.text_in(&row.label), TEXT_IN_PLAIN), "{} starts at 12", row.label);
    }
    assert_eq!(p.checks_in(&hardware), 0);
}

/// Opens the volume fader's learn menu on `gtr`'s strip and returns its popup measure helpers' rows.
fn strip_learn_menu(t: &mut T, gtr: transit_model::TrackId) -> Vec<crate::mixer::MenuItem> {
    t.app.ui.show_mixer = true;
    t.frames(3);
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == gtr.0).cloned().unwrap();
    t.click(strip.knob, "secondary");
    t.app.midi_learn.menu.clone()
}

#[test]
fn an_unmapped_controls_learn_menu_has_no_gutter_and_a_mapped_ones_keeps_it_with_the_takeover_mark() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    let menu = strip_learn_menu(&mut t, gtr);
    let learn = find(&menu, "MIDI Learn");
    Popup::of(&t, &learn).insets_plain("MIDI Learn", None);
    // Map the pan knob, then open the menu again: the takeover choices carry the check.
    t.app.run("midi.learn", json!({"target": format!("track:{}:pan", gtr.0)})).unwrap();
    t.frames(2);
    t.app.midi_learn_apply(
        (0, vec![transit_playback::MapMsg::Candidate { port: "KeyStep".into(), channel: 0, kind: transit_model::SourceKind::Cc, number: 74 }]),
        0.0,
    );
    t.frames(2);
    t.input("ui.key", json!({"key": "Escape"}));
    t.frames(4);
    let menu = strip_learn_menu(&mut t, gtr);
    let scaled = find(&menu, "Takeover: Scaled");
    let p = Popup::of(&t, &scaled);
    p.insets("Takeover: Scaled", None);
    assert!(near(p.text_in("MIDI Learn"), TEXT_IN), "the plain rows share the gutter");
    assert_eq!(p.checks_in(&scaled), 1, "the current takeover mode carries the mark");
}

#[test]
fn the_transport_learn_menu_never_has_a_check_so_it_has_no_gutter() {
    let mut t = T::new();
    let play = t.app.arrange.layout.transport["play"];
    t.click(play, "secondary");
    let menu = t.app.midi_learn.menu.clone();
    let learn = find(&menu, "MIDI Learn for this controller (all sessions)");
    Popup::of(&t, &learn).insets_plain("MIDI Learn for this controller (all sessions)", None);
}

fn learn_on_pan(t: &mut T, gtr: transit_model::TrackId) {
    t.app.run("midi.learn", json!({"target": format!("track:{}:pan", gtr.0)})).unwrap();
    t.frames(2);
}

fn map_pan(t: &mut T) {
    t.app.midi_learn_apply(
        (0, vec![transit_playback::MapMsg::Candidate { port: "KeyStep".into(), channel: 0, kind: transit_model::SourceKind::Cc, number: 74 }]),
        0.0,
    );
    t.frames(2);
}

#[test]
fn an_open_learn_menu_closes_when_learning_completes_under_it_and_reopens_with_the_mapped_rows() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    learn_on_pan(&mut t, gtr);
    // Open the menu while learning: Cancel, no gutter.
    let menu = strip_learn_menu(&mut t, gtr);
    let cancel = find(&menu, "Cancel MIDI Learn (Esc)");
    Popup::of(&t, &cancel).insets_plain("Cancel MIDI Learn (Esc)", None);
    // The controller moves: the mapping completes under the open menu. It closes (it does not
    // turn into the six mapped rows at 28 in the same popup).
    map_pan(&mut t);
    t.frames(2);
    assert!(texts(&t.shapes, "Cancel MIDI Learn (Esc)").is_empty(), "the Cancel row is gone");
    assert!(texts(&t.shapes, "Takeover: Scaled").is_empty(), "and the menu did not become the mapped one in place");
    // Reopening shows the mapped rows with the gutter.
    let strip = t.app.mixer.layout.strips.iter().find(|s| s.track == gtr.0).cloned().unwrap();
    t.click(strip.knob, "secondary");
    let scaled = find(&t.app.midi_learn.menu.clone(), "Takeover: Scaled");
    Popup::of(&t, &scaled).insets("Takeover: Scaled", None);
}

#[test]
fn an_open_unmapped_learn_menu_closes_when_learning_is_armed_elsewhere() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    let menu = strip_learn_menu(&mut t, gtr);
    Popup::of(&t, &find(&menu, "MIDI Learn")).insets_plain("MIDI Learn", None);
    learn_on_pan(&mut t, gtr);
    t.frames(2);
    assert!(texts(&t.shapes, "MIDI Learn for this controller (all sessions)").is_empty(), "the menu closed");
    assert!(texts(&t.shapes, "Cancel MIDI Learn (Esc)").is_empty(), "and did not turn into the Cancel menu");
}

#[test]
fn the_instrument_submenu_is_12_without_an_instrument_and_28_with_the_mark_when_it_has_one() {
    let mut t = T::new();
    let keys = t.track("instrument", "Keys");
    t.app.engine.session_mut().track_mut(keys).unwrap().instrument = None;
    t.frames(3);
    let h = t.header(keys);
    t.click([h[0] + 50.0, h[1], h[0] + 70.0, h[3]], "secondary");
    let sub = find(&t.app.arrange.menu.clone(), "Instrument");
    t.input("ui.move", json!({"x": (sub.rect[0] + sub.rect[2]) / 2.0, "y": (sub.rect[1] + sub.rect[3]) / 2.0}));
    t.frames(10);
    let menu = t.app.arrange.menu.clone();
    let sampler = menu.iter().find(|m| m.value == "sampler").cloned().expect("the submenu lists the instruments");
    let p = Popup::of(&t, &sampler);
    p.insets_plain(&sampler.label, None);
    assert_eq!(p.checks_in(&sampler), 0, "no instrument, no mark");
}

/// Whether any rect shape (a popup's frame) sits with its top left at `at` in `shapes`.
fn has_rect_at(shapes: &[egui::epaint::ClippedShape], at: egui::Pos2) -> bool {
    fn visit(s: &egui::Shape, at: egui::Pos2) -> bool {
        match s {
            egui::Shape::Rect(r) => (r.rect.min - at).abs().max_elem() < 2.0 && r.rect.width() > 50.0,
            egui::Shape::Vec(v) => v.iter().any(|x| visit(x, at)),
            _ => false,
        }
    }
    shapes.iter().any(|c| visit(&c.shape, at))
}

#[test]
fn a_send_context_menu_is_gone_on_the_frame_learning_is_armed_never_shortened() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    t.app.new_fx_channel(Some(gtr), 0.0).unwrap();
    t.app.focus(gtr);
    t.app.ui.show_devices = true;
    t.frames(4);
    let row = t.app.mixer.devices.sends[0].target_rect;
    t.click(row, "secondary");
    assert!(!texts(&t.shapes, "Remove Send").is_empty() && !texts(&t.shapes, "Pre-fader").is_empty(), "the menu is open");
    t.app.run("midi.learn", json!({"target": format!("track:{}:send:0", gtr.0)})).unwrap();
    t.frames(1);
    assert!(texts(&t.shapes, "Pre-fader").is_empty(), "no Pre-fader on the next painted frame");
    assert!(texts(&t.shapes, "Remove Send").is_empty(), "no Remove Send: the popup is gone, not shortened");
}

#[test]
fn the_rack_macro_menu_is_gone_on_the_frame_learning_is_armed_never_shortened() {
    let mut t = T::new();
    let a = t.track("instrument", "A");
    let b = t.track("instrument", "B");
    let rack = t.app.group_into_rack(&[a, b]).unwrap();
    t.app.focus(rack);
    t.app.ui.show_devices = true;
    t.frames(4);
    let k = t.app.mixer.devices.rack.clone().expect("the rack card").macros[0];
    t.click(k, "secondary");
    assert!(!texts(&t.shapes, "Map Parameter").is_empty(), "the macro menu is open");
    t.app.run("midi.learn", json!({"target": format!("track:{}:macro:1", rack.0)})).unwrap();
    t.frames(1);
    assert!(texts(&t.shapes, "Map Parameter").is_empty(), "the macro menu's own rows are not painted shortened");
    assert!(texts(&t.shapes, "Cancel MIDI Learn (Esc)").is_empty());
}

#[test]
fn a_learn_only_menu_leaves_no_empty_popup_frame_on_the_frame_learning_is_armed() {
    let mut t = T::new();
    let gtr = t.track("audio", "Gtr");
    let menu = strip_learn_menu(&mut t, gtr);
    let learn = find(&menu, "MIDI Learn");
    let frame = Popup::of(&t, &learn).frame;
    assert!(has_rect_at(&t.shapes, frame.min), "the popup's frame is painted while open");
    learn_on_pan_one_frame(&mut t, gtr);
    assert!(!has_rect_at(&t.shapes, frame.min), "no empty popup box on the next painted frame");
}

fn learn_on_pan_one_frame(t: &mut T, gtr: transit_model::TrackId) {
    t.app.run("midi.learn", json!({"target": format!("track:{}:pan", gtr.0)})).unwrap();
    t.frames(1);
}
