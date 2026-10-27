use super::*;

/// Captions over the header columns, boxed in their own row under the rulers.
pub(super) fn column_titles(ui: &Ui, rulers: Rect, cols: &[Column]) {
    if cols.is_empty() {
        return;
    }
    let t = Tokens::current();
    let line = Stroke::new(1.0, t.border);
    let mut x = rulers.min.x + HEADER_W;
    let end = rulers.min.x + header_width(cols);
    let row = Rect::from_min_max(pos2(x, rulers.max.y - COLUMN_TITLES_H - 3.0), pos2(end, rulers.max.y));
    ui.painter().rect_filled(row, 0.0, t.panel_bg2);
    ui.painter().hline(rulers.min.x..=end, row.min.y + 0.5, line);
    ui.painter().hline(rulers.min.x..=end, row.max.y - 0.5, line);
    ui.painter().vline(end - 0.5, row.y_range(), line);
    for &(_, _, title, w) in cols {
        ui.painter().vline(x + 0.5, row.y_range(), line);
        ui.painter().text(pos2(x + w / 2.0, row.center().y), Align2::CENTER_CENTER, title, bold(10.0), t.section_label);
        x += w;
    }
}

/// The Edit window view selector: which columns sit beside the track headers.
pub(super) fn views_button(app: &mut SoundApp, ui: &mut Ui, rulers: Rect) {
    let r = Rect::from_min_size(rulers.min + vec2(6.0, 5.0), vec2(50.0, 18.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    let resp = crate::widgets::selector_box(&mut child, r.width(), r.height(), "Views", Tokens::current().text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, child.is_enabled(), "Views"));
    let resp = resp.on_hover_text("Columns beside the track headers: inserts, sends, I/O…");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let shown = edit_columns(app.engine.session());
        for (id, label, _, _) in COLUMNS {
            if ui.add(egui::Button::new(label).selected(shown.iter().any(|c| c.0 == id))).clicked() {
                let _ = app.run(&format!("view.edit_{id}"), json!({}));
            }
        }
        ui.separator();
        if ui.button("All").clicked() {
            let _ = app.run("view.edit_all", json!({}));
        }
        if ui.button("Minimal").clicked() {
            let _ = app.run("view.edit_minimal", json!({}));
        }
    });
}

// ---- track rows ------------------------------------------------------------------------------

pub(super) fn track_row(app: &mut SoundApp, ui: &mut Ui, id: TrackId, row: Rect, tl: Rect, cols: &[Column]) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let Some(track) = s.track(id) else { return };
    let selected = s.edit.selected_tracks.contains(&id);
    let head = Rect::from_min_max(row.min, pos2(row.min.x + HEADER_W, row.max.y));
    let (head_bg, lane_bg) = crate::theme::track_backgrounds(track.color, selected, app.ui.track_tint);
    let mut x = head.max.x;
    for &(col, _, _, w) in cols {
        header_column(app, ui, track, col, Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height())), head_bg);
        x += w;
    }
    let main_h = track.height.points();
    let lane = lane_rect(track, row, tl);
    ui.painter().rect_filled(head, 0.0, head_bg);
    ui.painter().rect_filled(Rect::from_min_size(head.min, vec2(STRIP_W, head.height())), 0.0, rgb(track.color));
    ui.painter().line_segment([pos2(head.min.x, row.max.y + 0.5), pos2(tl.max.x, row.max.y + 0.5)], Stroke::new(1.0, t.border));
    ui.painter().hline(head.min.x + STRIP_W..=head.max.x, head.min.y + 0.5, Stroke::new(1.0, t.edge_light));
    track_header(app, ui, track, head, selected, head_bg);
    // Lane.
    ui.painter().vline(tl.min.x - 0.5, row.y_range(), Stroke::new(1.0, t.border));
    let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
    painter.rect_filled(lane, 0.0, lane_bg);
    let s = app.engine.session_arc();
    // Shown grid lines go over the clips; otherwise the bar lines stay behind them.
    if !s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    let view = Range::new(sample_at(&s, tl, tl.min.x), sample_at(&s, tl, tl.max.x));
    let auto_view = AutoParam::parse(&track.view);
    for clip in track.clips() {
        if !clip.range().overlaps(&view) {
            continue;
        }
        draw_clip(app, &painter, &s, track, clip, tl, lane, auto_view.is_some());
    }
    draw_live_take(app, &painter, &s, track, tl, lane);
    crate::video_track::draw_lane(app, &painter, &s, track, tl, lane);
    if s.edit.grid_lines {
        grid_lines(&painter, &s, tl, lane);
    }
    if let Some(p) = &auto_view {
        draw_automation(&painter, &s, track, p, tl, lane);
    }
    if track.kind == TrackKind::Master || track.kind == TrackKind::Aux || track.kind == TrackKind::Vca {
        let p = auto_view.clone().unwrap_or(AutoParam::Volume);
        draw_automation(&painter, &s, track, &p, tl, lane);
    }
    // Selection highlight on this track.
    let sel = s.edit.selection;
    if selected && !sel.is_empty() {
        let x0 = x_of(&s, tl, sel.start).max(lane.min.x);
        let x1 = x_of(&s, tl, sel.end).min(lane.max.x);
        if x1 > x0 {
            painter.rect_filled(Rect::from_min_max(pos2(x0, lane.min.y), pos2(x1, lane.max.y)), 0.0, t.selection);
        }
    }
    lane_interaction(app, ui, track, lane, tl);
    if track.view == "playlists" {
        playlist_lanes(app, ui, track, row, main_h, tl);
    }
}

