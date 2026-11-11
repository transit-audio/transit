//! Follow-ups from the review of pass M2, through the panel's rig: the fades' `(… used)` readout,
//! the reroll's entropy, the Make group Notice's keys, and the remove question for an alternative
//! the engine will not remove.

use super::super::test_rig::{T, mid};
use super::keyboard;
use super::rig::*;
use serde_json::json;
use transit_model::TrackId;

fn window_open(t: &T, tr: TrackId) -> bool {
    t.app.ui.plugin_windows.iter().any(|k| k.0 == tr)
}

#[test]
fn fades_read_the_widths_used_beside_the_stored_ones_when_they_differ() {
    let (mut t, tr) = layered();
    // Zone 1 spans keys 48 to 52: four keys of low fade fit; narrowing the zone to two keys leaves
    // the stored widths (checked when set) and plays fewer (clamped when used).
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"fades": {"key_lo": 4, "key_hi": 0, "vel_lo": 0, "vel_hi": 0}}}));
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, "fades 4 · 0 · 0 · 0", "equal: no used part");
    t.run(
        "mix.sampler_zone_map",
        json!({"track": tr.0, "zones": [{"zone": 1, "keys": [48, 49], "velocities": [1, 63], "root": 50, "membership": null}]}),
    );
    let st = view(&t).snapshot.data.unwrap();
    let z = st.zone(1).unwrap();
    assert_eq!(z.fades.all(), [4, 0, 0, 0], "the stored widths kept");
    let used = z.fades_used.all();
    assert_ne!(used, z.fades.all(), "playback uses less");
    let joined = used.iter().map(u8::to_string).collect::<Vec<_>>().join(" · ");
    assert_eq!(layout(&t, tr).zone.unwrap().fields[3].0, format!("fades 4 · 0 · 0 · 0 ({joined} used)"));
}

#[test]
fn the_reroll_sends_the_first_entropy_value_that_is_neither_zero_nor_the_stored_seed() {
    let (mut t, tr) = grouped();
    t.run("mix.sampler_seed", json!({"track": tr.0, "seed": 77}));
    let g = layout(&t, tr).group_panel.unwrap();
    t.click(mid(g.rr.unwrap().segments[1]), 1);
    t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().instrument.entropy = vec![0, 77, 4242, 9];
    let log = view(&t).sent.len();
    t.click(mid(layout(&t, tr).group_panel.unwrap().reroll), 1);
    assert_eq!(sent(&t, log), vec!["mix.sampler_seed {\"seed\":4242}".to_string()]);
    assert_eq!(t.patch(tr).seed, 4242);
}

#[test]
fn enter_answers_make_group_and_escape_cancels_keeping_the_window() {
    let (mut t, tr) = grouped();
    let z4 = layout(&t, tr).key_map.zone(4).unwrap().rect;
    t.click(mid(z4), 1);
    let add = layout(&t, tr).group_panel.unwrap().layers[0].add;
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    // Escape: the Notice goes, nothing is sent, the window stays.
    t.click(mid(add), 1);
    assert!(layout(&t, tr).group_panel.unwrap().notice.is_some(), "the Notice asks");
    t.key("Escape");
    assert!(layout(&t, tr).group_panel.unwrap().notice.is_none(), "Escape cancels");
    assert!(window_open(&t, tr), "the window stays");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
    // With no Notice, Escape is the window's again.
    // Enter: Make group, one step.
    t.click(mid(add), 1);
    assert!(layout(&t, tr).group_panel.unwrap().notice.is_some());
    t.key("Enter");
    assert_eq!(t.undo_len(), undo + 1);
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].starts_with("mix.sampler_zone_map ") && s[0].contains("\"pad\":null"), "{s:?}");
    assert_eq!(t.patch(tr).pad(0).zone, None, "pad A1 is empty");
    assert!(layout(&t, tr).group_panel.unwrap().notice.is_none());
    t.key("Escape");
    assert!(!window_open(&t, tr), "with nothing asking, Escape closes the window");
}

#[test]
fn removing_a_lower_alternative_says_it_will_be_refused_and_then_shows_the_engines_words() {
    let (mut t, tr) = grouped();
    // Zone 1 is alternative 0 of layer 0, zone 2 its alternative 1: removing zone 1 leaves a gap.
    let chip = layout(&t, tr).group_panel.unwrap().layers[1].chips[0].rect;
    t.click(mid(chip), 1);
    assert_eq!(view(&t).selected, Some(1));
    let uid = *t.app.builtin_editor.devices.sampler.views.keys().next().unwrap();
    let undo = t.undo_len();
    t.key("Delete");
    let m = crate::chrome::modal_layout(&t.ctx, keyboard::confirm_id(uid)).expect("the question");
    let words = {
        let e = t.app.engine.execute("mix.sampler_zone_clear", &json!({"track": tr.0, "zone": 1})).expect_err("the engine refuses");
        super::super::slices::edit::words(&e)
    };
    assert_eq!(m.detail_text.as_deref(), Some(keyboard::refusal_detail(&words).as_str()), "the question says what will happen");
    t.click(mid(m.buttons.last().unwrap().rect), 1);
    assert_eq!(layout(&t, tr).note.expect("the engine's words").0, words);
    assert!(t.patch(tr).zone(0).is_some() && t.undo_len() == undo, "nothing removed");
    assert!(view(&t).confirm.is_none(), "the question is answered");
    // The highest alternative goes: the plain undo detail, one step.
    let chip = layout(&t, tr).group_panel.unwrap().layers[1].chips[1].rect;
    t.click(mid(chip), 1);
    t.key("Delete");
    let m = crate::chrome::modal_layout(&t.ctx, keyboard::confirm_id(uid)).expect("the question");
    assert_eq!(m.detail_text.as_deref(), Some(crate::menus::undo_detail(false)));
    t.click(mid(m.buttons.last().unwrap().rect), 1);
    assert!(t.patch(tr).zone(1).is_none() && t.undo_len() == undo + 1);
}
