//! Issue #213: the unresponsive-plug-in marker in the mixer insert row, the device card and the
//! arrange track header, with one text and a hover that names the cause. A real device-free Player
//! whose hosted plug-in is made on a real plug-in host thread; the stuck call is a job that blocks
//! on a channel the test releases (E49). The worker cause is faked through the helper's test seam.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::devices::UNRESPONSIVE;
use crate::unresponsive::{FAKE, Hang, tip};
use crate::{Services, StudioApp};
use serde_json::json;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use transit_model::{Insert, TrackId};
use transit_playback::{PluginTestHooks, StuckCall};
use transit_plugin_host::{Config, Host};

const HOSTED: &str = "clap:transit.test.marker213";

/// A built-in synth behind a third-party id, with a state of its own (as a CLAP or VST3 has), so
/// the host proxy gives it a state reader.
struct Stateful(Box<dyn transit_dsp::Plugin>);

impl transit_dsp::Plugin for Stateful {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.0.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.0.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.0.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.0.process(io, frames);
    }
    fn state_reader(&mut self) -> Option<Box<dyn transit_dsp::PluginStateReader>> {
        Some(Box::new(|| Some(vec![1])))
    }
}

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    host: Host,
    release: Option<mpsc::Sender<()>>,
}

impl Drop for T {
    fn drop(&mut self) {
        self.release.take();
        FAKE.with(|f| f.set(None));
    }
}

impl T {
    fn new() -> T {
        let engine = transit_engine::Engine::default();
        let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
        let timeout = Duration::from_millis(200);
        let host = Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() });
        player.set_plugin_test_hooks(PluginTestHooks {
            host: host.clone(),
            factory: Arc::new(|spec: &transit_mix::InstanceSpec| {
                transit_mix::create_instance("macro_oscillator", None, 48_000.0, 512, spec.ch)
                    .map(|n| transit_mix::NewInstance { plugin: Box::new(Stateful(n.plugin)), ..n })
            }),
            deferred: Arc::new(|id: &str| id == HOSTED),
            clock: Arc::new(Instant::now),
            slow_after: Duration::from_secs(10),
        });
        let mut app = StudioApp::new(engine, Some(player), Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, host, release: None };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.app.player.as_ref().unwrap().step_device_free();
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
            // timing: stall-bound frame pacing while the host thread is held by its channel
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn frames_until(&mut self, limit: Duration, done: impl Fn(&T) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done(self) {
                return true;
            }
            self.frames(1);
        }
        done(self)
    }

    /// An audio track, focused, with the hosted plug-in in insert 0 and, if asked, loaded.
    fn fx_track(&mut self) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": "audio"})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(t);
        self.app.engine.session_mut().track_mut(t).unwrap().mixer.inserts[0] = Some(Insert::new(HOSTED));
        self.frames(3);
        t
    }

    fn row(&self, t: TrackId) -> crate::mixer::InsertRow {
        let strip = self.app.mixer.layout.strips.iter().find(|s| s.track == t.0).expect("a strip");
        strip.inserts.iter().find(|r| r.slot == 0).cloned().expect("an insert row")
    }

    fn card(&self) -> crate::mixer::DeviceCard {
        self.app.mixer.devices.cards.iter().find(|c| c.slot == Some(0)).cloned().expect("the insert's card")
    }

    fn header(&self, t: TrackId) -> crate::arrange::TrackRow {
        self.app.arrange.layout.tracks.iter().find(|r| r.id == t.0).cloned().expect("a track row")
    }

    fn marked(&self, t: TrackId) -> [bool; 3] {
        [self.row(t).status == UNRESPONSIVE, self.card().status == UNRESPONSIVE, self.header(t).unresponsive == UNRESPONSIVE]
    }
}

