//! Transient vertical zoom: preview in geometry, one ordinary command at a boundary.

use super::*;
use transit_model::TrackHeight;

#[derive(Debug)]
enum Input {
    Drag { press_y: f32 },
    Wheel { remainder: f64 },
}

#[derive(Debug)]
pub(crate) struct HeightGesture {
    anchor: TrackId,
    originals: Vec<(TrackId, TrackHeight)>,
    start: f32,
    points: f32,
    input: Input,
}

impl HeightGesture {
    fn new(app: &StudioApp, anchor: TrackId, all: bool, input: Input) -> Option<Self> {
        let s = app.engine.session();
        let start = s.track(anchor)?.height.points();
        let originals = s.tracks.iter().filter(|t| all || t.id == anchor).map(|t| (t.id, t.height)).collect();
        Some(Self { anchor, originals, start, points: start, input })
    }

    fn changed(&self) -> bool {
        // Returning an all-track drag to its anchor restores unequal original previews too.
        self.points != self.start
    }

    fn preview(&self, id: TrackId) -> Option<f32> {
        self.originals.iter().find(|(t, _)| *t == id).map(|(_, h)| if self.changed() { self.points } else { h.points() })
    }
}

pub(crate) fn points(app: &StudioApp, track: &Track) -> f32 {
    app.arrange.height_gesture.as_ref().and_then(|g| g.preview(track.id)).unwrap_or_else(|| track.height.points())
}

pub(crate) fn replacing(id: &str) -> bool {
    matches!(id, "session.new" | "session.open" | "session.close" | "session.revert")
}

fn query(id: &str) -> bool {
    transit_engine::cmd::command_specs().iter().any(|s| s.id == id && !s.journal && !s.undoable && !s.view)
}

impl StudioApp {
    pub(crate) fn drop_height_preview(&mut self) {
        self.arrange.height_gesture = None;
        self.arrange.height_anchor = None;
    }

    pub(crate) fn commit_height_preview(&mut self) {
        let Some(gesture) = self.arrange.height_gesture.take() else { return };
        if let Some(row) = self.arrange.layout.tracks.iter().find(|r| r.id == gesture.anchor.0) {
            self.arrange.height_anchor = Some((gesture.anchor, row.header[1]));
        }
        if !gesture.changed() {
            return;
        }
        let tracks: Vec<_> = gesture.originals.iter().map(|(id, _)| id.0).collect();
        if let Err(error) = self.engine.execute("track.height", &json!({"tracks": tracks, "height": gesture.points})) {
            self.status = error.to_string();
            log::warn!("track.height: {error}");
        }
    }

    pub(crate) fn before_height_command(&mut self, id: &str) {
        if replacing(id) {
            self.drop_height_preview();
        } else if !query(id) {
            self.commit_height_preview();
        }
    }

    /// Direct UI dispatch sites retain their existing error and merge handling.
    pub(crate) fn execute_ui(&mut self, id: &str, params: &serde_json::Value) -> transit_engine::Result<serde_json::Value> {
        self.before_height_command(id);
        self.engine.execute(id, params)
    }

    pub(crate) fn execute_ui_merged(&mut self, id: &str, params: &serde_json::Value, key: &str) -> transit_engine::Result<serde_json::Value> {
        self.before_height_command(id);
        self.engine.execute_merged(id, params, key)
    }
}

fn alt_only(m: egui::Modifiers) -> bool {
    m.alt && !m.ctrl && !m.command && !m.mac_cmd && !m.shift
}

/// egui scales wheel units before UI handlers run. Keep hostile deltas finite there too.
pub(crate) fn bound_raw_wheel(raw: &mut egui::RawInput) {
    for event in &mut raw.events {
        if let egui::Event::MouseWheel { delta, .. } = event {
            for value in [&mut delta.x, &mut delta.y] {
                *value = if value.is_finite() { value.clamp(-1_000_000.0, 1_000_000.0) } else { 0.0 };
            }
        }
    }
}

