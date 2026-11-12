//! The harness of the Sampler panel's tests: the app driven the way a user drives it, synthetic
//! input one frame at a time (no player, no sleeps, no wall clock: each frame advances a fixed
//! 1/60 s). Audio files are generated here; a dropped file is injected as egui receives one.

use super::SamplerLayout;
use crate::builtin_editor::EditorLayout;
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transit_model::TrackId;
use transit_model::sampler_patch::PatchV1;

pub struct T {
    pub app: StudioApp,
    pub ctx: egui::Context,
    pub time: f64,
    pub shapes: Vec<egui::epaint::ClippedShape>,
    /// The screen the frames run on (1400 by 900 unless a test sets another).
    pub screen: egui::Vec2,
    /// Files dropped in the next frame, and where the pointer reports itself in that frame (none:
    /// the OS drag sent no motion, as on Windows).
    drop: Option<(Vec<PathBuf>, Option<egui::Pos2>)>,
    /// Where the pointer moves in the next frame.
    point: Option<egui::Pos2>,
    /// The next frame reports the pointer gone (egui-winit's `CursorLeft`).
    gone: bool,
    /// Raw events the next frame receives (a press held across frames, say).
    events: Vec<egui::Event>,
    /// The folder of the test's generated files.
    pub dir: transit_test_support::TestDir,
}

pub fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// A file egui reports as dropped.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn header(b: &mut Vec<u8>, data_len: u32, rate: u32) {
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((2 * rate).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
}

/// A mono 16-bit WAV of `frames` frames at 48 kHz.
pub fn wav(path: &Path, frames: usize) {
    wav_at(path, frames, 48_000);
}