#[test]
fn a_responsive_plug_in_shows_no_marker() {
    let mut t = T::new();
    let track = t.fx_track();
    FAKE.with(|f| f.set(None));
    t.frames(5);
    assert_eq!(t.marked(track), [false; 3]);
    assert_eq!(t.header(track).unresponsive, "");
    assert_eq!(t.header(track).unresponsive_rect, [0.0; 4]);
}

#[test]
fn a_hung_worker_marks_the_row_the_card_and_the_track_header() {
    let mut t = T::new();
    let track = t.fx_track();
    let other = t.fx_track();
    t.app.focus(track);
    FAKE.with(|f| f.set(Some((track, Hang::Worker))));
    t.frames(5);
    assert_eq!(t.marked(track), [true; 3], "the same text in all three places");
    assert_ne!(t.header(track).unresponsive_rect, [0.0; 4]);
    assert_eq!(t.row(other).status, "", "another track's plug-in shows none");
    assert_eq!(t.header(other).unresponsive, "");
}

#[test]
fn the_hover_text_names_the_cause_and_what_to_do() {
    let call = |given_up| StuckCall {
        thread: "plugin-host-0".into(),
        plugin: "Guitar Rig 7".into(),
        call: "a window message",
        stuck_for: Duration::from_secs(130),
        given_up,
        hosted: 2,
    };
    let host = tip(Hang::HostThread, "Guitar Rig 7", &[call(false)]);
    assert!(
        host.starts_with("Guitar Rig 7 is unresponsive: its plug-in host thread has not returned from a window message in Guitar Rig 7 for 130 s"),
        "{host}"
    );
    assert!(host.contains("Audio keeps playing") && host.contains("Later or Give up"), "{host}");
    let gave = tip(Hang::HostThread, "Guitar Rig 7", &[call(true)]);
    assert!(gave.contains("you gave up") && !gave.contains("Later"), "after Give up the window is gone: {gave}");
    // Another plug-in's stuck call, or none: no call and no choice is named.
    for other in [tip(Hang::HostThread, "Amp", &[call(false)]), tip(Hang::HostThread, "Amp", &[call(true)]), tip(Hang::HostThread, "Amp", &[])] {
        assert_eq!(other, "Amp is unresponsive on its host thread.");
    }
    let worker = tip(Hang::Worker, "Amp", &[]);
    assert!(worker.contains("inside a plug-in on this track") && worker.contains("silent") && worker.contains("Reconnect"), "{worker}");
    assert!(!worker.contains("is unresponsive: a render worker is stuck inside the plug-in"), "no single plug-in is accused: {worker}");
}

/// The real state: a host thread stuck past its budget inside a call (#205) marks the loaded
/// plug-in's row, card and header; once the call returns the marker goes.
#[test]
fn a_stuck_host_thread_marks_its_loaded_plug_in_and_clears_when_it_returns() {
    let mut t = T::new();
    let track = t.fx_track();
    assert!(t.frames_until(Duration::from_secs(10), |t| t.app.player.as_ref().unwrap().live_name(track, 0).is_some()), "the hosted plug-in is live");
    assert_eq!(t.marked(track), [false; 3]);
    let (release, gate) = mpsc::channel::<()>();
    t.release = Some(release);
    let _hung = t.host.submit(move |ctx| {
        ctx.name_job("Guitar Rig 7");
        let _ = gate.recv();
    });
    assert!(t.frames_until(Duration::from_secs(5), |t| t.marked(track) == [true; 3]), "marked once the call is past its budget");
    let tip = t.app.hang_tip(Hang::HostThread, "Guitar Rig 7");
    assert!(tip.contains("in Guitar Rig 7 for") && tip.contains("Later or Give up"), "the live call is named: {tip}");
    assert_eq!(t.app.hang_tip(Hang::HostThread, "Amp"), "Amp is unresponsive on its host thread.", "another plug-in's call is not named");
    t.release.take();
    assert!(t.frames_until(Duration::from_secs(5), |t| t.marked(track) == [false; 3]), "cleared once the call returns");
}