/// Alternate playlists as lanes under the main one; clicking a clip promotes it (comping).
fn playlist_lanes(app: &mut SoundApp, ui: &mut Ui, track: &Track, row: Rect, main_h: f32, tl: Rect) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let mut y = row.min.y + main_h;
    for (i, pl) in track.playlists.iter().enumerate() {
        if i == track.active_playlist {
            continue;
        }
        let lane = Rect::from_min_max(pos2(tl.min.x, y), pos2(tl.max.x, y + LANE_H));
        let head = Rect::from_min_max(pos2(row.min.x + 12.0, y), pos2(tl.min.x, y + LANE_H));
        ui.painter().rect_filled(lane, 0.0, t.playlist_lane);
        ui.painter().text(pos2(head.min.x + 4.0, head.center().y), Align2::LEFT_CENTER, &pl.name, regular(10.5), t.text_dim);
        let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
        for c in &pl.clips {
            draw_clip(app, &painter, &s, track, c, tl, lane, false);
        }
        let resp = ui.interact(lane, ui.id().with(("pl_lane", track.id.0, i)), Sense::click());
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let at = sample_at(&s, tl, p.x);
            let sel = s.edit.selection;
            let range = if !sel.is_empty() && sel.contains(at) {
                Some((sel.start, sel.end))
            } else {
                pl.clips.iter().find(|c| c.range().contains(at)).map(|c| (c.start, c.end()))
            };
            if let Some((a, b)) = range {
                let _ = app.run("track.playlist_promote", json!({"track": track.id.0, "playlist": i, "start": a, "end": b}));
            }
        }
        resp.on_hover_text("Click a take (or the selection) to promote it to the main playlist");
        y += LANE_H;
    }
}

