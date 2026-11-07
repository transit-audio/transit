//! The harness of the Macro Oscillator panel's tests: the app driven the way a user drives it,
//! synthetic input one frame at a time (no player, no sleeps, no wall clock: each frame advances
//! a fixed 1/60 s).

use super::MacroLayout;
use crate::builtin_editor::{EditorLayout, RowLayout};
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

pub struct T {
    pub app: StudioApp,
    pub ctx: egui::Context,
    pub time: f64,
    pub shapes: Vec<egui::epaint::ClippedShape>,
    /// The screen the frames run on (1400 by 900 unless a test sets another).
    pub screen: egui::Vec2,
}

/// One frame of a [`T::sweep`].
#[derive(Debug, Clone, Copy)]
pub struct SweepFrame {
    pub before: f32,
    pub after: f32,
    pub painted_deg: f32,
}

pub fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

pub fn info() -> &'static transit_dsp::PluginInfo {
    transit_dsp::plugin_info(super::MACRO_OSC).unwrap()
}

impl T {
    pub fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new(), screen: egui::vec2(1400.0, 900.0) };
        t.frames(3);
        t
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame_with(Vec::new());
        }
    }

    /// One frame (1/60 s) with `events` appended after the synthetic queue's own.
    pub fn frame_with(&mut self, events: Vec<egui::Event>) {
        {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            raw.events.extend(events);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            self.shapes = out.shapes;
            out.textures_delta.clear();
        }
    }

    /// Drags the knob for `param` by `dy` points per frame (negative is up) for `frames` frames,
    /// one pointer move per frame through the real panel, then releases. Returns each frame's
    /// record: the session value the dial was drawn from (before the frame), the session value
    /// after it, and the pointer angle the dial painted.
    pub fn sweep(&mut self, tr: TrackId, param: &str, dy: f32, frames: usize) -> Vec<SweepFrame> {
        use egui::{Event, PointerButton};
        let dial = self.knob(tr, param);
        let at = egui::pos2(mid(dial).0, mid(dial).1);
        self.frame_with(vec![Event::PointerMoved(at)]);
        self.frame_with(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
        let mut out = Vec::new();
        for i in 1..=frames {
            let before = self.param(tr, param);
            self.frame_with(vec![Event::PointerMoved(at + egui::vec2(0.0, dy * i as f32))]);
            let painted = self.panel(tr).knob(param).expect("the knob").knob.pointer_deg;
            out.push(SweepFrame { before, after: self.param(tr, param), painted_deg: painted });
        }
        let end = at + egui::vec2(0.0, dy * frames as f32);
        self.frame_with(vec![Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
        self.frames(2);
        out
    }

    /// Sweeps `param`'s knob up over its whole travel (200 points and the drag threshold) at `dy`
    /// points per frame, from `start` (the parameter's minimum), and checks the guards of the
    /// accumulating drag: the value never falls, it visits every one of the `steps` steps and ends
    /// within one step of the maximum, and on every frame the dial points at the session value it
    /// was drawn from. `norm` maps a session value to the knob's position. One undo step.
    pub fn full_travel(&mut self, tr: TrackId, param: &str, start: f32, dy: f32, steps: usize, norm: impl Fn(f32) -> f32) -> Vec<SweepFrame> {
        self.set(tr, param, start);
        let before = self.undo_len();
        let frames = ((crate::widgets::knob::DRAG_PX + 8.0) / dy.abs()).ceil() as usize;
        let sweep = self.sweep(tr, param, -dy.abs(), frames);
        assert_eq!(self.undo_len(), before + 1, "{param}: one undo step for the gesture");
        let mut seen: Vec<i32> = Vec::new();
        for (i, f) in sweep.iter().enumerate() {
            assert!(norm(f.after) >= norm(f.before) - 1e-6, "{param}: frame {i} fell {f:?}");
            let want = crate::widgets::knob::angle(norm(f.before));
            assert!((f.painted_deg - want).abs() < 0.01, "{param}: frame {i} dial {} is not the session value's angle {want}", f.painted_deg);
            let key = (norm(f.after) * steps as f32).round() as i32;
            if !seen.contains(&key) {
                seen.push(key);
            }
        }
        let last = sweep.last().unwrap().after;
        assert!(norm(last) >= 1.0 - 1.0 / steps as f32 - 1e-4, "{param}: ended at {last} (norm {})", norm(last));
        assert!(seen.len() >= steps, "{param}: visited {} of {} steps", seen.len(), steps + 1);
        sweep
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

    /// Sets an instrument parameter on `t` through the command a control would send.
    pub fn set(&mut self, t: TrackId, id: &str, v: f32) {
        self.run("mix.instrument_param", json!({"track": t.0, "param": id, "value": v}));
    }

    pub fn instrument_track(&mut self) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": "instrument"})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.run("mix.instrument", json!({"track": t.0, "plugin": super::MACRO_OSC})).unwrap();
        self.app.focus(t);
        self.frames(4);
        t
    }

    /// A track with the Macro Oscillator, its editor open (the device card's editor button).
    pub fn open() -> (T, TrackId) {
        let mut t = T::new();
        let tr = t.instrument_track();
        t.open_card(0);
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

    pub fn panel(&self, t: TrackId) -> MacroLayout {
        *self.window(t).macro_osc.expect("the Macro Oscillator's panel")
    }

    pub fn row(&self, t: TrackId, param: &str) -> RowLayout {
        self.window(t).rows.into_iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a row for `{param}`"))
    }

    pub fn param(&self, t: TrackId, id: &str) -> f32 {
        let ins = self.app.engine.session().track(t).unwrap().instrument.clone().unwrap();
        ins.params.get(id).copied().unwrap_or_else(|| info().param(id).unwrap().default)
    }

    /// The center of the painted text reading `want` in the list `r` opened (above or below it,
    /// within its column), the nearest one.
    pub fn listed(&self, r: [f32; 4], want: &str) -> Option<(f32, f32)> {
        let dist = |p: &(f32, f32)| if p.1 > r[3] { p.1 - r[3] } else { r[1] - p.1 };
        self.painted_all(want)
            .into_iter()
            .filter(|p| (p.1 > r[3] || p.1 < r[1]) && p.0 > r[0] - 200.0 && p.0 < r[2] + 200.0)
            .min_by(|a, b| dist(a).total_cmp(&dist(b)))
    }

    /// The centers of every painted text reading `want`.
    pub fn painted_all(&self, want: &str) -> Vec<(f32, f32)> {
        fn walk(shape: &egui::Shape, want: &str, out: &mut Vec<(f32, f32)>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == want => out.push((t.pos.x + t.galley.size().x / 2.0, t.pos.y + t.galley.size().y / 2.0)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        self.shapes.iter().for_each(|s| walk(&s.shape, want, &mut found));
        found
    }

    /// How many painted texts read exactly `want`, and the center of the first.
    pub fn painted(&self, want: &str) -> (usize, Option<(f32, f32)>) {
        fn walk(shape: &egui::Shape, want: &str, out: &mut Vec<(f32, f32)>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == want => out.push((t.pos.x + t.galley.size().x / 2.0, t.pos.y + t.galley.size().y / 2.0)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        self.shapes.iter().for_each(|s| walk(&s.shape, want, &mut found));
        (found.len(), found.first().copied())
    }

    /// Scrolls the body (the wheel over the window's empty left margin) until `pick`'s rect is
    /// inside the window, and returns it.
    pub fn in_view(&mut self, tr: TrackId, pick: impl Fn(&MacroLayout) -> [f32; 4]) -> [f32; 4] {
        for _ in 0..16 {
            let p = self.panel(tr);
            let (r, w) = (pick(&p), p.window);
            let (top, bottom) = (w[1] + 56.0, w[3] - 36.0);
            let dy = if r[1] < top {
                top - r[1]
            } else if r[3] > bottom {
                bottom - r[3]
            } else {
                return r;
            };
            self.send("ui.scroll", json!({"x": w[0] + 6.0, "y": (w[1] + w[3]) / 2.0, "dy": dy}));
        }
        pick(&self.panel(tr))
    }

    /// A knob's dial, scrolled into view.
    pub fn knob(&mut self, tr: TrackId, param: &str) -> [f32; 4] {
        let id = param.to_string();
        self.in_view(tr, |p| p.knob(&id).unwrap_or_else(|| panic!("a knob for `{id}`")).knob.dial)
    }

    /// Opens the Shaper and Modulation section bars.
    pub fn expand(&mut self, tr: TrackId) {
        for shaper in [true, false] {
            let p = self.panel(tr);
            let open = if shaper { p.shaper_body.is_some() } else { p.modulation_body.is_some() };
            if !open {
                let b = self.in_view(tr, |p| if shaper { p.shaper_bar.as_ref() } else { p.modulation_bar.as_ref() }.unwrap().button);
                self.click(mid(b), 1);
            }
        }
    }
}
