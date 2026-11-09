#![allow(clippy::unwrap_used)]
use super::*;

fn step(app: &mut StudioApp) {
    app.player.as_ref().unwrap().step_device_free();
    app.apply_editor_edits();
    app.poll_player();
    app.handle_transport(0.0);
    app.sync_player_session();
    app.poll_low_latency();
}

#[test]
fn command_publishes_without_a_session_edit_and_query_reports_adopted_latency() {
    transit_mix::MixEngine::force_serial(true);
    let mut engine = transit_engine::Engine::default();
    let m = engine.session_mut().add_track(transit_model::TrackKind::Master, transit_model::ChannelFormat::Stereo, None);
    let mut ins = transit_model::Insert::new("compressor");
    ins.params.insert("lookahead".into(), 4.0);
    let uid = ins.uid.get();
    engine.session_mut().track_mut(m).unwrap().mixer.inserts[0] = Some(ins);
    engine.session_mut().edit.delay_compensation = true;
    let before = engine.session().to_json().unwrap();
    let mut player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    player.output_latency = Some(std::time::Duration::from_micros(1500));
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    app.run("transport.play", json!({"from":0})).unwrap();
    for _ in 0..8 {
        step(&mut app);
    }
    let off = app.run("query.path_latency", json!({})).unwrap();
    assert_eq!(off["engine"]["frames"], 192.0);
    assert_eq!(off["master"]["frames"], 192.0);
    assert_eq!(off["padding"]["frames"], 0.0);
    assert_eq!(off["device"]["frames"], 72.0);
    assert_eq!(off["total"]["ms"], 5.5);
    assert_eq!(app.run("options.low_latency", json!({})).unwrap(), json!({"value":true,"limit_ms":0.0}));
    assert!(app.low_latency.pending);
    assert!(!app.mode_bypassed(uid));
    assert_eq!(app.mode_bypass_tip(uid), None);
    assert_eq!(app.run("query.path_latency", json!({})).unwrap(), off, "requested preference is not adopted state");
    for _ in 0..8 {
        step(&mut app);
    }
    let on = app.run("query.path_latency", json!({})).unwrap();
    assert_eq!(on["low_latency"], true);
    assert_eq!(on["engine"]["frames"], 0.0);
    assert_eq!(on["total"]["ms"], 1.5);
    assert!(app.mode_bypassed(uid));
    assert_eq!(app.mode_bypass_tip(uid), Some("bypassed by Low Latency"));
    assert!(!app.low_latency.pending);
    assert_eq!(app.status, "Low Latency on: 1 inserts bypassed, 4.0 ms saved");
    assert_eq!(app.engine.session().to_json().unwrap(), before);
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    app.ui.focus_track = Some(m);
    let ctx = egui::Context::default();
    let mut now = 0.0;
    crate::tests::frames(&mut app, &ctx, 2, &mut now);
    // The docked pane is short: the main bus's inserts are in its FX list (#326).
    let fx = crate::tests::mid(app.mixer.layout.master.as_ref().unwrap().fx_button);
    crate::tests::click(&mut app, fx);
    crate::tests::frames(&mut app, &ctx, 6, &mut now);
    assert!(app.mixer.layout.master.as_ref().unwrap().inserts[0].low_latency_bypass);
    assert!(app.mixer.devices.cards.iter().any(|c| c.low_latency_bypass));
    assert!(app.arrange.layout.transport.contains_key("low_latency"));
    assert!(app.arrange.bar_menu.iter().any(|item| item.label == "Options"));
    assert_eq!(app.diagnostics_report(now).path_latency.as_deref(), Some("Path latency: 1.5 ms"));
    app.player.as_mut().unwrap().output_latency = None;
    let unknown = app.run("query.path_latency", json!({})).unwrap();
    assert_eq!(unknown["engine"]["frames"], 0.0);
    assert!(unknown["device"].is_null() && unknown["total"].is_null());
    assert_eq!(app.engine.path_latency.text(), "Path latency: unknown (device output latency unavailable)");
    app.player.as_mut().unwrap().output_latency = Some(std::time::Duration::from_micros(1500));
    app.status = "later message".into();
    for _ in 0..3 {
        step(&mut app);
    }
    assert_eq!(app.status, "later message");
    app.run("options.low_latency", json!({"limit_ms":5.0})).unwrap();
    for _ in 0..8 {
        step(&mut app);
    }
    assert!(!app.mode_bypassed(uid));
    assert_eq!(app.status, "later message", "limit refresh does not repeat toggle text");
    app.run("options.delay_compensation", json!({"value":false})).unwrap();
    for _ in 0..8 {
        step(&mut app);
    }
    let unaligned = app.run("query.path_latency", json!({})).unwrap();
    assert_eq!(unaligned["available"], false);
    assert!(unaligned["engine"].is_null());
    assert_eq!(app.engine.path_latency.text(), "Path latency: unavailable (Delay Compensation off; paths not aligned)");
    app.run("options.low_latency", json!({"value":false})).unwrap();
    for _ in 0..8 {
        step(&mut app);
    }
    assert_eq!(app.status, "Low Latency off");
}

