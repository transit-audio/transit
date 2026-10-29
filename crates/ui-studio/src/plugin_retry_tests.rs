//! Issue #82 (escape E48): what a hosted plug-in's slot says while it loads, and Retry. A real
//! device-free Player whose hosted instances are made on a real plug-in host thread by a factory
//! the test holds (a heavy instrument on a loaded machine); the display budget runs on a manual
//! clock where the text is checked to the second.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Services, StudioApp};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use transit_model::{Insert, TrackId};
use transit_playback::{Clock, Factory, PluginTestHooks};
use transit_plugin_host::{Config, Host};

/// The id the session names: a CLAP id, so the card reads "CLAP instrument" once it is live and
/// the stored state reaches the factory. The factory makes the built-in synth for it.
const HOSTED: &str = "clap:transit.test.retry82";
const SLOT: usize = transit_mix::INSTRUMENT_SLOT;

#[derive(Default)]
struct Gate {
    calls: AtomicUsize,
    states: Mutex<Vec<Option<Vec<u8>>>>,
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// A factory whose first `hold` creations wait until the gate opens.
fn gated(gate: &Arc<Gate>, hold: usize) -> Factory {
    let gate = Arc::clone(gate);
    Arc::new(move |spec: &transit_mix::InstanceSpec| {
        let n = gate.calls.fetch_add(1, Ordering::SeqCst);
        gate.states.lock().unwrap().push(spec.state.clone());
        if n < hold {
            let mut open = gate.open.lock().unwrap();
            while !*open {
                open = gate.opened.wait(open).unwrap();
            }
        }
        transit_mix::create_instance("subtractive_synth", None, 48_000.0, 512, spec.ch)
    })
}

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    shapes: Vec<egui::epaint::ClippedShape>,
    gate: Arc<Gate>,
}

impl Drop for T {
    fn drop(&mut self) {
        // Held creations return, so their host threads finish.
        self.gate.open();
    }
}

impl T {
    /// An app with a device-free player whose hosted plug-ins (`HOSTED`) are made by a factory
    /// holding its first `hold` creations, on a host with `timeout` and `creation`; the display
    /// budget is `slow_after` on `clock`.
    fn new(timeout: Duration, creation: Duration, clock: Clock, slow_after: Duration, hold: usize) -> T {
        let engine = transit_engine::Engine::default();
        let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
        let gate = Arc::new(Gate::default());
        player.set_plugin_test_hooks(PluginTestHooks {
            host: Host::new(Config { timeout, creation_timeout: creation, inline: false, ..Config::default() }),
            factory: gated(&gate, hold),
            deferred: Arc::new(|id: &str| id == HOSTED),
            clock,
            slow_after,
        });
        let mut app = StudioApp::new(engine, Some(player), Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new(), gate };
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
            self.shapes = out.shapes;
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Frames until `done`, at most `limit`; whether it came true.
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

    fn send(&mut self, id: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(id, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(8);
    }

    fn click(&mut self, r: [f32; 4], right: bool) {
        let (x, y) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        self.send("ui.click", json!({"x": x, "y": y, "button": if right { "right" } else { "left" }}));
    }

    /// A new track of `kind`, focused (its device chain shows).
    fn track(&mut self, kind: &str) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": kind})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(t);
        self.frames(3);
        t
    }

    /// Puts the hosted plug-in, with stored state `state`, in `t`'s instrument slot or insert
    /// `slot`, as a reopened session holds it.
    fn host_plugin(&mut self, t: TrackId, slot: Option<usize>, state: &[u8]) {
        let mut insert = Insert::new(HOSTED);
        insert.state = Some(transit_model::b64::encode(state));
        let s = self.app.engine.session_mut();
        let tr = s.track_mut(t).unwrap();
        match slot {
            Some(i) => tr.mixer.inserts[i] = Some(insert),
            None => tr.instrument = Some(insert),
        }
    }

    /// The focused track's instrument card.
    fn card(&self) -> crate::mixer::DeviceCard {
        self.app.mixer.devices.cards.iter().find(|c| c.slot.is_none()).cloned().expect("an instrument card")
    }

    fn row(&self, t: TrackId, slot: usize) -> crate::mixer::InsertRow {
        let strip = self.app.mixer.layout.strips.iter().find(|s| s.track == t.0).expect("a strip");
        strip.inserts.iter().find(|r| r.slot == slot).cloned().expect("an insert row")
    }

    fn stuck(&self, t: TrackId, slot: usize) -> bool {
        self.app.player.as_ref().unwrap().stuck(t, slot)
    }

    fn bound(&self, t: TrackId, slot: usize) -> bool {
        self.app.player.as_ref().unwrap().live_name(t, slot).is_some()
    }

    /// Whether the hosted plug-in has been taken into a plan (its load time is remembered). Not
    /// `bound`: an instrument track starts with the built-in synth, which stays live in the slot
    /// while the hosted one loads (the compile waits for it, #56), so `bound` holds from the start.
    fn hosted_taken(&self) -> bool {
        self.app.last_load_time(HOSTED).is_some()
    }

    /// Whether the last frame painted the retry glyph (its arc's signature) inside `r`.
    fn retry_glyph_in(&self, r: [f32; 4]) -> bool {
        fn arc(shape: &egui::Shape, r: egui::Rect) -> bool {
            match shape {
                egui::Shape::Path(p) => p.points.len() == crate::devices::RETRY_ARC_POINTS && p.points.iter().all(|q| r.contains(*q)),
                egui::Shape::Vec(v) => v.iter().any(|s| arc(s, r)),
                _ => false,
            }
        }
        let r = egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]));
        self.shapes.iter().any(|s| arc(&s.shape, r))
    }
}

