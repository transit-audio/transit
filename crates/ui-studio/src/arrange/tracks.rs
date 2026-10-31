use super::*;

/// One track's header: color strip, name, M S R I toggles, volume readout.
#[allow(clippy::too_many_arguments)]
pub(super) fn track_header(
    app: &mut StudioApp,
    ui: &mut Ui,
    t: &Track,
    r: Rect,
    list: Rect,
    selected: bool,
    outputs: Option<&crate::midi_output::OutputView>,
    row: &mut TrackRow,
) {
    let p = ui.painter().with_clip_rect(list);
    let bg = if selected { TOKENS.header_selected } else { TOKENS.header };
    p.rect_filled(r.shrink2(vec2(0.0, 0.5)), 0.0, bg);
    // Group members sit indented under their group (slice 2c); a group has a disclosure triangle.
    let indent = row.depth as f32 * crate::groups::INDENT;
    let tri = if t.is_folder() { 14.0 } else { 0.0 };
    let tx = r.min.x + indent + tri;
    p.rect_filled(Rect::from_min_size(pos2(r.min.x + indent, r.min.y), vec2(5.0, r.height())), 0.0, theme::rgb(t.color));
    p.line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, TOKENS.line));
    // Each ancestor group's bar runs down the left edge, from the group's own header through its
    // last visible member: full row height, no gap, drawn over the row separator. Dimmed so the
    // member's own stripe stays primary.
    {
        let s = app.engine.session();
        let bars = crate::groups::group_bars(r, &crate::groups::ancestors(s, t));
        for (bar, color) in &bars {
            p.rect_filled(*bar, 0.0, theme::rgb(*color).gamma_multiply(crate::groups::BAR_DIM));
            row.group_bars.push(r4(*bar));
        }
    }
    let name_rect = Rect::from_min_max(pos2(tx + 12.0, r.min.y + 4.0), pos2(r.max.x - 114.0, r.min.y + 20.0));
    row.name_rect = r4(Rect::from_min_max(pos2(name_rect.min.x - 4.0, name_rect.min.y - 2.0), pos2(name_rect.max.x, name_rect.max.y + 2.0)));
    p.with_clip_rect(name_rect.intersect(list)).text(name_rect.left_center(), Align2::LEFT_CENTER, &t.name, FontId::proportional(13.0), TOKENS.text);
    let kind = match t.kind {
        TrackKind::Instrument => t.instrument.as_ref().map_or("Instrument".to_string(), |i| short_plugin(&app.plugin_name(&i.plugin, true))),
        TrackKind::Audio => format!("Audio · {}", t.format.label()),
        TrackKind::Folder if t.is_rack() => {
            let n = app.engine.session().rack_members(t).count();
            format!("Rack · {n} instrument{}", if n == 1 { "" } else { "s" })
        }
        TrackKind::Folder => {
            let n = app.engine.session().tracks.iter().filter(|x| x.folder == Some(t.id) && x.id != t.id).count();
            format!("Group · {n} track{}", if n == 1 { "" } else { "s" })
        }
        k => format!("{k:?}"),
    };
    row.subtitle = kind.clone();
    // A rack (ADR 0034) wears a badge where a group's subtitle starts.
    let sub_x = if t.is_rack() {
        let badge = Rect::from_min_size(pos2(tx + 12.0, r.min.y + 24.0), vec2(30.0, 13.0));
        p.rect_filled(badge, crate::theme::RADIUS_SM, TOKENS.accent);
        p.text(badge.center(), Align2::CENTER_CENTER, "Rack", FontId::proportional(9.5), theme::text_on(TOKENS.accent));
        let resp = ui.interact(badge.intersect(list), ui.id().with(("rack_badge", t.id.0)), Sense::hover());
        crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderRack);
        row.rack_badge = r4(badge);
        badge.max.x + 5.0
    } else {
        tx + 12.0
    };
    let kind = if t.is_rack() { kind.trim_start_matches("Rack · ").to_string() } else { kind };
    let subtitle = p.text(pos2(sub_x, r.min.y + 30.0), Align2::LEFT_CENTER, kind, FontId::proportional(10.5), TOKENS.text_dim);
    // A loaded plug-in that does not answer (#213): the marker beside the subtitle.
    if let Some((plugin, hang)) = app.track_hang(t) {
        let name = app.plugin_name(&plugin, false);
        let room = Rect::from_min_max(pos2(subtitle.max.x + 6.0, r.min.y + 22.0), pos2(r.max.x - 100.0, r.min.y + 38.0));
        let m = p.with_clip_rect(room.intersect(list)).text(
            room.left_center(),
            Align2::LEFT_CENTER,
            crate::devices::UNRESPONSIVE,
            FontId::proportional(10.5),
            TOKENS.solo,
        );
        ui.interact(m.intersect(room).intersect(list), ui.id().with(("unresponsive", t.id.0)), Sense::hover())
            .on_hover_text(app.hang_tip(hang, &name));
        row.unresponsive = crate::devices::UNRESPONSIVE.to_string();
        row.unresponsive_rect = r4(m);
    }
    // Toggles, right-aligned on the first line.
    let m = &t.mixer;
    let mut x = r.max.x - 6.0;
    let mut toggle = |app: &mut StudioApp, label: &str, on: bool, color: Color32, cmd: &str, enabled: bool| -> R4 {
        let b = Rect::from_min_size(pos2(x - 20.0, r.min.y + 5.0), vec2(19.0, 16.0));
        x -= 22.0;
        let resp = ui.interact(b.intersect(list), ui.id().with((cmd, t.id.0)), Sense::click());
        let help = match cmd {
            "mix.input_monitor" => Some(crate::help::Id::ArrangeHeaderMonitor),
            "mix.record_arm" => Some(crate::help::Id::ArrangeHeaderArm),
            "mix.solo" => Some(crate::help::Id::ArrangeHeaderSolo),
            "mix.mute" => Some(crate::help::Id::ArrangeHeaderMute),
            _ => None,
        };
        if let Some(help) = help {
            crate::help::hover(ui, &resp, help);
        }
        let fill = if on {
            color
        } else if resp.hovered() && enabled {
            TOKENS.button_hover
        } else {
            TOKENS.button
        };
        p.rect_filled(b, crate::theme::RADIUS_SM, fill);
        let fg = if on {
            theme::text_on(color)
        } else if enabled {
            TOKENS.text
        } else {
            TOKENS.text_dim.gamma_multiply(0.5)
        };
        p.text(b.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), fg);
        if resp.clicked() && enabled {
            let _ = app.run(cmd, json!({"tracks": [t.id.0], "value": !on}));
        }
        r4(b)
    };
    let records = t.kind.has_playlist();
    // A rack monitors its MIDI input: while I is on, live notes play its members (ADR 0034).
    row.monitor = toggle(app, "I", m.input_monitor, TOKENS.monitor, "mix.input_monitor", t.kind == TrackKind::Audio || t.is_rack());
    row.arm = toggle(app, "R", m.record_arm, TOKENS.record, "mix.record_arm", records);
    row.solo = toggle(app, "S", m.solo, TOKENS.solo, "mix.solo", true);
    row.mute = toggle(app, "M", m.mute, TOKENS.mute, "mix.mute", true);
    // Slice 7: A shows the track's automation lanes (Ableton's automation toggle). View state.
    let shown = app.ui.auto_shown.contains(&t.id);
    let b = Rect::from_min_size(pos2(x - 20.0, r.min.y + 5.0), vec2(19.0, 16.0));
    let resp = ui.interact(b.intersect(list), ui.id().with(("auto_toggle", t.id.0)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderAutomation);
    let fill = if shown {
        TOKENS.accent
    } else if resp.hovered() {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(b, crate::theme::RADIUS_SM, fill);
    p.text(b.center(), Align2::CENTER_CENTER, "A", FontId::proportional(11.0), if shown { theme::text_on(TOKENS.accent) } else { TOKENS.text });
    if resp.on_hover_text(if shown { "Hide automation" } else { "Show automation" }).clicked() {
        app.toggle_automation(t.id);
    }
    row.automation = r4(b);
    row.automation_shown = shown;
    let stacked = t.playlists.len() > 1 && !app.takes_of(t).stacks.is_empty();
    // Volume readout: drag sideways to change, double-click for 0 dB.
    let vol = Rect::from_min_size(pos2(r.max.x - 72.0, r.min.y + 23.0), vec2(66.0, 15.0));
    row.volume = r4(vol);
    let resp = ui.interact(vol.intersect(list), ui.id().with(("vol", t.id.0)), Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderVolume);
    p.rect_filled(vol, crate::theme::RADIUS_SM, TOKENS.bg);
    let db = app.followed(t, &crate::automation::Lane::Volume).unwrap_or(m.volume_db);
    let text = if db <= -144.0 { "-inf dB".to_string() } else { format!("{db:+.1} dB") };
    p.text(vol.center(), Align2::CENTER_CENTER, text, crate::theme::mono(ui.ctx(), 10.5), if resp.hovered() { TOKENS.accent } else { TOKENS.text });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started() {
        app.arrange.gesture = Some(Gesture::Volume { track: t.id, start_db: db });
    }
    if resp.dragged()
        && let Some(Gesture::Volume { track, start_db }) = app.arrange.gesture.clone()
        && track == t.id
    {
        let dx = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(0.0, |(a, b)| b.x - a.x));
        let fine = ui.input(|i| i.modifiers.shift);
        let target = (start_db + dx * if fine { 0.02 } else { 0.1 }).clamp(-80.0, 12.0);
        let _ = app.engine.execute_merged("mix.volume", &json!({"tracks": [t.id.0], "db": target}), &crate::mixer::fader_key(t.id));
        crate::automation::note_pass_edit(app);
    }
    if resp.drag_stopped() {
        app.arrange.gesture = None;
    }
    if resp.double_clicked() {
        let _ = app.run("mix.volume", json!({"tracks": [t.id.0], "db": 0.0}));
    }
    // Freeze: the snowflake left of the volume readout (Unfreeze when frozen).
    if matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) {
        let frozen = t.freeze.is_some();
        let b = Rect::from_min_size(pos2(vol.min.x - 24.0, r.min.y + 22.0), vec2(19.0, 16.0));
        let resp = ui.interact(b.intersect(list), ui.id().with(("freeze", t.id.0)), Sense::click());
        crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderFreeze);
        let fill = if frozen {
            TOKENS.accent
        } else if resp.hovered() {
            TOKENS.button_hover
        } else {
            TOKENS.button
        };
        p.rect_filled(b, crate::theme::RADIUS_SM, fill);
        crate::icons::paint(&p, b.center(), crate::icons::Icon::Snowflake, 13.0, if frozen { theme::text_on(TOKENS.accent) } else { TOKENS.text });
        if resp.on_hover_text(if frozen { "Unfreeze" } else { "Freeze: render to audio and turn the plug-ins off" }).clicked() {
            let _ = app.toggle_freeze(t.id);
        }
        row.freeze_button = r4(b);
    }
    // Take lanes (#25): T, left of the freeze button, shows or hides the lanes of a track with
    // stacked takes (view state; Live's Show Take Lanes, Logic's take folder disclosure).
    if stacked {
        let frozen_button = if matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) { 24.0 } else { 0.0 };
        let shown = app.ui.takes_open.contains(&t.id);
        let b = Rect::from_min_size(pos2(vol.min.x - frozen_button - 24.0, r.min.y + 22.0), vec2(19.0, 16.0));
        let resp = ui.interact(b.intersect(list), ui.id().with(("takes_toggle", t.id.0)), Sense::click());
        crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderTakes);
        let fill = if shown {
            TOKENS.accent
        } else if resp.hovered() {
            TOKENS.button_hover
        } else {
            TOKENS.button
        };
        p.rect_filled(b, crate::theme::RADIUS_SM, fill);
        p.text(b.center(), Align2::CENTER_CENTER, "T", FontId::proportional(11.0), if shown { theme::text_on(TOKENS.accent) } else { TOKENS.text });
        if resp.on_hover_text(if shown { "Hide take lanes" } else { "Show take lanes" }).clicked() {
            app.toggle_takes(t.id);
        }
        row.takes = r4(b);
        row.takes_shown = shown;
    }
    // Meter: a thin bar at the right edge.
    if let Some(d) = app.meters.get(&t.id) {
        let lvl = d.level[0].max(d.level[1]).clamp(0.0, 1.0);
        let h = (r.height() - 4.0) * meter_pos(lvl);
        let bar = Rect::from_min_max(pos2(r.max.x - 3.0, r.max.y - 2.0 - h), pos2(r.max.x - 1.0, r.max.y - 2.0));
        p.rect_filled(bar, 0.0, if d.clip { TOKENS.record } else { TOKENS.monitor });
    }
    // Name area: click selects the track (Shift ranges, Ctrl adds); a drag moves it (slice 2c);
    // right-click opens the track menu.
    let name_area = Rect::from_min_max(r.min, pos2(r.max.x - 114.0, r.max.y));
    let resp = ui.interact(name_area.intersect(list), ui.id().with(("hdr", t.id.0)), Sense::click_and_drag());
    crate::help::hover(ui, &resp, crate::help::Id::ArrangeHeaderName);
    // Interacted after the name area so the chip wins where they overlap.
    let chip = outputs.and_then(|view| {
        let frozen_button = if matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) { 24.0 } else { 0.0 };
        let takes_button = if stacked { 24.0 } else { 0.0 };
        midi_output_chip(app, ui, &p, t, view, subtitle, vol.min.x - frozen_button - takes_button, list)
    });
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        app.click_header(t.id, m.shift, m.command || m.ctrl);
    }
    // Slice 6: double-click the name to rename the track in place.
    // Only on the name itself; a double-click elsewhere on the header keeps selecting.
    if resp.double_clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| crate::finish::rect(row.name_rect).contains(p)) {
        app.start_rename(crate::finish::RenameTarget::Track(t.id), row.name_rect);
    }
    if resp.drag_started() && t.kind != TrackKind::Master {
        app.arrange.header_drag = Some(crate::groups::HeaderDrag { track: t.id, drop: None });
    }
    if t.is_folder() {
        disclosure(app, ui, t, Rect::from_min_size(pos2(r.min.x + indent + 8.0, r.min.y + 5.0), vec2(14.0, 14.0)), list, row);
    }
    let id = t.id;
    let kind = t.kind;
    let has_instrument = t.instrument.is_some();
    resp.context_menu(|ui| track_menu(app, ui, id, kind, has_instrument));
    // The chip covers part of the name area, so it gets the same menu on right-click.
    if let Some(chip) = chip {
        // Its own popup id: the chip's response id already keys the destination picker.
        egui::Popup::context_menu(&chip).id(chip.id.with("track-menu")).show(|ui| track_menu(app, ui, id, kind, has_instrument));
    }
}