/// A bounded signed remainder, with constant work even for hostile wheel magnitudes.
fn notches(remainder: &mut f64, unit: egui::MouseWheelUnit, delta: f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    let units = match unit {
        egui::MouseWheelUnit::Point => f64::from(delta) / 40.0,
        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => f64::from(delta),
    };
    let sum = (*remainder + units).clamp(-1_000_000.0, 1_000_000.0);
    let whole = sum.trunc();
    *remainder = sum - whole;
    whole as i32
}

fn modal(app: &StudioApp, ctx: &egui::Context) -> bool {
    app.dialog.is_some() || ctx.memory(|m| m.top_modal_layer().is_some())
}

/// Runs before other UI handlers. Queries and idle frames leave a wheel gesture intact.
pub(crate) fn boundaries(app: &mut StudioApp, ctx: &egui::Context) {
    if ctx.input(|i| i.focused && i.pointer.primary_released())
        && let Some(g) = &mut app.arrange.height_gesture
        && let Input::Drag { press_y } = g.input
        && let Some(p) = ctx.input(|i| i.pointer.latest_pos())
    {
        g.points = (g.start + p.y - press_y).clamp(16.0, 440.0).round();
    }
    let Some(g) = &app.arrange.height_gesture else { return };
    let covered = modal(app, ctx);
    let end = ctx.input(|i| {
        if !i.focused || covered {
            return true;
        }
        match g.input {
            Input::Drag { .. } => i.pointer.primary_released(),
            Input::Wheel { .. } => {
                !alt_only(i.modifiers)
                    || i.pointer.any_pressed()
                    || !i.pointer.hover_pos().is_some_and(|p| {
                        app.arrange.layout.tracks.iter().find(|r| r.id == g.anchor.0).is_some_and(|r| {
                            let list = crate::finish::rect(app.arrange.layout.track_list);
                            let timeline = crate::finish::rect(app.arrange.layout.timeline);
                            crate::finish::rect(r.header).intersect(list).contains(p) || crate::finish::rect(r.lane).intersect(timeline).contains(p)
                        })
                    })
            }
        }
    });
    if end {
        app.commit_height_preview();
    }
}

/// Input precedes geometry and paint, so the current frame exposes the current preview.
pub(super) fn input(app: &mut StudioApp, ui: &mut Ui, list: Rect, tl: Rect) {
    let s = app.engine.session_arc();
    let rows: Vec<_> = s.tracks.iter().filter(|t| shown(app, &s, t)).collect();
    let geo = RowGeo::new(app, &rows);
    let pos = ui.input(|i| i.pointer.hover_pos());
    let over = pos.filter(|p| (list.contains(*p) || tl.contains(*p)) && ui.rect_contains_pointer(list.union(tl)));
    let hovered = over.and_then(|p| {
        geo.index_at(p.y - tl.min.y + app.arrange.vscroll).filter(|i| p.y < tl.min.y + geo.tops[*i] - app.arrange.vscroll + geo.main[*i])
    });
    // A press and its first movement can arrive in one frame. Capture the press event,
    // including its modifiers, rather than the frame's final hover position/modifiers.
    let press = ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers } => Some((*pos, *modifiers)),
            _ => None,
        })
    });
    if !modal(app, ui.ctx())
        && let Some((p, modifiers)) = press
        && list.intersect(ui.clip_rect()).contains(p)
        && ui.ctx().layer_id_at(p) == Some(ui.layer_id())
        && let Some(i) = geo.index_at(p.y - tl.min.y + app.arrange.vscroll)
        && (tl.min.y + geo.tops[i] - app.arrange.vscroll + geo.main[i] - 4.0..tl.min.y + geo.tops[i] - app.arrange.vscroll + geo.main[i])
            .contains(&p.y)
    {
        app.commit_height_preview();
        app.arrange.height_gesture = HeightGesture::new(app, rows[i].id, modifiers.alt, Input::Drag { press_y: p.y });
        app.arrange.header_drag = None;
        app.arrange.gesture = None;
    }
    if let Some(g) = &mut app.arrange.height_gesture
        && let Input::Drag { press_y } = g.input
        && let Some(p) = ui.input(|i| i.pointer.latest_pos())
    {
        g.points = (g.start + p.y - press_y).clamp(16.0, 440.0).round();
    }
    let events = ui.input(|i| i.events.clone());
    let mut consumed = false;
    for event in events {
        if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = event {
            if alt_only(modifiers)
                && !modal(app, ui.ctx())
                && let Some(i) = hovered
            {
                let id = rows[i].id;
                if app.arrange.height_gesture.as_ref().is_some_and(|g| g.anchor != id || matches!(g.input, Input::Drag { .. })) {
                    app.commit_height_preview();
                }
                if app.arrange.height_gesture.is_none() {
                    app.arrange.height_gesture = HeightGesture::new(app, id, false, Input::Wheel { remainder: 0.0 });
                }
                if let Some(g) = &mut app.arrange.height_gesture
                    && let Input::Wheel { remainder } = &mut g.input
                {
                    let delta = notches(remainder, unit, delta.y).saturating_mul(8);
                    g.points = (g.points as i32).saturating_add(delta).clamp(16, 440) as f32;
                }
                consumed = true;
            } else {
                app.arrange.height_wheel_consumed = false;
            }
        }
    }
    if consumed {
        app.arrange.height_wheel_consumed = true;
        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::MouseWheel { modifiers, .. } if alt_only(*modifiers))));
    }
    if app.arrange.height_wheel_consumed {
        ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    if let Some(g) = &app.arrange.height_gesture
        && let Some(i) = rows.iter().position(|t| t.id == g.anchor)
    {
        app.arrange.height_anchor = Some((g.anchor, tl.min.y + geo.tops[i] - app.arrange.vscroll));
    }
}

