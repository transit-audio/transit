//! The bench's pointer gestures of #469 through the panel's rig: a press on the waveform selects
//! the slice under the pointer and auditions it while held; a double-click adds one marker. The
//! double-click is the toolkit's own detection, driven by the rig's stepped input (no wall clock):
//! clicks at one place are kept past its double-click time with idle frames where they must not
//! pair.

use super::super::test_rig::{T, mid};
use super::rig::*;
use serde_json::json;
use transit_model::TrackId;

fn held(t: &T) -> Option<u8> {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().held
}

fn auditions(t: &T) -> u64 {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().auditions
}

fn last_audition(t: &T) -> Option<(u8, u8)> {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().last_audition
}

fn doc(t: &T) -> serde_json::Value {
    serde_json::to_value(t.app.engine.session()).unwrap()
}

fn velocity(t: &T) -> u8 {
    t.app.ui.keys.velocity.clamp(1, 127)
}

fn window_open(t: &T, tr: TrackId) -> bool {
    t.app.ui.plugin_windows.iter().any(|k| k.0 == tr)
}

#[test]
fn a_click_on_the_waveform_selects_and_auditions_and_changes_nothing() {
    let (mut t, tr) = sliced();
    let (before, undo, log) = (doc(&t), t.undo_len(), view(&t).sent.len());
    // Slice 3 runs from 22 050 to 33 075 and sits on pad A3 (note 38).
    let at = at_frame(&bench(&t, tr), 30_000);
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(view(&t).selected, 2, "the press selects the slice under the pointer");
    assert_eq!(held(&t), Some(38), "and plays it while the button is held");
    assert_eq!(last_audition(&t), Some((38, velocity(&t))), "its pad note at the piano roll's velocity");
    assert!(!t.app.is_playing(), "the song does not start");
    t.input(vec![press(at, false)]);
    assert_eq!(held(&t), None, "the release ends it");
    assert!(bench(&t, tr).selection.starts_with("Slice 3 · 0.500–0.750 s"));
    assert_eq!(sent(&t, log), Vec::<String>::new(), "a click sends no command");
    assert_eq!(doc(&t), before, "the kit is unchanged");
    assert_eq!(t.undo_len(), undo, "no undo step");
}

#[test]
fn a_double_click_adds_exactly_one_marker_in_one_step_and_leaves_no_audition() {
    let (mut t, tr) = sliced();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let at = at_frame(&bench(&t, tr), 5_000);
    t.click(at, 2);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "one command for one double-click: {s:?}");
    assert!(s[0].starts_with("mix.sampler_slice_marker") && s[0].contains("\"op\":\"add\""), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
    assert_eq!(frames(&t, tr).len(), 5, "exactly one marker");
    assert_eq!(held(&t), None, "the first click's audition ended");
    assert_eq!(view(&t).feedback.left_error, None);
}

#[test]
fn a_click_in_auto_no_longer_copies_the_markers_to_manual() {
    let (mut t, tr) = auto();
    let (before, undo, log) = (t.patch(tr).slicing.unwrap(), t.undo_len(), view(&t).sent.len());
    let at = at_frame(&bench(&t, tr), 30_000);
    t.click(at, 1);
    assert_eq!(view(&t).selected, 2, "the click selects");
    assert_eq!(t.patch(tr).slicing.unwrap(), before, "Auto stays, Manual is not written");
    assert_eq!(sent(&t, log), Vec::<String>::new());
    assert_eq!(t.undo_len(), undo);
    assert_eq!(bench(&t, tr).hint, super::bench::HINT_AUTO);
}

#[test]
fn a_held_audition_ends_on_release_when_the_pointer_leaves_and_when_the_window_goes_away() {
    let (mut t, tr) = sliced();
    let at = at_frame(&bench(&t, tr), 30_000);
    // The release.
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(38));
    t.input(vec![press(at, false)]);
    assert_eq!(held(&t), None);
    // Each press past the double-click time of the click before it (the toolkit's own count).
    t.frames(30);
    // The pointer leaves the window with the button still down; its release comes later.
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(38));
    t.input(vec![egui::Event::PointerGone]);
    assert_eq!(held(&t), None, "the pointer left: no note hangs");
    t.input(vec![press(at, false)]);
    t.frames(30);
    // The window loses the focus (the system may never deliver the release).
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(38));
    t.input(vec![egui::Event::WindowFocused(false), egui::Event::WindowFocused(true)]);
    assert_eq!(held(&t), None, "the window lost the focus: no note hangs");
    t.input(vec![press(at, false)]);
    t.frames(30);
    // The window closes (Escape) while the button is held.
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(38));
    steps(&mut t, vec![key(egui::Key::Escape, true, egui::Modifiers::NONE), key(egui::Key::Escape, false, egui::Modifiers::NONE)]);
    assert!(!window_open(&t, tr), "the window closed");
    assert_eq!(held(&t), None, "the window closed: no note hangs");
    t.input(vec![press(at, false)]);
}

#[test]
fn a_press_on_a_handle_does_not_audition() {
    let (mut t, tr) = sliced();
    let n = auditions(&t);
    let hd = mid(bench(&t, tr).flags[2].handle.unwrap());
    steps(&mut t, vec![moved(hd), press(hd, true)]);
    assert_eq!(held(&t), None, "a handle takes its own press");
    assert_eq!(auditions(&t), n);
    t.input(vec![press(hd, false)]);
    assert_eq!(auditions(&t), n, "nor does its click");
    assert!(view(&t).gesture.drag.is_none());
}

