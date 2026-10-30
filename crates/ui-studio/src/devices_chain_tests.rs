use crate::tests::*;

#[test]
fn plus_inserts_a_built_in_device_and_bypass_and_remove_round_trip() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.devices.track, Some(t.0));
    assert!(app.mixer.devices.cards.is_empty());
    let at = mid(app.mixer.devices.add);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    app.mixer.effect_filter = "gain".into();
    frames(&mut app, &ctx, 3, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "gain").cloned().expect("the + menu lists the built-in Gain");
    assert!(app.mixer.devices.menu.iter().all(|m| !m.value.contains(':')), "no hosted plug-ins in a hermetic test");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert_eq!(ins.iter().flatten().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), vec!["gain"]);
    let card = app.mixer.devices.cards.first().cloned().expect("a device card");
    assert_eq!((card.slot, card.plugin.as_str()), (Some(0), "gain"));
    assert!(card.editor_enabled, "a built-in has its own editor window, player or not");
    click(&mut app, mid(card.bypass_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    assert!(app.mixer.devices.cards[0].bypass);
    click(&mut app, mid(card.bypass_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(!app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    click(&mut app, mid(card.remove_button));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.inserts.iter().all(Option::is_none));
    assert!(app.mixer.devices.cards.is_empty());
}

#[test]
fn the_instrument_is_the_first_device() {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.add_device(t, "gain").unwrap();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let cards = &app.mixer.devices.cards;
    assert_eq!(cards.iter().map(|c| c.slot).collect::<Vec<_>>(), vec![None, Some(0)]);
    assert_eq!(cards[0].plugin, "macro_oscillator");
}

#[test]
fn a_new_instrument_track_carries_the_macro_oscillator_and_the_menu_no_longer_offers_the_old_synth() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    let ins = app.engine.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.plugin, transit_model::DEFAULT_INSTRUMENT);
    assert_eq!(ins.plugin, "macro_oscillator");
    let info = transit_dsp::plugin_info(&ins.plugin).unwrap();
    assert_eq!(info.param("voices").unwrap().default, 7.0, "the default is 8 voices (index 7)");
    let ids: Vec<String> = app.instrument_lists().builtin.into_iter().map(|e| e.id).collect();
    assert!(ids.iter().any(|i| i == "macro_oscillator"), "{ids:?}");
    assert!(ids.iter().all(|i| i != "subtractive_synth"), "{ids:?}");
}

fn hover(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.move", json!({"x": at.0, "y": at.1}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

fn instrument_of(app: &StudioApp, t: TrackId) -> Option<transit_model::Insert> {
    app.engine.session().track(t).unwrap().instrument.clone()
}

#[test]
fn the_instrument_menu_runs_mix_instrument_and_a_replace_is_one_undo_step() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument", "name": "Keys"}));
    let t2 = new_track(&mut app, json!({"kind": "instrument", "name": "Direct"}));
    app.ui.show_devices = true;
    app.focus(t);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let card = app.mixer.devices.cards.first().cloned().unwrap();
    assert_eq!((card.slot, card.plugin.as_str()), (None, "macro_oscillator"));
    let before = undo_len(&mut app);
    click(&mut app, mid(card.pick_button));
    frames(&mut app, &ctx, 6, &mut time);
    let menu = app.mixer.devices.menu.clone();
    assert!(menu.iter().any(|m| m.value == "sampler"), "built-in instruments are listed: {menu:?}");
    assert!(menu.iter().all(|m| m.value != "gain"), "no effects in the instrument menu");
    assert!(menu.iter().all(|m| !m.value.contains(':')), "no hosted plug-ins in a hermetic test");
    let item = menu.iter().find(|m| m.value == "drum_synth").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "drum_synth");
    assert_eq!(undo_len(&mut app), before + 1, "a replace is one undo step");
    app.run("mix.instrument", json!({"track": t2.0, "plugin": "drum_synth"})).unwrap();
    assert_eq!(instrument_of(&app, t), instrument_of(&app, t2), "the menu and mix.instrument give the same instrument");
    assert_eq!(app.mixer.devices.cards[0].plugin, "drum_synth");
    // Picking the instrument it already has changes nothing.
    let n = undo_len(&mut app);
    app.set_instrument(t, "drum_synth").unwrap();
    assert_eq!(undo_len(&mut app), n);
    app.run("edit.undo", json!({})).unwrap();
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "macro_oscillator");
}

