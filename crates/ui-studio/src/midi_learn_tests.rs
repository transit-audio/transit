#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn in the studio (ADR 0028): the control menu, the learn state and marker, applying
//! what the controller did, the profile in the preferences, and Window > MIDI Mappings. No
//! device: the worker's output is fed in as the `MapMsg`s it would publish.
use super::*;
use crate::midi_learn::{self, Action, Entry, Info};
use soundcraft_model::{Control, MapMode, Scope};
use soundcraft_playback::MapMsg;

fn new_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Gtr"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

fn frames(app: &mut StudioApp, ctx: &egui::Context, n: usize, t: &mut f64) {
    for _ in 0..n {
        *t += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
            time: Some(*t),
            ..Default::default()
        };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

fn press(app: &mut StudioApp, method: &str, params: Value) {
    let (req, _rx) = crate::ControlRequest::new(method, params);
    let _ = crate::control::handle_for_test(app, &egui::Context::default(), &req);
}

fn click(app: &mut StudioApp, at: (f32, f32), button: &str) {
    press(app, "ui.click", json!({"x": at.0, "y": at.1, "button": button}));
}

fn strip(app: &StudioApp, t: TrackId) -> crate::mixer::StripLayout {
    app.mixer.layout.strips.iter().find(|s| s.track == t.0).cloned().unwrap()
}

fn entries(app: &StudioApp) -> Vec<String> {
    app.midi_learn.menu.iter().map(|m| m.label.clone()).collect()
}

fn candidate(app: &mut StudioApp, port: &str, cc: u8) {
    let msgs = vec![MapMsg::Candidate { port: port.into(), channel: 0, kind: SourceKind::Cc, number: cc }];
    app.midi_learn_apply((0, msgs), 0.0);
}

use soundcraft_model::SourceKind;

#[test]
fn the_menu_state_machine_offers_learn_forget_and_cancel() {
    let mapped = Info { scope: Scope::Session, text: "KeyStep ch 1 CC 74".into(), overridden: false };
    assert_eq!(midi_learn::menu_entries(None, false), vec![Entry::Learn, Entry::LearnProfile]);
    assert_eq!(midi_learn::menu_entries(Some(&mapped), false), vec![Entry::Mapped, Entry::Learn, Entry::Forget]);
    assert_eq!(midi_learn::menu_entries(Some(&mapped), true), vec![Entry::Cancel]);
    assert_eq!(midi_learn::menu_entries(None, true), vec![Entry::Cancel]);
}

#[test]
fn right_click_learn_waits_for_a_message_then_maps_it_and_the_menu_shows_the_mapping() {
    let (mut app, ctx, mut time, t) = new_app();
    let fader = mid(strip(&app, t).fader);
    click(&mut app, fader, "right");
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(entries(&app), ["MIDI Learn", "MIDI Learn for this controller (all sessions)"]);
    let learn = app.midi_learn.menu.iter().find(|m| m.label == "MIDI Learn").unwrap().rect;
    click(&mut app, mid(learn), "left");
    frames(&mut app, &ctx, 4, &mut time);
    let arm = app.engine.learn().expect("armed");
    assert_eq!(arm.target.to_string(), format!("track:{}:volume", t.0));
    // The control shows the learn state.
    let layout = app.inspect(&ctx)["midi_learn"]["markers"].clone();
    assert!(layout.as_array().unwrap().iter().any(|m| m["learning"] == true && m["control"] == "Volume"), "{layout}");
    // The next message maps it.
    candidate(&mut app, "KeyStep", 74);
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.engine.learn().is_none());
    let m = app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.source.port.as_str(), m.source.channel, m.source.number, m.mode), ("KeyStep", Some(0), 74, MapMode::Pickup));
    assert!(app.status.starts_with("Mapped Volume to KeyStep ch 1 CC 74"), "{}", app.status);
    // The marker is drawn, and a right-click now shows the mapping and Forget.
    let markers = app.inspect(&ctx)["midi_learn"]["markers"].clone();
    assert!(markers.as_array().unwrap().iter().any(|m| m["mapped"] == true && m["control"] == "Volume" && m["learning"] == false), "{markers}");
    let fader = mid(strip(&app, t).fader);
    click(&mut app, fader, "right");
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(entries(&app), ["Mapped: KeyStep ch 1 CC 74", "MIDI Learn", "Forget MIDI Mapping"]);
    let forget = app.midi_learn.menu.iter().find(|m| m.label == "Forget MIDI Mapping").unwrap().rect;
    click(&mut app, mid(forget), "left");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().midi_mappings.is_empty(), "forgotten");
    assert!(app.engine.undo(), "and the forgetting is an undo step");
    assert_eq!(app.engine.session().midi_mappings.len(), 1);
}