/// A clock the test moves by hand.
#[derive(Clone)]
struct ManualClock {
    base: Instant,
    offset_ms: Arc<AtomicU64>,
}

impl ManualClock {
    fn new() -> ManualClock {
        ManualClock { base: Instant::now(), offset_ms: Arc::new(AtomicU64::new(0)) }
    }
    fn clock(&self) -> Clock {
        let c = self.clone();
        Arc::new(move || c.base + Duration::from_millis(c.offset_ms.load(Ordering::SeqCst)))
    }
    fn advance(&self, by: Duration) {
        self.offset_ms.fetch_add(u64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
    }
}

/// Issue #82: the card reads "Loading…", then "Loading… (slow) 11 s" past the 10 s display
/// budget, then the plug-in's kind once the late instance is adopted; nothing reads "Not
/// responding" and nothing is made twice.
#[test]
fn the_slot_reads_loading_then_slow_then_the_plug_in_once_the_late_instance_is_adopted() {
    let clock = ManualClock::new();
    let mut t = T::new(Duration::from_millis(100), Duration::from_secs(60), clock.clock(), Duration::from_secs(10), 1);
    let track = t.track("instrument");
    t.host_plugin(track, None, &[3]);
    let mut seen: Vec<String> = Vec::new();
    let note = |t: &T, seen: &mut Vec<String>| {
        let status = t.card().status;
        if (!seen.is_empty() || status == crate::devices::LOADING) && seen.last() != Some(&status) {
            seen.push(status);
        }
    };
    let gate = Arc::clone(&t.gate);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.card().status == crate::devices::LOADING && gate.calls() == 1));
    note(&t, &mut seen);
    clock.advance(Duration::from_secs(11));
    t.frames(2);
    note(&t, &mut seen);
    assert_eq!(t.card().status, "Loading… (slow) 11 s");
    assert!(!t.card().editor_enabled, "no editor while it loads");
    assert_eq!(t.card().retry_button, [0.0; 4], "no Retry while it is only slow");
    t.gate.open();
    assert!(t.frames_until(Duration::from_secs(10), |t| t.bound(track, SLOT) && t.card().status == "CLAP instrument"));
    note(&t, &mut seen);
    assert_eq!(seen, ["Loading…", "Loading… (slow) 11 s", "CLAP instrument"]);
    assert_eq!(t.gate.calls(), 1, "the late instance was adopted, not made again");
    assert!(!t.app.status.contains("not responding"), "{}", t.app.status);
}

