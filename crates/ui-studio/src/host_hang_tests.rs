//! Issue #205: a plug-in host thread stuck inside a plug-in shows a message naming the plug-in,
//! with Save now, Give up and Later. A real device-free Player whose plug-in host the test holds;
//! the stuck call is a job that blocks on a channel the test releases (E49).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Services, StudioApp};
use serde_json::json;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use transit_playback::PluginTestHooks;
use transit_plugin_host::{Config, Host};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    host: Host,
    release: Option<mpsc::Sender<()>>,
}

impl Drop for T {
    fn drop(&mut self) {
        // The held call returns, so its host thread parks or finishes.
        self.release.take();
    }
}

impl T {
    /// An app whose plug-in host (request and creation timeout 200 ms) has a call into
    /// "Guitar Rig 7" that does not return until the test releases it.
    fn stuck() -> T {
        let engine = transit_engine::Engine::default();
        let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
        let timeout = Duration::from_millis(200);
        let host = Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() });
        player.set_plugin_test_hooks(PluginTestHooks {
            host: host.clone(),
            factory: Arc::new(|_: &transit_mix::InstanceSpec| None),
            deferred: Arc::new(|_: &str| false),
            clock: Arc::new(Instant::now),
            slow_after: Duration::from_secs(10),
        });
        let app = StudioApp::new(engine, Some(player), Services::default());
        let (release, gate) = mpsc::channel::<()>();
        let _hung = host.submit(move |ctx| {
            ctx.name_job("Guitar Rig 7");
            let _ = gate.recv();
        });
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, host, release: Some(release) };
        let shown = t.frames_until(Duration::from_secs(5), |t| !t.app.host_hang.layout.lines.is_empty());
        assert!(shown, "the message shows once the call is past its budget");
        // The centered window settles its size over its first frames.
        t.frames(10);
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
            // timing: stall-bound frame pacing; the stuck call is held by its channel
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

    fn click(&mut self, r: [f32; 4]) {
        let (x, y) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": x, "y": y, "button": "left"}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(8);
    }
}

#[test]
fn the_message_names_the_plug_in_and_give_up_quarantines_it() {
    let mut t = T::stuck();
    let layout = t.app.host_hang.layout.clone();
    assert!(layout.lines[0].starts_with("Guitar Rig 7 has not returned from creating it"), "{:?}", layout.lines);
    assert!(layout.lines.iter().any(|l| l.contains("Audio keeps playing")), "{:?}", layout.lines);
    assert!(layout.lines.iter().any(|l| l.contains("End Task")), "{:?}", layout.lines);
    assert_eq!(layout.give_up_label, "Give up on Guitar Rig 7");
    let report = t.app.diagnostics_report(0.0).render(&crate::diagnostics::Redaction::default());
    assert!(report.contains("Plug-in host: Guitar Rig 7 stuck in creating it"), "{report}");
    t.click(layout.give_up.expect("the Give up button"));
    assert_eq!(t.host.given_up(), 1, "the stuck thread was given up on");
    assert!(t.app.status.starts_with("Gave up on Guitar Rig 7"), "{}", t.app.status);
    assert!(t.app.host_hang.layout.lines.is_empty(), "the message is gone");
    // Later frames do not report the thread as having returned.
    t.frames(5);
    assert!(t.app.status.starts_with("Gave up on Guitar Rig 7"), "{}", t.app.status);
    // New plug-ins load on the fresh thread.
    assert_eq!(t.host.submit(|_| 1).wait(Duration::from_secs(5)), Some(1));
    let report = t.app.diagnostics_report(0.0).render(&crate::diagnostics::Redaction::default());
    assert!(report.contains("(given up: its plug-ins are quarantined until transit restarts)"), "{report}");
}

#[test]
fn later_hides_the_message_for_that_thread_and_save_now_saves() {
    let mut t = T::stuck();
    let dir = std::env::temp_dir().join(format!("transit-host-hang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hang.transit");
    t.app.run("session.save_as", json!({"path": path, "replace": true})).unwrap();
    std::fs::remove_file(&path).unwrap();
    let save = t.app.host_hang.layout.save.expect("Save now");
    t.click(save);
    assert!(path.exists(), "Save now saved the session");
    assert!(t.app.status.starts_with("Saved; Guitar Rig 7 keeps"), "{}", t.app.status);
    let later = t.app.host_hang.layout.later.expect("Later");
    t.click(later);
    assert!(t.app.host_hang.layout.lines.is_empty(), "Later hides it");
    t.frames(5);
    assert!(t.app.host_hang.layout.lines.is_empty(), "and it stays hidden for that thread");
    assert_eq!(t.host.given_up(), 0, "Later gives up on nothing");
    let _ = std::fs::remove_dir_all(&dir);
}
