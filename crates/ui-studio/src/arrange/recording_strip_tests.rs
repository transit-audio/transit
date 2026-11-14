//! #634: the strip of an audio take being recorded follows the laps of a cycle recording. After
//! each wrap it restarts at the loop start and grows to the playhead, as the take of that lap
//! will lie, and never draws past the loop end. Driven through whole frames on the simulated
//! transport (no audio device), which wraps and counts its own laps as the player does.
#![allow(clippy::unwrap_used)]

use super::*;
use transit_time::Range;

/// One second at 48 kHz: 50 frames of 20 ms.
const LOOP: Range = Range { start: 48_000, end: 96_000 };

struct Rig {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

impl Rig {
    /// An armed audio track, Cycle over [`LOOP`], the view at 200 samples a point.
    fn new() -> Self {
        let mut app = crate::tests::app();
        let t = app.run("track.new", json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.run("mix.record_arm", json!({"tracks": [t], "value": true})).unwrap();
        app.run("view.zoom_set", json!({"samples_per_px": 200.0})).unwrap();
        app.ui.show_mixer = false;
        app.ui.show_devices = false;
        let edit = &mut app.engine.session_mut().edit;
        edit.loop_playback = true;
        edit.loop_range = LOOP;
        let mut r = Self { app, ctx: egui::Context::default(), time: 0.0 };
        r.frame();
        r
    }

    /// One 20 ms frame: `logic` (the transport moves) and then the drawing.
    fn frame(&mut self) {
        self.time += 0.02;
        let mut raw =
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 800.0))), time: Some(self.time), ..Default::default() };
        self.app.raw_input_hook(&mut raw);
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    /// A recording pass from `at` as `plan_recording` sets one up, `pass` the lap length when it
    /// loop records from the loop start; the simulated transport plays the loop from there.
    fn record(&mut self, at: Samples, pass: Option<Samples>) {
        let app = &mut self.app;
        app.take_cycle = app.wanted_loop();
        assert_eq!(app.take_cycle, Some(LOOP));
        app.record_start = at;
        app.take_plan = Some(transit_engine::io::Recording {
            start: at,
            pass,
            punch: None,
            rate: 48_000,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
            cycle: app.take_cycle,
            end: None,
        });
        app.engine.transport.recording = true;
        app.start_play(at, None, Some(LOOP));
    }

    /// Frames until the simulated transport has wrapped `laps` times in all.
    fn until_wrapped(&mut self, laps: i64) {
        for _ in 0..200 {
            if self.app.sim_laps >= laps * LOOP.len() {
                return;
            }
            self.frame();
        }
        panic!("the transport never wrapped {laps} times: at {}", self.app.engine.transport.position);
    }

    /// The x of timeline sample `at` as the last frame mapped it.
    fn x(&self, at: Samples) -> f32 {
        let l = &self.app.arrange.layout;
        l.timeline[0] + ((at - l.scroll) as f64 / l.samples_per_px) as f32
    }

    /// The one recording strip the last frame drew: (left, right).
    fn strip(&self) -> (f32, f32) {
        let lanes = &self.app.arrange.layout.recording;
        assert_eq!(lanes.len(), 1, "one strip on the armed lane at {}", self.app.engine.transport.position);
        (lanes[0].rect[0], lanes[0].rect[2])
    }

    /// The strip reaches the playhead, starting at `from`.
    fn assert_strip(&self, from: Samples, what: &str) {
        let at = self.app.engine.transport.position;
        assert_eq!(self.strip(), (self.x(from), self.x(at)), "{what}: playhead at {at}");
        assert!(self.strip().1 <= self.x(LOOP.end), "{what}: never past the loop end");
    }
}

/// Record pressed while the loop plays, halfway in (#567): the first lap's strip runs from the
/// latch to the playhead; after each wrap the strip restarts at the loop start.
#[test]
fn a_strip_recorded_from_inside_the_loop_restarts_at_the_loop_start_at_each_wrap() {
    let mut r = Rig::new();
    let at = LOOP.start + LOOP.len() / 2;
    r.record(at, None);
    for _ in 0..5 {
        r.frame();
    }
    assert!(r.app.sim_laps == 0 && r.app.engine.transport.position > at);
    r.assert_strip(at, "first lap");
    r.until_wrapped(1);
    r.assert_strip(LOOP.start, "second lap");
    r.frame();
    r.assert_strip(LOOP.start, "second lap, a frame on");
    r.until_wrapped(2);
    r.assert_strip(LOOP.start, "third lap");
}

/// Record from stopped with Cycle on loop records in passes of the loop from its start: the strip
/// of each lap starts at the loop start, and the live waveform under it is that lap's, its first
/// sample at the loop start (where the take of that lap will lie).
#[test]
fn a_strip_recorded_in_passes_draws_the_lap_under_way_with_its_own_waveform() {
    let mut r = Rig::new();
    r.record(LOOP.start, Some(LOOP.len()));
    for _ in 0..5 {
        r.frame();
    }
    r.assert_strip(LOOP.start, "first lap");
    assert_eq!(recording_preview::span(&r.app).map(|(origin, _)| origin), Some(LOOP.start), "lap 1's audio starts at the loop start");
    r.until_wrapped(2);
    r.assert_strip(LOOP.start, "third lap");
    assert_eq!(
        recording_preview::span(&r.app).map(|(origin, _)| origin),
        Some(LOOP.start - 2 * LOOP.len()),
        "the waveform drawn is lap 3's: the take's first sample lies two loops before the loop start"
    );
}
