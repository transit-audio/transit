//! ADR 0062 slice S2b: the device-free step. `step_host` is the transport part of `logic()`: an
//! app driven by `logic` through a window context and one driven by `step_host`, each on its own
//! device-free Player stepped block for block on the same frame times, agree on everything the
//! transport shows after every step, through play, stop, a counted-in take and its placement.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::testing::{DeviceFree, device_free_app};
use crate::StudioApp;
use serde_json::json;

/// What a step leaves of the transport, as both drivers must agree on it.
#[derive(Debug, PartialEq)]
struct Seen {
    phase: std::mem::Discriminant<super::lifecycle::TransportPhase>,
    status: String,
    playing: bool,
    position: i64,
    recording: bool,
    count_in: Option<(i64, i64)>,
    requests: [bool; 4],
    take_plan: bool,
    clips: usize,
}

fn seen(app: &StudioApp) -> Seen {
    let r = &app.player_requests;
    Seen {
        phase: std::mem::discriminant(&app.transport_phase()),
        status: app.status.clone(),
        playing: app.is_playing(),
        position: app.player.as_ref().unwrap().position(),
        recording: app.engine.transport.recording,
        count_in: app.count_in_state(),
        requests: [r.play.is_some(), r.stop.is_some(), r.locate.is_some(), r.recording.is_some()],
        take_plan: app.take_plan.is_some(),
        clips: app.engine.session().tracks.iter().map(|t| t.playlist().map_or(0, |p| p.clips.len())).sum(),
    }
}

/// The two drivers: one through `logic` and a window context, one through `step_host`.
struct Pair {
    window: DeviceFree,
    ctx: egui::Context,
    host: DeviceFree,
    frames: u32,
}

impl Pair {
    fn new() -> Self {
        let mut pair = Pair { window: device_free_app(), ctx: egui::Context::default(), host: device_free_app(), frames: 0 };
        pair.both(|app| {
            let t = app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap();
            app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
            app.ui.count_in = true;
            app.ui.count_in_bars = 1;
        });
        pair
    }

    fn both(&mut self, mut f: impl FnMut(&mut StudioApp)) {
        f(&mut self.window.app);
        f(&mut self.host.app);
    }

    /// One audio block on each Player, then a frame through `logic` and a host step, at the
    /// same frame time; then the two must agree.
    fn step(&mut self) -> Seen {
        self.frames += 1;
        let now = DeviceFree::now(self.frames);
        self.window.app.player.as_ref().unwrap().step_device_free();
        let input = egui::RawInput { time: Some(now.as_secs_f64()), ..Default::default() };
        let app = &mut self.window.app;
        let mut out = self.ctx.run_ui(input, |ui| app.logic(ui.ctx()));
        out.textures_delta.clear();
        // The adapter's own step: the same block, then `step_host` at the same frame time.
        self.host.step();
        assert_eq!(self.host.frames(), self.frames);
        let (window, host) = (seen(&self.window.app), seen(&self.host.app));
        assert_eq!(window, host, "frame {}: logic and step_host disagree", self.frames);
        host
    }

    fn until(&mut self, limit: usize, done: impl Fn(&Seen) -> bool) -> Seen {
        for _ in 0..limit {
            let s = self.step();
            if done(&s) {
                return s;
            }
        }
        panic!("not reached in {limit} frames: {:?}", seen(&self.host.app));
    }
}

#[test]
fn step_host_matches_logic_for_the_transport_part() {
    let mut pair = Pair::new();
    pair.step();
    pair.both(|app| {
        app.run("transport.play", json!({})).unwrap();
    });
    let playing = pair.until(30, |s| s.playing && !s.requests[0]);
    assert!(playing.position > 0);
    pair.both(|app| {
        app.run("transport.stop", json!({})).unwrap();
    });
    pair.until(30, |s| !s.playing && !s.requests[1]);
    // A counted-in take: Count-in, the latch, Recording, then Stop places the take.
    pair.both(|app| {
        app.run("transport.record", json!({})).unwrap();
    });
    let counting = pair.until(30, |s| s.count_in.is_some());
    assert_eq!(counting.status, "Count-in");
    pair.until(600, |s| s.recording && s.playing);
    for _ in 0..20 {
        pair.step();
    }
    pair.both(|app| {
        app.run("transport.stop", json!({})).unwrap();
    });
    let stopped = pair.until(60, |s| !s.playing && s.requests == [false; 4] && !s.take_plan);
    assert!(!stopped.recording);
}