fn track_header(app: &mut SoundApp, ui: &mut Ui, track: &Track, head: Rect, selected: bool, head_bg: Color32) {
    let t = Tokens::current();
    let id = track.id;
    // Drag the bottom edge to resize (snaps to the height presets).
    let edge = Rect::from_min_max(pos2(head.min.x, head.max.y - 4.0), pos2(head.max.x, head.max.y + 2.0));
    let eresp = ui.interact(edge, ui.id().with(("hedge", id.0)), Sense::drag());
    if eresp.hovered() || eresp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if eresp.drag_started() {
        ui.data_mut(|d| d.insert_temp(eresp.id, track.height));
    }
    if eresp.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin()).filter(|o| o.y != p.y)
    {
        let start = ui.data(|d| d.get_temp::<TrackHeight>(eresp.id)).unwrap_or(track.height);
        let want = start.points() + p.y - origin.y;
        let m = ui.input(|i| i.modifiers);
        let best = if m.command {
            TrackHeight::from_points(want)
        } else {
            TrackHeight::ALL.into_iter().min_by_key(|h| ((h.points() - want).abs() * 10.0) as i64).unwrap_or(start)
        };
        let s = app.engine.session();
        let tracks = height_targets(s, m, &[id]);
        if tracks.iter().any(|t| s.track(TrackId(*t)).is_some_and(|tr| tr.height != best)) {
            let _ = app.engine.execute_merged("track.height", &json!({"tracks": tracks, "height": best.points()}), &format!("track_height:{}", id.0));
            ui.ctx().request_repaint();
        }
    }
    let h = head.height();
    // Click on header background selects the track.
    let bg = ui.interact(head, ui.id().with(("hdr", id.0)), Sense::click());
    if bg.clicked() {
        click_track(app, id, ui.input(|i| i.modifiers), in_edit_window);
    }
    bg.context_menu(|ui| track_context_menu(app, ui, id));
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(head.min.x + x, head.min.y + y), vec2(w, h));
    let strip = rgb(track.color);
    let ink = shade(strip, -0.8);
    // The strip's round button opens a folder, or any other track's options.
    let round = at(2.0, 2.0, 10.0, 10.0);
    let rresp = ui.interact(round, ui.id().with(("options", id.0)), Sense::click());
    ui.painter().circle_filled(round.center(), 5.0, shade(strip, if rresp.hovered() { 0.6 } else { 0.45 }));
    arrow(ui.painter(), round.center(), !track.is_folder() || track.folder_open, ink);
    if track.is_folder() {
        if rresp.on_hover_text(if track.folder_open { "Close folder" } else { "Open folder" }).clicked() {
            let _ = app.run("track.folder_toggle", json!({"track": id.0}));
        }
    } else {
        egui::Popup::menu(&rresp.on_hover_text("Track options")).show(|ui| track_context_menu(app, ui, id));
    }
    if h >= 40.0 && track.kind.has_playlist() {
        let ar = at(3.0, h - 12.0, 9.0, 9.0);
        let aresp = ui.interact(ar, ui.id().with(("auto_lane", id.0)), Sense::click());
        let showing = AutoParam::parse(&track.view).is_some();
        ui.painter().rect_filled(ar, 1.5, shade(strip, if aresp.hovered() || showing { 0.6 } else { 0.45 }));
        let (a, b) = (ar.min + vec2(2.5, 2.5), ar.max - vec2(2.5, 2.5));
        ui.painter().line_segment([a, b], Stroke::new(1.0, ink));
        ui.painter().circle_filled(a, 1.3, ink);
        ui.painter().circle_filled(b, 1.3, ink);
        if aresp.on_hover_text(if showing { "Hide automation" } else { "Show volume automation" }).clicked() {
            let view = if !showing {
                "volume"
            } else if track.kind.is_midi() {
                "notes"
            } else {
                "waveform"
            };
            let _ = app.run("track.view", json!({"track": id.0, "view": view}));
        }
    }
    // Name, highlighted when the track is selected, and the playlist menu beside it.
    let indent = folder_depth(app.engine.session(), track) as f32 * 10.0;
    let name_r = at(16.0 + indent, 1.5, 97.0 - indent, 15.0);
    if selected {
        ui.painter().rect_filled(name_r, 2.0, t.name_field_sel);
    }
    let shown_name = if app.engine.session().edit.flag("edit_view.track_number") {
        let n = app.engine.session().track_index(id).map_or(0, |i| i + 1);
        format!("{n}  {}", track.name)
    } else {
        track.name.clone()
    };
    let name_color = if selected { t.text_dark } else { t.text.lerp_to_gamma(t.text_dim, 0.4) };
    ui.painter().with_clip_rect(name_r).text(pos2(name_r.min.x + 1.5, name_r.center().y), Align2::LEFT_CENTER, &shown_name, bold(12.5), name_color);
    let nresp = ui.interact(name_r, ui.id().with(("name", id.0)), Sense::click_and_drag());
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    } else if nresp.clicked() {
        click_track(app, id, ui.input(|i| i.modifiers), in_edit_window);
    }
    nresp.context_menu(|ui| track_context_menu(app, ui, id));
    // Drag the name plate to reorder tracks.
    if nresp.dragged()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let line_y = app
            .edit_layout
            .rows
            .iter()
            .map(|(_, r)| r[1])
            .chain(app.edit_layout.rows.last().map(|(_, r)| r[3]))
            .min_by_key(|y| ((y - p.y).abs() * 10.0) as i64)
            .unwrap_or(p.y);
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("reorder")))
            .line_segment([pos2(head.min.x, line_y), pos2(head.max.x + 600.0, line_y)], Stroke::new(3.0, Tokens::current().accent));
    }
    if nresp.drag_stopped()
        && let Some(p) = ui.ctx().pointer_latest_pos()
    {
        let rows = app.edit_layout.rows.clone();
        let target_row = rows.iter().position(|(_, r)| p.y < (r[1] + r[3]) * 0.5).unwrap_or(rows.len());
        // Convert the visible-row index to a session index.
        let s = app.engine.session();
        let to = rows.get(target_row).and_then(|(tid, _)| s.track_index(TrackId(*tid))).unwrap_or(s.tracks.len());
        let from = s.track_index(id).unwrap_or(0);
        let to = if to > from { to - 1 } else { to };
        if to != from {
            let _ = app.run("track.move", json!({"track": id.0, "to": to}));
        }
    }
    let pl_r = at(114.0, 1.0, 11.0, 14.5);
    let presp = ui.interact(pl_r, ui.id().with(("pl", id.0)), Sense::click());
    ui.painter().rect_filled(pl_r, 1.5, shade(head_bg, if presp.hovered() { -0.2 } else { -0.35 }));
    arrow(ui.painter(), pos2(pl_r.center().x, pl_r.min.y + 5.75), true, t.text);
    egui::Popup::menu(&presp.on_hover_text("Playlists")).show(|ui| {
        if ui.button("New...").clicked() {
            let _ = app.run("track.playlist_new", json!({"track": id.0}));
        }
        if ui.button("Duplicate...").clicked() {
            let _ = app.run("track.playlist_duplicate", json!({"track": id.0}));
        }
        if ui.button("Delete Unused...").clicked() {
            let _ = app.run("track.playlist_delete_unused", json!({"track": id.0}));
        }
        ui.separator();
        for (i, p) in track.playlists.iter().enumerate() {
            if ui.selectable_label(i == track.active_playlist, &p.name).clicked() {
                let _ = app.run("track.playlist_select", json!({"track": id.0, "index": i}));
            }
        }
    });
    let m = &track.mixer;
    if h >= 37.0 {
        let (records, mixes) = (track.kind.has_playlist(), track.kind != TrackKind::Master);
        let implied = !m.solo && app.engine.session().tracks.iter().any(|t| t.mixer.solo);
        let keys = [
            (HeaderKey::Rec, m.record_arm, false, "mix.record_arm", records),
            (HeaderKey::Input, m.input_monitor, false, "mix.input_monitor", records),
            (HeaderKey::Solo, m.solo, false, "mix.solo", mixes),
            (HeaderKey::Mute, m.mute, implied, "mix.mute", mixes),
        ];
        for (i, (key, lit, implied, cmd, shown)) in keys.into_iter().enumerate() {
            if shown && header_toggle(ui, at(23.0 + i as f32 * 26.0, 20.0, 16.0, 16.0), id, key, lit, implied).clicked() {
                let _ = app.run(cmd, json!({"track": id.0}));
            }
        }
    }
    if h >= 58.0 {
        let vresp = header_selector(ui, at(17.0, 39.0, 105.0, 17.0), ui.id().with(("view", id.0)), &view_label(&track.view), t.text, true);
        egui::Popup::menu(&vresp).show(|ui| view_menu(app, ui, track));
    }
    if h >= 76.0 {
        if matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Aux) {
            let r = at(17.0, 57.0, 13.0, 17.0);
            let resp = ui.interact(r, ui.id().with(("freeze", id.0)), Sense::click());
            let frozen = track.freeze.is_some();
            raised_key(ui.painter(), r, if frozen { t.slot_bypassed } else { t.slot_face }, resp.hovered());
            for a in [0.0f32, 60.0, 120.0] {
                let d = vec2(a.to_radians().sin(), a.to_radians().cos()) * 3.5;
                ui.painter().line_segment([r.center() - d, r.center() + d], Stroke::new(1.5, t.text));
            }
            if resp.on_hover_text(if frozen { "Unfreeze" } else { "Freeze" }).clicked() {
                let _ = app.run(if frozen { "track.unfreeze" } else { "track.freeze" }, json!({"tracks": [id.0]}));
            }
        }
        if !matches!(track.kind, TrackKind::Master | TrackKind::Vca | TrackKind::Folder) {
            let (voice, color) = if track.inactive { ("off", t.text_dim) } else { ("dyn", t.text) };
            let resp = header_selector(ui, at(31.0, 57.0, 31.0, 17.0), ui.id().with(("voice", id.0)), voice, color, false);
            egui::Popup::menu(&resp.on_hover_text("Voice: dyn plays the track, off makes it inactive")).show(|ui| {
                for (label, inactive) in [("dyn", false), ("off", true)] {
                    if ui.selectable_label(track.inactive == inactive, label).clicked() {
                        let _ = app.run("track.make_inactive", json!({"tracks": [id.0], "inactive": inactive}));
                    }
                }
            });
        }
        let mode = m.automation_mode;
        let col = match mode {
            transit_model::AutomationMode::Read => t.auto_read,
            transit_model::AutomationMode::Write => t.auto_write,
            transit_model::AutomationMode::Touch | transit_model::AutomationMode::TouchLatch => t.auto_touch,
            transit_model::AutomationMode::Latch => t.auto_latch,
            _ => t.text_dim,
        };
        let aresp = header_selector(ui, at(63.0, 57.0, 59.0, 17.0), ui.id().with(("auto_mode", id.0)), mode.label(), col, true);
        egui::Popup::menu(&aresp).show(|ui| {
            for m in transit_model::AutomationMode::ALL {
                if ui.selectable_label(m == mode, m.label()).clicked() {
                    let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
                }
            }
        });
    }
    if h >= 94.0 {
        if track.kind.has_playlist() {
            let r = at(17.0, 75.0, 13.0, 17.0);
            let resp = ui.interact(r, ui.id().with(("timebase", id.0)), Sense::click());
            raised_key(ui.painter(), r, t.slot_face, resp.hovered());
            let c = r.center();
            if track.ticks_timebase {
                let green = Color32::from_rgb(86, 172, 103);
                ui.painter().add(Shape::convex_polygon(
                    vec![c + vec2(-3.0, 3.25), c + vec2(3.0, 3.25), c + vec2(1.0, -3.25), c + vec2(-1.0, -3.25)],
                    green,
                    Stroke::NONE,
                ));
                ui.painter().line_segment([c + vec2(0.0, 2.0), c + vec2(2.0, -3.0)], Stroke::new(1.0, t.text));
            } else {
                ui.painter().circle_filled(c, 3.25, Color32::from_rgb(100, 184, 249));
                ui.painter().line_segment([c, c + vec2(0.0, -3.25)], Stroke::new(1.0, t.slot_face[1]));
                ui.painter().line_segment([c, c + vec2(3.25, 0.0)], Stroke::new(1.0, t.slot_face[1]));
            }
            let tip = if track.ticks_timebase { "Timebase: ticks" } else { "Timebase: samples" };
            egui::Popup::menu(&resp.on_hover_text(tip)).show(|ui| {
                for (label, ticks) in [("Samples", false), ("Ticks", true)] {
                    if ui.selectable_label(track.ticks_timebase == ticks, label).clicked() {
                        let _ = app.run("track.timebase", json!({"tracks": [id.0], "ticks": ticks}));
                    }
                }
            });
        }
        let field = at(30.0, 75.0, 92.0, 17.0);
        if matches!(track.kind, TrackKind::Audio | TrackKind::Instrument) {
            let resp = ui.interact(field, ui.id().with(("field", id.0)), Sense::click());
            ui.painter().rect_filled(field, 2.0, shade(head_bg, if resp.hovered() { -0.02 } else { -0.1 }));
            if track.kind == TrackKind::Instrument {
                let name =
                    track.instrument.as_ref().and_then(|i| crate::mix_window::plugin_info(&i.plugin)).map_or("no instrument", |p| p.short_name);
                ui.painter().with_clip_rect(field).text(pos2(field.min.x + 5.0, field.center().y), Align2::LEFT_CENTER, name, bold(10.0), t.text);
                egui::Popup::menu(&resp.on_hover_text("Instrument"))
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| crate::mix_window::instrument_menu(app, ui, id));
            } else {
                let c = pos2(field.min.x + 7.0, field.center().y);
                ui.painter().line_segment([c + vec2(0.0, -3.5), c + vec2(0.0, 1.0)], Stroke::new(1.0, t.text));
                ui.painter().add(Shape::convex_polygon(vec![c + vec2(0.0, 0.5), c + vec2(3.0, 3.5), c + vec2(-3.0, 3.5)], t.text, Stroke::NONE));
                if let Some(alg) = &track.elastic {
                    ui.painter().with_clip_rect(field).text(pos2(field.min.x + 14.0, field.center().y), Align2::LEFT_CENTER, alg, bold(10.0), t.text);
                }
                egui::Popup::menu(&resp.on_hover_text("Elastic Audio")).show(|ui| {
                    for alg in ["none", "polyphonic", "rhythmic", "monophonic", "varispeed", "x-form"] {
                        let current = track.elastic.as_deref().unwrap_or("none") == alg;
                        if ui.selectable_label(current, alg).clicked() {
                            let _ = app.run("track.elastic", json!({"tracks": [id.0], "algorithm": alg}));
                        }
                    }
                });
            }
        }
    }
    let md = app.meters.get(&id).copied().unwrap_or_default();
    header_meter(ui, Rect::from_min_max(pos2(head.max.x - 8.0, head.min.y + 1.0), pos2(head.max.x, head.max.y - 1.5)), &md, track.channels() >= 2);
}

