//! A pad drag on the Pads view lives only while its button is down (#481): Escape cancels it (the
//! window stays, as on the Slices and Instrument views), and one whose release the view did not see
//! is dropped, never kept to swap on a later click. Step by step, a drag still swaps or moves in one
//! undo step, and one released off the grid changes nothing.

use super::slices::rig::{moved, press, steps};
use super::test_rig::{T, mid};
use transit_model::TrackId;

fn drag_of(t: &T) -> Option<u8> {
    t.app.builtin_editor.devices.sampler.views.values().find_map(|v| v.drag)
}

/// Presses pad `from` and moves past the drag threshold to `to`, the button held.
fn hold_drag(t: &mut T, tr: TrackId, from: u8, to: (f32, f32)) {
    let a = t.pad_at(tr, from);
    steps(t, vec![moved(a), press(a, true), moved((a.0 + 20.0, a.1)), moved(to)]);
    assert_eq!(drag_of(t), Some(from), "the drag started on pad {from}");
}

#[test]
fn escape_during_a_pad_drag_cancels_it_and_the_next_click_swaps_nothing() {
    let (mut t, tr) = T::kit();
    let (before, undo) = (t.patch(tr), t.undo_len());
    let strip = mid(t.panel(tr).strip.panel);
    hold_drag(&mut t, tr, 0, strip);
    t.key("Escape");
    assert_eq!(drag_of(&t), None, "Escape canceled the drag");
    assert!(!t.app.builtin_editor.windows.is_empty(), "Escape canceled the drag; the window stays");
    steps(&mut t, vec![press(strip, false)]);
    // The issue's next click: another pad, pressed and released.
    t.click(t.pad_at(tr, 1), 1);
    assert_eq!(t.patch(tr), before, "no swap");
    assert_eq!(t.undo_len(), undo, "no step");
    // With no drag, Escape is the window's again.
    t.key("Escape");
    assert!(t.app.builtin_editor.windows.is_empty(), "Escape closes the window");
}

#[test]
fn a_pad_drag_whose_release_the_view_did_not_see_is_dropped() {
    let (mut t, tr) = T::kit();
    let (before, undo) = (t.patch(tr), t.undo_len());
    let strip = mid(t.panel(tr).strip.panel);
    hold_drag(&mut t, tr, 0, strip);
    // The window closes under the held button; the release lands with no Pads view drawn.
    let key = t.window(tr).key;
    t.app.close_editor(key);
    t.frames(2);
    steps(&mut t, vec![press(strip, false)]);
    // Reopened without a click of its own (a click's release on the window would take the drag).
    t.app.toggle_editor(key);
    t.frames(3);
    assert!(!t.app.builtin_editor.windows.is_empty(), "the window is open again");
    t.click(t.pad_at(tr, 1), 1);
    assert_eq!(t.patch(tr), before, "the stale drag swapped nothing");
    assert_eq!(t.undo_len(), undo, "no step");
}

#[test]
fn a_pad_drag_step_by_step_still_swaps_moves_or_lands_off_the_grid() {
    let (mut t, tr) = T::kit();
    let (a, b) = (t.patch(tr).pad(0), t.patch(tr).pad(1));
    let undo = t.undo_len();
    let to = t.pad_at(tr, 1);
    hold_drag(&mut t, tr, 0, to);
    steps(&mut t, vec![press(to, false)]);
    let patch = t.patch(tr);
    assert_eq!((patch.pad(0).zone, patch.pad(1).zone), (b.zone, a.zone), "swapped");
    assert_eq!(t.undo_len(), undo + 1, "one step");
    assert_eq!(drag_of(&t), None);
    // Onto an empty pad: the sound moves, one step.
    let to = t.pad_at(tr, 6);
    hold_drag(&mut t, tr, 2, to);
    steps(&mut t, vec![press(to, false)]);
    let patch = t.patch(tr);
    assert!(patch.pad(2).zone.is_none() && patch.pad(6).zone == Some(2), "moved");
    assert_eq!(t.undo_len(), undo + 2);
    // Released off the grid (on the strip): nothing changes, as before.
    let before = t.patch(tr);
    let strip = mid(t.panel(tr).strip.panel);
    hold_drag(&mut t, tr, 0, strip);
    steps(&mut t, vec![press(strip, false)]);
    assert_eq!(t.patch(tr), before);
    assert_eq!(t.undo_len(), undo + 2);
    assert_eq!(drag_of(&t), None);
    assert!(!t.app.builtin_editor.windows.is_empty());
}