pub(super) fn anchor(app: &mut StudioApp, rows: &[&Track], geo: &RowGeo, tl: Rect) {
    if let Some((id, y)) = app.arrange.height_anchor.take()
        && let Some(i) = rows.iter().position(|t| t.id == id)
    {
        app.arrange.vscroll = tl.min.y + geo.tops[i] - y;
    }
}

pub(super) fn handle(app: &StudioApp, ui: &mut Ui, r: Rect, list: Rect, row: &mut TrackRow) {
    let h = Rect::from_min_max(pos2(r.min.x, r.max.y - 4.0), r.max).intersect(list);
    if !h.is_positive() {
        return;
    }
    row.height_handle = Some(r4(h));
    let response = ui.interact(h, ui.id().with(("track_height", row.id)), Sense::drag());
    crate::help::hover(ui, &response, crate::help::Id::ArrangeHeaderHeight);
    if response.hovered() || app.arrange.height_gesture.as_ref().is_some_and(|g| g.anchor.0 == row.id && matches!(g.input, Input::Drag { .. })) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    response.on_hover_text(format!("{} points. Drag to resize; Alt-drag resizes every track.", row.height));
}

pub(crate) fn menu(app: &mut StudioApp, ui: &mut Ui, items: &mut Vec<crate::mixer::MenuItem>) {
    let response = ui.menu_button("Track Height", |ui| {
        for (label, command, params) in [
            ("Increase All", "track.height_zoom", json!({"direction": "in"})),
            ("Decrease All", "track.height_zoom", json!({"direction": "out"})),
            ("Reset Track Heights", "track.height_reset", json!({})),
        ] {
            let button = ui.button(label);
            crate::help::hover(ui, &button, crate::help::Id::ViewTrackHeight);
            items.push(crate::mixer::MenuItem::at(ui, label.into(), command.into(), button.rect));
            if button.clicked() {
                let _ = app.run(command, params);
                ui.close();
            }
        }
    });
    crate::help::hover(ui, &response.response, crate::help::Id::ViewTrackHeight);
    items.push(crate::mixer::MenuItem::at(ui, "Track Height".into(), String::new(), response.response.rect));
}

#[cfg(test)]
#[path = "track_height_tests.rs"]
mod tests;