/// A small menu arrow, pointing down, or right for a closed folder.
fn arrow(p: &egui::Painter, c: Pos2, down: bool, color: Color32) {
    let pts = if down {
        vec![c + vec2(-2.5, -1.75), c + vec2(2.5, -1.75), c + vec2(0.0, 1.75)]
    } else {
        vec![c + vec2(-1.75, -2.5), c + vec2(1.75, 0.0), c + vec2(-1.75, 2.5)]
    };
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// A header selector (track view, automation mode…): the value centred, and for menus an arrow
/// in the top-right corner.
fn header_selector(ui: &Ui, r: Rect, id: egui::Id, text: &str, color: Color32, menu: bool) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    raised_key(ui.painter(), r, Tokens::current().slot_face, resp.hovered());
    let tr = if menu { r.with_max_x(r.max.x - 7.0) } else { r };
    ui.painter().with_clip_rect(tr.shrink(1.0)).text(tr.center(), Align2::CENTER_CENTER, text, bold(10.0), color);
    if menu {
        arrow(ui.painter(), pos2(r.max.x - 5.5, r.min.y + 5.75), true, Tokens::current().text);
    }
    resp
}

#[derive(Clone, Copy, PartialEq)]
enum HeaderKey {
    Rec,
    Input,
    Solo,
    Mute,
}