/// A mono 16-bit WAV of `frames` frames at `rate`.
pub fn wav_at(path: &Path, frames: usize, rate: u32) {
    let samples: Vec<i16> = (0..frames).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
    let mut b = Vec::new();
    header(&mut b, (samples.len() * 2) as u32, rate);
    for s in samples {
        b.extend(s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl T {
    pub fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let n = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = transit_test_support::TestDir::new(&format!("sampler-pads-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = T {
            app,
            ctx: egui::Context::default(),
            time: 0.0,
            shapes: Vec::new(),
            screen: egui::vec2(1400.0, 900.0),
            drop: None,
            point: None,
            gone: false,
            events: Vec::new(),
            dir,
        };
        t.frames(3);
        t
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
                time: Some(self.time),
                ..Default::default()
            };
            if let Some(at) = self.point.take() {
                raw.events.push(egui::Event::PointerMoved(at));
            }
            if let Some((files, at)) = self.drop.take() {
                raw.events.extend(at.map(egui::Event::PointerMoved));
                raw.dropped_files = files.into_iter().map(|p| Arc::new(Dropped(p)) as egui::DroppedFileHandle).collect();
            }
            raw.events.extend(std::mem::take(&mut self.events));
            if std::mem::take(&mut self.gone) {
                raw.events.push(egui::Event::PointerGone);
            }
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            self.shapes = out.shapes;
            out.textures_delta.clear();
        }
    }

    pub fn send(&mut self, id: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(id, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(12);
    }

    pub fn click(&mut self, at: (f32, f32), count: u32) {
        self.send("ui.click", json!({"x": at.0, "y": at.1, "count": count}));
    }

    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32), shift: bool) {
        self.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1, "shift": shift}));
    }

    pub fn key(&mut self, key: &str) {
        self.send("ui.key", json!({"key": key}));
    }

    pub fn run(&mut self, id: &str, p: Value) {
        self.app.run(id, p).unwrap();
        self.frames(3);
    }

    /// Drops `path` at `at` (the next frame sees it), then runs a few frames.
    pub fn drop_file(&mut self, path: &Path, at: (f32, f32)) {
        self.drop = Some((vec![path.to_path_buf()], Some(egui::pos2(at.0, at.1))));
        self.frames(3);
    }

    /// Drops `path` with no pointer motion in the drop's frame (egui's pointer stays where it last
    /// was), then runs a few frames.
    pub fn drop_unplaced(&mut self, path: &Path) {
        self.drop = Some((vec![path.to_path_buf()], None));
        self.frames(3);
    }

    /// Drops `path` in a frame that reports the pointer gone and no position (what the maintainer's
    /// Windows machine produced before the cursor query), and runs that one frame only.
    pub fn drop_gone(&mut self, path: &Path) {
        self.drop = Some((vec![path.to_path_buf()], None));
        self.gone = true;
        self.frames(1);
    }

    /// Moves the pointer to `at` in the next frame, then runs a few frames.
    pub fn point(&mut self, at: (f32, f32)) {
        self.point = Some(egui::pos2(at.0, at.1));
        self.frames(3);
    }

    /// Sends `events` in the next frame, then runs a few frames.
    pub fn input(&mut self, events: Vec<egui::Event>) {
        self.events = events;
        self.frames(3);
    }

    /// Every clip in the arrangement.
    pub fn clip_count(&self) -> usize {
        self.app.engine.session().tracks.iter().map(|t| t.clips().len()).sum()
    }

    /// A generated WAV of `frames` frames named `name` in the test's folder.
    pub fn sample(&self, name: &str, frames: usize) -> PathBuf {
        let p = self.dir.join(name);
        wav(&p, frames);
        p
    }

    /// A generated WAV of `frames` frames at `rate` named `name` in the test's folder.
    pub fn sample_at(&self, name: &str, frames: usize, rate: u32) -> PathBuf {
        let p = self.dir.join(name);
        wav_at(&p, frames, rate);
        p
    }

    /// An instrument track with a new Sampler (an empty kit, #454), focused.
    pub fn sampler_track(&mut self) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": "instrument", "instrument": "sampler"})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(t);
        self.frames(4);
        t
    }

    /// A Sampler with an empty kit, its editor open.
    pub fn open() -> (T, TrackId) {
        let mut t = T::new();
        let tr = t.sampler_track();
        t.open_card(0);
        (t, tr)
    }

    /// A kit with samples on pads A1, A2 and A3 (zones 1 to 3), its editor open.
    pub fn kit() -> (T, TrackId) {
        let (mut t, tr) = T::open();
        for (i, name) in ["Kick.wav", "CH Hat.wav", "OH Hat.wav"].iter().enumerate() {
            let p = t.sample(name, 4800);
            t.app.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": i + 1, "pad": i + 1, "path": p.to_string_lossy()})).unwrap();
        }
        t.frames(3);
        (t, tr)
    }

    /// Clicks the focused track's editor button on card `card`.
    pub fn open_card(&mut self, card: usize) {
        let at = mid(self.app.mixer.devices.cards[card].editor_button);
        self.click(at, 1);
    }

    pub fn undo_len(&mut self) -> usize {
        self.app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
    }

    pub fn window(&self, t: TrackId) -> EditorLayout {
        self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).cloned().expect("an editor window is open")
    }

    pub fn panel(&self, t: TrackId) -> SamplerLayout {
        *self.window(t).sampler.expect("the Sampler's panel")
    }

    /// The track's patch.
    pub fn patch(&self, t: TrackId) -> PatchV1 {
        super::patch_at(self.app.engine.session(), (t, transit_mix::INSTRUMENT_SLOT)).expect("a patch")
    }

    /// The center of pad `index` (it must be in the shown bank).
    pub fn pad_at(&self, t: TrackId, index: u8) -> (f32, f32) {
        mid(self.panel(t).pad(index).unwrap_or_else(|| panic!("pad {index} is shown")).pad.rect)
    }

    /// A tap for the instrument on `t`, read by the panel (no player in tests).
    pub fn tap(&mut self, t: TrackId) -> Arc<transit_mix::ScopeTap> {
        let hub = Arc::new(transit_mix::ScopeHub::default());
        let tap = Arc::new(transit_mix::ScopeTap::new());
        tap.set_enabled(true);
        hub.register(t, transit_mix::INSTRUMENT_UID, &tap);
        self.app.builtin_editor.hub = Some(hub);
        tap
    }
}
