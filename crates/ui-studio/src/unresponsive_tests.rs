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
        // The docked pane at its tallest, in a window tall enough for it and the arrangement: the
        // strips are full height and list their insert rows (#326).
        app.ui.mixer_height = 600.0;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, host, release: None };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.app.player.as_ref().unwrap().step_device_free();
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 1300.0))),
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
    // #326, negative control: a card whose plug-in answers is not hatched.
    assert!(t.card().hatch.is_none() && t.card().suspended_note == [0.0; 4]);
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
    // #326: the card cannot take input, so the hatch fills its body and the reason sits on a
    // solid card inside it.
    let card = t.card();
    let hatch = card.hatch.expect("the card's body is hatched");
    assert!(crate::widgets::test_support::inside(card.suspended_note, card.rect) && crate::widgets::test_support::inside(hatch.rect, card.rect));
    assert!(!hatch.lines.is_empty());
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

impl T {
    fn strip(&self, t: TrackId) -> crate::mixer::StripLayout {
        self.app.mixer.layout.strips.iter().find(|s| s.track == t.0).cloned().expect("a strip")
    }
}

/// #326, Strip details (13): on a full-height slot the circle-alert in `meter.hot` takes the power
/// glyph's place, the name drops to `text.muted`, and a click opens the device card.
#[test]
fn an_unresponsive_insert_slot_shows_the_alert_glyph_and_opens_its_card() {
    let mut t = T::new();
    let track = t.fx_track();
    let other = t.fx_track();
    t.app.focus(other);
    FAKE.with(|f| f.set(Some((track, Hang::Worker))));
    t.frames(5);
    assert!(!t.strip(track).short, "full-height strips");
    let row = t.row(track);
    assert!(row.alert, "the alert glyph");
    assert_eq!((row.glyph_color, row.name_color), (crate::theme::DARK.meter_hot.to_array(), crate::theme::DARK.text_muted.to_array()));
    assert_eq!(row.status, UNRESPONSIVE, "the state stays readable");
    // Negative control: the other track's plug-in answers, so it keeps the power glyph in `signal`.
    let fine = t.row(other);
    assert!(!fine.alert && fine.glyph_color == crate::theme::DARK.signal.to_array());
    t.app.ui.show_devices = false;
    t.frames(2);
    let at = crate::tests::mid(t.row(track).name_rect);
    crate::tests::click(&mut t.app, at);
    t.frames(3);
    assert!(t.app.ui.show_devices && t.app.focus_track() == Some(track), "a click opens the device card");
}

/// #326, Strip details (13): the short strip's FX button carries one 10 px circle-alert riding its
/// top-right corner, and its tooltip names every insert that does not answer.
#[test]
fn the_short_fx_button_carries_one_alert_naming_the_inserts_that_do_not_answer() {
    let mut t = T::new();
    t.app.ui.mixer_height = 0.0;
    let track = t.fx_track();
    t.app.engine.session_mut().track_mut(track).unwrap().mixer.inserts[1] = Some(Insert::new(HOSTED));
    t.frames(3);
    assert!(t.strip(track).short, "the docked pane at its minimum shows the short strip");
    // Negative control: nothing hung, no glyph and no tip.
    FAKE.with(|f| f.set(None));
    t.frames(3);
    let s = t.strip(track);
    assert_eq!((s.fx_alert, s.fx_alert_tip.as_str()), ([0.0; 4], ""));
    FAKE.with(|f| f.set(Some((track, Hang::Worker))));
    t.frames(3);
    let s = t.strip(track);
    let (a, b) = (s.fx_alert, s.fx_button);
    assert_eq!((a[2] - a[0], a[3] - a[1]), (10.0, 10.0), "a 10 px glyph");
    assert_eq!((a[2], a[1]), (b[2] + 4.0, b[1] - 4.0), "riding the top-right corner");
    let name = t.app.plugin_name(HOSTED, false);
    assert_eq!(s.fx_alert_tip, format!("{name} and {name} are not responding"), "two: one glyph, the tooltip lists them");
}

#[test]
fn the_not_responding_tip_names_one_or_lists_several() {
    let n = |v: &[&str]| crate::mixer::not_responding_tip(&v.iter().map(ToString::to_string).collect::<Vec<_>>());
    assert_eq!(n(&["Room Reverb"]), "Room Reverb is not responding");
    assert_eq!(n(&["Room Reverb", "Bite"]), "Room Reverb and Bite are not responding");
    assert_eq!(n(&["A", "B", "C"]), "A, B and C are not responding");
    assert_eq!(n(&[]), "", "none: no tip");
}