/// An external MIDI destination on the second header line (#69): a state dot, always, and the
/// destination text when it fits between the subtitle and the volume readout (so the name and the
/// subtitle are never clipped); with no room even for the dot, nothing is drawn. Returns the chip's
/// response so the caller can give it the track menu. Opens the mixer's picker and sends the same `midi.output` command.
#[allow(clippy::too_many_arguments)]
fn midi_output_chip(
    app: &mut StudioApp,
    ui: &mut Ui,
    p: &egui::Painter,
    t: &Track,
    view: &crate::midi_output::OutputView,
    subtitle: Rect,
    right: f32,
    list: Rect,
) -> Option<egui::Response> {
    let chip = view.header_chip(t.id, &t.midi_output)?;
    let font = FontId::proportional(10.0);
    let text_w = p.layout_no_wrap(chip.text.clone(), font.clone(), TOKENS.text).size().x;
    let dot = 8.0;
    let full_w = dot + 4.0 + text_w;
    if right - 6.0 - dot < subtitle.max.x + 4.0 {
        return None;
    }
    let show_text = right - 6.0 - full_w >= subtitle.max.x + 6.0;
    let w = if show_text { full_w } else { dot };
    let cy = subtitle.center().y;
    let b = Rect::from_min_size(pos2(right - 6.0 - w, cy - 7.0), vec2(w, 14.0));
    let resp = ui.interact(b.intersect(list), ui.id().with(("midi-output-chip", t.id.0)), Sense::click());
    let color = if chip.missing { TOKENS.record } else { TOKENS.accent };
    p.circle_filled(pos2(b.min.x + dot / 2.0, cy), dot / 2.0, color);
    if show_text {
        p.text(pos2(b.min.x + dot + 4.0, cy), Align2::LEFT_CENTER, &chip.text, font, if chip.missing { TOKENS.record } else { TOKENS.text_dim });
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "MIDI output"));
    if let Some(params) = view.popup(&resp, t.id, &t.midi_output, t.external_instrument().is_some()) {
        let _ = app.run("midi.output", params);
    }
    Some(resp)
}

