//! The bench's gestures through the panel's rig: what each sends, one undo step each, the refusals
//! in the engine's words, and the previews from the model's own rules.

use super::super::test_rig::mid;
use super::bench_gestures;
use super::rig::*;
use crate::theme::TOKENS;
use serde_json::json;

#[test]
fn bench_swallows_the_second_click_of_a_double_click() {
    let (mut t, tr) = sliced();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    // A double-click on the empty bench: one add, the second click swallowed.
    let at = at_frame(&bench(&t, tr), 5_000);
    t.click(at, 2);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "one command for one double-click: {s:?}");
    assert!(s[0].starts_with("mix.sampler_slice_marker") && s[0].contains("\"op\":\"add\""), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(frames(&t, tr).len(), 5);
    assert_eq!(view(&t).feedback.left_error, None, "no second add refused");
    // A double-click on a flag: one remove, never a remove and then an add.
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let flag = bench(&t, tr).flags.iter().find(|f| f.frame == 22_050).unwrap().flag;
    t.click(mid(flag), 2);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("\"op\":\"remove\""), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(!frames(&t, tr).contains(&22_050));
    assert_eq!(view(&t).gesture.renaming, None, "the swallowed click renames nothing");
}

#[test]
fn double_click_on_a_cell_renames() {
    let (mut t, tr) = sliced();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 1).unwrap().1;
    t.click(mid(cell), 2);
    assert!(view(&t).gesture.renaming.is_some(), "a double-click on a cell opens its name");
    assert!(bench(&t, tr).rename.is_some());
    t.send("ui.text", json!({"text": "Snare 2"}));
    t.key("Enter");
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("\"op\":\"label\"") && s[0].contains("Snare 2"), "{s:?}");
    let l = t.patch(tr).slicing.unwrap();
    assert_eq!(l.manual.unwrap().markers[1].label.as_deref(), Some("Snare 2"));
    assert_eq!(t.undo_len(), undo + 1, "one step");
    assert_eq!(view(&t).feedback.left_success.as_deref(), Some("Named slice 2 Snare 2"));
}

#[test]
fn escape_cancels_a_drag_and_sends_nothing() {
    let (mut t, tr) = sliced();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let hd = mid(bench(&t, tr).flags[1].handle.unwrap());
    let to = (hd.0 + 60.0, hd.1);
    steps(&mut t, vec![moved(hd), press(hd, true), moved((hd.0 + 30.0, hd.1)), moved(to)]);
    let b = bench(&t, tr);
    let (_, tip, _) = b.drag.clone().expect("the drag shows its line and tooltip");
    assert!(tip.ends_with("· snapped"), "{tip}");
    steps(&mut t, vec![key(egui::Key::Escape, true, egui::Modifiers::NONE), key(egui::Key::Escape, false, egui::Modifiers::NONE), press(to, false)]);
    assert_eq!(sent(&t, log), Vec::<String>::new(), "a canceled drag sends nothing");
    assert_eq!(frames(&t, tr), [0, 11_025, 22_050, 33_075]);
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.ui.plugin_windows.iter().any(|k| k.0 == tr), "Escape canceled the drag; the window stays");
    // A completed drag sends one move.
    t.drag(hd, to, false);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("\"op\":\"move\""), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert_ne!(frames(&t, tr)[1], 11_025);
}

#[test]
fn a_drag_into_a_neighbor_says_where_it_clamps() {
    let (mut t, tr) = sliced();
    let hd = mid(bench(&t, tr).flags[1].handle.unwrap());
    let past = at_frame(&bench(&t, tr), 30_000).0;
    steps(&mut t, vec![moved(hd), press(hd, true), moved((hd.0 + 30.0, hd.1)), moved((past, hd.1))]);
    let (_, tip, _) = bench(&t, tr).drag.expect("dragging");
    assert!(tip.ends_with("· clamped at marker 3"), "{tip}");
    t.input(vec![press((past, hd.1), false)]);
    // Clamped one frame short of marker 3 (22,049), then snapped inside its interval.
    let f = frames(&t, tr)[1];
    assert!(f > 11_025 && f < 22_050, "{f}");
    assert!(view(&t).feedback.left_success.unwrap().ends_with("· clamped at marker 3"));
}

