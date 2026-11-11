//! The Instrument view's keys through the panel's rig: zone selection by arrows, the handle
//! nudges, Delete's question, the registry rows, and keys that never reach the arrangement while a
//! popup holds them.

use super::super::test_rig::{T, mid};
use super::keyboard;
use super::rig::*;
use crate::shortcuts::{Action, Scope};
use serde_json::json;

fn press(t: &mut T, key: &str, alt: bool, shift: bool) {
    t.send("ui.key", json!({"key": key, "alt": alt, "shift": shift}));
}

#[test]
fn arrows_select_the_neighboring_zone_and_send_nothing() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    assert_eq!(view(&t).selected, Some(1));
    // Zone 1 (keys 48 to 52, velocities 1 to 63) has zone 2 above it; the pad's zone (A1, key 36)
    // is to its left.
    press(&mut t, egui::Key::ArrowUp.name(), false, false);
    assert_eq!(view(&t).selected, Some(2));
    press(&mut t, egui::Key::ArrowDown.name(), false, false);
    assert_eq!(view(&t).selected, Some(1));
    press(&mut t, egui::Key::ArrowRight.name(), false, false);
    assert_eq!(view(&t).selected, Some(2), "right: the next by lowest key, then velocity");
    press(&mut t, egui::Key::ArrowLeft.name(), false, false);
    press(&mut t, egui::Key::ArrowLeft.name(), false, false);
    assert_eq!(view(&t).selected, Some(3), "left of zone 1: the pad's zone");
    press(&mut t, egui::Key::ArrowLeft.name(), false, false);
    assert_eq!(view(&t).selected, Some(3), "nothing further left");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
    assert!(layout(&t, tr).key_map.zone(3).unwrap().selected);
}

#[test]
fn alt_arrows_nudge_the_last_pressed_handle_one_or_ten_frames() {
    let (mut t, tr) = T::open();
    zone(&mut t, tr, 1, "One.wav", 48_000, json!({"keys": [48, 52]}));
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"start": 1000, "end": 20_000}}));
    show_instrument(&mut t, tr);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    press(&mut t, egui::Key::ArrowRight.name(), true, false);
    assert_eq!(t.patch(tr).zone(0).unwrap().start, 1001, "S until a handle is pressed");
    let e = layout(&t, tr).zone.unwrap().wave.end_flag;
    t.click(mid(e), 1);
    press(&mut t, egui::Key::ArrowLeft.name(), true, true);
    assert_eq!(t.patch(tr).zone(0).unwrap().end, 19_990, "E after a press on it, ten frames");
    assert_eq!(
        sent(&t, log),
        vec![
            "mix.sampler_zone_edit {\"region\":{\"start\":1001},\"zone\":1}".to_string(),
            "mix.sampler_zone_edit {\"region\":{\"end\":19990},\"zone\":1}".to_string()
        ]
    );
    assert_eq!(t.undo_len(), undo + 2, "one step per press");
}