/// Record, input, solo or mute: `lit` gives the key its colour, `implied` a dark shade of it (a
/// mute that another track's solo implies). Record keys stay dark red when off, as in Pro Tools.
fn header_toggle(ui: &Ui, r: Rect, track: TrackId, key: HeaderKey, lit: bool, implied: bool) -> egui::Response {
    let t = Tokens::current();
    let (label, color) = match key {
        HeaderKey::Rec => ("Record enable", t.rec),
        HeaderKey::Input => ("Input monitoring", t.input),
        HeaderKey::Solo => ("Solo", t.solo),
        HeaderKey::Mute => ("Mute", t.mute),
    };
    let resp = ui.interact(r, ui.id().with((label, track.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), lit, label));
    let rec = key == HeaderKey::Rec;
    let (tint, k) = if lit {
        (color, 0.85)
    } else if implied {
        (shade(color, -0.35), 0.55)
    } else if rec {
        (shade(color, -0.5), 0.35)
    } else {
        (color, 0.0)
    };
    let [top, bottom] = t.slot_face.map(|f| f.lerp_to_gamma(tint, k));
    raised_key(ui.painter(), r, [shade(top, 0.08), shade(bottom, -0.12)], resp.hovered());
    let ink = if lit {
        t.text_dark
    } else if implied {
        t.text.lerp_to_gamma(color, 0.3)
    } else {
        t.text
    };
    if rec {
        ui.painter().circle_filled(r.center(), 4.0, if lit { Color32::WHITE } else { t.text.lerp_to_gamma(color, 0.3) });
    } else {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label.get(..1).unwrap_or_default(), bold(11.0), ink);
    }
    resp.on_hover_text(label)
}

