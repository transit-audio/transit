use super::*;

// ---- interaction -----------------------------------------------------------------------------

pub(super) fn snap(s: &Session, at: Samples) -> Samples {
    let on = s.edit.edit_mode.is_grid() || s.edit.flag(ops::SNAP_TO_GRID);
    if on { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

fn clip_at(track: &Track, at: Samples) -> Option<&Clip> {
    track.clips().iter().rev().find(|c| c.range().contains(at))
}

/// The unity-gain end of each existing fade, at the top of the clip body.
fn fade_handle_at(s: &Session, tl: Rect, lane: Rect, clip: &Clip, p: Pos2) -> Option<bool> {
    if clip.length <= 0 || clip.end() <= clip.start {
        return None;
    }
    let y = lane.min.y + 1.0 + clip_name_height(s, lane);
    [(true, clip.fade_in), (false, clip.fade_out)]
        .into_iter()
        .filter(|(_, fade)| fade.len > 0)
        .filter_map(|(fade_in, fade)| {
            let len = fade.len.min(clip.length);
            let at = if fade_in { clip.start.saturating_add(len).min(clip.end()) } else { clip.end().saturating_sub(len).max(clip.start) };
            let head = pos2(x_of(s, tl, at), y);
            let delta = p - head;
            (lane.contains(head) && delta.x.abs() <= 8.0 && delta.y.abs() <= 8.0).then_some((fade_in, delta.length_sq()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(fade_in, _)| fade_in)
}

pub(super) fn near_fade_midpoint(p: Pos2, midpoint: Pos2) -> bool {
    let delta = p - midpoint;
    delta.x.abs() <= 8.0 && delta.y.abs() <= 8.0
}

fn fade_midpoint(s: &Session, tl: Rect, lane: Rect, clip: &Clip, fade_in: bool) -> Option<Pos2> {
    let fade = if fade_in { clip.fade_in } else { clip.fade_out };
    if fade.len <= 0 || clip.length <= 0 {
        return None;
    }
    let len = fade.len.min(clip.length);
    let (a, b) = if fade_in { (clip.start, clip.start.saturating_add(len)) } else { (clip.end().saturating_sub(len), clip.end()) };
    let (a, b) = (x_of(s, tl, a), x_of(s, tl, b));
    if b - a < 32.0 {
        return None;
    }
    let body = Rect::from_min_max(pos2(a, lane.min.y + 1.0 + clip_name_height(s, lane)), pos2(b, lane.max.y - 1.0));
    let p = fade_control_position(fade, a, b, body);
    lane.contains(p).then_some(p)
}

fn fade_curve_handle_at(s: &Session, tl: Rect, lane: Rect, clip: &Clip, p: Pos2) -> Option<bool> {
    [true, false].into_iter().find(|side| fade_midpoint(s, tl, lane, clip, *side).is_some_and(|midpoint| near_fade_midpoint(p, midpoint)))
}

pub const CLIP_FADER_TRAVEL: f32 = 200.0;

/// Bottom-left of an audio clip's visible part, when Clip Gain Info is on or the clip has gain and it fits.
pub fn clip_gain_icon(s: &Session, tl: Rect, lane: Rect, clip: &Clip) -> Option<Rect> {
    let x0 = x_of(s, tl, clip.start).max(lane.min.x);
    let x1 = x_of(s, tl, clip.end()).min(lane.max.x);
    let shown = matches!(clip.content, ClipContent::Audio { .. }) && (s.edit.flag("view.clip.gain_info") || clip.gain_db.abs() > 0.01);
    (shown && x1 - x0 >= 48.0 && lane.height() >= 30.0).then(|| Rect::from_min_size(pos2(x0 + 4.0, lane.max.y - 16.0), vec2(9.0, 13.0)))
}

fn clip_gain_popup(ctx: &egui::Context, fader_bottom: Pos2, pos: f32) {
    let t = Tokens::current();
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("clip_gain_fader")));
    let track = Rect::from_min_max(pos2(fader_bottom.x - 2.0, fader_bottom.y - CLIP_FADER_TRAVEL), pos2(fader_bottom.x + 2.0, fader_bottom.y));
    let panel = Rect::from_min_max(pos2(track.min.x - 22.0, track.min.y - 26.0), pos2(track.max.x + 22.0, track.max.y + 8.0));
    p.rect(panel, 3.0, Color32::from_rgb(38, 38, 40), Stroke::new(1.0, Color32::from_rgb(10, 10, 10)), StrokeKind::Inside);
    p.rect_filled(track, 2.0, t.fader_track);
    let zero = track.max.y - CLIP_FADER_TRAVEL * crate::widgets::clip_gain_to_pos(0.0);
    p.line_segment([pos2(track.min.x - 8.0, zero), pos2(track.min.x - 2.0, zero)], Stroke::new(1.0, Color32::from_rgb(150, 150, 150)));
    let cap = Rect::from_center_size(pos2(track.center().x, track.max.y - CLIP_FADER_TRAVEL * pos), vec2(22.0, 9.0));
    p.rect(cap, 2.0, Color32::from_rgb(210, 210, 210), Stroke::new(1.0, Color32::from_rgb(20, 20, 20)), StrokeKind::Inside);
    let db = crate::widgets::clip_gain_from_pos(pos);
    p.text(pos2(panel.center().x, panel.min.y + 12.0), Align2::CENTER_CENTER, format!("{db:+.1}"), regular(10.5), t.counter_text);
}

pub(super) fn lane_interaction(app: &mut SoundApp, ui: &mut Ui, track: &Track, lane: Rect, tl: Rect) {
    let id = ui.id().with(("lane", track.id.0));
    let resp = ui.interact(lane, id, Sense::click_and_drag());
    // The clip menu is attached before the early return below: once the menu is open the pointer
    // is over the popup, the lane has no hover position, and the rest of this function bails out.
    // The clip is remembered at the right-click so the menu keeps its target while it is used.
    let menu_clip_id = id.with("menu_clip");
    if resp.secondary_clicked() {
        let clip = resp.interact_pointer_pos().and_then(|p| {
            let s = app.engine.session();
            clip_at(track, sample_at(s, tl, p.x).max(0)).map(|c| c.id)
        });
        ui.ctx().data_mut(|d| d.insert_temp(menu_clip_id, clip));
    }
    // The clip may have been removed or moved since the right-click; only use it if it still exists.
    let menu_clip = ui.ctx().data(|d| d.get_temp::<Option<ClipId>>(menu_clip_id)).flatten().filter(|c| track.clips().iter().any(|x| x.id == *c));
    resp.context_menu(|ui| clip_context_menu(app, ui, menu_clip));
    // Audio files dropped from the Clip List.
    if let Some(src) = resp.dnd_release_payload::<crate::DragSource>()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        let s = app.engine.session();
        let at = drag_snap(s, sample_at(s, tl, p.x).max(0), ui.input(|i| i.modifiers.command));
        let spot = Spot::source(s, transit_model::SourceId(src.0), track.id, at);
        spot_or_run(app, spot);
    }
    if resp.dnd_hover_payload::<crate::DragSource>().is_some() {
        ui.painter().rect_stroke(lane, 0.0, Stroke::new(2.0, Tokens::current().accent), StrokeKind::Inside);
    }
    // Audio files dropped from outside onto an instrument track load its sampler.
    if track.kind == transit_model::TrackKind::Instrument {
        crate::sampler_window::drop_target(app, ui.ctx(), lane, track.id);
    }
    let s = app.engine.session_arc();
    let tool = s.edit.tool;
    let mods = ui.input(|i| i.modifiers);
    let Some(p) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(p) } else { p };
    let at_origin = sample_at(&s, tl, origin.x).max(0);
    let auto_view = AutoParam::parse(&track.view);
    let body_hit = clip_at(track, at_origin);
    // Check fade endpoints before clip bodies: a full-length fade can end exactly
    // at the next clip's start, where `clip_at` would otherwise choose that neighbour.
    let fade_handle = if tool == Tool::Smart && auto_view.is_none() {
        track
            .clips()
            .iter()
            .rev()
            .filter(|c| body_hit.is_none_or(|front| front.id == c.id || front.start == c.end()))
            .find_map(|c| fade_handle_at(&s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let curve_handle = if tool == Tool::Smart && auto_view.is_none() && fade_handle.is_none() {
        body_hit.and_then(|c| fade_curve_handle_at(&s, tl, lane, c, origin).map(|side| (c, side)))
    } else {
        None
    };
    let hit = fade_handle.or(curve_handle).map(|(c, _)| c).or(body_hit).cloned();
    let upper = origin.y < lane.center().y;
    let near_edge = |c: &Clip| -> Option<bool> {
        let xs = x_of(&s, tl, c.start);
        let xe = x_of(&s, tl, c.end());
        if (origin.x - xs).abs() < 6.0 {
            Some(true)
        } else if (origin.x - xe).abs() < 6.0 {
            Some(false)
        } else {
            None
        }
    };
    // Effective tool under the Smart tool.
    let eff = match tool {
        Tool::Smart => {
            if let Some(c) = &hit {
                if near_edge(c).is_some() {
                    Tool::Trim
                } else if upper {
                    Tool::Selector
                } else {
                    Tool::Grabber
                }
            } else {
                Tool::Selector
            }
        }
        other => other,
    };
    if resp.hovered() {
        let icon = match eff {
            Tool::Trim => egui::CursorIcon::ResizeHorizontal,
            Tool::Grabber => egui::CursorIcon::Grab,
            Tool::Selector => egui::CursorIcon::Text,
            Tool::Zoom => egui::CursorIcon::ZoomIn,
            Tool::Pencil => egui::CursorIcon::Crosshair,
            _ => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }
    let fade_corner = if tool == Tool::Smart {
        fade_handle.map(|(_, side)| side).or_else(|| {
            hit.as_ref().and_then(|c| {
                let xs = x_of(&s, tl, c.start);
                let xe = x_of(&s, tl, c.end());
                let top = origin.y < lane.min.y + lane.height() * 0.28;
                if !top {
                    None
                } else if (origin.x - xs) > 0.0 && (origin.x - xs) < 14.0 {
                    Some(true)
                } else if (xe - origin.x) > 0.0 && (xe - origin.x) < 14.0 {
                    Some(false)
                } else {
                    None
                }
            })
        })
    } else {
        None
    };
    if fade_corner.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    }
    if curve_handle.is_some() && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let gain_icon_at = |q: Pos2| {
        let c = clip_at(track, sample_at(&s, tl, q.x).max(0)).filter(|_| auto_view.is_none())?;
        clip_gain_icon(&s, tl, lane, c).filter(|r| r.contains(q)).map(|_| c.clone())
    };
    if resp.hovered() && gain_icon_at(p).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let selector_press = eff == Tool::Selector && fade_corner.is_none() && curve_handle.is_none() && gain_icon_at(origin).is_none();
    if selector_press && resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        place_insertion(app, ui, &s, track.id, at, mods.shift);
    }
    let gain_drag = if resp.drag_started() { gain_icon_at(origin) } else { None };
    if let Some(c) = gain_drag {
        // `clip.gain` refuses to run with nothing selected.
        if s.edit.selected_tracks.is_empty() && s.edit.selected_clips.is_empty() {
            let _ = app.engine.execute("edit.select", &json!({"clips": [c.id.0]}));
        }
        let pos = crate::widgets::clip_gain_to_pos(c.gain_db);
        let screen = ui.ctx().content_rect();
        let y = (origin.y + CLIP_FADER_TRAVEL * pos).max(screen.min.y + CLIP_FADER_TRAVEL + 30.0).min(screen.max.y - 10.0);
        let fader_bottom = pos2(origin.x.max(screen.min.x + 26.0), y);
        let fine = mods.command || mods.ctrl;
        app.gesture = Some(Gesture::ClipGain { clip: c.id, track: track.id, pos, anchor: (origin.y, pos), fine, fader_bottom });
    } else if resp.drag_started()
        && let Some((c, fade_in)) = curve_handle
    {
        let fade = if fade_in { c.fade_in } else { c.fade_out };
        let anchor = (origin.y, fade.gain(0.5));
        let height = (lane.height() - 2.0 - clip_name_height(&s, lane)).max(1.0);
        app.gesture = Some(Gesture::FadeCurve { clip: c.id, fade_in, curvature: fade_curve_drag_value(fade, anchor, height, p.y), anchor, height });
        ui.ctx().request_repaint();
    } else if resp.drag_started()
        && let (Some(fade_in), Some(c)) = (fade_corner, hit.as_ref())
    {
        app.gesture = fade_drag_range(c, fade_in, at).map(|range| Gesture::Fade {
            clip: c.id,
            track: track.id,
            fade_in,
            to: if fade_in { range.end } else { range.start },
        });
        ui.ctx().request_repaint();
    } else if resp.drag_started() {
        app.gesture = match eff {
            Tool::Selector | Tool::Smart => Some(Gesture::Select { track: track.id, anchor: snap(&s, at_origin), tracks: vec![track.id] }),
            Tool::Grabber => hit.as_ref().map(|c| {
                let clips = if s.edit.selected_clips.contains(&c.id) { s.edit.selected_clips.clone() } else { vec![c.id] };
                Gesture::MoveClips { clips, grab_at: at_origin, delta: 0, from_track: track.id, to_track: track.id }
            }),
            Tool::Trim => hit.as_ref().map(|c| match near_edge(c) {
                Some(false) => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
                _ if at_origin - c.start < c.end() - at_origin => Gesture::TrimStart { clip: c.id, track: track.id, to: c.start },
                _ => Gesture::TrimEnd { clip: c.id, track: track.id, to: c.end() },
            }),
            Tool::Scrubber => Some(Gesture::Scrub { last: at }),
            Tool::Pencil if auto_view.is_none() && track.kind.is_midi() => {
                hit.is_none().then_some(Gesture::MidiClip { track: track.id, from: at_origin, to: at_origin })
            }
            Tool::Pencil => Some(Gesture::Pencil { track: track.id, points: Vec::new() }),
            Tool::Zoom => Some(Gesture::ZoomBox { start: origin.x }),
        };
    }
    if let Some(Gesture::ClipGain { clip, track: t, pos, anchor, fine, fader_bottom }) = &mut app.gesture
        && *t == track.id
        && resp.dragged()
    {
        // The fader follows the pointer from where it was pressed; ⌘ re-anchors for fine moves.
        if *fine != (mods.command || mods.ctrl) {
            *fine = !*fine;
            *anchor = (p.y, *pos);
        }
        let scale = if *fine { 0.1 } else { 1.0 };
        *pos = (anchor.1 + (anchor.0 - p.y) / CLIP_FADER_TRAVEL * scale).clamp(0.0, 1.0);
        let db = crate::widgets::clip_gain_from_pos(*pos);
        if s.find_clip(*clip).is_some_and(|(_, c)| (c.gain_db - db).abs() > 0.001) {
            let _ = app.engine.execute_merged("clip.gain", &json!({"clips": [clip.0], "db": db}), &format!("clip_gain:{}", clip.0));
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        clip_gain_popup(ui.ctx(), *fader_bottom, *pos);
    }
    if resp.clicked() && curve_handle.is_none() {
        match eff {
            Tool::Grabber => {
                if let Some(c) = &hit {
                    let mut ids = if mods.shift { s.edit.selected_clips.iter().map(|x| x.0).collect::<Vec<_>>() } else { Vec::new() };
                    ids.push(c.id.0);
                    let _ = app.run("edit.select", json!({"clips": ids}));
                    if s.edit.edit_mode == EditMode::Spot {
                        app.dialogs.open = Spot::clips(&s, vec![c.id], None, 0).map(Dialog::Spot);
                    }
                }
            }
            Tool::Trim if s.edit.edit_mode == EditMode::Spot && hit.is_some() => {
                if let Some(c) = &hit {
                    let start = at - c.start < c.end() - at;
                    app.dialogs.open = Spot::trim(&s, c.id, start, if start { c.start } else { c.end() }).map(Dialog::Spot);
                }
            }
            Tool::Zoom => {
                let _ = app.run(if mods.alt { "view.zoom_out" } else { "view.zoom_in" }, json!({}));
            }
            Tool::Pencil => {
                if let Some(param) = &auto_view {
                    let (lo, hi, _) = param.range();
                    let (lo, hi) = if matches!(param, AutoParam::Volume | AutoParam::SendLevel(_)) {
                        (-60.0, 12.0)
                    } else if matches!(param, AutoParam::Plugin { .. }) {
                        plugin_range(track, param)
                    } else {
                        (lo, hi)
                    };
                    let k = ((lane.max.y - 4.0 - p.y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
                    let _ = app.run(
                        "automation.set_point",
                        json!({"track": track.id.0, "param": automation_id(param), "at": at, "value": lo + (hi - lo) * k}),
                    );
                } else if track.kind.is_midi() && hit.is_none() {
                    new_midi_clip(app, &s, track.id, at, at);
                }
            }
            _ if !selector_press => place_insertion(app, ui, &s, track.id, at, mods.shift),
            _ => {}
        }
    }
    if resp.double_clicked()
        && curve_handle.is_none()
        && eff != Tool::Pencil
        && let Some(c) = &hit
    {
        let _ = app.run("edit.select", json!({"clips": [c.id.0]}));
        if eff == Tool::Grabber && matches!(c.content, ClipContent::Midi { .. }) {
            app.ui.show_midi_editor = true;
        }
    }
}

fn place_insertion(app: &mut SoundApp, ui: &Ui, s: &Session, track: TrackId, at: Samples, extend: bool) {
    let at = snap(s, at);
    let sel = s.edit.selection;
    let params = if extend && s.edit.selected_tracks.contains(&track) {
        let r = if at < sel.start { Range::new(at, sel.end) } else { Range::new(sel.start, at) };
        json!({"start": r.start, "end": r.end})
    } else if extend && !s.edit.selected_tracks.is_empty() {
        let tracks: Vec<u64> = s.edit.selected_tracks.iter().chain([&track]).map(|t| t.0).collect();
        json!({"tracks": tracks, "start": sel.start.min(at), "end": sel.end.max(at)})
    } else {
        json!({"tracks": [track.0], "start": at})
    };
    let rev = app.engine.revision;
    let _ = app.run("edit.select", params);
    if app.engine.revision != rev {
        ui.ctx().request_discard("insertion point moved");
    }
}

/// The same bounded range drives the preview and the committed fade.
fn fade_drag_range(clip: &Clip, fade_in: bool, to: Samples) -> Option<Range> {
    let end = clip.end();
    if clip.length <= 0 || end <= clip.start {
        return None;
    }
    Some(if fade_in {
        Range::new(clip.start, to.clamp(clip.start.saturating_add(1), end))
    } else {
        Range::new(to.clamp(clip.start, end.saturating_sub(1)), end)
    })
}

pub(super) fn fade_drag_preview(clip: &Clip, fade_in: bool, to: Samples) -> Option<transit_model::Fade> {
    let range = fade_drag_range(clip, fade_in, to)?;
    let original = if fade_in { clip.fade_in } else { clip.fade_out };
    Some(transit_model::Fade {
        len: range.end.saturating_sub(range.start),
        shape: if original.len > 0 { original.shape } else { transit_model::FadeShape::SCurve },
        curvature: if original.len > 0 { original.curvature } else { 0.0 },
    })
}

fn fade_curve_drag_value(fade: transit_model::Fade, anchor: (f32, f32), height: f32, y: f32) -> f32 {
    fade.curvature_for_midpoint_gain(anchor.1 + (anchor.0 - y) / height.max(1.0))
}

/// Runs before anything is painted, against the last frame's layout.
pub(super) fn drive_drag(app: &mut SoundApp, ctx: &egui::Context) {
    if app.gesture.is_none() || matches!(app.gesture, Some(Gesture::Fader { .. })) {
        return;
    }
    let (s, tl) = (app.engine.session_arc(), rect(app.edit_layout.timeline));
    let p = ctx.pointer_latest_pos();
    if !ctx.input(|i| i.pointer.any_down()) {
        finish_drag(app, &s, tl, p);
        return;
    }
    let Some(p) = p else { return };
    let at = sample_at(&s, tl, p.x).max(0);
    let inside = pos2(p.x.max(tl.min.x).min(tl.max.x), p.y.max(tl.min.y).min(tl.max.y));
    let rows = &app.edit_layout.rows;
    let gap = |r: &[f32; 4]| (r[1] - inside.y).max(inside.y - r[3]).max(0.0);
    let to_track = rows.iter().min_by(|a, b| gap(&a.1).total_cmp(&gap(&b.1))).map(|(i, _)| TrackId(*i));
    let mut invalid_fade = false;
    match &mut app.gesture {
        Some(Gesture::Select { track, tracks, .. }) => {
            let row_of = |t: TrackId| rows.iter().position(|(i, _)| *i == t.0);
            if let (Some(a), Some(b)) = (row_of(*track), to_track.and_then(row_of)) {
                *tracks = rows.get(a.min(b)..=a.max(b)).unwrap_or_default().iter().map(|(i, _)| TrackId(*i)).collect();
            }
        }
        Some(Gesture::MoveClips { clips, grab_at, delta, from_track, to_track: tt }) => {
            if let Some(t2) = to_track {
                *tt = t2;
            }
            let plan = drag_plan(ctx, &s, app.engine.revision, clips, (*from_track, *tt), *grab_at);
            *delta = move_delta(&plan, &s, *from_track, at - *grab_at, ctx.input(|i| i.modifiers.command));
        }
        Some(Gesture::TrimStart { to, .. } | Gesture::TrimEnd { to, .. }) => *to = drag_snap(&s, at, ctx.input(|i| i.modifiers.command)),
        Some(Gesture::MidiClip { to, .. }) => *to = at,
        Some(Gesture::Fade { clip, fade_in, to, .. }) => {
            if let Some(range) = s.find_clip(*clip).and_then(|(_, c)| fade_drag_range(c, *fade_in, at)) {
                *to = if *fade_in { range.end } else { range.start };
            } else {
                invalid_fade = true;
            }
        }
        Some(Gesture::FadeCurve { clip, fade_in, curvature, anchor, height }) => {
            if let Some((_, c)) = s.find_clip(*clip).filter(|(_, c)| c.length > 0) {
                let fade = if *fade_in { c.fade_in } else { c.fade_out };
                if fade.len > 0 {
                    *curvature = fade_curve_drag_value(fade, *anchor, *height, p.y);
                } else {
                    invalid_fade = true;
                }
            } else {
                invalid_fade = true;
            }
        }
        Some(Gesture::Scrub { last }) => {
            *last = at;
            let _ = app.engine.execute("transport.locate", &json!({"at": at}));
        }
        Some(Gesture::Pencil { track, points }) => {
            if let Some(t) = s.track(*track)
                && let Some(param) = AutoParam::parse(&t.view)
                && let Some((_, row)) = rows.iter().find(|(i, _)| *i == t.id.0)
            {
                let lane = lane_rect(t, rect(*row), tl);
                let (lo, hi, _) = param.range();
                let (lo, hi) = if matches!(param, AutoParam::Volume | AutoParam::SendLevel(_)) {
                    (-60.0, 12.0)
                } else if matches!(param, AutoParam::Plugin { .. }) {
                    plugin_range(t, &param)
                } else {
                    (lo, hi)
                };
                let k = ((lane.max.y - 4.0 - p.y) / (lane.height() - 8.0)).clamp(0.0, 1.0);
                if points.last().is_none_or(|(a, _)| (at - a).abs() as f64 > s.edit.zoom.samples_per_px * 3.0) {
                    points.push((at, lo + (hi - lo) * k));
                }
            }
        }
        _ => {}
    }
    if invalid_fade {
        app.gesture = None;
    }
    if let Some(Gesture::Select { anchor, tracks, .. }) = &app.gesture {
        let r = Range::new(*anchor, snap(&s, at));
        if *tracks != s.edit.selected_tracks || r != s.edit.selection {
            let ids: Vec<u64> = tracks.iter().map(|x| x.0).collect();
            let _ = app.run("edit.select", json!({"tracks": ids, "start": r.start, "end": r.end}));
        }
    }
    let out = p - inside;
    if out != egui::Vec2::ZERO && matches!(app.gesture, Some(Gesture::Select { .. } | Gesture::MoveClips { .. })) {
        let k = 12.0 * ctx.input(|i| i.stable_dt).min(0.1);
        if out.x != 0.0 {
            let _ = app.engine.execute("view.scroll", &json!({"by_px": out.x * k}));
        }
        app.edit_layout.scroll_by(out.y * k, tl.height());
        ctx.request_repaint();
    }
}

fn finish_drag(app: &mut SoundApp, s: &Session, tl: Rect, p: Option<Pos2>) {
    match app.gesture.take() {
        Some(Gesture::MoveClips { clips, delta, from_track, to_track, .. }) => drop_clips(app, clips, delta, from_track, to_track),
        Some(Gesture::Fade { clip, fade_in, to, .. }) => {
            let to = p.map_or(to, |p| sample_at(s, tl, p.x).max(0));
            if let Some(fade) = s.find_clip(clip).and_then(|(_, c)| fade_drag_preview(c, fade_in, to)) {
                let _ = app.run(
                    "edit.fades_create",
                    json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": fade.len, "shape": fade.shape.label(), "curvature": fade.curvature}),
                );
            }
        }
        Some(Gesture::FadeCurve { clip, fade_in, curvature, anchor, height }) => {
            if let Some((_, c)) = s.find_clip(clip).filter(|(_, c)| c.length > 0) {
                let fade = if fade_in { c.fade_in } else { c.fade_out };
                let curvature = p.map_or(curvature, |p| fade_curve_drag_value(fade, anchor, height, p.y));
                if fade.len > 0 && (fade.curvature - curvature).abs() > 1e-6 {
                    let _ = app.run(
                        "edit.fades_create",
                        json!({"clip": clip.0, "side": if fade_in { "in" } else { "out" }, "length": fade.len, "shape": fade.shape.label(), "curvature": curvature}),
                    );
                }
            }
        }
        Some(Gesture::TrimStart { clip, to, .. }) => trim_clip(app, clip, to, true),
        Some(Gesture::TrimEnd { clip, to, .. }) => trim_clip(app, clip, to, false),
        Some(Gesture::MidiClip { track, from, to }) => new_midi_clip(app, s, track, from, to),
        Some(Gesture::Pencil { track, points }) => {
            if let Some(param) = s.track(track).and_then(|t| AutoParam::parse(&t.view)) {
                let pid = automation_id(&param);
                for (a, v) in points {
                    let _ = app.run("automation.set_point", json!({"track": track.0, "param": pid, "at": a, "value": v}));
                }
            }
        }
        Some(Gesture::ZoomBox { start }) => {
            let Some(p) = p else { return };
            let a = sample_at(s, tl, start.min(p.x));
            let b = sample_at(s, tl, start.max(p.x));
            if b > a {
                let spp = ((b - a) as f64 / f64::from(tl.width())).max(0.25);
                let _ = app.run("view.zoom_set", json!({"samples_per_px": spp}));
                let _ = app.run("view.scroll", json!({"to": a}));
            }
        }
        _ => {}
    }
}

fn automation_id(p: &AutoParam) -> String {
    match p {
        AutoParam::Volume => "volume".into(),
        AutoParam::Pan(0) => "pan".into(),
        AutoParam::Pan(_) => "pan2".into(),
        AutoParam::Mute => "mute".into(),
        AutoParam::SendLevel(i) => format!("send_{}_level", (b'a' + i) as char),
        AutoParam::SendPan(i) => format!("send_{}_pan", (b'a' + i) as char),
        AutoParam::SendMute(i) => format!("send_{}_mute", (b'a' + i) as char),
        AutoParam::Plugin { slot, param } => format!("plugin:{}:{param}", (b'a' + slot) as char),
    }
}

/// Grid snapping while dragging; holding Cmd/Ctrl swaps Grid and Slip.
fn drag_snap(s: &Session, at: Samples, flip: bool) -> Samples {
    if s.edit.edit_mode.is_grid() != flip { s.edit.grid.snap(at, s.sample_rate, &s.tempo, s.frame_rate) } else { at }
}

/// What a clip drag works out once per target track rather than every frame: where the tracks go
/// (None when the clips can't land there), each destination's earliest clip and Shuffle slots,
/// the clips that move (track, start, length) and the clip Grid snaps.
pub(super) struct DragPlan {
    pub(super) dest: Option<Vec<(TrackId, TrackId)>>,
    pub(super) groups: Vec<(TrackId, Samples, Vec<Samples>)>,
    pub(super) moving: Vec<(TrackId, Samples, Samples)>,
    grabbed: Option<Clip>,
}

/// The [`DragPlan`] for `clips` grabbed at `grab_at` on track `from` and dragged onto `to`, kept
/// until the document, the Edit mode or the target changes.
pub(super) fn drag_plan(
    ctx: &egui::Context,
    s: &Session,
    revision: u64,
    clips: &[ClipId],
    (from, to): (TrackId, TrackId),
    grab_at: Samples,
) -> Arc<DragPlan> {
    type Key = (u64, EditMode, Vec<ClipId>, TrackId, TrackId, Samples);
    let id = egui::Id::new("clip_drag_plan");
    let key: Key = (revision, s.edit.edit_mode, clips.to_vec(), from, to, grab_at);
    if let Some((k, plan)) = ctx.data(|d| d.get_temp::<(Key, Arc<DragPlan>)>(id))
        && k == key
    {
        return plan;
    }
    let dest = if to == from { Some(Vec::new()) } else { ops::destinations(s, clips, from, to) };
    let groups = match &dest {
        Some(d) if s.edit.edit_mode == EditMode::Shuffle => {
            ops::shuffle_groups(s, clips, d).into_iter().map(|(t, first)| (t, first, ops::shuffle_slots(s, t, clips))).collect()
        }
        _ => Vec::new(),
    };
    let ids: HashSet<ClipId> = clips.iter().copied().collect();
    let movable = |c: &&Clip| ids.contains(&c.id) && !ops::locked(c);
    let moving = s.tracks.iter().flat_map(|t| t.clips().iter().filter(movable).map(|c| (t.id, c.start, c.length))).collect();
    let mine: Vec<&Clip> = s.track(from).map(|t| t.clips().iter().filter(movable).collect()).unwrap_or_default();
    let grabbed = mine.iter().rev().find(|c| c.range().contains(grab_at)).or_else(|| mine.iter().min_by_key(|c| c.start)).map(|c| (*c).clone());
    let plan = Arc::new(DragPlan { dest, groups, moving, grabbed });
    ctx.data_mut(|d| d.insert_temp(id, (key, Arc::clone(&plan))));
    plan
}

/// How far a Grabber drag by `raw` from track `from` takes the clips in the current Edit mode.
/// Shuffle lands the earliest clip bound for the target track on a slot there, as
/// `edit.move_clips` does; Grid snaps the grabbed clip.
fn move_delta(plan: &DragPlan, s: &Session, from: TrackId, raw: Samples, flip: bool) -> Samples {
    let mode = s.edit.edit_mode;
    if mode == EditMode::Shuffle {
        let to = plan.dest.as_ref().map_or(from, |d| ops::lands(d, from));
        plan.groups
            .iter()
            .find(|g| g.0 == to)
            .map_or(raw, |(_, first, slots)| ops::nearest_slot(slots, first.saturating_add(raw)).saturating_sub(*first))
    } else if mode.is_grid() != flip
        && let Some(c) = &plan.grabbed
    {
        ops::grid_delta(s, c, raw, mode == EditMode::GridRelative)
    } else {
        raw
    }
}

/// The dragged clips with one from the grabbed track first: `edit.move_clips` sends that track to
/// the target and the others along with it.
fn grabbed_first(s: &Session, clips: &[ClipId], from: TrackId) -> Vec<ClipId> {
    let mut v = clips.to_vec();
    v.sort_by_key(|c| s.find_clip(*c).is_none_or(|(t, _)| t != from));
    v
}

fn drop_clips(app: &mut SoundApp, clips: Vec<ClipId>, delta: Samples, from: TrackId, to: TrackId) {
    let s = app.engine.session();
    let spot = Spot::clips(s, grabbed_first(s, &clips, from), (to != from).then_some(to), delta);
    spot_or_run(app, spot);
}

fn trim_clip(app: &mut SoundApp, clip: ClipId, to: Samples, start: bool) {
    let spot = Spot::trim(app.engine.session(), clip, start, to);
    spot_or_run(app, spot);
}

/// Spot mode asks for exact positions first; the other modes make the edit straight away.
fn spot_or_run(app: &mut SoundApp, spot: Option<Spot>) {
    let Some(spot) = spot else { return };
    if app.engine.session().edit.edit_mode == EditMode::Spot {
        app.dialogs.open = Some(Dialog::Spot(spot));
    } else if let Some((id, params)) = spot.command(app.engine.session()) {
        let _ = app.run(&id, params);
    }
}

/// Whole bars from `from` to `to` (at least one), inside the gap between clips where `from` is.
pub(super) fn midi_clip_span(s: &Session, track: TrackId, from: Samples, to: Samples) -> Option<Range> {
    let clips = s.track(track)?.clips();
    let bar = GridValue::Note { value: NoteValue::Bar, dotted: false, triplet: false };
    let start = bar.line_at_or_before(from.min(to), s.sample_rate, &s.tempo, s.frame_rate);
    let last = from.max(to);
    let mut end = bar.line_at_or_before(last, s.sample_rate, &s.tempo, s.frame_rate);
    if end < last || end <= start {
        end = bar.next_line(end, s.sample_rate, &s.tempo, s.frame_rate);
    }
    let prev = clips.iter().map(Clip::end).filter(|&e| e <= from).max().unwrap_or(start);
    let next = clips.iter().map(|c| c.start).filter(|&b| b > from).min().unwrap_or(end);
    let r = Range::new(start.max(prev), end.min(next));
    (!r.is_empty()).then_some(r)
}

fn new_midi_clip(app: &mut SoundApp, s: &Session, track: TrackId, from: Samples, to: Samples) {
    if let Some(r) = midi_clip_span(s, track, from, to)
        && let Ok(v) = app.run("midi.new_clip", json!({"track": track.0, "start": r.start, "end": r.end}))
    {
        let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
    }
}

fn clip_context_menu(app: &mut SoundApp, ui: &mut Ui, clip: Option<ClipId>) {
    let Some(c) = clip else {
        if ui.button("Paste").clicked() {
            let _ = app.run("edit.paste", json!({}));
        }
        return;
    };
    let ids = json!([c.0]);
    for (label, cmd) in [
        ("Mute/Unmute", "edit.mute_clips"),
        ("Edit Lock/Unlock", "clip.edit_lock"),
        ("Time Lock/Unlock", "clip.time_lock"),
        ("Quantize to Grid", "clip.quantize_to_grid"),
        ("Render Clip Gain", "clip.gain_render"),
        ("Bring to Front", "clip.bring_to_front"),
        ("Send to Back", "clip.send_to_back"),
    ] {
        if ui.button(label).clicked() {
            let _ = app.run(cmd, json!({"clips": ids}));
        }
    }
    ui.separator();
    if ui.button("Rename...").clicked() {
        let name = app.engine.session().find_clip(c).map(|(_, x)| x.name.clone()).unwrap_or_default();
        app.dialogs.open_rename_clip(c, &name);
    }
    if ui.button("Delete").clicked() {
        let _ = app.run("edit.select", json!({"clips": ids}));
        let _ = app.run("edit.clear", json!({}));
    }
}

#[cfg(test)]
#[path = "tests/interaction.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/fades.rs"]
mod fade_tests;
