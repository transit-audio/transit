//! The Slices view's keys through the panel's rig: the registry rows, the selection and nudges,
//! Delete, and the guards (typing, modals, a held audition when its window goes away).

use super::super::test_rig::{T, mid};
use super::rig::*;
use crate::shortcuts::{self, Action, Scope};
use egui::{Key, Modifiers};

const ALT: Modifiers = Modifiers::ALT;
const ALT_SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::ALT };

/// A key press and release with `m` held, through the control channel (the modifiers held for the
/// frame, as a keyboard reports them).
fn tap(t: &mut T, k: Key, m: Modifiers) {
    t.send("ui.key", serde_json::json!({"key": k.name(), "alt": m.alt, "shift": m.shift}));
}

#[test]
fn keyboard_nudge_moves_one_frame_and_ten_with_shift() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 0).unwrap().1;
    t.click(mid(cell), 1);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let edit = t.app.engine.session().edit.clone();
    tap(&mut t, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(view(&t).selected, 1, "Right selects the next slice");
    tap(&mut t, Key::ArrowRight, ALT);
    assert_eq!(frames(&t, tr)[1], 11_026, "one frame");
    tap(&mut t, Key::ArrowLeft, ALT_SHIFT);
    assert_eq!(frames(&t, tr)[1], 11_016, "ten frames");
    assert_eq!(t.undo_len(), undo + 2, "one step each");
    let s = sent(&t, log);
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|c| c.contains("\"op\":\"move\"") && c.contains("\"snap\":false")), "a nudge moves exact frames: {s:?}");
    tap(&mut t, Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(view(&t).selected, 0);
    // Delete on S sends nothing; on another slice it removes that slice's start marker.
    tap(&mut t, Key::Delete, Modifiers::NONE);
    assert_eq!(sent(&t, log).len(), 2, "Delete never removes S");
    tap(&mut t, Key::ArrowRight, Modifiers::NONE);
    tap(&mut t, Key::ArrowRight, Modifiers::NONE);
    tap(&mut t, Key::Delete, Modifiers::NONE);
    assert_eq!(frames(&t, tr), [0, 11_016, 33_075]);
    assert_eq!(t.undo_len(), undo + 3);
    let after = &t.app.engine.session().edit;
    assert_eq!(
        (after.selection, after.selected_clips.clone()),
        (edit.selection, edit.selected_clips.clone()),
        "the keys never reach the arrangement"
    );
}

#[test]
fn shortcuts_are_registered_in_the_sampler_scope_and_listed() {
    let rows = |a: Action| shortcuts::rows(a).map(|r| (r.scope, shortcuts::text(r))).collect::<Vec<_>>();
    assert_eq!(rows(Action::SamplerPreviousSlice), [(Scope::SamplerSlices, "Left".to_string())]);
    assert_eq!(rows(Action::SamplerNextSlice), [(Scope::SamplerSlices, "Right".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeEarlier), [(Scope::SamplerSlices, "Alt+Left".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeLater), [(Scope::SamplerSlices, "Alt+Right".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeEarlierTen), [(Scope::SamplerSlices, "Alt+Shift+Left".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeLaterTen), [(Scope::SamplerSlices, "Alt+Shift+Right".to_string())]);
    assert_eq!(rows(Action::SamplerRemoveMarker), [(Scope::SamplerSlices, "Delete".to_string())]);
    assert_eq!(rows(Action::SamplerCancelDrag), [(Scope::SamplerSlices, "Escape".to_string())]);
    let listed = shortcuts::filtered("slice");
    assert!(listed.iter().any(|(s, _)| *s == Scope::SamplerSlices), "Help › Keyboard Shortcuts lists the Sampler slices group");
    assert!(Scope::ALL.contains(&Scope::SamplerSlices));
    // Each key is in its control's tooltip.
    let tip = super::tip("Remove: click, or", &[Action::SamplerRemoveMarker]);
    assert_eq!(tip, "Remove: click, or: Delete");
}

#[test]
fn a_held_audition_ends_when_its_window_closes() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 3).unwrap().1;
    t.click(mid(cell), 1);
    t.input(vec![key(Key::Space, true, Modifiers::NONE)]);
    let held = |t: &T| t.app.builtin_editor.devices.sampler.views.values().next().unwrap().held;
    assert_eq!(held(&t), Some(39));
    // Escape closes the window while Space is still held, then Space comes up.
    t.input(vec![key(Key::Escape, true, Modifiers::NONE), key(Key::Escape, false, Modifiers::NONE)]);
    assert!(!t.app.ui.plugin_windows.iter().any(|k| k.0 == tr), "the window closed");
    t.input(vec![key(Key::Space, false, Modifiers::NONE)]);
    assert_eq!(held(&t), None, "no note is left hanging");
}

#[test]
fn a_modal_holds_the_slices_keys() {
    let (mut t, tr) = sliced();
    let cell = bench(&t, tr).cells.iter().find(|c| c.0 == 3).unwrap().1;
    t.click(mid(cell), 1);
    let auditions = |t: &T| t.app.builtin_editor.devices.sampler.views.values().next().unwrap().auditions;
    let before = auditions(&t);
    assert!(t.app.dialog.is_none(), "a toolkit modal that is not one of the app's dialogs");
    // The modal is open first (a frame), then Space goes down and up under it.
    for event in [None, Some(true), Some(false)] {
        t.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, t.screen)),
            time: Some(t.time),
            events: event.map(|pressed| key(Key::Space, pressed, Modifiers::NONE)).into_iter().collect(),
            ..Default::default()
        };
        let app = &mut t.app;
        let mut out = t.ctx.run_ui(raw, |ui| {
            egui::Modal::new(egui::Id::new("a test modal")).show(ui.ctx(), |ui| ui.label("modal"));
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
    assert_eq!(auditions(&t), before, "Space does not audition under a modal");
}
