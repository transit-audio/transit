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
