//! The MIDI effects group of the device chain (#201): ahead of the instrument, "+" adds a built-in,
//! power bypasses, the row's menu moves and the cross removes, each one undo step; parameter rows;
//! the mixer strip's marker.

use crate::tests::*;

fn keys(app: &mut StudioApp) -> TrackId {
    let t = new_track(app, json!({"kind": "instrument", "name": "Keys"}));
    app.ui.show_devices = true;
    app.focus(t);
    t
}

fn chain(app: &StudioApp, t: TrackId) -> Vec<(String, bool)> {
    app.engine.session().track(t).unwrap().midi_effects.iter().map(|f| (f.plugin.clone(), f.bypass)).collect()
}

#[test]
fn the_group_comes_before_the_instrument_and_edits_are_undoable() {
    let mut app = app();
    let t = keys(&mut app);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let group = app.mixer.devices.midi_fx.clone().expect("an instrument track shows the MIDI effects group");
    let instrument = app.mixer.devices.cards.first().cloned().unwrap();
    assert_eq!(instrument.slot, None);
    assert!(group.rect[2] < instrument.rect[0], "the group is ahead of the instrument");
    assert!(group.rows.is_empty());
    // "+" lists the built-ins.
    for id in ["midi_transpose", "midi_velocity"] {
        let before = undo_len(&mut app);
        let add = mid(app.mixer.devices.midi_fx.clone().unwrap().add);
        click(&mut app, add);
        frames(&mut app, &ctx, 6, &mut time);
        let item = app.mixer.devices.menu.iter().find(|m| m.value == id).cloned().unwrap_or_else(|| panic!("{id} in the menu"));
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_eq!(undo_len(&mut app), before + 1, "adding {id} is one undo step");
    }
    assert_eq!(chain(&app, t), [("midi_transpose".into(), false), ("midi_velocity".into(), false)]);
    let group = app.mixer.devices.midi_fx.clone().unwrap();
    assert_eq!(group.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Transpose", "Velocity"]);
    assert_eq!(group.rows[0].params.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["semitones"]);
    assert_eq!(group.rows[1].params.len(), 4, "Velocity: scale, offset, minimum, maximum");
    // Bypass, then remove, each one step; undo brings the effect back.
    let before = undo_len(&mut app);
    click(&mut app, mid(group.rows[1].bypass_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(chain(&app, t)[1], ("midi_velocity".into(), true));
    let remove = mid(app.mixer.devices.midi_fx.clone().unwrap().rows[0].remove_button);
    click(&mut app, remove);
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(chain(&app, t), [("midi_velocity".into(), true)]);
    assert_eq!(undo_len(&mut app), before + 2);
    app.run("edit.undo", json!({})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(chain(&app, t).len(), 2);
}

#[test]
fn a_row_menu_moves_the_effect() {
    let mut app = app();
    let t = keys(&mut app);
    for id in ["midi_transpose", "midi_velocity"] {
        app.add_midi_effect(t, id).unwrap();
    }
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let row = app.mixer.devices.midi_fx.clone().unwrap().rows[0].clone();
    // Right-click the name area: the context menu.
    let at = ((row.rect[0] + row.rect[2]) / 2.0, row.rect[1] + 10.0);
    right_click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.label == "Move Down").cloned().expect("Move Down in the row's menu");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(chain(&app, t), [("midi_velocity".into(), false), ("midi_transpose".into(), false)]);
}

#[test]
fn only_instrument_tracks_show_the_group_and_the_strip_marks_a_chain() {
    let mut app = app();
    let audio = new_track(&mut app, json!({"kind": "audio", "name": "Gtr"}));
    app.ui.show_devices = true;
    app.focus(audio);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.mixer.devices.midi_fx.is_none());
    let t = keys(&mut app);
    app.add_midi_effect(t, "midi_transpose").unwrap();
    app.ui.show_mixer = true;
    frames(&mut app, &ctx, 3, &mut time);
    let strip = app.mixer.layout.strips.iter().find(|s| s.track == t.0).cloned().expect("the strip");
    assert_eq!(strip.midi_fx_count, 1);
    assert!(strip.midi_fx[2] > strip.midi_fx[0], "the marker is drawn");
    let gtr = app.mixer.layout.strips.iter().find(|s| s.track == audio.0).cloned().unwrap();
    assert_eq!(gtr.midi_fx_count, 0);
}

fn fx_params(app: &StudioApp, t: TrackId) -> std::collections::BTreeMap<String, f32> {
    app.engine.session().track(t).unwrap().midi_effects[0].params.clone()
}

#[test]
fn the_random_sequencer_shows_sections_choices_a_credit_and_its_presets() {
    let mut app = app();
    let t = keys(&mut app);
    app.add_midi_effect(t, "random_sequencer").unwrap();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let row = app.mixer.devices.midi_fx.clone().unwrap().rows[0].clone();
    assert_eq!(row.name, "Random Sequencer");
    let sections: Vec<(&str, bool)> = row.sections.iter().map(|(g, open, _)| (g.as_str(), *open)).collect();
    assert_eq!(sections, [("Rhythm", true), ("Pitch", false), ("Loop", false), ("Velocity", false), ("Seed", false)]);
    assert_eq!(
        row.params.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
        ["rate", "model", "gate_bias", "jitter", "gate_length", "length_random", "play"]
    );
    assert!(row.credit.is_some(), "the port's credit line");
    // Every shown control lies inside the card.
    let card = app.mixer.devices.midi_fx.clone().unwrap().rect;
    for (id, _, r) in &row.params {
        assert!(r[0] >= card[0] && r[2] <= card[2] && r[3] <= card[3] + 0.5, "{id} at {r:?} outside {card:?}");
    }
    // A header opens its section (view state: no undo step).
    let before = undo_len(&mut app);
    click(&mut app, mid(row.sections[1].2));
    // The pane grows with the card a frame after the click.
    frames(&mut app, &ctx, 6, &mut time);
    let row = app.mixer.devices.midi_fx.clone().unwrap().rows[0].clone();
    assert!(row.sections[1].1, "Pitch is open: {:?}", row.sections);
    assert!(row.params.iter().any(|p| p.0 == "scale"));
    assert_eq!(undo_len(&mut app), before);
    // A choice lists its values (in a scrolling list): Rate 1/2 is one undo step.
    let rate = row.params.iter().find(|p| p.0 == "rate").unwrap().2;
    click(&mut app, mid(rate));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "rate=7").cloned().expect("1/2 in Rate's list");
    assert_eq!(item.label, "1/2");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(fx_params(&app, t)["rate"], 7.0);
    assert_eq!(undo_len(&mut app), before + 1);
    // The row's menu recalls a factory preset in one step.
    let row = app.mixer.devices.midi_fx.clone().unwrap().rows[0].clone();
    right_click(&mut app, ((row.rect[0] + row.rect[2]) / 2.0, row.rect[1] + 10.0));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.label == "Preset: Locked Loop").cloned().expect("the preset in the row's menu");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let p = fx_params(&app, t);
    assert_eq!((p["freeze"], p["play"], p["rate"]), (1.0, 3.0, 16.0));
    assert_eq!(undo_len(&mut app), before + 2);
    // The card grew with the open sections; the pane follows it.
    assert!(crate::midi_fx_panel::pane_height(&app) > 300.0);
}
