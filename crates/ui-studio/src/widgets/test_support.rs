//! A one-widget test rig: a themed context, a fixed screen, and the pointer, keyboard and wheel
//! events a user sends, one frame at a time (no wall clock: the frame time advances by a fixed
//! step).

use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Ui, pos2, vec2};

pub struct Rig {
    pub ctx: egui::Context,
    time: f64,
    pub modifiers: Modifiers,
}

impl Rig {
    pub fn new() -> Rig {
        Rig::themed(egui::Theme::Dark)
    }

    pub fn themed(theme: egui::Theme) -> Rig {
        let ctx = egui::Context::default();
        crate::theme::apply_pinned(&ctx, theme);
        Rig { ctx, time: 0.0, modifiers: Modifiers::NONE }
    }

    /// One frame with `events`, the widget drawn by `f` in a ui at the screen's top left.
    pub fn frame<R>(&mut self, mut events: Vec<Event>, mut f: impl FnMut(&mut Ui) -> R) -> R {
        self.time += 1.0 / 60.0;
        events.insert(0, Event::ModifiersChanged(self.modifiers));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(self.time),
            events,
            focused: true,
            ..Default::default()
        };
        let mut out = None;
        let mut full = self.ctx.run_ui(raw, |ui| {
            crate::theme::follow(ui.ctx());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(20.0, 20.0), vec2(700.0, 500.0))));
            out = Some(f(&mut child));
        });
        full.textures_delta.clear();
        crate::theme::set_current(egui::Theme::Dark);
        out.expect("the frame ran")
    }

    /// One idle frame, returning what `f` returned and every shape the frame painted.
    pub fn shapes<R>(&mut self, mut f: impl FnMut(&mut Ui) -> R) -> (R, Vec<egui::epaint::ClippedShape>) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(self.time),
            focused: true,
            ..Default::default()
        };
        let mut out = None;
        let mut full = self.ctx.run_ui(raw, |ui| {
            crate::theme::follow(ui.ctx());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(20.0, 20.0), vec2(700.0, 500.0))));
            out = Some(f(&mut child));
        });
        full.textures_delta.clear();
        crate::theme::set_current(egui::Theme::Dark);
        (out.expect("the frame ran"), full.shapes)
    }

    pub fn idle<R>(&mut self, f: impl FnMut(&mut Ui) -> R) -> R {
        self.frame(vec![], f)
    }

    pub fn press(p: Pos2, m: Modifiers) -> Vec<Event> {
        vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: m }]
    }

    pub fn release(p: Pos2, m: Modifiers) -> Vec<Event> {
        vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: m }]
    }

    pub fn wheel(p: Pos2, dy: f32, m: Modifiers) -> Vec<Event> {
        vec![
            Event::PointerMoved(p),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers: m },
        ]
    }

    /// A click at `p`: press in one frame, release in the next, drawn by `f` both times; returns
    /// the second frame's result.
    pub fn click<R>(&mut self, p: Pos2, mut f: impl FnMut(&mut Ui) -> R) -> R {
        let m = self.modifiers;
        self.frame(Rig::press(p, m), &mut f);
        self.frame(Rig::release(p, m), &mut f)
    }

    /// A drag from `from` by `by` in `steps` frames; returns each frame's result.
    pub fn drag<R>(&mut self, from: Pos2, by: egui::Vec2, steps: usize, mut f: impl FnMut(&mut Ui) -> R) -> Vec<R> {
        let m = self.modifiers;
        let mut out = vec![self.frame(Rig::press(from, m), &mut f)];
        for i in 1..=steps {
            let p = from + by * (i as f32 / steps as f32);
            out.push(self.frame(vec![Event::PointerMoved(p)], &mut f));
        }
        out.push(self.frame(Rig::release(from + by, m), &mut f));
        out
    }
}

/// The center of a layout rect.
pub fn mid(r: [f32; 4]) -> Pos2 {
    pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// Whether `inner` lies inside `outer`.
pub fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.01 && inner[1] >= outer[1] - 0.01 && inner[2] <= outer[2] + 0.01 && inner[3] <= outer[3] + 0.01
}

pub fn size(r: [f32; 4]) -> (f32, f32) {
    (r[2] - r[0], r[3] - r[1])
}