/// The header's level meter: black-rimmed bars, dim where unlit, with a clip light on top.
fn header_meter(ui: &Ui, r: Rect, md: &crate::MeterDisplay, stereo: bool) {
    let t = Tokens::current();
    let p = ui.painter();
    p.rect_filled(r, 0.0, Color32::BLACK);
    let inner = r.shrink(1.0);
    let clip = inner.with_max_y(inner.min.y + 4.0);
    p.rect_filled(clip, 0.0, if md.clip { t.meter_red } else { t.meter_red.gamma_multiply(0.18) });
    let body = inner.with_min_y(clip.max.y + 1.0);
    let n = if stereo { 2 } else { 1 };
    let w = (body.width() - (n - 1) as f32) / n as f32;
    let (amber, red) = (meter_pos(-12.0), meter_pos(-3.0));
    for ch in 0..n {
        let bar = Rect::from_min_size(pos2(body.min.x + ch as f32 * (w + 1.0), body.min.y), vec2(w, body.height()));
        let y = |f: f32| bar.max.y - bar.height() * f;
        let lv = level_pos(md.level[ch]);
        for (lo, hi, col) in [(0.0, amber, t.meter_green), (amber, red, t.meter_yellow), (red, 1.0, t.meter_red)] {
            p.rect_filled(Rect::from_x_y_ranges(bar.x_range(), y(hi)..=y(lo)), 0.0, col.gamma_multiply(0.18));
            if lv > lo {
                p.rect_filled(Rect::from_x_y_ranges(bar.x_range(), y(lv.min(hi))..=y(lo)), 0.0, col);
            }
        }
        let hv = level_pos(md.hold[ch]);
        if hv > 0.01 {
            p.hline(bar.x_range(), y(hv), Stroke::new(1.0, if hv > red { t.meter_red } else { t.meter_yellow }));
        }
    }
}

/// A header column's own Ui, so its slots' ids can't collide with the track header's.
fn column_ui(ui: &mut Ui, col: &str, r: Rect) -> Ui {
    ui.new_child(egui::UiBuilder::new().id_salt(("edit_column", col)).max_rect(r))
}