#[test]
fn an_instrument_track_without_an_instrument_offers_choose() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    // A session saved without an instrument (test setup; the UI itself only executes commands).
    app.engine.session_mut().track_mut(t).unwrap().instrument = None;
    app.ui.show_devices = true;
    app.focus(t);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let card = app.mixer.devices.cards.first().cloned().unwrap();
    assert_eq!((card.slot, card.plugin.as_str(), card.name.as_str()), (None, "", "No instrument"));
    assert_ne!(card.pick_button, [0.0; 4], "a Choose… button");
    click(&mut app, mid(card.pick_button));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "sampler").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "sampler");
}

#[test]
fn the_track_menu_has_an_instrument_submenu() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().header;
    right_click(&mut app, (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0));
    frames(&mut app, &ctx, 6, &mut time);
    let sub = app.arrange.menu.iter().find(|m| m.label == "Instrument").cloned().expect("Instrument in the track menu");
    hover(&mut app, mid(sub.rect));
    frames(&mut app, &ctx, 10, &mut time);
    let item = app.arrange.menu.iter().find(|m| m.value == "drum_synth").cloned().expect("the submenu lists the instruments");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().plugin, "drum_synth");
}

#[test]
fn a_long_instrument_list_filters_and_groups_by_vendor() {
    use crate::devices::{HostedEntry, InstrumentLists, instrument_menu};
    let hosted =
        |v: &'static str, n: usize| (0..n).map(move |i| HostedEntry { id: format!("vst3:{v}{i}"), name: format!("{v} Synth {i}"), vendor: v.into() });
    let lists =
        InstrumentLists { builtin: Vec::new(), vst3: Some(hosted("Arturia", 10).chain(hosted("Native", 10)).collect()), clap: Some(Vec::new()) };
    let ctx = egui::Context::default();
    let mut items = Vec::new();
    let mut filter = "arturia".to_string();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        instrument_menu(ui, &lists, None, &mut filter, &mut Default::default(), &mut items);
    });
    out.textures_delta.clear();
    assert_eq!(items.len(), 10, "{items:?}");
    assert!(items.iter().all(|m| m.value.starts_with("vst3:Arturia")));
}

#[test]
fn plus_adds_after_the_last_device() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio"}));
    app.add_device(t, "gain").unwrap();
    app.add_device(t, "eq_7band").unwrap();
    app.remove_device(t, 0).unwrap();
    let r = app.add_device(t, "gain").unwrap();
    assert_eq!(r["slot"], 2, "after the last occupied slot, not into the gap");
    let order: Vec<&str> = app.engine.session().track(t).unwrap().mixer.inserts.iter().flatten().map(|i| i.plugin.as_str()).collect();
    assert_eq!(order, vec!["eq_7band", "gain"]);
}

#[test]
fn the_device_chain_lists_the_sends_with_pre_and_post() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    app.new_fx_channel(Some(t), -6.0).unwrap();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let row = app.mixer.devices.sends.first().cloned().expect("a sends section");
    assert_eq!((row.target.as_str(), row.level_db, row.pre_fader), ("FX 1", -6.0, false));
    assert_ne!(app.mixer.devices.add_send, [0.0; 4]);
    click(&mut app, mid(row.pre));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().pre_fader);
    let at = mid(app.mixer.devices.sends[0].remove);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

// ---- Mixer follow-up ----

#[test]
fn hosted_plugins_read_as_names_never_as_class_ids() {
    use crate::devices::hosted_label;
    let id = "vst3:75747241564153494A5550382D303030";
    assert_eq!(hosted_label(id, true, Some("Jup-8000 V"), false), "Jup-8000 V", "the cache's name wins");
    assert_eq!(hosted_label(id, true, None, true), "Scanning…", "while the scan runs");
    assert_eq!(hosted_label(id, true, None, false), "VST3 instrument", "unknown after the scan");
    assert_eq!(hosted_label("clap:com.x.y", true, None, false), "CLAP instrument");
    assert_eq!(hosted_label("clap:com.x.y", false, None, false), "CLAP effect");
    // Built-ins keep their names, and an unknown id that is not hosted keeps its tail.
    let mut app = app();
    assert_eq!(app.plugin_name(id, true), "VST3 instrument", "no scan in tests, so no cache entry");
    let builtin = app.builtin_plugins().into_iter().find(|p| p.instrument).expect("a built-in instrument");
    assert_eq!(app.plugin_name(&builtin.id, true), builtin.name);
}

