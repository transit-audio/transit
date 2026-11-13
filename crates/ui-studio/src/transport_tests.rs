//! Stop gestures through the real UI and host queue. No player or audio device is opened.

use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl T {
    fn new() -> Self {
        let mut t =
            Self { app: StudioApp::new(transit_engine::Engine::default(), None, Services::default()), ctx: egui::Context::default(), time: 0.0 };
        t.app.set_cursor(48_000, None);
        t.frames(3);
        t
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame(Vec::new());
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        self.app.raw_input_hook(&mut raw);
        let mut output = self.ctx.run_ui(raw, |ui| {
            self.app.logic(ui.ctx());
            self.app.ui(ui);
        });
        output.textures_delta.clear();
    }

    fn input(&mut self, method: &str, params: Value) {
        let (request, _rx) = crate::ControlRequest::new(method, params);
        let reply = crate::control::handle_for_test(&mut self.app, &self.ctx, &request).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        self.frames(8);
    }

    fn stop(&mut self, count: u32) {
        let r = self.app.arrange.layout.transport["stop"];
        self.input("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0, "count": count}));
    }

    fn play(&mut self) {
        self.app.run("transport.play", json!({})).unwrap();
        self.frames(3);
        assert!(self.app.is_playing());
        assert!(self.app.position() > 48_000);
    }

    fn assert_stopped_at(&self, at: i64) {
        assert!(!self.app.is_playing());
        assert!(!self.app.engine.transport.recording);
        assert_eq!(self.app.cursor(), at);
        assert_eq!(self.app.position(), at);
        assert_eq!(self.app.engine.transport.position, at);
    }
}

#[test]
fn double_click_stop_while_playing_returns_to_zero_without_undo() {
    let mut t = T::new();
    t.app.run("track.new", json!({})).unwrap();
    t.app.run("edit.undo", json!({})).unwrap();
    let history = t.app.engine.undo_history();
    assert!(t.app.engine.can_redo());
    t.play();
    t.stop(2);
    t.assert_stopped_at(0);
    assert_eq!(t.app.engine.session().edit.playhead, 0);
    assert_eq!(t.app.engine.undo_history(), history, "transport preserves undo history");
    assert!(t.app.engine.can_redo(), "transport preserves redo history");
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.app.engine.session().tracks.len(), 1);
}

#[test]
fn single_stop_while_stopped_returns_to_zero() {
    let mut t = T::new();
    t.stop(1);
    t.assert_stopped_at(0);
    assert!(t.app.engine.undo_history().is_empty());
}

#[test]
fn double_click_stop_in_one_frame_stops_before_returning_to_zero() {
    let mut t = T::new();
    t.play();
    let r = t.app.arrange.layout.transport["stop"];
    let pos = egui::pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    t.frame(vec![egui::Event::PointerMoved(pos)]);
    let clicks = [true, false, true, false]
        .into_iter()
        .map(|pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE })
        .collect();
    t.frame(clicks);
    t.frames(2);
    t.assert_stopped_at(0);
}

#[test]
fn first_stop_keeps_play_start_cursor_second_stop_returns_to_zero() {
    let mut t = T::new();
    t.play();
    t.stop(1);
    t.assert_stopped_at(48_000);
    // A second Stop is independent of the double-click timeout.
    t.frames(60);
    t.stop(1);
    t.assert_stopped_at(0);
}

#[test]
fn space_still_stops_at_the_cursor_and_restarts_there() {
    let mut t = T::new();
    t.play();
    t.input("ui.key", json!({"key": "Space"}));
    t.assert_stopped_at(48_000);
    t.input("ui.key", json!({"key": "Space"}));
    assert!(t.app.is_playing());
    assert_eq!(t.app.cursor(), 48_000);
    assert!(t.app.position() > 48_000);
}