#[test]
fn auto_gesture_copies_then_edits_with_one_undo_and_the_notice_offers_undo() {
    let (mut t, tr) = auto();
    let before = t.patch(tr).slicing.unwrap();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let at = at_frame(&bench(&t, tr), 5_000);
    t.click(at, 1);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "the engine copies and adds in one command: {s:?}");
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
    let l = t.patch(tr).slicing.unwrap();
    assert_eq!(l.mode, transit_model::sampler_patch::SliceMode::Manual);
    assert_eq!(l.manual.as_ref().unwrap().markers.len(), 5);
    let p = panels(&t, tr);
    let notice = p.manual.and_then(|m| m.notice).expect("the Notice in the Manual panel");
    assert_eq!(notice.text, "Copied Auto's 4 markers to Manual, then added marker 2.");
    assert_eq!(notice.edge, TOKENS.line_strong, "a Notice has no color");
    // Undo on the Notice is the ordinary single step: back to Auto, Manual as it was.
    t.click(mid(notice.action.unwrap()), 1);
    assert_eq!(t.patch(tr).slicing.unwrap(), before);
    assert_eq!(t.undo_len(), undo);
    assert!(panels(&t, tr).manual.is_none_or(|m| m.notice.is_none()));
    assert_eq!(bench(&t, tr).hint, super::bench::HINT_AUTO);
}

#[test]
fn ghost_turns_hot_where_the_engine_would_refuse() {
    let (mut t, tr) = sliced();
    // The ghost's rule is the model's: for every probe, the ghost is refused exactly where the
    // engine refuses the add.
    let l = view(&t).snapshot.data.unwrap();
    for frame in [0u64, 1, 5_000, 11_024, 11_025, 11_026, 22_050, 44_099] {
        let g = bench_gestures::ghost(&l, None, frame);
        let r = t.app.engine.execute("mix.sampler_slice_marker", &json!({"track": tr.0, "op": "add", "frame": frame, "snap": false}));
        if r.is_ok() {
            t.app.engine.undo();
        }
        assert_eq!(g.refused, r.is_err(), "frame {frame}: ghost {g:?}, engine {r:?}");
    }
    // On the bench: over a marker the ghost is `meter.hot` and says `too close`; elsewhere it is
    // `text` with its snapped time.
    let b = bench(&t, tr);
    let v = view(&t);
    let (left, w) = (b.well[0] + 1.0, b.well[2] - b.well[0] - 2.0);
    let x = v.viewport.x_of(&l, 11_025.0, left, w);
    t.point((x, at_frame(&b, 0).1));
    let g = bench(&t, tr).ghost.expect("the ghost follows the pointer");
    assert_eq!((g.color, g.label.as_str()), (TOKENS.meter_hot, "too close"));
    t.point(at_frame(&b, 5_000));
    let g = bench(&t, tr).ghost.unwrap();
    assert_eq!(g.color, TOKENS.text);
    assert_eq!(g.label, format!("+ {:.3} s", g.frame as f64 / 44_100.0));
}

#[test]
fn snap_toggle_unavailable_in_auto() {
    let (mut t, tr) = auto();
    let log = view(&t).sent.len();
    let snap = bench(&t, tr).snap.unwrap();
    assert!(!snap.available);
    t.click(mid(snap.rect), 1);
    assert_eq!(sent(&t, log), Vec::<String>::new(), "SNAP takes no click in Auto");
    // The mode switch: one step, then SNAP is live in Manual and sends `snap` only.
    let undo = t.undo_len();
    let mode = bench(&t, tr).mode.unwrap();
    t.click(mid(mode.segments[1]), 1);
    assert_eq!(sent(&t, log), ["mix.sampler_slice_mode {\"mode\":\"manual\"}"]);
    assert_eq!(t.undo_len(), undo + 1);
    let snap = bench(&t, tr).snap.unwrap();
    assert!(snap.available && snap.on);
    t.click(mid(snap.rect), 1);
    assert_eq!(sent(&t, log)[1], "mix.sampler_slice_set {\"snap\":false}");
    assert_eq!(t.undo_len(), undo + 2);
    assert!(!t.patch(tr).slicing.unwrap().snap);
}