/// A group's disclosure chevron: collapses or expands its members (view state only).
fn disclosure(app: &mut StudioApp, ui: &mut Ui, t: &Track, b: Rect, list: Rect, row: &mut TrackRow) {
    row.disclosure = r4(b);
    let resp = ui.interact(b.intersect(list), ui.id().with(("disclose", t.id.0)), Sense::click());
    let ink = if resp.hovered() { TOKENS.text } else { TOKENS.text_dim };
    let icon = if row.collapsed { crate::icons::Icon::ChevronRight } else { crate::icons::Icon::ChevronDown };
    crate::icons::paint(&ui.painter().with_clip_rect(list), b.center(), icon, 14.0, ink);
    if resp.clicked() {
        app.toggle_group(t.id);
    }
}

/// A collapsed group's lane: its members' clips as thin bars in their colors, one row of bars per
/// member, read-only (they are not in `layout.clips`, so a click there is a click on the lane).
pub(super) fn group_summaries(app: &StudioApp, ui: &Ui, s: &Session, map: Map, tl: Rect, layout: &mut Layout) {
    let p = ui.painter().with_clip_rect(tl);
    for row in layout.tracks.iter_mut().filter(|r| r.collapsed) {
        let g = TrackId(row.id);
        let members: Vec<&Track> = s.tracks.iter().filter(|t| t.id != g && crate::groups::within(s, t.id, g) && t.kind.has_playlist()).collect();
        let lane = Rect::from_min_max(pos2(row.lane[0], row.lane[1]), pos2(row.lane[2], row.lane[3]));
        let n = members.len().max(1) as f32;
        let h = ((lane.height() - 8.0) / n).clamp(1.0, 5.0);
        let pitch = ((lane.height() - 8.0) / n).max(h);
        for (k, t) in members.iter().enumerate() {
            let y = lane.min.y + 4.0 + k as f32 * pitch;
            for c in t.clips() {
                let bar = Rect::from_min_max(pos2(map.x(c.start), y), pos2(map.x(c.end()).max(map.x(c.start) + 2.0), y + h));
                row.summary_bars += 1;
                if bar.max.x >= tl.min.x && bar.min.x <= tl.max.x {
                    p.rect_filled(bar, crate::theme::RADIUS_SM, theme::rgb(c.color.unwrap_or(t.color)));
                }
            }
        }
    }
    let _ = app;
}