#[test]
fn the_plus_menu_is_shaped_like_the_instrument_menu_and_publishes_its_rows() {
    use crate::devices::{HostedEntry, InstrumentLists, plugin_menu};
    let hosted =
        |v: &'static str, n: usize| (0..n).map(move |i| HostedEntry { id: format!("vst3:{v}{i}"), name: format!("{v} Amp {i}"), vendor: v.into() });
    let builtin = |id: &str| crate::mixer::PluginEntry { id: id.into(), name: id.into(), category: String::new(), instrument: false };
    let lists = InstrumentLists {
        builtin: vec![builtin("gain"), builtin("dither")],
        vst3: Some(hosted("Native", 8).chain(hosted("Arturia", 8)).collect()),
        clap: Some(hosted("Clapper", 2).collect()),
    };
    let ctx = egui::Context::default();
    let run = |filter: &mut String, rect: &mut [f32; 4], items: &mut Vec<crate::mixer::MenuItem>| {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            plugin_menu(ui, &lists, "effects", None, filter, rect, items);
        });
        out.textures_delta.clear();
    };
    let (mut items, mut rect, mut filter) = (Vec::new(), [0.0; 4], String::new());
    run(&mut filter, &mut rect, &mut items);
    assert_ne!(rect, [0.0; 4], "a filter box above 15 entries");
    assert_eq!(items.len(), 20, "built-in and hosted rows are all published");
    assert!(items.iter().any(|m| m.value == "vst3:Native3" && m.label == "Native Amp 3"), "hosted names come from the scan");
    assert!(items.iter().all(|m| m.rect != [0.0; 4]));
    filter = "guitar".into();
    items.clear();
    run(&mut filter, &mut rect, &mut items);
    assert!(items.is_empty());
    filter = "clapper".into();
    items.clear();
    run(&mut filter, &mut rect, &mut items);
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|m| m.value.starts_with("vst3:Clapper")));
    // A short list has no filter box.
    let short = InstrumentLists { builtin: vec![builtin("gain")], vst3: Some(Vec::new()), clap: None };
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        plugin_menu(ui, &short, "effects", None, &mut String::new(), &mut rect, &mut items);
    });
    out.textures_delta.clear();
    assert_eq!(rect, [0.0; 4]);
}

#[test]
fn the_device_chain_and_strip_plus_menus_list_effects_only_and_both_add() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(app.mixer.devices.add);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let menu = app.mixer.devices.menu.clone();
    assert!(menu.iter().any(|m| m.value == "dither" && !m.visible), "rows scrolled out of the menu are flagged: {menu:?}");
    assert!(menu.iter().any(|m| m.value == "gain"), "{menu:?}");
    assert_ne!(app.mixer.filter_rect, [0.0; 4], "the filter box is published");
    assert!(menu.iter().all(|m| m.value != "sampler" && m.value != "macro_oscillator"), "instruments are refused in an effects menu");
    let fr = mid(app.mixer.filter_rect);
    click(&mut app, fr);
    frames(&mut app, &ctx, 3, &mut time);
    app.mixer.effect_filter = "gain".into();
    frames(&mut app, &ctx, 3, &mut time);
    // The Hardware Insert entry is not an effect: the filter leaves it alone.
    let menu: Vec<_> = app.mixer.devices.menu.iter().filter(|m| m.value != "hardware_insert").cloned().collect();
    assert_eq!(menu.len(), 1, "the filter narrows the rows: {menu:?}");
    assert!(menu[0].visible);
    // The chain menu's rows are reachable by UI input, like the instrument menu's.
    let item = menu[0].clone();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().plugin, "gain");
    // The strip's "+" is the same menu.
    let at = mid(strip(&app, t).add_insert);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert_ne!(app.mixer.filter_rect, [0.0; 4]);
    app.mixer.effect_filter = "dither".into();
    frames(&mut app, &ctx, 3, &mut time);
    let strip_menu = app.mixer.layout.menu.clone();
    assert!(strip_menu.iter().any(|m| m.value == "dither"));
    assert!(strip_menu.iter().all(|m| m.value != "sampler"));
    let item = strip_menu.iter().find(|m| m.value == "dither").cloned().expect("Dither in the strip menu");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert_eq!(ins.iter().flatten().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), vec!["gain", "dither"]);
    // The card publishes its buttons.
    let card = app.mixer.devices.cards[0].clone();
    assert!(card.editor_button != [0.0; 4] && card.bypass_button != [0.0; 4] && card.remove_button != [0.0; 4]);
    assert!(!card.editor_open);
}