#[test]
fn a_flag_click_removes_and_s_refuses_in_the_engines_words() {
    let (mut t, tr) = sliced();
    let undo = t.undo_len();
    let s1 = bench(&t, tr).flags[0].flag;
    t.click(mid(s1), 1);
    assert_eq!(t.undo_len(), undo, "S stays");
    let p = panels(&t, tr);
    let e = p.left_error.expect("the refusal is the Manual panel's error banner");
    assert_eq!(e.text, "The loop start cannot be removed.");
    assert_eq!(e.edge, TOKENS.meter_hot);
    let f3 = bench(&t, tr).flags[2].flag;
    t.click(mid(f3), 1);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(frames(&t, tr), [0, 11_025, 33_075]);
    let p = panels(&t, tr);
    assert!(p.left_error.is_none(), "an accepted edit clears it");
    let line = p.left_line.unwrap();
    assert!(line.check);
    assert_eq!(line.text, "Removed marker 3; slice 2 now runs to 0.750 s");
}

/// Selects slice `index` by its cell, then picks zoom level `zoom` (the selected slice centered).
fn zoomed(t: &mut super::super::test_rig::T, tr: transit_model::TrackId, index: usize, zoom: usize) {
    let cell = bench(t, tr).cells.iter().find(|c| c.0 == index).unwrap().1;
    t.click(mid(cell), 1);
    let z = bench(t, tr).zoom.unwrap();
    t.click(mid(z.segments[zoom]), 1);
}

#[test]
fn a_drag_released_past_the_well_edge_sends_one_move_and_ends() {
    let (mut t, tr) = sliced();
    zoomed(&mut t, tr, 2, 3);
    // Scroll so that marker 3 (22 050) sits in the middle of the 8× view (the scroll bar's job).
    {
        let v = &mut t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices;
        let l = v.snapshot.data.clone().unwrap();
        v.viewport.scroll_to(&l, 22_050.0 - v.viewport.span(&l) / 2.0);
    }
    t.frames(3);
    let b = bench(&t, tr);
    let f = b.flags.iter().find(|f| f.index == 2).expect("marker 3 in view at 8×");
    let hd = mid(f.handle.unwrap());
    let past = (b.well[0] - 40.0, hd.1);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    steps(&mut t, vec![moved(hd), press(hd, true), moved((hd.0 - 30.0, hd.1)), moved(((hd.0 + past.0) / 2.0, hd.1)), moved(past)]);
    assert!(view(&t).gesture.drag.is_some(), "the drag is held past the edge");
    t.input(vec![press(past, false)]);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "the release commits one move: {s:?}");
    assert!(s[0].contains("\"op\":\"move\""), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(frames(&t, tr)[2] < 22_050, "the marker moved toward the pointer, as the move rule clamps it");
    assert!(view(&t).gesture.drag.is_none(), "no drag is left behind");
    // Escape closes the window as usual.
    steps(&mut t, vec![key(egui::Key::Escape, true, egui::Modifiers::NONE), key(egui::Key::Escape, false, egui::Modifiers::NONE)]);
    assert!(!t.app.ui.plugin_windows.iter().any(|k| k.0 == tr), "Escape closed the window");
}

#[test]
fn a_press_on_a_handle_past_1x_starts_a_drag_not_a_scroll() {
    let (mut t, tr) = sliced();
    zoomed(&mut t, tr, 1, 1);
    let b = bench(&t, tr);
    assert!(b.scroll.is_some(), "the scroll bar shows at 2×");
    let range = b.view;
    let hd = mid(b.flags.iter().find(|f| f.index == 1).expect("marker 2 in view").handle.unwrap());
    steps(&mut t, vec![moved(hd), press(hd, true), moved((hd.0 + 10.0, hd.1)), moved((hd.0 + 20.0, hd.1))]);
    assert!(view(&t).gesture.drag.is_some_and(|d| d.index == 1), "the press at the handle's center starts a marker drag");
    assert_eq!(bench(&t, tr).view, range, "the view does not scroll");
    t.input(vec![press((hd.0 + 20.0, hd.1), false)]);
}

#[test]
fn a_drag_whose_release_was_not_seen_ends_with_no_button_down() {
    let (mut t, tr) = sliced();
    let log = view(&t).sent.len();
    // A drag left over with no button down (its release went to no frame of the bench).
    let id = t.patch(tr).slicing.unwrap().manual.unwrap().markers[1].id;
    t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices.gesture.drag =
        Some(super::bench_gestures::Drag { index: 1, id, requested: 15_000, cancelled: false });
    t.frames(2);
    assert!(view(&t).gesture.drag.is_none(), "the bench drops it");
    assert_eq!(sent(&t, log), Vec::<String>::new(), "and sends nothing");
    assert_eq!(frames(&t, tr), [0, 11_025, 22_050, 33_075]);
}