#[test]
fn delete_asks_and_removes_the_zone_in_one_step() {
    let (mut t, tr) = layered();
    let z2 = layout(&t, tr).key_map.zone(2).unwrap().rect;
    t.click(mid(z2), 1);
    let uid = *t.app.builtin_editor.devices.sampler.views.keys().next().unwrap();
    let undo = t.undo_len();
    t.key("Delete");
    let m = crate::chrome::modal_layout(&t.ctx, keyboard::confirm_id(uid)).expect("the question");
    let n50 = transit_midi::note_name(50, t.app.ui.middle_c);
    assert_eq!(m.buttons.last().unwrap().text, keyboard::DELETE_LABEL);
    assert_eq!(view(&t).confirm, Some(2), "{n50} ff");
    t.key("Escape");
    assert!(view(&t).confirm.is_none());
    assert!(t.patch(tr).zone(1).is_some() && t.undo_len() == undo, "Cancel changes nothing");
    assert!(!t.app.builtin_editor.windows.is_empty(), "the window stays");
    t.key("Delete");
    let m = crate::chrome::modal_layout(&t.ctx, keyboard::confirm_id(uid)).expect("the question");
    t.click(mid(m.buttons.last().unwrap().rect), 1);
    assert!(t.patch(tr).zone(1).is_none(), "zone 2 removed");
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn the_keys_are_registered_under_the_sampler_instrument_scope() {
    let rows = |a: Action| crate::shortcuts::rows(a).map(|s| (s.scope, crate::shortcuts::text(s))).collect::<Vec<_>>();
    assert_eq!(rows(Action::SamplerPreviousZone), [(Scope::SamplerInstrument, "Left".to_string())]);
    assert_eq!(rows(Action::SamplerNextZone), [(Scope::SamplerInstrument, "Right".to_string())]);
    assert_eq!(rows(Action::SamplerZoneAbove), [(Scope::SamplerInstrument, "Up".to_string())]);
    assert_eq!(rows(Action::SamplerZoneBelow), [(Scope::SamplerInstrument, "Down".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeHandleEarlier), [(Scope::SamplerInstrument, "Alt+Left".to_string())]);
    assert_eq!(rows(Action::SamplerNudgeHandleLaterTen), [(Scope::SamplerInstrument, "Alt+Shift+Right".to_string())]);
    assert_eq!(rows(Action::SamplerRemoveZone), [(Scope::SamplerInstrument, "Delete".to_string())]);
    assert!(Scope::ALL.contains(&Scope::SamplerInstrument));
    assert!(super::key_map::zone_tip().contains("Delete"), "the key map's tooltip names the keys");
}

#[test]
fn keys_under_an_open_popup_do_nothing_and_never_reach_the_arrangement() {
    let (mut t, tr) = layered();
    let r = t.app.run("midi.new_clip", json!({"tracks": [tr.0], "start": 0, "end": 96_000})).unwrap();
    let clip = r["clip"].as_u64().expect("a clip id");
    t.run("edit.select", json!({"clips": [clip]}));
    let doc = |t: &T| serde_json::to_value(t.app.engine.session()).unwrap();
    let before = doc(&t);
    // The title bar's device menu: a popup inside the Sampler window (the window keeps the keys).
    // (The preset list was this popup until the Sampler's presets became unavailable, #455.)
    let menu = t.panel(tr).title.menu;
    t.click(mid(menu), 1);
    assert!(egui::Popup::is_any_open(&t.ctx), "the popup is open");
    let (undo, sel) = (t.undo_len(), view(&t).selected);
    press(&mut t, "Delete", false, false);
    press(&mut t, egui::Key::ArrowRight.name(), true, false);
    press(&mut t, egui::Key::ArrowUp.name(), false, false);
    assert!(doc(&t) == before, "the clip is neither deleted nor nudged, and nothing else changed");
    assert_eq!((t.undo_len(), view(&t).selected, view(&t).confirm), (undo, sel, None), "and the view does nothing");
    assert!(t.app.engine.session().tracks.iter().any(|x| x.clips().iter().any(|c| c.id.0 == clip)));
}

#[test]
fn the_view_takes_its_keys_under_a_popup_so_none_is_left_for_others() {
    let (mut t, tr) = layered();
    let menu = t.panel(tr).title.menu;
    t.click(mid(menu), 1);
    assert!(egui::Popup::is_any_open(&t.ctx), "the popup is open");
    // One frame with Delete down: the view's scope is exempt from the app's popup guard, so the
    // view consumes the key (and does nothing with it); a held scope would leave it in the input.
    t.time += 1.0 / 60.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, t.screen)),
        time: Some(t.time),
        events: vec![super::super::slices::rig::key(egui::Key::Delete, true, egui::Modifiers::NONE)],
        ..Default::default()
    };
    let app = &mut t.app;
    let mut left = true;
    let mut out = t.ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
        left = ui.ctx().input(|i| i.key_pressed(egui::Key::Delete));
    });
    out.textures_delta.clear();
    assert!(!left, "the view took Delete");
    assert_eq!(view(&t).confirm, None, "and asked nothing");
}
