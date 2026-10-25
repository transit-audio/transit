//! File > Bounce in real time (ADR 0027) with the Player's render faked: the question's three
//! choices, the refusals shown before starting, the spec handed to the Player, the progress
//! window's "bar N of M" and Cancel, the transport refusal, and the outcome on the status line.

use super::{CANCELLED, FakeRender, REFUSED_TRANSPORT};
use crate::{Services, StudioApp};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_playback::realtime_render::{RealtimeError, Rendered};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    track: TrackId,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl T {
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
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
        }
    }

    fn click(&mut self, r: [f32; 4]) {
        assert_ne!(r, [0.0; 4], "nothing to click");
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(6);
    }

    fn fake(&mut self) -> &mut FakeRender {
        self.app.finish.realtime.fake.as_mut().unwrap()
    }

    /// Open File > Bounce for the whole song to `mix.wav` and ask: the External Instrument question.
    fn ask(&mut self) {
        self.app.open_bounce();
        let path = self.dir.join("mix.wav").to_string_lossy().into_owned();
        if let Some(f) = &mut self.app.finish.bounce_form {
            f.path = path;
            f.range = crate::finish::BounceRange::Whole;
        }
        assert!(self.app.start_bounce().is_err(), "the question comes first");
        self.frames(3);
    }
}