#[test]
fn the_card_editor_button_toggles_the_editor_window() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio"}));
    let key = (t, 0usize);
    assert!(app.ui.plugin_windows.is_empty());
    app.toggle_editor(key);
    assert_eq!(app.ui.plugin_windows, vec![key], "first click opens");
    assert_eq!(app.editors.len(), 1);
    app.toggle_editor(key);
    assert!(app.ui.plugin_windows.is_empty(), "second click closes");
    assert!(app.editors.is_empty());
    app.toggle_editor(key);
    assert_eq!(app.ui.plugin_windows, vec![key], "and again opens");
}

/// A chain of gain, saturator, compressor on the focused track; returns (app, ctx, time, track).
fn chain_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let (mut app, ctx, mut time, t) = mixer_app();
    for plugin in ["gain", "saturator", "compressor"] {
        app.add_device(t, plugin).unwrap();
    }
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

fn chain_order(app: &StudioApp, t: TrackId) -> Vec<String> {
    app.engine.session().track(t).unwrap().mixer.inserts.iter().flatten().map(|i| i.plugin.clone()).collect()
}

#[test]
fn dragging_a_device_title_reorders_the_chain_in_one_undo_step() {
    let (mut app, ctx, mut time, t) = chain_app();
    assert_eq!(app.mixer.devices.cards.len(), 3);
    let uid = |app: &StudioApp, plugin: &str| {
        app.engine.session().track(t).unwrap().mixer.inserts.iter().flatten().find(|i| i.plugin == plugin).map(|i| i.uid.get())
    };
    let gain = uid(&app, "gain");
    let steps = undo_len(&mut app);
    // An editor window open on the gain (slot 0) follows it.
    app.ui.plugin_windows = vec![(t, 0)];
    let c = app.mixer.devices.cards.clone();
    let grab = mid(c[0].drag_handle);
    let past_last = (c[2].rect[0] + 100.0, grab.1);
    drag(&mut app, grab, past_last, json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(chain_order(&app, t), ["saturator", "compressor", "gain"], "dropped after the last device");
    assert_eq!(uid(&app, "gain"), gain, "the insert kept its identity");
    assert_eq!(undo_len(&mut app), steps + 1, "one undo step");
    assert_eq!(app.ui.plugin_windows, vec![(t, 2)], "the open editor followed its plug-in");
    assert_eq!(app.mixer.devices.drop_x, None, "the indicator is gone after the drop");
    // Back to the front: drag the last card before the first.
    let c = app.mixer.devices.cards.clone();
    drag(&mut app, mid(c[2].drag_handle), (c[0].rect[0] + 10.0, grab.1), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(chain_order(&app, t), ["gain", "saturator", "compressor"]);
    assert_eq!(undo_len(&mut app), steps + 2);
    assert_eq!(app.ui.plugin_windows, vec![(t, 0)]);
}

#[test]
fn dropping_a_device_where_it_is_does_nothing() {
    let (mut app, ctx, mut time, t) = chain_app();
    let steps = undo_len(&mut app);
    let c = app.mixer.devices.cards.clone();
    // The middle card, dragged a little within its own bounds, and to just past the next card's
    // left edge (still before that card's center): both drop where it already is.
    let grab = mid(c[1].drag_handle);
    drag(&mut app, grab, (grab.0 + 30.0, grab.1), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    drag(&mut app, grab, (c[2].rect[0] + 10.0, grab.1), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(chain_order(&app, t), ["gain", "saturator", "compressor"]);
    assert_eq!(undo_len(&mut app), steps, "no command, no undo entry");
}

#[test]
fn alt_dragging_a_device_copies_it_into_the_drop_position() {
    let (mut app, ctx, mut time, t) = chain_app();
    app.run("mix.insert_bypass", json!({"track": t.0, "slot": 2, "value": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let steps = undo_len(&mut app);
    let c = app.mixer.devices.cards.clone();
    let grab = mid(c[2].drag_handle);
    drag(&mut app, grab, (c[0].rect[0] + 10.0, grab.1), json!({"alt": true}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(chain_order(&app, t), ["compressor", "gain", "saturator", "compressor"], "the copy is first, the original stays");
    let ins = app.engine.session().track(t).unwrap().mixer.inserts.clone();
    assert!(ins[0].as_ref().unwrap().bypass, "the copy has the original's settings");
    assert_ne!(ins[0].as_ref().unwrap().uid.get(), ins[3].as_ref().unwrap().uid.get());
    assert_eq!(undo_len(&mut app), steps + 1, "one undo step");
}

// ---- External Instrument (ADR 0020 slice 1) ----

#[test]
fn the_instrument_menu_puts_an_external_instrument_on_the_track_and_its_card_edits_it() {
    let mut app = app();
    app.engine.set_io_map(transit_model::IoMap::for_device("test", "test", 2, 2));
    let t = new_track(&mut app, json!({"kind": "instrument", "name": "Model D"}));
    app.ui.show_devices = true;
    app.focus(t);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let before = undo_len(&mut app);
    let pick = app.mixer.devices.cards[0].pick_button;
    click(&mut app, mid(pick));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == transit_model::EXTERNAL_INSTRUMENT_PLUGIN).cloned();
    let item = item.expect("the instrument menu offers the External Instrument");
    assert_eq!(item.label, "External Instrument");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let ins = instrument_of(&app, t).unwrap();
    assert_eq!(ins.plugin, transit_model::EXTERNAL_INSTRUMENT_PLUGIN);
    assert_eq!(ins.external.as_ref().unwrap().ret, "In 1", "the first input path is the return to start with");
    assert_eq!(undo_len(&mut app), before + 1, "one undo step");
    let card = app.mixer.devices.cards[0].clone();
    assert_eq!(card.name, "External Instrument");
    assert_eq!(card.external_state, "No audio device", "no player in a test");
    assert_ne!(card.external_return, [0.0; 4]);
    assert_ne!(card.external_gain, [0.0; 4]);
    assert_ne!(card.external_latency, [0.0; 4]);
    // The return dropdown lists the input paths; a pick is one instrument.external.
    click(&mut app, mid(card.external_return));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.devices.menu.iter().find(|m| m.value == "return:In 2").cloned().expect("In 2 is offered");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().external.unwrap().ret, "In 2");
    // The latency field: typed ms, committed when it loses focus.
    let field = app.mixer.devices.cards[0].external_latency;
    click(&mut app, mid(field));
    frames(&mut app, &ctx, 2, &mut time);
    send_control(&mut app, "ui.text", json!({"text": "19.5"}));
    frames(&mut app, &ctx, 2, &mut time);
    send_control(&mut app, "ui.key", json!({"key": "Enter"}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(instrument_of(&app, t).unwrap().external.unwrap().manual_ms, Some(19.5), "{}", app.status);
}

#[test]
fn a_bounce_with_an_external_instrument_asks_and_leaves_it_out_only_when_chosen() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument", "name": "Model D"}));
    app.run("instrument.external", json!({"track": t.0, "return": "In 1"})).unwrap();
    app.run("midi.new_clip", json!({"tracks": [t.0], "start": 0, "end": 4_800})).unwrap();
    let dir = std::env::temp_dir().join(format!("transit-ext-bounce-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("mix.wav");
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let open = |app: &mut StudioApp| {
        app.open_bounce();
        if let Some(f) = &mut app.finish.bounce_form {
            f.path = out.to_string_lossy().into_owned();
            f.range = crate::finish::BounceRange::Whole;
        }
    };
    open(&mut app);
    assert!(app.start_bounce().is_err(), "never a silent skip");
    assert_eq!(app.finish.external_question.as_deref(), Some(&["Model D".to_string()][..]));
    assert!(app.status.contains("External Instrument (Model D)"), "{}", app.status);
    assert!(app.finish.job.is_none());
    // Cancel: nothing is written, the form is back.
    frames(&mut app, &ctx, 3, &mut time);
    let cancel = app.finish.external_buttons.1;
    click(&mut app, mid(cancel));
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.finish.external_question.is_none() && app.finish.job.is_none());
    assert_eq!(app.status, "Bounce not started");
    // Render without it: the bounce runs and the status names the track left out.
    assert!(app.start_bounce().is_err());
    frames(&mut app, &ctx, 3, &mut time);
    let without = app.finish.external_buttons.0;
    click(&mut app, mid(without));
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.finish.external_question.is_none());
    assert!(app.status.contains("Left out: Model D"), "{}", app.status);
    for _ in 0..200 {
        if app.finish.job.is_none() {
            break;
        }
        frames(&mut app, &ctx, 1, &mut time);
    }
    assert!(out.exists(), "{}", app.status);
    let _ = std::fs::remove_dir_all(&dir);
}