#[test]
fn invalid_preferences_commands_and_engine_only_host_cannot_edit_the_session() {
    let mut app = crate::tests::app();
    let before = app.engine.session().to_json().unwrap();
    for bad in [json!({"limit_ms":-1}), json!({"limit_ms":"fast"}), json!({"value":3})] {
        assert!(app.run("options.low_latency", bad).is_err());
    }
    assert!(!app.ui.low_latency);
    assert_eq!(app.ui.low_latency_limit_ms, 0.0);
    app.run("options.low_latency", json!({"limit_ms":5})).unwrap();
    assert!(!app.ui.low_latency);
    app.run("options.low_latency", json!({})).unwrap();
    assert!(app.ui.low_latency);
    assert_eq!(app.status, "Low Latency on: 0 inserts bypassed, 0.0 ms saved");
    assert_eq!(app.engine.session().to_json().unwrap(), before);
    assert!(app.engine.execute("options.low_latency", &json!({})).is_err());
    for value in [json!(-1), json!("invalid"), Value::Null] {
        let ui: UiState = serde_json::from_value(json!({"low_latency_limit_ms":value})).unwrap();
        assert_eq!(ui.low_latency_limit_ms, 0.0);
    }
    let ui: UiState = serde_json::from_str("{}").unwrap();
    assert!(!ui.low_latency);
    assert_eq!(ui.low_latency_limit_ms, 0.0);
    assert_eq!(app.run("query.path_latency", json!({})).unwrap()["total"], Value::Null);
}

/// The zap's and the metronome's rects with `pending` and the preference as given. Without a
/// player the poll clears `pending` every frame, so the buttons are drawn directly.
fn rects(on: bool, pending: bool) -> ([f32; 4], [f32; 4]) {
    let mut app = crate::tests::app();
    app.ui.low_latency = on;
    app.low_latency.pending = pending;
    let ctx = egui::Context::default();
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        ui.horizontal(|ui| {
            crate::metronome::button(&mut app, ui);
            app.low_latency_button(ui);
        });
    });
    out.textures_delta.clear();
    (app.arrange.layout.transport["low_latency"], app.arrange.layout.transport["metronome"])
}

/// The control is the size of the transport's other buttons and the same in every state. (Mutation
/// check: the old label swap without `min_size` fails the equal-width line.)
#[test]
fn the_low_latency_button_matches_the_metronome_in_off_on_and_pending() {
    let size = |r: [f32; 4]| (r[2] - r[0], r[3] - r[1]);
    for (on, pending) in [(false, false), (true, false), (true, true)] {
        let (zap, metronome) = rects(on, pending);
        assert_eq!(size(zap), size(metronome), "on {on} pending {pending}");
    }
    let (off, _) = rects(false, false);
    assert_eq!(off[0], rects(true, true).0[0]);
}

#[test]
fn the_title_says_pending_only_while_pending() {
    let mut app = crate::tests::app();
    assert_eq!(app.low_latency_title(), "Low Latency");
    app.low_latency.pending = true;
    assert_eq!(app.low_latency_title(), "Low Latency (pending)");
}

#[test]
fn the_button_is_text_off_lit_on_and_dimmed_lit_while_pending() {
    use crate::theme::TOKENS;
    use crate::tokens::alpha::VOICE_RELEASING;
    assert_eq!(look(false, false, false).1, TOKENS.text);
    assert_eq!(look(true, false, false).1, TOKENS.signal);
    assert_eq!(look(true, true, false).1, TOKENS.signal.gamma_multiply(VOICE_RELEASING));
    assert_ne!(look(true, true, false).1, look(true, false, false).1);
}

#[test]
fn the_button_carries_the_zap_icon_and_a_click_toggles() {
    let mut app = crate::tests::app();
    let ctx = egui::Context::default();
    let mut now = 0.0;
    crate::tests::frames(&mut app, &ctx, 3, &mut now);
    assert_eq!(app.arrange.layout.transport_icons["low_latency"], "zap");
    let at = crate::tests::mid(app.arrange.layout.transport["low_latency"]);
    crate::tests::click(&mut app, at);
    crate::tests::frames(&mut app, &ctx, 3, &mut now);
    assert!(app.ui.low_latency);
}