/// A track header drag in progress (slice 2c): show where it would land; on release, move it
/// there (`StudioApp::drop_track`, one undo step).
pub(super) fn header_drag(app: &mut StudioApp, ui: &Ui, s: &Session, layout: &mut Layout) {
    let Some(drag) = app.arrange.header_drag else { return };
    let (down, pos) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
    let at = pos.and_then(|p| crate::groups::drop_at(s, &layout.tracks, drag.track, p.y));
    if down {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        app.arrange.header_drag = Some(crate::groups::HeaderDrag { drop: at.map(|a| a.0), ..drag });
        if let Some((d, r)) = at {
            let rect = Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]));
            let p = ui.painter();
            match d {
                crate::groups::Drop::Gap { .. } => p.rect_filled(rect, 0.0, TOKENS.accent),
                crate::groups::Drop::Into(_) => {
                    p.rect_stroke(rect.shrink(1.0), crate::theme::RADIUS_SM, Stroke::new(2.0, TOKENS.accent), StrokeKind::Inside)
                }
            };
            layout.drop = Some(r);
        }
        return;
    }
    app.arrange.header_drag = None;
    if let Some((d, _)) = at {
        let _ = app.drop_track(drag.track, d);
    }
}

/// The track's right-click menu.
fn track_menu(app: &mut StudioApp, ui: &mut Ui, id: TrackId, kind: TrackKind, has_instrument: bool) {
    if kind == TrackKind::Instrument {
        // Instrument ▸: the device chain's instrument menu (a click in its filter box keeps it open).
        let config = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let mut picked = None;
        let (button, _) = egui::containers::menu::SubMenuButton::new("Instrument").config(config).ui(ui, |ui| {
            let lists = app.instrument_lists();
            let current = app.engine.session().track(id).and_then(|t| t.instrument.as_ref()).map(|i| i.plugin.clone());
            let mut filter = std::mem::take(&mut app.mixer.instrument_filter);
            let mut filter_rect = app.mixer.filter_rect;
            let mut items = Vec::new();
            picked = crate::devices::instrument_menu(ui, &lists, current.as_deref(), &mut filter, &mut filter_rect, &mut items);
            app.mixer.instrument_filter = filter;
            app.mixer.filter_rect = filter_rect;
            app.arrange.menu.extend(items);
        });
        app.arrange.menu.push(crate::mixer::MenuItem { label: "Instrument".into(), value: String::new(), rect: r4(button.rect), visible: true });
        if let Some(plugin) = picked {
            app.mixer.instrument_filter.clear();
            let _ = app.set_instrument(id, &plugin);
            ui.close();
        }
        if ui.add_enabled(has_instrument && app.player.is_some(), egui::Button::new("Open Instrument Editor")).clicked() {
            app.show_editor((id, transit_mix::INSTRUMENT_SLOT));
            ui.close();
        }
        ui.separator();
    }
    if kind.is_midi() && ui.button("New MIDI Clip at Cursor").clicked() {
        let _ = app.new_midi_clip(Some(id));
        ui.close();
    }
    if kind == TrackKind::Audio {
        width_item(app, ui, id);
    }
    // Take lanes (#25): shown while the track has stacked takes.
    if app.has_takes(id) {
        let label = if app.ui.takes_open.contains(&id) { "Hide Take Lanes" } else { "Show Take Lanes" };
        let b = ui.button(label);
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, label.into(), String::new(), b.rect));
        if b.clicked() {
            app.toggle_takes(id);
            ui.close();
        }
        super::take_lanes::flatten_item(app, ui, id, None);
        ui.separator();
    }
    if matches!(kind, TrackKind::Audio | TrackKind::Instrument) {
        let frozen = app.engine.session().track(id).is_some_and(|t| t.freeze.is_some());
        let label = if frozen { "Unfreeze" } else { "Freeze" };
        let b = ui.button(label);
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, label.into(), String::new(), b.rect));
        if b.clicked() {
            let _ = app.toggle_freeze(id);
            ui.close();
        }
        let b = ui.button("Flatten");
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Flatten".into(), String::new(), b.rect));
        if b.clicked() {
            let _ = app.flatten_track(id);
            ui.close();
        }
    }
    if kind != TrackKind::Master {
        crate::groups::menu_items(app, ui, Some(id));
        ui.separator();
    }
    ui.menu_button("Color", |ui| {
        for (i, c) in PALETTE.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(120.0, 16.0), Sense::click());
            // The outline carries the 3:1 boundary where a pale swatch meets the light popup.
            ui.painter().rect_filled(r.shrink(1.0), crate::theme::RADIUS_SM, theme::rgb(*c));
            ui.painter().rect_stroke(r.shrink(1.0), crate::theme::RADIUS_SM, Stroke::new(1.0, TOKENS.text_dim), StrokeKind::Inside);
            if resp.clicked() {
                let _ = app.run("track.color", json!({"tracks": [id.0], "color": c}));
                ui.close();
            }
            let _ = i;
        }
    });
    ui.separator();
    let del = ui.button("Delete Track");
    app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Delete Track".into(), String::new(), del.rect));
    if del.clicked() {
        let _ = app.request_delete_tracks(vec![id]);
        ui.close();
    }
}