#[test]
fn a_double_click_on_a_marker_is_refused_as_too_close() {
    let (mut t, tr) = sliced();
    let undo = t.undo_len();
    let b = bench(&t, tr);
    let l = view(&t).snapshot.data.unwrap();
    let x = view(&t).viewport.x_of(&l, 22_050.0, b.well[0] + 1.0, b.well[2] - b.well[0] - 2.0);
    let at = (x, at_frame(&b, 0).1);
    t.point(at);
    assert_eq!(bench(&t, tr).ghost.expect("the ghost").label, "too close");
    t.click(at, 2);
    assert_eq!(frames(&t, tr), [0, 11_025, 22_050, 33_075], "nothing added");
    assert_eq!(t.undo_len(), undo, "no step");
    assert!(panels(&t, tr).left_error.is_some(), "the refusal is the Manual panel's error banner");
    assert_eq!(held(&t), None);
}

#[test]
fn the_gestures_under_an_open_dropdown_do_nothing() {
    let (mut t, tr) = sliced();
    t.click(mid(panels(&t, tr).uses.root), 1);
    assert!(egui::Popup::is_any_open(&t.ctx), "the Root dropdown is open");
    let (n, undo, log, selected) = (auditions(&t), t.undo_len(), view(&t).sent.len(), view(&t).selected);
    let at = at_frame(&bench(&t, tr), 30_000);
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!((held(&t), auditions(&t), view(&t).selected), (None, n, selected), "the press under the dropdown plays and selects nothing");
    t.input(vec![press(at, false)]);
    // A double-click that starts under the dropdown adds nothing: its first click is not counted.
    t.click(mid(panels(&t, tr).uses.root), 1);
    assert!(egui::Popup::is_any_open(&t.ctx));
    t.click(at, 2);
    assert_eq!(frames(&t, tr), [0, 11_025, 22_050, 33_075], "nothing added");
    assert_eq!(t.undo_len(), undo);
    assert!(sent(&t, log).iter().all(|s| !s.contains("\"op\":\"add\"")), "{:?}", sent(&t, log));
    assert_eq!(held(&t), None);
}

#[test]
fn a_press_and_space_share_one_audition_and_each_ends_only_its_own() {
    let (mut t, tr) = sliced();
    let space = |pressed| key(egui::Key::Space, pressed, egui::Modifiers::NONE);
    // A press, then Space while the button is held: Space takes the audition over; the button's
    // release leaves it, Space's release ends it.
    let at = at_frame(&bench(&t, tr), 30_000);
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(38));
    t.input(vec![space(true)]);
    assert!(view(&t).key_held, "Space owns it now");
    t.input(vec![press(at, false)]);
    assert_eq!(held(&t), Some(38), "the button's release leaves Space's audition");
    t.input(vec![space(false)]);
    assert_eq!(held(&t), None);
    // Space on slice 4, then a press on slice 2: the press takes it over; Space's release leaves
    // it, the button's release ends it.
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 3).unwrap().1;
    t.click(mid(cell), 1);
    t.input(vec![space(true)]);
    assert_eq!(held(&t), Some(39));
    let at = at_frame(&bench(&t, tr), 15_000);
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(held(&t), Some(37), "one audition: the press replaced Space's note");
    t.input(vec![space(false)]);
    assert_eq!(held(&t), Some(37), "Space's release leaves the press's audition");
    t.input(vec![press(at, false)]);
    assert_eq!(held(&t), None, "no note is left hanging");
}

#[test]
fn clicks_work_zoomed_and_scrolled_at_one_and_a_half_pixels_per_point() {
    let (mut t, tr) = sliced();
    t.run("mix.sampler_slice_set", json!({"track": tr.0, "snap": false}));
    t.ctx.set_pixels_per_point(1.5);
    t.frames(4);
    let z = bench(&t, tr).zoom.unwrap();
    t.click(mid(z.segments[3]), 1);
    // Scroll so that the view starts inside slice 2 (an 8× view is an eighth of the loop).
    {
        let v = &mut t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices;
        let l = v.snapshot.data.clone().unwrap();
        v.viewport.scroll_to(&l, 20_000.0);
    }
    t.frames(3);
    let b = bench(&t, tr);
    let range = b.view;
    assert!(range.0 < 22_050.0 && range.1 > 22_050.0, "marker 3 in view: {range:?}");
    let l = view(&t).snapshot.data.unwrap();
    let (left, w) = (b.well[0] + 1.0, b.well[2] - b.well[0] - 2.0);
    let x = view(&t).viewport.x_of(&l, 23_500.0, left, w);
    let at = (x, at_frame(&b, 0).1);
    steps(&mut t, vec![moved(at), press(at, true)]);
    assert_eq!(view(&t).selected, 2, "the slice under the pointer, zoomed and scrolled");
    assert_eq!(held(&t), Some(38));
    t.input(vec![press(at, false)]);
    assert_eq!(bench(&t, tr).view, range, "the click does not scroll the view");
    let want = view(&t).viewport.frame_at(&l, x, left, w);
    // Past the double-click time of the click before (the toolkit's own count).
    t.frames(30);
    t.click(at, 2);
    assert_eq!(frames(&t, tr), [0, 11_025, 22_050, want, 33_075], "the marker lands under the pointer");
    assert_eq!(held(&t), None);
}

#[test]
fn the_manual_hint_names_the_double_click_and_fits_beside_the_selection_line() {
    let (mut t, tr) = sliced();
    t.screen = egui::vec2(super::super::MIN_WIDTH + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(3);
    let b = bench(&t, tr);
    assert!(b.hint.contains("Click plays") && b.hint.contains("double-click adds"), "{}", b.hint);
    assert!(b.hint_rect[2] < b.selection_rect[0], "at the minimum width: hint {:?}, selection {:?}", b.hint_rect, b.selection_rect);
}