#[test]
fn queued_stop_stop_play_locates_before_restarting_and_pause_does_not_rewind() {
    let mut t = T::new();
    t.play();
    for command in ["transport.stop", "transport.stop", "transport.play"] {
        t.app.run(command, json!({})).unwrap();
    }
    t.app.handle_transport(0.0);
    assert!(t.app.is_playing());
    assert_eq!(t.app.position(), 0);
    assert!(t.app.engine.transport_requests.is_empty());

    t.app.run("transport.locate", json!({"at": 96_000})).unwrap();
    for _ in 0..2 {
        t.app.run("transport.pause", json!({})).unwrap();
    }
    t.app.handle_transport(0.0);
    t.assert_stopped_at(96_000);
}

struct TakeFile(transit_test_support::TestDir);

impl TakeFile {
    fn new(tag: &str) -> Self {
        let path = transit_test_support::TestDir::file(&format!("stop-{tag}"), "wav");
        // One second of synthetic mono PCM, just as the recording writer supplies to the host.
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend(96_036u32.to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(48_000u32.to_le_bytes());
        wav.extend(96_000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(96_000u32.to_le_bytes());
        for _ in 0..48_000 {
            wav.extend(1000i16.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        Self(path)
    }
}

impl Drop for TakeFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stop a one-second audio take, the pass having started approximately or on a validated clock.
fn stop_take(tag: &str, approximate: bool) -> T {
    let take = TakeFile::new(tag);
    let mut t = T::new();
    let track = TrackId(t.app.run("track.new", json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.app.record_start = 48_000;
    t.app.take_plan = Some(transit_engine::io::Recording {
        start: 48_000,
        pass: None,
        punch: None,
        rate: 48_000,
        takes: Vec::new(),
        midi_journal: None,
        midi_placed: false,
        cycle: None,
        end: None,
    });
    t.app.finished_test_takes = Some((vec![(track, take.0.to_path_buf())], None));
    t.app.test_approximate_start = approximate;
    t.app.engine.transport.recording = true;
    t.app.start_play(96_000, None, None);
    t.stop(1);
    assert_eq!(t.app.engine.session().track(track).unwrap().clips().len(), 1);
    t
}

#[test]
fn a_take_that_started_without_a_validated_clock_says_so_once_at_stop() {
    let mut t = stop_take("approximate", true);
    assert!(t.app.status.contains("Recorded with approximate alignment: the audio device clock did not validate"), "{}", t.app.status);
    assert!(t.app.status.starts_with("Recorded 1 clip"), "{}", t.app.status);
    assert_eq!(t.app.telemetry.approximate_takes, 1);
    t.frames(60);
    assert_eq!(t.app.telemetry.approximate_takes, 1, "counted at Stop only");
}

#[test]
fn a_take_on_a_validated_clock_is_not_flagged() {
    let t = stop_take("validated", false);
    assert!(!t.app.status.contains("approximate"), "{}", t.app.status);
    assert_eq!(t.app.telemetry.approximate_takes, 0);
}

fn recording_stop(count: u32) {
    let take = TakeFile::new(if count == 1 { "single" } else { "double" });
    let mut t = T::new();
    let track = TrackId(t.app.run("track.new", json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.app.record_start = 48_000;
    t.app.take_plan = Some(transit_engine::io::Recording {
        start: 48_000,
        pass: None,
        punch: None,
        rate: 48_000,
        takes: Vec::new(),
        midi_journal: None,
        midi_placed: false,
        cycle: None,
        end: None,
    });
    t.app.finished_test_takes = Some((vec![(track, take.0.to_path_buf())], None));
    t.app.engine.transport.recording = true;
    t.app.start_play(96_000, None, None);
    let before = t.app.engine.undo_history();
    t.stop(count);
    t.assert_stopped_at(if count == 1 { 48_000 } else { 0 });
    assert!(t.app.finished_test_takes.is_none(), "Stop must finish the take");
    assert!(t.app.take_plan.is_none());
    let clips = t.app.engine.session().track(track).unwrap().clips();
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].start, clips[0].length), (48_000, 48_000), "the take stays at its recorded position and full length");
    let clip = clips[0].clone();
    let source = &t.app.engine.session().sources[0];
    assert_eq!(source.frames, 48_000);
    assert!(take.0.is_file());
    let after = t.app.engine.undo_history();
    assert_eq!(after.len(), before.len() + 1, "only the take adds an undo step");
    assert_eq!(after.last().map(String::as_str), Some("Record"));
    t.frames(60);
    t.stop(1);
    t.assert_stopped_at(0);
    assert_eq!(t.app.engine.undo_history(), after);
    assert_eq!(t.app.engine.session().track(track).unwrap().clips()[0].id, clip.id);
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.app.engine.session().track(track).unwrap().clips().is_empty(), "Undo reaches the take, not the return to zero");
    t.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(t.app.engine.session().track(track).unwrap().clips()[0].id, clip.id);
}

#[test]
fn double_click_stop_keeps_recorded_take_then_returns_to_zero() {
    recording_stop(2);
}

#[test]
fn first_stop_keeps_recorded_take_at_play_start_then_second_returns_to_zero() {
    recording_stop(1);
}

fn click_play(t: &mut T) {
    let r = t.app.arrange.layout.transport["play"];
    t.input("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0, "count": 1}));
}

#[test]
fn play_is_muted_with_a_reason_while_play_is_pending() {
    let mut t = T::new();
    assert_eq!(t.app.transport_wait(), None);
    t.app.player_requests.play = Some(Default::default());
    t.frames(2);
    assert!(t.app.play_pending());
    assert_eq!(t.app.transport_wait(), Some(("Play pending".to_string(), true)));
    // The button is still laid out (the tooltip and click target), now in the muted state.
    assert!(t.app.arrange.layout.transport.contains_key("play"));
}

#[test]
fn clicking_play_while_pending_issues_no_second_play() {
    let mut t = T::new();
    t.app.player_requests.play = Some(Default::default());
    t.frames(2);
    click_play(&mut t);
    assert_eq!(t.app.player_requests.play, Some(Default::default()), "the pending request is untouched");
    assert!(!t.app.is_playing(), "no play started");
    assert!(t.app.engine.transport_requests.is_empty(), "no request queued");
}

#[test]
fn play_returns_to_normal_once_the_request_clears() {
    let mut t = T::new();
    t.app.player_requests.play = Some(Default::default());
    t.frames(2);
    assert!(t.app.transport_wait().is_some());
    t.app.player_requests.play = None;
    t.frames(2);
    assert_eq!(t.app.transport_wait(), None);
    click_play(&mut t);
    assert!(t.app.is_playing(), "Play works again");
}

#[test]
fn the_low_latency_button_matches_the_transport_buttons_and_keeps_its_place_in_off_on_and_pending() {
    // Without a player the poll clears `pending` every frame, so draw the button directly.
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    let ctx = egui::Context::default();
    let mut draw = |on: bool, pending: bool| {
        app.ui.low_latency = on;
        app.low_latency.pending = pending;
        let mut next = egui::Rect::NOTHING;
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))), ..Default::default() };
        let mut out = ctx.run_ui(raw, |ui| {
            ui.horizontal(|ui| {
                app.low_latency_button(ui);
                next = ui.allocate_exact_size(egui::vec2(40.0, 20.0), egui::Sense::hover()).0;
            });
        });
        out.textures_delta.clear();
        (app.arrange.layout.transport["low_latency"], next)
    };
    draw(false, false); // the first frame sizes the fonts
    let idle = draw(false, false);
    let on = draw(true, false);
    let pending = draw(true, true);
    assert_eq!(idle, on, "the button and the item to its right do not move");
    assert_eq!(idle, pending, "the button and the item to its right do not move");
}

#[test]
fn the_low_latency_button_is_the_size_of_the_cycle_and_metronome_buttons() {
    let mut t = T::new();
    t.frames(3);
    let size = |r: [f32; 4]| (r[2] - r[0], r[3] - r[1]);
    let tr = &t.app.arrange.layout.transport;
    assert_eq!(size(tr["low_latency"]), size(tr["cycle"]));
    assert_eq!(size(tr["low_latency"]), size(tr["metronome"]));
}

/// #567: Record pressed while cycling, away from the loop start. The pass keeps the loop it
/// records under, and at Stop its audio take is cut at each wrap, a take per lap, as its MIDI is:
/// not one clip running past the loop end.
#[test]
fn record_pressed_mid_loop_while_cycling_stacks_an_audio_take_per_lap_issue_567() {
    let take = TakeFile::new("mid-loop");
    let mut t = T::new();
    let track = TrackId(t.app.run("track.new", json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    let cycle = transit_time::Range::new(48_000, 64_000);
    let edit = &mut t.app.engine.session_mut().edit;
    edit.loop_playback = true;
    edit.loop_range = cycle;
    t.app.engine.transport.playing = true;
    t.app.engine.transport.position = 56_000;
    t.app.begin_pass(Vec::new());
    let plan = t.app.take_plan.clone().expect("a pass is planned");
    assert_eq!((plan.start, plan.pass, plan.cycle), (56_000, None, Some(cycle)), "the plan keeps the loop the pass records under");
    t.app.finished_test_takes = Some((vec![(track, take.0.to_path_buf())], None));
    t.app.engine.transport.recording = true;
    t.stop(1);
    let tr = t.app.engine.session().track(track).unwrap();
    let mut laps: Vec<(i64, i64, i64)> = tr.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.source_offset(), c.start, c.length))).collect();
    laps.sort_unstable();
    // One second recorded from the middle of a third-of-a-second loop: half a lap, two whole laps
    // and a half lap. The newest (half) lap plays over its range and the lap before it plays
    // after it, so that lap is in two pieces (ADR 0039, item 4).
    assert_eq!(laps, vec![(0, 56_000, 8_000), (8_000, 48_000, 16_000), (24_000, 48_000, 8_000), (32_000, 56_000, 8_000), (40_000, 48_000, 8_000)]);
    assert!(laps.iter().all(|c| c.1 + c.2 <= cycle.end), "nothing past the loop end");
    assert_eq!((tr.playlists.len(), tr.active_playlist), (4, 0));
    assert!(t.app.status.starts_with("Recorded 4 takes"), "{}", t.app.status);
    assert_eq!(t.app.ui.takes_open, vec![track], "the lanes open, as after loop recording");
}