/// One optional Edit-window column for a track (I/O, inserts, sends, comments).
fn header_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, col: &str, r: Rect, head_bg: Color32) {
    match col {
        "io" => io_column(app, ui, track, r),
        "comments" => {
            ui.painter().rect_filled(r, 0.0, head_bg);
            ui.painter().vline(r.min.x, r.y_range(), Stroke::new(1.0, Tokens::current().border));
            let mut c = track.comments.clone();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(3.0)));
            if child.add(egui::TextEdit::multiline(&mut c).desired_width(r.width() - 6.0).font(regular(10.0))).changed() {
                let _ =
                    app.engine.execute_merged("track.comments", &json!({"track": track.id.0, "comments": c}), &format!("comments:{}", track.id.0));
            }
        }
        _ => plugin_column(app, ui, track, col, r),
    }
}

/// The I/O column as Pro Tools draws it: input and output selectors, then volume and pan.
fn io_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    ui.painter().rect_filled(r, 0.0, t.slot_well);
    let ui = column_ui(ui, "io", r);
    let row = |y: f32, h: f32| (r.height() >= y + h + 1.0).then_some(Rect::from_min_size(r.min + vec2(7.0, y), vec2(r.width() - 14.0, h)));
    if let Some(input) = row(4.0, 15.0) {
        let resp = header_selector(&ui, input, ui.id().with(("in", id.0)), &crate::mix_window::input_name(app, track), t.text, true);
        crate::mix_window::route_popup(app, &resp.on_hover_text("Input"), id, true);
    }
    if let Some(out) = row(21.0, 17.0) {
        let (sel, fader) = (out.with_max_x(out.max.x - 11.0), out.with_min_x(out.max.x - 11.0));
        let resp = ui.interact(sel, ui.id().with(("out", id.0)), Sense::click());
        let fresp = ui.interact(fader, ui.id().with(("out_fader", id.0)), Sense::click());
        raised_key(ui.painter(), out, t.slot_face, resp.hovered() || fresp.hovered());
        groove(ui.painter(), fader.min.x - 0.5, out.min.y + 1.0..=out.max.y - 1.0, t.slot_face[0]);
        let name = crate::mix_window::route_name(app, &track.mixer.output);
        ui.painter().with_clip_rect(sel.shrink(1.0)).text(sel.center(), Align2::CENTER_CENTER, name, bold(10.0), t.text);
        let c = fader.center() + vec2(0.5, 0.0);
        ui.painter().vline(c.x, c.y - 5.0..=c.y + 5.0, Stroke::new(1.0, t.text));
        ui.painter().rect_filled(Rect::from_center_size(c + vec2(0.0, -1.5), vec2(5.0, 2.5)), 0.5, t.text);
        crate::mix_window::route_popup(app, &resp.on_hover_text("Output"), id, false);
        if fresp.on_hover_text("Show in the Mix window").clicked() {
            let _ = app.run("edit.select", json!({"tracks": [id.0]}));
            let _ = app.run("window.mix", json!({}));
        }
    }
    if let Some(vr) = row(45.0, 30.0).map(|vr| vr.shrink2(vec2(1.0, 0.0))) {
        ui.painter().rect_filled(vr, 2.0, Color32::from_gray(15));
        ui.painter().rect_filled(vr.shrink(1.5), 1.5, Color32::BLACK);
        let m = &track.mixer;
        let pan = m.pan.first().map_or("-".into(), |p| crate::widgets::pan_text(*p));
        // The readout is black in both themes, so it takes the dark palette's green.
        let ink = Tokens::DARK.counter_text;
        for (y, label, value) in [(7.5, "vol", crate::widgets::db_text(m.volume_db)), (22.0, "pan", pan)] {
            ui.painter().text(pos2(vr.min.x + 7.0, vr.min.y + y), Align2::LEFT_CENTER, label, bold(10.0), ink);
            ui.painter().text(pos2(vr.max.x - 6.5, vr.min.y + y), Align2::RIGHT_CENTER, value, bold(10.0), ink);
        }
        ui.painter().hline(vr.min.x + 7.0..=vr.max.x - 6.0, vr.min.y + 14.5, Stroke::new(1.0, Color32::from_gray(35)));
    }
}

/// Inserts, sends or the instrument, laid out as Pro Tools does: 15-point cells every 18 points
/// on a neutral well, or on tracks too short for three of them, a row of squares.
fn plugin_column(app: &mut SoundApp, ui: &mut Ui, track: &Track, col: &str, r: Rect) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.slot_well);
    let rows = if r.height() < 16.0 { 0 } else { ((r.height() - 16.0) / 18.0) as usize + 1 };
    let cell = |row: usize| Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 1.0 + row as f32 * 18.0), vec2(r.width() - 8.0, 15.0));
    let square = |i: usize, row: usize| Rect::from_min_size(cell(row).min + vec2(i as f32 * 14.0, 0.0), vec2(10.0, 15.0));
    let mut ui = column_ui(ui, col, r);
    if col == "instruments" {
        if rows > 0 && track.kind == TrackKind::Instrument {
            crate::mix_window::instrument_slot(app, &mut ui, track, cell(0));
        }
        return;
    }
    let off = if col.ends_with("ae") { 0 } else { 5 };
    let slot = if col.starts_with("inserts") { crate::mix_window::insert_slot } else { crate::mix_window::send_slot };
    let n = match rows {
        0 => 0,
        1 | 2 => 5,
        _ => rows.min(5),
    };
    for i in 0..n {
        slot(app, &mut ui, track, off + i, if rows < 3 { square(i, 0) } else { cell(i) });
    }
    if rows == 2 {
        for i in 0..5 {
            ui.painter().rect_filled(square(i, 1), 1.5, t.slot_empty);
        }
    }
}