#[test]
fn escape_cancels_an_armed_learn_and_leaves_other_escapes_alone() {
    let (mut app, ctx, mut time, t) = new_app();
    press(&mut app, "ui.key", json!({"key": "Escape"}));
    frames(&mut app, &ctx, 2, &mut time);
    app.run("midi.learn", json!({"target": format!("track:{}:mute", t.0)})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert!(app.engine.learn().is_some());
    press(&mut app, "ui.key", json!({"key": "Escape"}));
    frames(&mut app, &ctx, 2, &mut time);
    assert!(app.engine.learn().is_none(), "Escape cancels");
    assert!(app.engine.session().midi_mappings.is_empty());
}

#[test]
fn buttons_and_the_pan_knob_have_the_menu_too() {
    let (mut app, ctx, mut time, t) = new_app();
    for (rect, control) in [(strip(&app, t).mute, "mute"), (strip(&app, t).solo, "solo"), (strip(&app, t).arm, "arm"), (strip(&app, t).knob, "pan")] {
        click(&mut app, mid(rect), "right");
        frames(&mut app, &ctx, 4, &mut time);
        let learn = app.midi_learn.menu.iter().find(|m| m.label == "MIDI Learn").cloned().unwrap_or_else(|| panic!("{control} has no menu"));
        click(&mut app, mid(learn.rect), "left");
        frames(&mut app, &ctx, 4, &mut time);
        assert_eq!(app.engine.learn().unwrap().target.to_string(), format!("track:{}:{control}", t.0));
        app.run("midi.learn_cancel", json!({})).unwrap();
        frames(&mut app, &ctx, 2, &mut time);
    }
}

fn map(app: &mut StudioApp, t: TrackId, control: &str, cc: u8, extra: Value) {
    let mut p = json!({"source": {"port": "KeyStep", "channel": 1, "kind": "cc", "number": cc}, "target": format!("track:{}:{control}", t.0)});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    app.run("midi.map", p).unwrap();
}

#[test]
fn a_mapped_sweep_moves_the_fader_as_one_undo_step_and_ends_when_the_controller_is_still() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let steps = app.engine.undo_history().len();
    let before = app.engine.session().track(t).unwrap().mixer.volume_db;
    for (i, v) in [0.2f32, 0.4, 0.6, 0.8].into_iter().enumerate() {
        app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: v, seq: 1 }]), 10.0 + i as f64 * 0.05);
    }
    let after = app.engine.session().track(t).unwrap().mixer.volume_db;
    assert!((after - crate::mixer::k_to_db(0.8)).abs() < 1e-3 && after != before, "{after}");
    // The sweep is one step, and a pause shorter than the gesture window keeps it going.
    app.midi_learn_feed(10.2);
    app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: 0.5, seq: 1 }]), 10.3);
    assert_eq!(app.engine.undo_history().len() - steps, 1, "still the first sweep's step");
    // A still controller ends it, and the next sweep is another step.
    app.midi_learn_feed(10.8);
    app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: 0.3, seq: 1 }]), 11.0);
    app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: 0.2, seq: 1 }]), 11.1);
    assert_eq!(app.engine.undo_history().len() - steps, 2, "two sweeps, two steps");
}

#[test]
fn undoing_a_sweep_restores_the_fader_in_one_step() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let before = app.engine.session().track(t).unwrap().mixer.volume_db;
    let depth = app.engine.undo_history().len();
    for (i, v) in [0.2f32, 0.4, 0.6, 0.8].into_iter().enumerate() {
        app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: v, seq: 1 }]), 10.0 + i as f64 * 0.05);
    }
    assert_eq!(app.engine.undo_history().len(), depth + 1, "four moves, one step");
    assert!(app.engine.undo());
    assert_eq!(app.engine.session().track(t).unwrap().mixer.volume_db, before);
}

#[test]
fn a_stale_epoch_and_a_move_with_no_mapping_do_nothing() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let before = app.engine.session().track(t).unwrap().mixer.volume_db;
    app.midi_learn_apply((3, vec![MapMsg::Move { epoch: 2, index: 0, value: 0.1, seq: 1 }]), 1.0);
    app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 9, value: 0.1, seq: 1 }]), 1.0);
    assert_eq!(app.engine.session().track(t).unwrap().mixer.volume_db, before);
}