/// "Make Stereo" on a mono audio track, "Make Mono" on a stereo one (`track.change_width`, one
/// undo step). Other widths (surround) are left to the engine command.
pub(crate) fn width_item(app: &mut StudioApp, ui: &mut Ui, id: TrackId) -> Option<egui::Response> {
    let channels = app.engine.session().track(id)?.channels();
    let (label, format) = match channels {
        1 => ("Make Stereo", "stereo"),
        2 => ("Make Mono", "mono"),
        _ => return None,
    };
    let reason = app
        .width_change_disabled_reason()
        .map(str::to_string)
        .or_else(|| app.engine.session().track(id).and_then(|track| app.width_input(track, format).err()));
    let response = ui.add_enabled(reason.is_none(), egui::Button::new(label)).on_disabled_hover_text(reason.unwrap_or_default());
    if response.clicked() {
        let _ = app.run("track.change_width", json!({"tracks": [id.0], "format": format}));
        ui.close();
    }
    Some(response)
}

/// Track colors offered in the track menu (muted, distinguishable on the dark lanes).
pub(crate) const PALETTE: [[u8; 3]; 10] = [
    [0xd9, 0x5f, 0x5f],
    [0xe0, 0x8c, 0x4a],
    [0xd6, 0xbe, 0x4f],
    [0x8f, 0xc0, 0x5a],
    [0x4f, 0xb8, 0x8c],
    [0x4a, 0xb4, 0xc8],
    [0x5b, 0x8f, 0xdb],
    [0x8e, 0x74, 0xd6],
    [0xc7, 0x6a, 0xc0],
    [0x9a, 0x9c, 0xa3],
];

fn short_plugin(id: &str) -> String {
    let name = id.rsplit(':').next().unwrap_or(id);
    if name.len() > 24 { format!("{}…", name.chars().take(22).collect::<String>()) } else { name.to_string() }
}

/// 0..1 meter position for a linear peak (-60 dB at the bottom).
fn meter_pos(lin: f32) -> f32 {
    if lin <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * lin.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}