/// Stop a pass of the one-second take `take` on `track`, loop recorded in laps of `pass` frames
/// from 0, through the Stop button.
fn stop_laps(t: &mut T, track: TrackId, take: &TakeFile, pass: i64) {
    t.app.record_start = 0;
    t.app.take_plan = Some(transit_engine::io::Recording { pass: Some(pass), rate: 48_000, ..transit_engine::io::Recording::new(0) });
    t.app.finished_test_takes = Some((vec![(track, take.0.to_path_buf())], None));
    t.app.engine.transport.recording = true;
    t.app.start_play(4, None, None);
    t.stop(1);
}

/// #522: past the most laps one recording keeps, the status line after Stop says how many earlier
/// laps were left out and that the take file still holds them (they used to vanish without a word).
#[test]
fn laps_left_out_past_the_most_a_recording_keeps_are_said_in_the_status_line_issue_522() {
    let take = TakeFile::new("many-laps");
    let mut t = T::new();
    let track = TrackId(t.app.run("track.new", json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    // One second in laps of four frames: 12,000 laps.
    stop_laps(&mut t, track, &take, 4);
    let status = &t.app.status;
    assert!(status.starts_with("Recorded 10000 takes"), "{status}");
    let said = format!("2000 earlier laps were left out; their audio is kept in {} (import that file to reach it).", take.0.display());
    assert!(status.ends_with(&said), "{status}");
}

/// #634: a last lap shorter than an eighth of the loop is no take, and the status line after Stop
/// says so, with the frames and the file that keeps them.
#[test]
fn a_last_lap_too_short_to_be_a_take_is_said_at_stop_issue_634() {
    let take = TakeFile::new("short-lap");
    let mut t = T::new();
    let track = TrackId(t.app.run("track.new", json!({"kind": "audio", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
    // 48,000 frames in laps of 15,950: three laps and 150 frames.
    stop_laps(&mut t, track, &take, 15_950);
    let status = &t.app.status;
    assert!(status.starts_with("Recorded 3 takes"), "{status}");
    let said = format!("{}: the last 150 frames (3.1 ms) stay in {}.", transit_engine::take_laps::SHORT_LAP_NOTE, take.0.display());
    assert!(status.ends_with(&said), "{status}");
}
