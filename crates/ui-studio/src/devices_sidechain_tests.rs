use crate::tests::*;

/// An app with the device pane on `bass`, whose slot 0 is a compressor; a Kick track, a Verb aux
/// track (also with a compressor) and a "Drums" bus exist. Returns (app, ctx, time, bass, kick, verb).
fn key_app() -> (StudioApp, egui::Context, f64, TrackId, TrackId, TrackId) {
    let mut app = app();
    let kick = new_track(&mut app, json!({"kind": "audio", "format": "stereo", "name": "Kick"}));
    let bass = new_track(&mut app, json!({"kind": "audio", "format": "stereo", "name": "Bass"}));
    let verb = new_track(&mut app, json!({"kind": "aux", "name": "Verb"}));
    app.run("mix.new_bus", json!({"name": "Drums"})).unwrap();
    app.add_device(bass, "compressor").unwrap();
    app.add_device(verb, "compressor").unwrap();
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    app.focus(bass);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, bass, kick, verb)
}

fn key_of(app: &StudioApp, t: TrackId) -> Option<transit_model::KeySource> {
    app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().key
}

#[test]
fn the_key_dropdown_lists_other_tracks_and_a_pick_runs_one_insert_key() {
    let (mut app, ctx, mut time, bass, kick, _verb) = key_app();
    let card = app.mixer.devices.cards[0].clone();
    assert_ne!(card.key_button, [0.0; 4], "a compressor takes a key");
    assert_eq!(card.key_label, "Key: None");
    click(&mut app, mid(card.key_button));
    frames(&mut app, &ctx, 6, &mut time);
    let menu = app.mixer.devices.menu.clone();
    let values: Vec<&str> = menu.iter().map(|m| m.value.as_str()).collect();
    assert!(values.contains(&"key:none") && values.contains(&"tap:post") && values.contains(&"tap:pre"), "{values:?}");
    assert!(values.contains(&format!("track:{}", kick.0).as_str()), "the kick is listed: {values:?}");
    assert!(!values.contains(&format!("track:{}", bass.0).as_str()), "a track cannot key itself: {values:?}");
    assert!(values.iter().all(|v| !v.starts_with("bus:")), "an audio track takes no bus key: {values:?}");
    let before = undo_len(&mut app);
    let item = menu.iter().find(|m| m.value == format!("track:{}", kick.0)).cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let k = key_of(&app, bass).expect("the key is set");
    assert_eq!((k.from, k.tap), (transit_model::KeyFrom::Track(kick), transit_model::Tap::Post));
    assert_eq!(undo_len(&mut app), before + 1, "one mix.insert_key, one undo step");
    assert_eq!(app.mixer.devices.cards[0].key_label, "Key: Kick (post)");
    // Pre-fader keeps the source; None clears it.
    let at = mid(app.mixer.devices.cards[0].key_button);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let pre = app.mixer.devices.menu.iter().find(|m| m.value == "tap:pre").cloned().unwrap();
    click(&mut app, mid(pre.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(key_of(&app, bass).unwrap().tap, transit_model::Tap::Pre);
    assert_eq!(app.mixer.devices.cards[0].key_label, "Key: Kick (pre)");
    let at = mid(app.mixer.devices.cards[0].key_button);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let none = app.mixer.devices.menu.iter().find(|m| m.value == "key:none").cloned().unwrap();
    click(&mut app, mid(none.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(key_of(&app, bass), None);
    assert_eq!(app.mixer.devices.cards[0].key_label, "Key: None");
}

#[test]
fn an_aux_track_lists_busses_and_keys_from_one() {
    let (mut app, ctx, mut time, _bass, _kick, verb) = key_app();
    app.focus(verb);
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(app.mixer.devices.cards[0].key_button);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let drums = app.engine.session().busses[0].id;
    let item = app.mixer.devices.menu.iter().find(|m| m.value == format!("bus:{}", drums.0)).cloned().expect("the Drums bus is listed");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(key_of(&app, verb).map(|k| k.from), Some(transit_model::KeyFrom::Bus(drums)));
    assert_eq!(app.mixer.devices.cards[0].key_label, "Key: Drums (post)");
}

#[test]
fn a_key_that_would_loop_shows_the_engine_error_in_the_status_line() {
    let (mut app, ctx, mut time, bass, kick, _verb) = key_app();
    // Kick is keyed from Bass first; keying Bass from Kick would close a loop.
    app.add_device(kick, "compressor").unwrap();
    app.run("mix.insert_key", json!({"track": kick.0, "slot": 0, "from": {"track": bass.0}})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let before = undo_len(&mut app);
    let at = mid(app.mixer.devices.cards[0].key_button);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == format!("track:{}", kick.0)).cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(key_of(&app, bass), None, "the refused key is not set");
    assert!(app.status.contains("loop"), "{}", app.status);
    assert_eq!(undo_len(&mut app), before, "a refusal is not an undo step");
}

#[test]
fn a_device_without_a_key_input_has_no_key_dropdown() {
    let (mut app, ctx, mut time, bass, _kick, _verb) = key_app();
    app.add_device(bass, "gain").unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let cards = app.mixer.devices.cards.clone();
    assert_eq!(cards.len(), 2);
    assert_ne!(cards[0].key_button, [0.0; 4]);
    assert_eq!(cards[1].plugin, "gain");
    assert_eq!(cards[1].key_button, [0.0; 4], "gain has no sidechain");
}

mod hardware_inserts {
    use super::*;
    use transit_model::IoMap;

    fn open_add_menu(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64) {
        let at = mid(app.mixer.devices.add);
        click(app, at);
        frames(app, ctx, 6, time);
    }

    #[test]
    fn the_plus_menu_adds_a_hardware_insert_through_a_picker_that_lists_measured_pairs_first() {
        let (mut app, ctx, mut time, t) = mixer_app();
        app.engine.set_io_map(IoMap::for_device("test", "test", 4, 4));
        app.engine.store_hardware_latency("Out 3-4", "In 3-4", 612);
        app.focus(t);
        frames(&mut app, &ctx, 3, &mut time);
        open_add_menu(&mut app, &ctx, &mut time);
        let item = app.mixer.devices.menu.iter().find(|m| m.value == "hardware_insert").cloned().expect("the + menu offers Hardware Insert…");
        assert_eq!(item.label, "Hardware Insert…");
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        let picker = app.mixer.hw_picker_layout.clone();
        assert!(picker.open && picker.track == Some(t.0));
        let values: Vec<&str> = picker.rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(values.first(), Some(&"Out 3-4|In 3-4"), "the measured pair comes first: {values:?}");
        let io = app.engine.io_map();
        let want = io.paths_of(transit_model::IoDirection::Output).count() * io.paths_of(transit_model::IoDirection::Input).count();
        assert_eq!(values.len(), want, "every output with every input, the measured pair among them once");
        assert!(picker.rows[0].label.contains("612 frames (12.8 ms)"), "{}", picker.rows[0].label);
        assert!(picker.rows[1..].iter().all(|r| r.label.ends_with("not measured")), "{:?}", picker.rows);
        let steps = undo_len(&mut app);
        click(&mut app, mid(picker.rows[0].rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert!(!app.mixer.hw_picker_layout.open, "the picker closes on a pick");
        assert_eq!(undo_len(&mut app), steps + 1);
        let h = app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().hardware.clone().unwrap();
        assert_eq!((h.send.as_str(), h.ret.as_str()), ("Out 3-4", "In 3-4"));
        // The card names the pair and the state: no audio device here, so no live input.
        let card = app.mixer.devices.cards.first().cloned().expect("a card");
        assert_eq!((card.name.as_str(), card.hardware.as_str()), ("Hardware Insert", "HW: Out 3-4 → In 3-4"));
        assert_eq!(card.hardware_state, "No input");
        // Bypass works like any insert's.
        click(&mut app, mid(card.bypass_button));
        frames(&mut app, &ctx, 6, &mut time);
        assert!(app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
        assert!(app.mixer.devices.cards[0].bypass);
    }

    #[test]
    fn the_card_says_why_a_pair_does_not_run() {
        let (mut app, ctx, mut time, t) = mixer_app();
        app.engine.set_io_map(IoMap::for_device("test", "test", 4, 4));
        app.run("mix.insert", json!({"track": t.0, "slot": 0, "hardware": {"send": "Nowhere", "return": "In 3-4"}})).unwrap();
        app.run("mix.insert", json!({"track": t.0, "slot": 1, "hardware": {"send": "Out 3-4", "return": "Nothing"}})).unwrap();
        app.focus(t);
        frames(&mut app, &ctx, 4, &mut time);
        let states: Vec<String> = app.mixer.devices.cards.iter().map(|c| c.hardware_state.clone()).collect();
        assert_eq!(states, ["Missing send", "Missing return"]);
        // A copy of the track keeps its insert, disabled, and the card gives the reason.
        app.run("mix.insert_remove", json!({"track": t.0, "slot": 1})).unwrap();
        app.run("mix.insert_remove", json!({"track": t.0, "slot": 0})).unwrap();
        app.run("mix.insert", json!({"track": t.0, "slot": 0, "hardware": {"send": "Out 3-4", "return": "In 3-4"}})).unwrap();
        app.run("track.duplicate", json!({"track": t.0})).unwrap();
        let copy = app.engine.session().tracks.iter().find(|x| x.name.ends_with(".dup")).map(|x| x.id).unwrap();
        app.focus(copy);
        frames(&mut app, &ctx, 4, &mut time);
        let card = app.mixer.devices.cards.first().cloned().unwrap();
        assert_eq!(card.hardware_state, "Disabled: send path in use by Gtr");
        // Pressing the state makes the insert again; the original still holds the path, so it is refused.
        click(&mut app, mid(card.hardware_button));
        frames(&mut app, &ctx, 4, &mut time);
        assert!(app.status.contains("Out 3-4"), "{}", app.status);
        assert!(app.mixer.devices.cards[0].hardware_state.starts_with("Disabled"));
    }

    #[test]
    fn the_state_words_and_the_blockers_are_plain() {
        use crate::hardware_ui::{measure_blocker, state_text};
        use transit_mix::HardwareState as S;
        assert_eq!(state_text(&S::Ready, Some(612), 48_000), "12.8 ms");
        assert_eq!(state_text(&S::Unmeasured, None, 48_000), "Unmeasured");
        assert_eq!(state_text(&S::NoInput, None, 48_000), "No input");
        assert_eq!(state_text(&S::MissingSend, None, 48_000), "Missing send");
        assert_eq!(state_text(&S::MissingReturn, None, 48_000), "Missing return");
        assert_eq!(state_text(&S::Disabled("x".into()), None, 48_000), "Disabled: x");
        assert_eq!(measure_blocker(true, true, false), None);
        assert!(measure_blocker(false, false, false).unwrap().contains("audio device"));
        assert!(measure_blocker(true, false, false).unwrap().contains("duplex"));
        assert!(measure_blocker(true, true, true).unwrap().contains("Stop the transport"));
    }
}
