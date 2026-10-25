//! The switchable CC lane. Edits commit only on release, one undo step per gesture.
use super::*;
use soundcraft_midi::{CtrlEvent, CtrlKind};

#[derive(Debug, Default)]
pub struct State {
    pub controller: Option<u8>,
    pub channel: u8,
    drag: Option<(ClipId, usize, CtrlEvent)>,
}

impl State {
    pub(super) fn cancel(&mut self) {
        self.drag = None;
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Layout {
    pub selector: R4,
    pub channel: R4,
    pub choices: Vec<(u8, R4)>,
    pub controller: Option<u8>,
    pub points: Vec<(usize, R4)>,
    pub sustain: Vec<R4>,
}

pub(super) fn selector(app: &mut StudioApp, ui: &mut Ui, rect: Rect, layout: &mut Layout) {
    let current = app.keys.cc.controller;
    let label = match current {
        None => "Velocity".to_owned(),
        Some(64) => "Sustain".to_owned(),
        Some(n) => format!("CC {n}"),
    };
    let button = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    let resp = ui.put(button, egui::Button::new(label).small());
    layout.selector = r4(resp.rect);
    egui::Popup::menu(&resp).show(|ui| {
        if ui.selectable_label(current.is_none(), "Velocity").clicked() {
            app.keys.cc.controller = None;
            ui.close();
        }
        for (n, label) in [(64, "Sustain"), (1, "Mod wheel"), (11, "Expression")] {
            let choice = ui.selectable_label(current == Some(n), label);
            layout.choices.push((n, r4(choice.rect)));
            if choice.clicked() {
                app.keys.cc.controller = Some(n);
                ui.close();
            }
        }
        ui.horizontal(|ui| {
            ui.label("CC");
            let mut n = current.unwrap_or(64);
            if ui.add(egui::DragValue::new(&mut n).range(0..=127)).changed() {
                app.keys.cc.controller = Some(n);
            }
        });
    });
    if app.keys.cc.controller.is_some() {
        let r = Rect::from_min_size(pos2(rect.min.x, rect.min.y + 25.0), vec2(rect.width(), 24.0));
        let mut channel = app.keys.cc.channel.saturating_add(1).clamp(1, 16);
        let response = ui.put(r, egui::DragValue::new(&mut channel).range(1..=16).prefix("Ch "));
        layout.channel = r4(response.rect);
        if app.keys.cc.channel != channel - 1 {
            app.keys.cc.drag = None;
        }
        app.keys.cc.channel = channel - 1;
    }
    if current != app.keys.cc.controller {
        app.keys.cc.drag = None;
    }
    layout.controller = app.keys.cc.controller;
}

fn at(map: &Map, rect: Rect, point: CtrlEvent) -> Pos2 {
    pos2(map.x(point.tick), rect.max.y - 4.0 - (rect.height() - 8.0) * point.value.clamp(0, 127) as f32 / 127.0)
}

fn value(rect: Rect, y: f32) -> i32 {
    ((rect.max.y - 4.0 - y) / (rect.height() - 8.0).max(1.0) * 127.0).round().clamp(0.0, 127.0) as i32
}

pub(super) fn lane(app: &mut StudioApp, ui: &mut Ui, map: &Map, rect: Rect, cv: &ClipView, layout: &mut Layout) {
    let Some(controller) = app.keys.cc.controller else { return };
    let Some(clip) = app.keys.clip else { return };
    let channel = app.keys.cc.channel;
    let points: Vec<_> = cv.ctrls.iter().copied().enumerate().filter(|(_, c)| c.kind == CtrlKind::Cc(controller) && c.channel == channel).collect();
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, TOKENS.panel);
    let mut last = None;
    for (index, c) in &points {
        if let Some(prev) = last {
            let a = at(map, rect, prev);
            let b = at(map, rect, *c);
            painter.line_segment([a, pos2(b.x, a.y)], Stroke::new(1.0, TOKENS.text_dim));
        }
        last = Some(*c);
        let pos = at(map, rect, *c);
        let hit = Rect::from_center_size(pos, vec2(10.0, 10.0));
        if rect.intersects(hit) {
            painter.circle_filled(pos, 3.5, TOKENS.accent);
            layout.points.push((*index, r4(hit)));
        }
    }
    if controller == 64 {
        for (i, (_, point)) in points.iter().enumerate() {
            if point.value < 64 {
                continue;
            }
            let end = points.get(i + 1).map_or(cv.len, |(_, next)| next.tick).min(cv.len);
            if end <= point.tick {
                continue;
            }
            let region = Rect::from_min_max(pos2(map.x(point.tick.max(0)), rect.min.y), pos2(map.x(end), rect.max.y)).intersect(rect);
            if region.is_positive() {
                painter.rect_filled(region, 0.0, TOKENS.accent.gamma_multiply(0.18));
                layout.sustain.push(r4(region));
            }
        }
    }
    let near = |pos: Pos2| points.iter().rev().find(|(_, c)| at(map, rect, *c).distance(pos) <= 7.0).copied();
    let resp = ui.interact(rect, ui.id().with("keys_cc"), Sense::click_and_drag());
    if resp.secondary_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some((index, _)) = near(pos)
    {
        let _ = app.run("midi.cc_delete", json!({"clip": clip.0, "index": index}));
    }
    if resp.drag_started()
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        && let Some((index, old)) = near(pos)
    {
        app.keys.cc.drag = Some((clip, index, old));
    }
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let preview = CtrlEvent {
            tick: map.round(map.tick(pos.x)).clamp(0, cv.len.saturating_sub(1).max(0)),
            channel,
            kind: CtrlKind::Cc(controller),
            value: value(rect, pos.y),
        };
        painter.circle_stroke(at(map, rect, preview), 5.0, Stroke::new(2.0, TOKENS.accent));
    }
    if (resp.clicked() || resp.drag_stopped())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let tick = map.round(map.tick(pos.x)).clamp(0, cv.len.saturating_sub(1).max(0));
        let value = value(rect, pos.y);
        if let Some((drag_clip, index, old)) = app.keys.cc.drag.take() {
            // A clip switch or external edit during a drag must never edit a different point.
            if drag_clip == clip && cv.ctrls.get(index) == Some(&old) && (tick != old.tick || value != old.value) {
                let _ = app.run("midi.cc_edit", json!({"clip": clip.0, "index": index, "tick": tick, "value": value}));
            }
        } else if near(pos).is_none() && cv.len > 0 {
            let _ = app.run("midi.cc_add", json!({"clip": clip.0, "tick": tick, "controller": controller, "channel": channel, "value": value}));
        }
    }
    if resp.drag_stopped() {
        app.keys.cc.cancel();
    }
    resp.on_hover_text(
        "Click or drag empty space to draw a CC point. Drag a point to move it; right-click a point to delete. Values 64–127 hold sustain.",
    );
}

#[cfg(test)]
#[path = "cc_tests.rs"]
mod tests;
