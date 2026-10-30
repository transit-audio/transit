//! Issue #173 (ADR 0030): plug-in editor windows follow the main window. Driven like the app runs:
//! frames with the main window's focus and minimized state, over the fake devices' connection,
//! which stands in for the plug-in host thread (no plug-in, no audio device) and applies the
//! requests as the real thread does, or holds them while it is "stuck in a plug-in". Asserted
//! through `ui.inspect` (`editors_open`, `editor_windows`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{Main, requests};
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_dsp::EditorWindowRequest as R;
use transit_model::TrackId;

const KEY: (TrackId, usize) = (TrackId(7), 0);

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    focused: bool,
    minimized: bool,
}

impl T {
    fn new() -> T {
        let app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, focused: true, minimized: false };
        t.frames(2);
        let devices = json!({"systems": [{"name": "WASAPI", "is_default": true,
            "outputs": [{"name": "Speakers", "is_default": true}], "inputs": []}]});
        t.control("audio.fake_devices", devices);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            let vp = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
            vp.focused = Some(self.focused);
            vp.minimized = Some(self.minimized);
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn control(&mut self, method: &str, p: Value) -> Value {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let v = crate::control::handle_for_test(&mut self.app, &self.ctx.clone(), &req).unwrap_or(json!({"ok": true}));
        self.frames(3);
        assert_eq!(v["ok"], json!(true), "{method}: {v}");
        v["result"].clone()
    }

    /// `window.editors_follow` through the control channel's command method.
    fn follow(&mut self, p: Value) -> Value {
        self.control("command", json!({"command": "window.editors_follow", "params": p}))
    }

    fn windows(&self) -> Value {
        self.app.inspect(&self.ctx)["editor_windows"].clone()
    }