fn view_label(v: &str) -> String {
    match AutoParam::parse(v) {
        Some(p) => p.label(),
        None => v.to_string(),
    }
}

fn view_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track) {
    let id = track.id.0;
    let mut opts: Vec<&str> = Vec::new();
    if track.kind.is_midi() {
        opts.extend(["notes", "regions", "blocks"]);
    } else if track.kind.has_playlist() {
        opts.extend(["waveform", "blocks", "clip_gain", "playlists"]);
    }
    opts.extend(["volume", "pan", "mute"]);
    for o in opts {
        if ui.selectable_label(track.view == o, view_label(o)).clicked() {
            let _ = app.run("track.view", json!({"track": id, "view": o}));
        }
    }
    for (slot, ins) in track.mixer.inserts.iter().enumerate() {
        let Some(ins) = ins else { continue };
        let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { continue };
        ui.menu_button(info.name, |ui| {
            for p in info.params {
                let v = format!("plugin:{}:{}", (b'a' + slot as u8) as char, p.id);
                if ui.button(p.name).clicked() {
                    let _ = app.run("track.view", json!({"track": id, "view": v}));
                }
            }
        });
    }
}

pub(crate) fn track_context_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let tid = id.0;
    let items: [(&str, &str); 12] = [
        ("Rename...", "ui.rename_track"),
        ("Duplicate...", "track.duplicate"),
        ("Make Inactive", "track.make_inactive"),
        ("Hide", "track.hide"),
        ("Delete", "track.delete"),
        ("Split into Mono", "track.split_into_mono"),
        ("Commit...", "track.commit"),
        ("Freeze", "track.freeze"),
        ("Unfreeze", "track.unfreeze"),
        ("Flatten", "track.flatten"),
        ("Move to New Folder...", "track.move_to_new_folder"),
        ("Edit Group...", "track.group"),
    ];
    let selection: Vec<u64> = {
        let selected = &app.engine.session().edit.selected_tracks;
        if selected.contains(&id) { selected.iter().map(|track| track.0).collect() } else { vec![tid] }
    };
    for (label, cmd) in items {
        if ui.button(label).clicked() {
            if cmd == "ui.rename_track" {
                let name = app.engine.session().track(id).map(|t| t.name.clone()).unwrap_or_default();
                app.dialogs.open_rename_track(id, &name);
            } else {
                let _ = app.run(cmd, json!({"tracks": selection}));
            }
        }
    }
    let folders: Vec<(u64, String)> = app
        .engine
        .session()
        .tracks
        .iter()
        .filter(|track| track.is_folder() && !selection.contains(&track.id.0))
        .map(|track| (track.id.0, track.name.clone()))
        .collect();
    if !folders.is_empty() {
        ui.menu_button("Move to Folder", |ui| {
            for (folder, name) in &folders {
                if ui.button(name).clicked() {
                    let _ = app.run("track.move_to_folder", json!({"tracks": selection, "folder": folder}));
                }
            }
        });
    }
    if app.engine.session().track(id).is_some_and(|track| track.folder.is_some()) && ui.button("Remove from Folder").clicked() {
        let _ = app.run("track.remove_from_folder", json!({"tracks": selection}));
    }
    ui.separator();
    ui.menu_button("Track Height", |ui| {
        for h in TrackHeight::ALL {
            if ui.button(h.label()).clicked() {
                let own: Vec<TrackId> = selection.iter().map(|&t| TrackId(t)).collect();
                let tracks = height_targets(app.engine.session(), ui.input(|i| i.modifiers), &own);
                let _ = app.run("track.height", json!({"tracks": tracks, "height": h.label()}));
            }
        }
    });
    ui.menu_button("Track Color", |ui| {
        for (i, c) in transit_model::TRACK_COLORS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 14.0), Sense::click());
            ui.painter().rect_filled(r, 2.0, rgb(*c));
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": selection, "color": i}));
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/tracks.rs"]
mod tests;