/// Issue #82: a creation past the host's creation budget reads "Not responding" on the card and
/// the mixer row, each with a Retry button, and the status line names it. The card's Retry button
/// makes it again with its stored state; the mixer row's context menu has Retry too.
#[test]
fn a_not_responding_slot_offers_retry_on_the_card_and_the_mixer_row() {
    let mut t = T::new(Duration::from_millis(100), Duration::from_millis(400), Arc::new(Instant::now), Duration::from_millis(150), 2);
    let keys = t.track("instrument");
    t.host_plugin(keys, None, &[1]);
    let fx = t.track("audio");
    t.host_plugin(fx, Some(0), &[2]);
    t.app.focus(keys);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.stuck(keys, SLOT) && t.stuck(fx, 0)), "both read Not responding");
    t.frames(2);
    assert!(t.app.status.contains("2 plug-ins are not responding"), "{}", t.app.status);
    assert!(t.app.retry_all_rect.is_some(), "Retry all shows while more than one slot is not responding");
    let card = t.card();
    assert_eq!(card.status, crate::devices::NOT_RESPONDING);
    assert_ne!(card.retry_button, [0.0; 4], "the card has a Retry button");
    assert!(t.retry_glyph_in(card.retry_button), "the card's Retry button is the painted circular arrow");
    let row = t.row(fx, 0);
    assert_eq!(row.status, crate::devices::NOT_RESPONDING);
    assert_ne!(row.retry, [0.0; 4], "the mixer row has a Retry button");
    assert!(t.retry_glyph_in(row.retry), "the row's Retry button is the painted circular arrow");
    let history = |t: &mut T| t.app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len();
    let undo = history(&mut t);

    // The card's Retry button: made again with its stored state.
    t.click(card.retry_button, false);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.bound(keys, SLOT) && !t.stuck(keys, SLOT)), "the instrument was made again");
    assert_eq!(t.gate.states.lock().unwrap().iter().filter(|s| s.as_deref() == Some(&[1][..])).count(), 2, "made again with its stored state");
    assert_eq!(t.card().status, "CLAP instrument");
    assert!(t.app.retry_all_rect.is_none(), "one slot left: no Retry all");
    assert_eq!(history(&mut t), undo, "Retry is not an undo step");

    // The mixer row's context menu.
    t.click(t.row(fx, 0).rect, true);
    let retry = t.app.mixer.layout.menu.iter().find(|m| m.label == "Retry").cloned().expect("a Retry entry in the row's menu");
    t.click(retry.rect, false);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.bound(fx, 0) && !t.stuck(fx, 0)), "the effect was made again");
    assert_eq!(t.gate.states.lock().unwrap().iter().filter(|s| s.as_deref() == Some(&[2][..])).count(), 2);
    assert_eq!(t.row(fx, 0).status, "");
    assert_eq!(t.gate.calls(), 4);
}

/// Issue #82: the status line's "Retry all" retries every slot that is not responding.
#[test]
fn retry_all_in_the_status_line_makes_every_not_responding_plug_in_again() {
    let mut t = T::new(Duration::from_millis(100), Duration::from_millis(400), Arc::new(Instant::now), Duration::from_millis(150), 2);
    let a = t.track("instrument");
    t.host_plugin(a, None, &[5]);
    let b = t.track("instrument");
    t.host_plugin(b, None, &[6]);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.stuck(a, SLOT) && t.stuck(b, SLOT)));
    t.frames(2);
    let r = t.app.retry_all_rect.expect("Retry all");
    t.click([r.min.x + 6.0, r.min.y, r.min.x + 6.0, r.max.y], false);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.bound(a, SLOT) && t.bound(b, SLOT)), "both were made again");
    assert!(!t.app.player.as_ref().unwrap().any_stuck());
    assert_eq!(t.gate.calls(), 4);
    let states = t.gate.states.lock().unwrap().clone();
    assert_eq!(states[2..].iter().filter(|s| matches!(s.as_deref(), Some([5] | [6]))).count(), 2, "with their stored states: {states:?}");
}

/// Issue #75 follow-up: the Play button pulses only while the wait is on the normal path. A
/// pending request or a load inside the display budget pulses; a slow load turns it steady with
/// the plug-in named in the tooltip; the pulse is back, and then Play itself, once the load ends.
#[test]
fn play_pulses_while_loading_is_normal_and_goes_steady_once_the_load_is_slow() {
    let clock = ManualClock::new();
    let mut t = T::new(Duration::from_millis(100), Duration::from_secs(60), clock.clock(), Duration::from_secs(10), 1);
    let track = t.track("instrument");
    t.host_plugin(track, None, &[3]);
    let gate = Arc::clone(&t.gate);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.card().status == crate::devices::LOADING && gate.calls() == 1));
    assert_eq!(t.app.transport_wait(), Some(("Waiting for plug-ins to load".to_string(), true)), "normal load: pulse");
    t.app.player_requests.play = Some(Default::default());
    assert_eq!(t.app.transport_wait(), Some(("Play pending".to_string(), true)), "normal pending: pulse");
    t.app.player_requests.play = None;
    clock.advance(Duration::from_secs(11));
    t.frames(2);
    let name = t.app.plugin_name(HOSTED, true);
    let slow = Some((format!("Waiting for {name} to load (slow)"), false));
    assert_eq!(t.app.transport_wait(), slow, "slow load: steady with the reason");
    t.app.player_requests.play = Some(Default::default());
    assert_eq!(t.app.transport_wait(), slow, "slow load with Play pending: steady too");
    t.app.player_requests.play = None;
    t.gate.open();
    assert!(t.frames_until(Duration::from_secs(10), |t| t.hosted_taken()));
    t.frames(2);
    assert_eq!(t.app.transport_wait(), None, "loaded: Play is ready again");
}