    fn state(&self) -> Value {
        let w = self.windows();
        w["editors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["track"] == json!(KEY.0) && e["slot"] == json!(KEY.1))
            .map_or(json!(null), |e| e["state"].clone())
    }

    fn posted(&self) -> Vec<String> {
        serde_json::from_value(self.windows()["posted"].clone()).unwrap()
    }

    /// Frames with this main window state; the requests they posted.
    fn main(&mut self, focused: bool, minimized: bool) -> Vec<String> {
        let before = self.posted().len();
        (self.focused, self.minimized) = (focused, minimized);
        self.frames(2);
        self.posted()[before..].to_vec()
    }

    fn fake<R>(&self, f: impl FnOnce(&crate::audio_connection::FakeConnection) -> R) -> R {
        f(self.app.audio_devices.fake.as_ref().unwrap().connection.as_ref().unwrap())
    }

    /// An editor wanted and opened, as `ui.set {plugin_windows}` does.
    fn open_editor(&mut self) {
        self.control("ui.set", json!({"plugin_windows": [[KEY.0, KEY.1]]}));
        self.frames(2);
        assert_eq!(self.app.inspect(&self.ctx)["editors_open"], json!([[KEY.0, KEY.1]]));
    }
}

#[test]
fn the_main_windows_transitions_ask_for_the_matching_requests() {
    let m = |focused, minimized| Main { focused, minimized };
    assert_eq!(requests(m(false, false), m(true, false)), [R::Front], "activated");
    assert_eq!(requests(m(true, false), m(false, false)), [], "deactivated: lowered with the app by the OS");
    assert_eq!(requests(m(true, false), m(true, true)), [R::Park], "minimized");
    assert_eq!(requests(m(false, true), m(false, true)), [], "still minimized");
    assert_eq!(requests(m(false, true), m(true, false)), [R::Unpark, R::Front], "restored and active");
    assert_eq!(requests(m(false, true), m(false, false)), [R::Unpark], "restored, focus not back yet");
    assert_eq!(requests(m(true, false), m(true, false)), [], "no change");
}

#[test]
fn editors_park_with_a_minimized_main_window_and_come_back_with_it() {
    let mut t = T::new();
    assert_eq!(t.windows()["follow"], json!(true), "the default");
    assert!(t.posted().contains(&"follow".to_string()), "the setting reached the host thread: {:?}", t.posted());
    t.open_editor();
    assert_eq!(t.state(), json!("shown"));
    assert_eq!(t.windows()["main"], json!("active"));
    // Deactivate: nothing posted, the editors stay up (behind the other application).
    assert_eq!(t.main(false, false), Vec::<String>::new());
    assert_eq!(t.windows()["main"], json!("inactive"));
    assert_eq!(t.state(), json!("shown"));
    // Activate: brought above the main window.
    assert_eq!(t.main(true, false), ["front"]);
    // Minimize: parked, still wanted (not a user close).
    assert_eq!(t.main(false, true), ["park"]);
    assert_eq!((t.windows()["main"].clone(), t.state()), (json!("minimized"), json!("parked")));
    assert_eq!(t.app.ui.plugin_windows, [KEY], "a parked editor is still open");
    // Restore: back, then in front once focused.
    assert_eq!(t.main(true, false), ["unpark", "front"]);
    assert_eq!(t.state(), json!("shown"));
    assert_eq!(t.fake(|f| f.editors.borrow().len()), 1, "one editor all along");
}

#[test]
fn selecting_an_open_plugin_raises_its_editor_and_never_opens_a_second_one() {
    let mut t = T::new();
    t.open_editor();
    let opened = t.fake(|f| f.editors.borrow().len());
    t.app.show_editor(KEY);
    t.frames(3);
    t.app.show_editor(KEY);
    t.frames(3);
    assert_eq!(t.fake(|f| f.windows.borrow().raised.clone()), [KEY, KEY], "raised each time");
    assert_eq!(t.fake(|f| f.editors.borrow().len()), opened, "no duplicate");
    assert_eq!(t.app.ui.plugin_windows, [KEY]);
    // A plug-in without an open editor is opened instead.
    let other = (TrackId(8), 0);
    t.app.show_editor(other);
    t.frames(3);
    assert!(t.fake(|f| f.editors.borrow().contains(&other)), "opened");
    assert_eq!(t.fake(|f| f.windows.borrow().raised.len()), 2, "not raised");
}

#[test]
fn closing_an_editor_never_closes_transit_and_the_grouping_goes_on() {
    let mut t = T::new();
    t.open_editor();
    // The user closes the editor's window (the fake host thread reports it closed).
    t.fake(|f| f.editors.borrow_mut().clear());
    t.frames(3);
    assert!(t.app.ui.plugin_windows.is_empty(), "forgotten as a user close");
    assert_eq!(t.app.inspect(&t.ctx)["editors_open"], json!([]));
    assert!(!t.app.quit_requested, "transit keeps running");
    assert_eq!(t.main(true, true), ["park"], "the grouping goes on with no editor");
    assert_eq!(t.main(true, false), ["unpark", "front"]);
}

#[test]
fn the_setting_off_keeps_editors_apart_from_the_main_window() {
    let mut t = T::new();
    t.open_editor();
    assert_eq!(t.follow(json!({"value": false})), json!({"editors_follow": false}));
    assert_eq!(t.posted().last().map(String::as_str), Some("no_follow"));
    assert_eq!(t.main(true, true), Vec::<String>::new(), "minimizing transit leaves them up, as before #173");
    assert_eq!(t.state(), json!("shown"));
    assert_eq!(t.main(true, false), Vec::<String>::new());
    // On again: sent once, and the grouping resumes.
    t.follow(json!({}));
    assert_eq!(t.windows()["follow"], json!(true));
    assert_eq!(t.posted().last().map(String::as_str), Some("follow"));
    assert_eq!(t.main(true, true), ["park"]);
    // The preference is saved with the others; an older ui.json without it follows.
    let saved: crate::UiState = serde_json::from_value(json!({})).unwrap();
    assert!(saved.editors_follow);
}

/// The fake host thread is stuck inside a plug-in: the main window's frames go on, the requests
/// are posted (and wait in its queue), `ui.inspect` answers, and when the thread returns it
/// applies them in order. Nothing in the frame loop depends on a request being applied.
#[test]
fn a_stuck_host_thread_leaves_the_frame_loop_running_and_applies_the_requests_later() {
    let mut t = T::new();
    t.open_editor();
    t.fake(|f| f.windows.borrow_mut().held = true);
    assert_eq!(t.main(false, true), ["park"]);
    t.frames(30);
    assert_eq!(t.state(), json!("shown"), "not applied while the thread is stuck");
    assert_eq!(t.windows()["main"], json!("minimized"), "the UI moved on");
    t.app.show_editor(KEY);
    t.frames(3);
    assert_eq!(t.fake(|f| f.windows.borrow().queued.clone()), [R::Park]);
    t.fake(|f| {
        let open = f.editors.borrow().clone();
        f.windows.borrow_mut().release(&open);
    });
    assert_eq!(t.state(), json!("parked"), "applied once the thread returned");
    assert_eq!(t.main(true, false), ["unpark", "front"]);
    assert_eq!(t.state(), json!("shown"));
}

#[test]
fn the_control_channel_can_minimize_and_focus_the_main_window() {
    let mut t = T::new();
    // The commands go to the window; the frames the test runs stand in for its events.
    assert_eq!(t.control("ui.window", json!({"minimized": true, "focus": true})), json!({}));
    assert_eq!(t.follow(json!({"value": true}))["editors_follow"], json!(true));
}