#[test]
fn a_note_press_toggles_mute_and_a_cc_pan_maps_the_balance() {
    let (mut app, ctx, mut time, t) = new_app();
    app.run("midi.map", json!({"source": {"port": "KeyStep", "channel": 1, "kind": "note", "number": 36}, "target": format!("track:{}:mute", t.0)}))
        .unwrap();
    map(&mut app, t, "pan", 10, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let mapped = |app: &StudioApp| app.midi_learn.resolved_len();
    assert_eq!(mapped(&app), 2);
    app.midi_learn_apply((0, vec![MapMsg::Press { epoch: 0, index: 0, set: None }]), 1.0);
    assert!(app.engine.session().track(t).unwrap().mixer.mute);
    app.midi_learn_apply((0, vec![MapMsg::Press { epoch: 0, index: 0, set: None }]), 1.1);
    assert!(!app.engine.session().track(t).unwrap().mixer.mute, "the second press flips it back");
    app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 1, value: 1.0, seq: 1 }]), 1.2);
    assert!(app.engine.session().track(t).unwrap().mixer.pan.iter().all(|p| *p > 0.99), "knob at 127 is hard right");
}

#[test]
fn the_session_mapping_overrides_the_profile_and_the_menu_says_so() {
    let (mut app, ctx, mut time, t) = new_app();
    app.run(
        "midi.map",
        json!({"source": {"port": "KeyStep", "channel": 1, "kind": "cc", "number": 74}, "target": "strip:1:volume", "scope": "profile"}),
    )
    .unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert_eq!(midi_learn::info(&app, t, &Control::Volume).unwrap().scope, Scope::Profile);
    assert_eq!(app.midi_learn.resolved_len(), 1);
    map(&mut app, t, "pan", 74, json!({}));
    frames(&mut app, &ctx, 2, &mut time);
    let v = midi_learn::info(&app, t, &Control::Volume).unwrap();
    assert!(v.overridden, "CC 74 now belongs to the session's pan");
    assert_eq!(app.midi_learn.resolved_len(), 1, "only the session's mapping is in force");
    let rows = midi_learn::rows(&app);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].scope, rows[0].overridden), (Scope::Session, false));
    assert_eq!((rows[1].scope, rows[1].overridden), (Scope::Profile, true));
}

#[test]
fn the_profile_is_kept_in_the_preferences_and_old_preferences_load_without_it() {
    let (mut app, ctx, mut time, _t) = new_app();
    app.run(
        "midi.map",
        json!({"source": {"port": "KeyStep", "channel": 2, "kind": "cc", "number": 21}, "target": "strip:1:volume", "scope": "profile"}),
    )
    .unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert_eq!(app.ui.midi_profile.len(), 1, "the engine's profile is copied into the preferences");
    let saved = serde_json::to_value(&app.ui).unwrap();
    // A fresh run: the preferences' profile goes to the engine on the first frame.
    let ui: UiState = serde_json::from_value(saved).unwrap();
    let mut fresh = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    fresh.ui = ui;
    let ctx = egui::Context::default();
    let mut t2 = 0.0;
    frames(&mut fresh, &ctx, 2, &mut t2);
    assert_eq!(fresh.engine.midi_profile().len(), 1);
    let mut old = serde_json::to_value(UiState::default()).unwrap();
    old.as_object_mut().unwrap().remove("midi_profile");
    old.as_object_mut().unwrap().remove("midi_mappings");
    let loaded: UiState = serde_json::from_value(old).unwrap();
    assert!(loaded.midi_profile.is_empty() && !loaded.midi_mappings.open);
}

