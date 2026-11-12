//! A Sampler window collapsed to its title bar shows no view, so its views take no keys (#482):
//! Delete changes nothing on the Pads and Slices views, Space reaches the transport instead of
//! auditioning, and collapsing ends a held audition (a key's or the pointer's). Expanded again,
//! the keys act as before. (The Instrument view's case is in `instrument/keyboard_tests.rs`.)

use super::slices::rig::{bench, frames, key, press, sliced};
use super::test_rig::{T, mid};
use egui::{Key, Modifiers};
use serde_json::json;

fn uid(t: &T) -> u64 {
    *t.app.builtin_editor.devices.sampler.views.keys().next().expect("a Sampler view")
}

/// Collapses every device window (Window > Collapse All Editors); the Sampler keeps the keyboard.
fn collapse(t: &mut T) {
    t.run("window.collapse_editors", json!({}));
    let u = uid(t);
    assert!(t.app.builtin_editor.devices.collapsed.contains(&u), "collapsed");
    assert_eq!(t.app.builtin_editor.devices.focused, Some(u), "the folded window still has the keyboard");
}

fn held(t: &T) -> Option<u8> {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().held
}

fn auditions(t: &T) -> u64 {
    t.app.builtin_editor.devices.sampler.views.values().next().unwrap().auditions
}

#[test]
fn collapsed_the_pads_view_takes_no_delete_and_expanded_it_does() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 1), 1);
    collapse(&mut t);
    let (before, undo) = (serde_json::to_value(t.app.engine.session()).unwrap(), t.undo_len());
    for k in ["Delete", "Backspace"] {
        t.key(k);
        assert!(t.patch(tr).pad(1).zone.is_some(), "{k}: the pad stays");
    }
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before, "the session is unchanged");
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.dialog.is_none(), "no dialog");
    t.run("window.expand_editors", json!({}));
    t.key("Delete");
    assert!(t.patch(tr).pad(1).zone.is_none(), "expanded, Delete clears the selected pad");
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn collapsed_the_slices_view_takes_no_delete_and_space_reaches_the_transport() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 2).unwrap().1;
    t.click(mid(cell), 1);
    let markers = frames(&t, tr);
    let a = auditions(&t);
    collapse(&mut t);
    let undo = t.undo_len();
    t.key("Delete");
    assert_eq!(frames(&t, tr), markers, "Delete removes no marker");
    assert_eq!(t.undo_len(), undo);
    assert!(t.app.dialog.is_none());
    assert!(!t.app.is_playing());
    t.key("Space");
    assert!(t.app.is_playing(), "Space plays");
    assert_eq!((auditions(&t), held(&t)), (a, None), "no audition");
    t.key("Space");
    assert!(!t.app.is_playing(), "and stops");
    // Expanded, Space auditions again and the transport stays.
    t.run("window.expand_editors", json!({}));
    t.key("Space");
    assert_eq!(auditions(&t), a + 1, "expanded, Space auditions");
    assert!(!t.app.is_playing());
}

#[test]
fn collapsing_ends_a_held_audition() {
    // Space held on the Slices view.
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 3).unwrap().1;
    t.click(mid(cell), 1);
    t.input(vec![key(Key::Space, true, Modifiers::NONE)]);
    assert_eq!(held(&t), Some(39), "Space holds the slice's note");
    collapse(&mut t);
    assert_eq!(held(&t), None, "collapsing ended the key's audition");
    t.input(vec![key(Key::Space, false, Modifiers::NONE)]);
    // A pad held by the pointer on the Pads view.
    let (mut t, tr) = T::kit();
    let at = t.pad_at(tr, 0);
    t.input(vec![press(at, true)]);
    assert!(held(&t).is_some(), "the press holds the pad's note");
    collapse(&mut t);
    assert_eq!(held(&t), None, "collapsing ended the pointer's audition");
    t.input(vec![press(at, false)]);
}