/// Issue #75 follow-up: with Play pending on a plug-in that is not responding, the button is
/// steady and the tooltip points at Retry; after Retry loads it, the wait is over.
#[test]
fn play_is_steady_with_the_retry_hint_while_a_plug_in_is_not_responding() {
    let mut t = T::new(Duration::from_millis(100), Duration::from_millis(400), Arc::new(Instant::now), Duration::from_millis(150), 1);
    let track = t.track("instrument");
    t.host_plugin(track, None, &[1]);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.stuck(track, SLOT)));
    t.app.player_requests.play = Some(Default::default());
    let name = t.app.plugin_name(HOSTED, true);
    assert_eq!(t.app.transport_wait(), Some((format!("{name} not responding: Retry in the mixer"), false)));
    t.app.player_requests.play = None;
    t.app.retry_plugin(track, SLOT).unwrap();
    assert!(t.frames_until(Duration::from_secs(10), |t| t.bound(track, SLOT)));
    t.frames(2);
    assert_eq!(t.app.transport_wait(), None, "retried and loaded: Play is ready");
}

/// Issue #67: a load over 2 s is reported once in the status line when the plug-in binds, every
/// load updates the plug-in type's remembered load time, and a load under 2 s stays quiet.
#[test]
fn a_slow_load_is_reported_in_the_status_line_and_every_load_is_remembered() {
    let clock = ManualClock::new();
    let mut t = T::new(Duration::from_millis(100), Duration::from_secs(60), clock.clock(), Duration::from_secs(10), 1);
    let track = t.track("instrument");
    t.host_plugin(track, None, &[3]);
    let gate = Arc::clone(&t.gate);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.card().status == crate::devices::LOADING && gate.calls() == 1));
    assert_eq!(t.app.last_load_time(HOSTED), None, "never loaded before");
    clock.advance(Duration::from_secs(3));
    t.gate.open();
    assert!(t.frames_until(Duration::from_secs(10), |t| t.hosted_taken()));
    t.frames(2);
    let name = t.app.player.as_ref().unwrap().live_name(track, SLOT).unwrap();
    assert_eq!(t.app.status, format!("{name} loaded in 3.0 s"));
    assert_eq!(t.app.last_load_time(HOSTED), Some(Duration::from_secs(3)), "remembered for the type");

    // A quick one (the gate is open): no status line, the remembered time is replaced.
    t.app.status.clear();
    let second = t.track("instrument");
    t.host_plugin(second, None, &[4]);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.gate.calls() == 2 && t.app.player.as_ref().unwrap().slot_load(second, SLOT).is_none()));
    t.frames(2);
    assert!(!t.app.status.contains("loaded in"), "{}", t.app.status);
    assert!(t.app.last_load_time(HOSTED).unwrap() < Duration::from_secs(2), "updated on every successful load");
}

/// Issue #67: the loading readout carries how long the plug-in type took last time, next to the
/// "Loading…" and the "Loading… (slow) N s" text, and the tooltip names the phase.
#[test]
fn the_loading_readout_shows_the_last_load_time_and_the_tooltip_the_phase() {
    use transit_playback::LoadPhase;
    use transit_playback::SlotLoad;
    let clock = ManualClock::new();
    let mut t = T::new(Duration::from_millis(100), Duration::from_secs(60), clock.clock(), Duration::from_secs(10), 1);
    t.app.ui.plugin_load_ms.insert(HOSTED.to_string(), 14_100);
    let track = t.track("instrument");
    t.host_plugin(track, None, &[3]);
    let gate = Arc::clone(&t.gate);
    assert!(t.frames_until(Duration::from_secs(10), |t| t.card().status.starts_with("Loading…") && gate.calls() == 1));
    assert_eq!(t.card().status, "Loading… · last time 14 s");
    assert_eq!(t.app.player.as_ref().unwrap().slot_phase(track, SLOT), Some(LoadPhase::Creating));
    clock.advance(Duration::from_secs(11));
    t.frames(2);
    assert_eq!(t.card().status, "Loading… (slow) 11 s · last time 14 s");
    // The tooltip: elapsed, phase and the last time.
    let tip = crate::devices::load_tip(
        SlotLoad::Loading { elapsed: Duration::from_millis(12_300) },
        "Jup-8 V",
        Some(LoadPhase::Creating),
        Some(Duration::from_secs(14)),
    );
    assert_eq!(tip, "Loading Jup-8 V… 12.3 s (creating instance); last time 14 s");
}