#[test]
fn the_mappings_window_lists_edits_relearns_and_deletes_rows() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"range": [0.1, 0.9]}));
    app.run(
        "midi.map",
        json!({"source": {"port": "Pads", "channel": 10, "kind": "note", "number": 36}, "target": "strip:1:mute", "scope": "profile"}),
    )
    .unwrap();
    assert_eq!(app.run("window.midi_mappings", json!({})).unwrap()["open"], true);
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.inspect(&ctx)["midi_learn"]["window"][2].as_f64().unwrap() > 100.0, "the window is drawn");
    let rows = midi_learn::rows(&app);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].port.as_str(), rows[0].channel.as_str(), rows[0].source.as_str(), rows[0].target.as_str(), rows[0].mode, rows[0].range),
        ("KeyStep", "1", "CC 7", "Gtr: Volume", MapMode::Pickup, [0.1, 0.9])
    );
    assert_eq!(
        (rows[1].port.as_str(), rows[1].channel.as_str(), rows[1].source.as_str(), rows[1].target.as_str()),
        ("Pads", "10", "Note C1 (36)", "Gtr: Mute")
    );
    assert_eq!(rows[1].scope, Scope::Profile);
    // Mode and range edits are map commands (one undo step each in the session).
    let steps = app.engine.undo_pushes();
    app.mapping_row_action(&rows[0], Action::Mode(MapMode::Jump)).unwrap();
    app.mapping_row_action(&rows[0], Action::Range(0.0, 0.5)).unwrap();
    let m = app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.mode, m.min, m.max), (MapMode::Jump, 0.0, 0.5));
    assert_eq!(app.engine.undo_pushes(), steps + 2);
    // Learn re-learns the row: the new source keeps the mode and range.
    let rows = midi_learn::rows(&app);
    app.mapping_row_action(&rows[0], Action::Learn).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert!(midi_learn::rows(&app)[0].learning);
    candidate(&mut app, "KeyStep", 9);
    let m = app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.source.number, m.mode, m.max), (9, MapMode::Jump, 0.5));
    // Delete.
    let rows = midi_learn::rows(&app);
    app.mapping_row_action(&rows[1], Action::Delete).unwrap();
    app.mapping_row_action(&rows[0], Action::Delete).unwrap();
    assert!(midi_learn::rows(&app).is_empty());
    assert!(app.engine.midi_profile().is_empty() && app.engine.session().midi_mappings.is_empty());
}

#[test]
fn window_mappings_is_in_the_window_menu_and_the_markers_follow_the_strip() {
    let (mut app, ctx, mut time, t) = new_app();
    let window = app.arrange.bar_menu.iter().find(|m| m.label == "Window").cloned().expect("the Window menu is in the bar");
    click(&mut app, mid(window.rect), "left");
    frames(&mut app, &ctx, 4, &mut time);
    let item = app.arrange.bar_menu.iter().find(|m| m.label == "MIDI Mappings").cloned().expect("Window > MIDI Mappings");
    click(&mut app, mid(item.rect), "left");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.ui.midi_mappings.open, "the menu item opens the window");
    map(&mut app, t, "volume", 7, json!({}));
    map(&mut app, t, "mute", 8, json!({}));
    frames(&mut app, &ctx, 3, &mut time);
    let markers = app.inspect(&ctx)["midi_learn"]["markers"].clone();
    let fader = strip(&app, t).fader;
    let m = markers.as_array().unwrap().iter().find(|m| m["control"] == "Volume").unwrap();
    let r: Vec<f64> = m["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!(r[1] <= f64::from(fader[1]) + 1.0 && r[3] >= f64::from(fader[3]) - 1.0, "the marker belongs to the fader: {r:?} {fader:?}");
    assert!(markers.as_array().unwrap().iter().any(|m| m["control"] == "Mute" && m["mapped"] == true));
    assert!(!markers.as_array().unwrap().iter().any(|m| m["control"] == "Solo"), "unmapped controls draw nothing");
}

// ---- review fixes: one gesture, a live UI, stale state ----

fn mv(i: u32, value: f32, seq: u32) -> MapMsg {
    MapMsg::Move { epoch: 0, index: i, value, seq }
}

#[test]
fn two_knobs_moved_together_are_one_undo_step() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    map(&mut app, t, "pan", 10, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let depth = app.engine.undo_history().len();
    let before = app.engine.session().track(t).unwrap().clone();
    for i in 0..10u32 {
        let now = 20.0 + f64::from(i) * 0.02;
        // The mappings are in resolve order: session first, in the order they were made.
        app.midi_learn_apply((0, vec![mv(0, 0.1 * i as f32, i + 1)]), now);
        app.midi_learn_apply((0, vec![mv(1, 1.0 - 0.1 * i as f32, i + 1)]), now);
    }
    assert_eq!(app.engine.undo_history().len(), depth + 1, "an XY move is one step, not one per control");
    assert!(app.engine.undo());
    let after = app.engine.session().track(t).unwrap();
    assert_eq!((after.mixer.volume_db, &after.mixer.pan), (before.mixer.volume_db, &before.mixer.pan), "both controls come back");
}

#[test]
fn a_two_second_sweep_applied_at_sixty_hertz_is_one_undo_step_ending_at_the_last_value() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 2, &mut time);
    let depth = app.engine.undo_history().len();
    let mut last = 0.0;
    for frame in 0..120u32 {
        let now = 30.0 + f64::from(frame) / 60.0;
        last = frame as f32 / 120.0;
        app.midi_learn_apply((0, vec![mv(0, last, frame + 1)]), now);
        app.midi_learn_feed(now);
    }
    assert_eq!(app.engine.undo_history().len(), depth + 1, "one sweep, one step");
    let db = app.engine.session().track(t).unwrap().mixer.volume_db;
    assert!((db - crate::mixer::k_to_db(last)).abs() < 1e-3, "the fader ends at the knob's last value: {db}");
}

