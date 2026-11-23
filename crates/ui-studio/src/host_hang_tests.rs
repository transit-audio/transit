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
        T::stuck_named("Guitar Rig 7", 1.0)
    }

    /// [`T::stuck`] with the plug-in called `name`, at interface scale `zoom`.
    fn stuck_named(name: &'static str, zoom: f32) -> T {
        T::stuck_with(name, zoom, |_| {})
    }

    /// [`T::stuck_named`], with `before` run on the app before its first frame (a dialog it has open
    /// when the plug-in stops answering).
    fn stuck_with(name: &'static str, zoom: f32, before: impl FnOnce(&mut StudioApp)) -> T {
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
        let mut app = StudioApp::new(engine, Some(player), Services::default());
        app.ui.ui_zoom = zoom;
        before(&mut app);
        let (release, gate) = mpsc::channel::<()>();
        let _hung = host.submit(move |ctx| {
            ctx.name_job(name);
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
            self.frame(Vec::new());
        }
    }

    /// One frame carrying `events`.
    fn frame(&mut self, events: Vec<egui::Event>) {
        {
            self.app.player.as_ref().unwrap().step_device_free();
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                events,
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
    assert_eq!(layout.lines.iter().any(|l| l.contains("End Task")), cfg!(windows), "End Task is a Windows sentence: {:?}", layout.lines);
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
    let dir = transit_test_support::TestDir::new("host-hang");
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

/// The dialog layout gate (#735) on the plug-in-not-responding window, at 1x and 1.5x, with a
/// plug-in name long enough to wrap every line of the message and the Give up label.
#[test]
fn the_message_keeps_its_buttons_whole_with_the_longest_plug_in_name() {
    let layer = crate::layout_gate::modal_window(crate::host_hang::TITLE);
    for zoom in crate::layout_gate::SCALES {
        let name = "Guitar Rig With A Very Long Plug-in Name That Wraps Over Several Lines Of The Message 7";
        let mut t = T::stuck_named(name, zoom);
        crate::layout_gate::watch(&t.ctx, layer);
        t.frames(10);
        assert_eq!(t.ctx.pixels_per_point(), zoom);
        assert!(t.app.host_hang.layout.give_up_label.contains(name));
        crate::layout_gate::assert_clean(&t.ctx, layer);
    }
}

/// #770 round 2: the message is a modal above any other dialog. With About open when the plug-in
/// stops answering, its buttons work; Later returns to About, which still holds the app.
#[test]
fn the_message_sits_over_an_open_dialog_and_its_buttons_work() {
    let mut t = T::stuck_with("Guitar Rig 7", 1.0, |app| app.about.open = true);
    assert!(t.app.about.open, "About is open under the message");
    let top = crate::theme::modal_window_layer(crate::host_hang::TITLE);
    assert!(crate::modal::is_top(&t.ctx, top), "the message is the top dialog");
    let later = t.app.host_hang.layout.later.expect("Later");
    t.click(later);
    assert!(t.app.host_hang.layout.lines.is_empty(), "Later, clicked over About, hides the message");
    assert!(t.app.about.open, "About is still open");
    t.frames(3);
    assert!(crate::modal::is_top(&t.ctx, crate::theme::modal_window_layer("About transit")), "About holds the app again");
}

/// Give up works over an open dialog too.
#[test]
fn give_up_works_over_an_open_dialog() {
    let mut t = T::stuck_with("Guitar Rig 7", 1.0, |app| app.about.open = true);
    let give_up = t.app.host_hang.layout.give_up.expect("Give up");
    t.click(give_up);
    assert_eq!(t.host.given_up(), 1, "Give up, clicked over About, gave up on the thread");
    assert!(t.app.about.open);
}

/// #770 round 3: the message coming back over About and a save question is the dialog that owns
/// the keys from the frame it is raised: neither on that frame nor the next does Escape close About
/// or cancel the question, nor Enter answer it.
#[test]
fn the_message_owns_the_keys_from_the_frame_it_comes_back() {
    let mut t = T::stuck_with("Guitar Rig 7", 1.0, |_| {});
    let later = t.app.host_hang.layout.later.expect("Later");
    t.click(later);
    assert!(t.app.host_hang.layout.lines.is_empty(), "Later hid the message");
    t.app.about.open = true;
    t.frames(3);
    t.app.dialog = Some(crate::Dialog::SaveChanges { then: ("session.new".into(), json!({})) });
    t.frames(3);
    t.app.host_hang.forget_later();
    let keys = || {
        vec![
            egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
            egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE },
        ]
    };
    t.frame(keys());
    assert!(!t.app.host_hang.layout.lines.is_empty(), "the message is back");
    t.frame(keys());
    t.frames(2);
    assert!(t.app.about.open, "About stays open under the message");
    assert!(matches!(t.app.dialog, Some(crate::Dialog::SaveChanges { .. })), "the save question is neither cancelled nor answered");
}
