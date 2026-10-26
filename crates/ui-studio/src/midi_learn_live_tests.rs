#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn end to end (ADR 0028): the real app frame loop over the real Player and ingress
//! worker, on the fake MIDI backend. A mixer fader is learned from a controller knob, then the
//! knob moves the fader: the port stays open after the learn ends, the worker gets the mapping, and
//! Main applies what it publishes. Both with the port unrouted and routed to an armed track.
use crate::midi_learn::Entry;
use crate::{Services, StudioApp};
use serde_json::json;
use soundcraft_model::{Control, TrackId};
use std::time::{Duration, Instant};
use transit_midi_io::fake::{Fake, FakeInputPort};

const PORT: &str = "KeyStep Pro MIDI IN";

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl T {
    fn new(fake: &Fake) -> T {
        let engine = soundcraft_engine::Engine::default();
        let mut player = soundcraft_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
        player.enable_midi_for_test(fake.io());
        let app = StudioApp::new(engine, Some(player), Services::default());
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
        t
    }

    /// One frame of the app as the host runs it (logic, then the panes), a little apart in real
    /// time so the ingress worker and the MIDI thread can run.
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                ..Default::default()
            };
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Frames until `ready`, sending `cc` (CC 74 on channel 1) before each when given.
    fn until(&mut self, what: &str, port: &FakeInputPort, cc: Option<u8>, mut ready: impl FnMut(&StudioApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(&self.app) {
            assert!(Instant::now() < deadline, "did not reach: {what} (status: {})", self.app.status);
            if let Some(v) = cc {
                port.deliver(0, &[0xB0, 74, v]);
            }
            self.frames(1);
        }
    }
}

fn learn_then_move(routed: bool) {
    let fake = Fake::default();
    let key = fake.add_input(PORT);
    let port = fake.input_port(&key).unwrap();
    let mut t = T::new(&fake);
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tr = TrackId(r["tracks"][0].as_u64().unwrap());
    if routed {
        // The usual setup: the controller also plays the armed track (its input: all ports).
        t.app.run("mix.record_arm", json!({"tracks": [tr.0], "value": true})).unwrap();
    }
    t.frames(4);
    t.app.learn_choose(Entry::Learn, tr, Control::Volume, None).unwrap();
    t.until("the learn is armed and the port open", &port, None, |app| {
        let p = app.player.as_ref().unwrap();
        p.midi_learn_armed() && p.midi_status().is_ok_and(|s| s.open_inputs == 1)
    });
    t.until("the knob is learned", &port, Some(10), |app| !app.engine.session().midi_mappings.is_empty());
    assert!(t.app.engine.learn().is_none());
    let m = t.app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.source.port.as_str(), m.source.number), (PORT, 74));
    t.frames(4);
    assert_eq!(t.app.player.as_ref().unwrap().midi_status().unwrap().open_inputs, 1, "the mapped port stays open after the learn");
    // Scaled (the default): the knob sweeps up from the bottom and the fader meets it at the top.
    for v in (0..=127u8).step_by(8) {
        port.deliver(0, &[0xB0, 74, v]);
        t.frames(2);
    }
    let top = crate::mixer::k_to_db(1.0);
    t.until("the fader follows the knob to the top", &port, Some(127), |app| {
        (app.engine.session().track(tr).unwrap().mixer.volume_db - top).abs() < 1e-3
    });
}

#[test]
fn a_learned_knob_moves_the_fader_on_an_unrouted_port() {
    learn_then_move(false);
}

#[test]
fn a_learned_knob_moves_the_fader_on_a_port_routed_to_an_armed_track() {
    learn_then_move(true);
}

#[test]
fn a_knob_pickup_holds_back_is_reported_once_and_the_fader_stays() {
    let fake = Fake::default();
    let key = fake.add_input(PORT);
    let port = fake.input_port(&key).unwrap();
    let mut t = T::new(&fake);
    t.app.ui.midi_takeover = soundcraft_model::MapMode::Pickup;
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let tr = TrackId(r["tracks"][0].as_u64().unwrap());
    t.frames(4);
    t.app.learn_choose(Entry::Learn, tr, Control::Volume, None).unwrap();
    t.until("the learn is armed", &port, None, |app| app.player.as_ref().unwrap().midi_learn_armed());
    t.until("the knob is learned", &port, Some(10), |app| !app.engine.session().midi_mappings.is_empty());
    let before = t.app.engine.session().track(tr).unwrap().mixer.volume_db;
    // The fader sits at 0 dB (about 70 % of its travel); the knob wiggles near the bottom, as a
    // knob left there (or a relative encoder around its center) does.
    t.until("pickup says why nothing moves", &port, Some(12), |app| app.status.contains("waits for pickup"));
    assert!(t.app.status.contains(&format!("{PORT} ch 1 CC 74 at 9%")), "{}", t.app.status);
    for v in [10, 12, 10, 12] {
        port.deliver(0, &[0xB0, 74, v]);
        t.frames(2);
    }
    assert_eq!(t.app.engine.session().track(tr).unwrap().mixer.volume_db, before, "pickup holds the knob back");
}