#[test]
fn the_ui_polls_lightly_when_mapped_and_quiet_and_at_full_rate_only_on_news() {
    // `ui()` turns the feed's answer into a repaint request: none without mappings, a light poll
    // while mapped and quiet, the normal rate when the worker published something.
    let (mut app, ctx, mut time, t) = new_app();
    app.player = Some(soundcraft_playback::Player::prepare_device_free(std::sync::Arc::new(app.engine.session().clone())).unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.midi_learn_feed(time), None, "nothing mapped, nothing waiting: the UI may idle");
    map(&mut app, t, "volume", 7, json!({"mode": "jump"}));
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::IDLE_POLL), "mapped and quiet: no redraw storm");
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::IDLE_POLL));
    app.midi_learn.pretend_news();
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::BUSY_POLL), "the worker published: look again at once");
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::IDLE_POLL), "and back to the light poll when nothing more comes");
    app.run("midi.learn", json!({"target": format!("track:{}:mute", t.0)})).unwrap();
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::BUSY_POLL), "a waiting learn polls at the normal rate");
    app.run("midi.learn_cancel", json!({})).unwrap();
    assert_eq!(app.midi_learn_feed(time), Some(midi_learn::IDLE_POLL));
}

#[test]
fn a_candidate_after_the_session_was_replaced_maps_nothing() {
    let (mut app, ctx, mut time, t) = new_app();
    app.run("midi.learn", json!({"target": format!("track:{}:volume", t.0)})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    app.engine.replace_session(soundcraft_model::Session::default());
    frames(&mut app, &ctx, 2, &mut time);
    candidate(&mut app, "KeyStep", 74);
    assert!(app.engine.session().midi_mappings.is_empty() && app.engine.midi_profile().is_empty());
}

#[test]
fn an_audio_reconnect_rearms_the_new_worker_and_resets_pickup_counters() {
    let (mut app, ctx, mut time, t) = new_app();
    let new_player = |app: &StudioApp| soundcraft_playback::Player::prepare_device_free(std::sync::Arc::new(app.engine.session().clone())).unwrap();
    app.player = Some(new_player(&app));
    app.run("midi.learn", json!({"target": format!("track:{}:volume", t.0)})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    assert!(app.player.as_ref().unwrap().midi_learn_armed(), "the worker is armed");
    // What a reconnect does: a new Player, whose worker is not armed.
    app.player = Some(new_player(&app));
    assert!(!app.player.as_ref().unwrap().midi_learn_armed());
    frames(&mut app, &ctx, 2, &mut time);
    assert!(app.player.as_ref().unwrap().midi_learn_armed(), "the next frame arms it again");
    // The pickup counters start over with the new worker.
    app.run("midi.map", json!({"source": {"port": "KeyStep", "channel": 1, "kind": "cc", "number": 7}, "target": format!("track:{}:volume", t.0)}))
        .unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    app.midi_learn_apply((app.player.as_ref().unwrap().midi_map_epoch(), vec![mv(0, 0.5, 50)]), time);
    app.player = Some(new_player(&app));
    frames(&mut app, &ctx, 2, &mut time);
    assert!(app.midi_learn.applied_counts().iter().all(|n| *n == 0), "{:?}", app.midi_learn.applied_counts());
}

#[test]
fn pass_through_is_a_column_of_the_window_and_the_candidate_keeps_it() {
    let (mut app, ctx, mut time, t) = new_app();
    map(&mut app, t, "volume", 7, json!({}));
    frames(&mut app, &ctx, 2, &mut time);
    let rows = midi_learn::rows(&app);
    assert!(!rows[0].pass_through, "consumed by default");
    app.mapping_row_action(&rows[0], Action::PassThrough(true)).unwrap();
    assert!(app.engine.session().midi_mappings.iter().next().unwrap().pass_through);
    app.run("midi.learn", json!({"target": format!("track:{}:volume", t.0)})).unwrap();
    candidate(&mut app, "KeyStep", 9);
    let m = app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.source.number, m.pass_through), (9, true), "re-learning keeps the option");
    app.run("window.midi_mappings", json!({"value": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.inspect(&ctx)["midi_learn"]["window"][2].as_f64().unwrap() > 100.0);
}