/// A Model D track (MIDI to "MODEL D", return In 1, 20 ms set by hand) with a two-bar clip and a
/// faked real-time render.
fn rig() -> T {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.engine.set_io_map(soundcraft_model::IoMap::for_device("ASIO", "Focusrite USB ASIO", 2, 2));
    let r = app.run("track.new", json!({"kind": "instrument", "name": "Model D"})).unwrap();
    let track = TrackId(r["tracks"][0].as_u64().unwrap());
    app.run("instrument.external", json!({"track": track.0, "port": {"backend": "winmm", "name": "MODEL D"}, "return": "In 1", "manual_ms": 20.0}))
        .unwrap();
    // Two bars at 120 bpm, 4/4.
    app.run("midi.new_clip", json!({"tracks": [track.0], "start": 0, "end": 192_000})).unwrap();
    app.finish.realtime.fake = Some(FakeRender::default());
    let dir = std::env::temp_dir().join(format!("transit-rt-ui-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut t = T { app, ctx: egui::Context::default(), time: 0.0, track, dir };
    t.frames(3);
    t
}

#[test]
fn render_in_real_time_runs_shows_bars_and_cancel_releases() {
    let mut t = rig();
    t.ask();
    assert_eq!(t.app.finish.realtime_blocker, None, "nothing stands in the way");
    t.click(t.app.finish.realtime_button);
    assert!(t.app.finish.external_question.is_none());
    let spec = t.fake().started.last().cloned().expect("the Player was asked to render");
    assert_eq!((spec.start, spec.len, spec.main_channels), (0, 192_000, 2));
    assert_eq!(spec.stems.len(), 1);
    assert_eq!((spec.stems[0].track, spec.stems[0].channels.as_slice(), spec.stems[0].latency), (t.track, &[0u16][..], 960));
    assert_eq!(spec.preroll, 960, "the pre-roll is max(C, one block)");
    t.frames(2);
    let s = t.app.engine.session();
    let of = s.tempo.bar_beat_at(191_999, s.sample_rate).bar - s.tempo.bar_beat_at(0, s.sample_rate).bar + 1;
    assert!(of >= 2, "{of}");
    assert_eq!(t.app.realtime_progress().map(|p| (p.0, p.1)), Some((1, of)), "bar 1 of {of}");
    t.fake().done = 191_999;
    t.frames(2);
    assert_eq!(t.app.realtime_progress().map(|p| (p.0, p.1)), Some((of, of)), "the last bar");
    assert_eq!(super::progress_text(2, 3), "Rendering in real time\u{2026} bar 2 of 3");
    // The transport is the render's while it plays.
    assert_eq!(t.app.run("transport.play", json!({})).unwrap_err(), REFUSED_TRANSPORT);
    assert!(t.app.run("session.new", json!({})).is_err(), "a session-replacing command waits for the render");
    let cancel = t.app.finish.cancel;
    t.click(cancel);
    assert_eq!(t.fake().cancelled, 1);
    assert!(t.app.finish.realtime.running.is_none());
    assert_eq!(t.app.status, CANCELLED);
    assert!(t.app.dialog.is_none());
}

#[test]
fn a_finished_or_failed_render_is_said_on_the_status_line() {
    let mut t = rig();
    t.ask();
    t.app.finish.bounce_form.as_mut().unwrap().realtime_stems = true;
    t.click(t.app.finish.realtime_button);
    let running = t.app.finish.realtime.running.clone().unwrap();
    assert!(running.stems[0].ends_with("mix - Model D.wav"), "{:?}", running.stems);
    t.fake().result = Some(Ok(Rendered { frames: 192_000, latency: 960, peak: 0.5 }));
    t.frames(3);
    assert!(t.app.finish.realtime.running.is_none());
    assert!(t.app.status.starts_with("Rendered in real time to ") && t.app.status.contains("and 1 stem"), "{}", t.app.status);
    assert_eq!(t.app.finish.layout.last_bounce.as_ref().unwrap()["realtime"], json!(true));
    // A failure says why and writes nothing.
    t.ask();
    t.click(t.app.finish.realtime_button);
    t.fake().result = Some(Err(RealtimeError::Overrun { blocks: 1 }));
    t.frames(3);
    assert_eq!(t.app.status, "Render failed: the file writer fell behind and 1 block of audio was dropped. Nothing was written: render again.");
    assert!(t.app.finish.realtime.running.is_none());
}

/// ADR 0027 decision 2: every refusal is shown in the question before starting, the button is off,
/// and "Render without hardware (offline, faster)" stays.
#[test]
fn each_refusal_is_shown_before_starting() {
    let mut t = rig();
    let blocked = |t: &mut T| {
        t.ask();
        let why = t.app.finish.realtime_blocker.clone().expect("a reason");
        let _ = t.app.bounce_in_realtime();
        assert!(t.app.finish.realtime.running.is_none() && t.fake().started.is_empty(), "{why}: nothing started");
        t.app.cancel_bounce();
        why
    };
    t.fake().no_device = true;
    assert_eq!(blocked(&mut t), "No audio device");
    t.fake().no_device = false;
    t.fake().output_only = true;
    assert_eq!(blocked(&mut t), "Needs a duplex device");
    t.fake().output_only = false;
    t.fake().measuring = true;
    assert_eq!(blocked(&mut t), "A measurement is running: wait for it or cancel it.");
    t.fake().measuring = false;
    t.app.engine.transport.playing = true;
    assert_eq!(blocked(&mut t), "Stop the transport to render in real time.");
    t.app.engine.transport.playing = false;
    t.app.run("instrument.external", json!({"track": t.track.0, "manual_ms": null})).unwrap();
    assert_eq!(blocked(&mut t), "Model D is not measured: measure it, or render uncompensated (its notes land late by the round trip)");
    // Accepting "uncompensated" lifts it.
    t.ask();
    t.app.finish.bounce_form.as_mut().unwrap().uncompensated = true;
    t.frames(2);
    assert_eq!(t.app.finish.realtime_blocker, None);
    t.app.bounce_in_realtime().unwrap();
    assert_eq!(t.fake().started.last().unwrap().stems[0].latency, 0, "uncompensated: C is 0");
    t.app.cancel_bounce();
    t.fake().started.clear();
    t.app.run("instrument.external", json!({"track": t.track.0, "return": ""})).unwrap();
    assert_eq!(blocked(&mut t), "Choose the return input of Model D");
    t.app.run("instrument.external", json!({"track": t.track.0, "return": "In 9"})).unwrap();
    assert_eq!(blocked(&mut t), "Return In 9 of Model D is not on this interface");
    // The offline alternative is still there.
    t.ask();
    let without = t.app.finish.external_buttons.0;
    t.click(without);
    assert!(t.fake().started.is_empty(), "offline: the Player was not asked");
    assert!(t.app.status.contains("Left out: Model D") || t.app.status.starts_with("Bounced to"), "{}", t.app.status);
}

/// All MIDI Notes Off during a render cancels it (release, no file) instead of letting it finish
/// with the notes cut off; the other transport commands are refused while it plays.
#[test]
fn all_notes_off_cancels_the_render_and_transport_moves_are_refused() {
    let mut t = rig();
    t.ask();
    t.click(t.app.finish.realtime_button);
    assert!(t.app.finish.realtime.running.is_some());
    for id in ["transport.pause", "transport.rewind", "transport.fast_forward", "transport.go_to_end", "transport.loop"] {
        assert_eq!(t.app.run(id, json!({})).unwrap_err(), REFUSED_TRANSPORT, "{id}");
    }
    assert!(t.app.finish.realtime.running.is_some(), "a refused command leaves the render running");
    t.app.run("event.all_notes_off", json!({})).unwrap();
    assert_eq!(t.fake().cancelled, 1, "the Player's render was cancelled");
    assert!(t.app.finish.realtime.running.is_none());
    assert_eq!(t.app.finish.realtime.ended.as_deref(), Some(super::CANCELLED_PANIC));
    t.frames(3);
    assert!(t.app.dialog.is_none());
}
