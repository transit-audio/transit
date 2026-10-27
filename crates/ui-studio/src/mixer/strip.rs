use super::*;

/// The bottom panes: device chain (lowest) and the mixer above it. Call before the central panel.
pub fn panes(app: &mut StudioApp, ui: &mut Ui) {
    app.mixer.layout.menu.clear();
    app.mixer.devices.menu.clear();
    app.mixer.filter_rect = Default::default();
    app.mixer.hover = None;
    app.mixer.wheel_taken = false;
    if app.mixer.edit.is_none() {
        app.mixer.edit_rect = [0.0; 4];
    }
    app.mixer.devices.visible = app.ui.show_devices;
    if app.ui.show_devices {
        egui::Panel::bottom("devices")
            .exact_size(crate::devices::pane_height(app))
            .frame(egui::Frame::NONE.fill(TOKENS.panel))
            .show(ui, |ui| crate::devices::chain(app, ui));
    }
    app.mixer.layout.visible = app.ui.show_mixer || app.ui.mixer_full;
    if app.ui.show_mixer && !app.ui.mixer_full {
        let h = app.ui.mixer_height.clamp(260.0, 600.0);
        egui::Panel::bottom("mixer").exact_size(h).frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
    }
    crate::keys::switcher_panel(app, ui);
}

/// The mixer filling the window (F3).
pub fn show_full(app: &mut StudioApp, ui: &mut Ui) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(TOKENS.bg)).show(ui, |ui| strips(app, ui));
}

fn strips(app: &mut StudioApp, ui: &mut Ui) {
    let area = ui.max_rect();
    let p = ui.painter();
    p.line_segment([area.left_top(), area.right_top()], Stroke::new(1.0, TOKENS.line));
    let s = app.engine.session_arc();
    let focus = app.chosen_track();
    let master = s.tracks.iter().find(|t| t.kind == TrackKind::Master && !t.hidden);
    // Stable partition of the visible strips only: the document/arranger order never changes.
    // Routing folders are tracks and stay inline with their members, not in the aux block.
    let (mut channels, aux): (Vec<&Track>, Vec<&Track>) =
        s.tracks.iter().filter(|t| t.kind != TrackKind::Master && !t.hidden && !app.tucked(&s, t)).partition(|t| t.kind != TrackKind::Aux);
    let track_count = channels.len();
    let block_gap = if track_count > 0 && !aux.is_empty() { FX_GAP } else { 0.0 };
    channels.extend(aux);
    // The master strip is pinned right; the others scroll under it.
    let master_rect = Rect::from_min_max(pos2(area.max.x - STRIP_W - 6.0, area.min.y + 4.0), pos2(area.max.x - 6.0, area.max.y - 4.0));
    let lane = Rect::from_min_max(pos2(area.min.x + 4.0, area.min.y + 4.0), pos2(master_rect.min.x - 10.0, area.max.y - 4.0));
    let content_w = channels.len() as f32 * (STRIP_W + GAP) + block_gap + 110.0;
    app.mixer.hscroll = app.mixer.hscroll.clamp(0.0, (content_w - lane.width()).max(0.0));
    let mut layout = MixerLayout { visible: true, area: r4(area), menu: std::mem::take(&mut app.mixer.layout.menu), ..MixerLayout::default() };
    // The sends block is as tall on every strip (the most sends plus "+ Send"), so the faders
    // line up, and never so tall that a fader gets shorter than MIN_FADER.
    let wanted = channels.iter().filter(|t| takes_sends(t.kind)).map(|t| send_rows_of(t)).max().unwrap_or(0);
    // Insert rows come first (up to INSERT_ROWS, fewer in a short pane); sends get what is left.
    let free = lane.height() - STRIP_FIXED_H - MIN_FADER;
    let ins_rows = (((free - 2.0 * SEND_ROW_H) / INSERT_ROW_H).floor().max(1.0) as usize).min(INSERT_ROWS);
    let room = ((free - ins_rows as f32 * INSERT_ROW_H) / SEND_ROW_H).floor().max(0.0) as usize;
    let rows = wanted.min(room);
    // Empty mixer space deselects, like empty space in the arranger. Registered first so every
    // strip and control above it wins the click.
    if ui.interact(area, ui.id().with("mixer_empty"), Sense::click()).clicked() {
        app.deselect_all();
    }
    for (i, t) in channels.iter().enumerate() {
        let x = lane.min.x + i as f32 * (STRIP_W + GAP) + if i >= track_count { block_gap } else { 0.0 } - app.mixer.hscroll;
        let r = Rect::from_min_size(pos2(x, lane.min.y), vec2(STRIP_W, lane.height()));
        if r.max.x < lane.min.x || r.min.x > lane.max.x {
            continue;
        }
        let sl = strip(app, ui, &s, t, r, lane, focus == Some(t.id), (rows, ins_rows), &mut layout.menu);
        layout.strips.push(sl);
    }
    layout.groups = group_links(ui, &s, &channels[..track_count], lane, area, app.mixer.hscroll);
    if block_gap > 0.0 {
        let x = lane.min.x + track_count as f32 * (STRIP_W + GAP) + block_gap / 2.0 - app.mixer.hscroll;
        let divider = Rect::from_min_max(pos2(x, lane.min.y), pos2(x + 1.0, lane.max.y)).intersect(lane);
        if divider.is_positive() {
            ui.painter().with_clip_rect(lane).rect_filled(divider, 0.0, TOKENS.line);
            layout.fx_divider = Some(r4(divider));
        }
    }
    // "Add FX Channel" after the last strip.
    let ax = lane.min.x + channels.len() as f32 * (STRIP_W + GAP) + block_gap - app.mixer.hscroll + 6.0;
    let add = Rect::from_min_size(pos2(ax, lane.min.y + 4.0), vec2(96.0, 24.0));
    if add.min.x < lane.max.x {
        let resp = ui.interact(add.intersect(lane), ui.id().with("add_fx"), Sense::click());
        let p = ui.painter().with_clip_rect(lane);
        p.rect_filled(add, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        crate::icons::paint_labeled(&p, add, false, crate::icons::Icon::Plus, 12.0, "FX Channel", FontId::proportional(11.5), TOKENS.text);
        if resp.on_hover_text("A new FX channel (add a send to it from a strip's + Send)").clicked() {
            let _ = app.add_fx_channel();
        }
    }
    layout.add_fx = r4(add);
    // Master.
    ui.painter().line_segment([pos2(master_rect.min.x - 5.0, area.min.y), pos2(master_rect.min.x - 5.0, area.max.y)], Stroke::new(1.0, TOKENS.line));
    match master {
        Some(m) => {
            let sl = strip(app, ui, &s, m, master_rect, master_rect, focus == Some(m.id), (rows, ins_rows), &mut layout.menu);
            layout.master = Some(sl);
        }
        None => {
            let (sl, add) = main_out(app, ui, master_rect);
            layout.master = Some(sl);
            layout.add_master = add;
        }
    }
    // Wheel over the strips scrolls them sideways.
    if let Some(pos) = ui.input(|i| i.pointer.hover_pos())
        && lane.contains(pos)
        && app.mixer.gesture.is_none()
        && !app.mixer.wheel_taken
    {
        let d = ui.input(|i| i.smooth_scroll_delta);
        let d = if d.x.abs() > d.y.abs() { d.x } else { d.y };
        if d.abs() > 0.1 {
            app.mixer.hscroll = (app.mixer.hscroll - d).clamp(0.0, (content_w - lane.width()).max(0.0));
        }
    }
    app.mixer.layout = layout;
}

/// Above the strips, a thin accent bar from each group's strip over its members' strips.
fn group_links(ui: &Ui, s: &Session, channels: &[&Track], lane: Rect, area: Rect, hscroll: f32) -> Vec<GroupLink> {
    let x = |i: usize| lane.min.x + i as f32 * (STRIP_W + GAP) - hscroll;
    let p = ui.painter().with_clip_rect(Rect::from_min_max(pos2(lane.min.x, area.min.y), pos2(lane.max.x, lane.min.y)));
    let mut out = Vec::new();
    for (i, g) in channels.iter().enumerate().filter(|(_, t)| t.is_folder()) {
        let members: Vec<u64> = channels[i + 1..]
            .iter()
            .take_while(|t| crate::groups::within(s, t.id, g.id))
            .filter(|t| t.folder == Some(g.id))
            .map(|t| t.id.0)
            .collect();
        let last = channels[i + 1..].iter().take_while(|t| crate::groups::within(s, t.id, g.id)).count();
        if last == 0 {
            continue;
        }
        let r = Rect::from_min_max(pos2(x(i) + 2.0, area.min.y + 1.0), pos2(x(i + last) + STRIP_W - 2.0, area.min.y + 3.0));
        p.rect_filled(r, crate::theme::RADIUS_SM, TOKENS.accent);
        out.push(GroupLink { folder: g.id.0, members, rect: r4(r) });
    }
    out
}

/// The main-out strip when the session has no master track: the main meter and an offer.
fn main_out(app: &mut StudioApp, ui: &mut Ui, r: Rect) -> (StripLayout, R4) {
    let p = ui.painter();
    p.rect_filled(r, crate::theme::RADIUS, TOKENS.panel);
    p.text(pos2(r.center().x, r.min.y + 12.0), Align2::CENTER_CENTER, "Main", FontId::proportional(12.0), TOKENS.text);
    let add = Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 28.0), vec2(r.width() - 12.0, 34.0));
    let resp = ui.interact(add, ui.id().with("add_master"), Sense::click());
    p.rect_filled(add, crate::theme::RADIUS, if resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
    crate::icons::paint_labeled(p, add, false, crate::icons::Icon::Plus, 11.0, "Master\nfader", FontId::proportional(10.5), TOKENS.text);
    if resp.clicked() {
        let _ = app.run("track.new", json!({"kind": "master"}));
    }
    let meter = Rect::from_min_max(pos2(r.center().x - 8.0, r.min.y + 76.0), pos2(r.center().x + 8.0, r.max.y - 26.0));
    draw_meter(p, meter, app.main_meter.level, app.main_meter.clip);
    let peak_r = Rect::from_min_max(pos2(r.min.x + 6.0, r.min.y + 63.0), pos2(r.max.x - 6.0, r.min.y + 75.0));
    let (text, over) = peak_text(app.main_meter.peak);
    let resp = ui.interact(peak_r, ui.id().with("main_peak"), Sense::click());
    p.rect_filled(peak_r, crate::theme::RADIUS_SM, TOKENS.bg);
    let color = if over || app.main_meter.clip { TOKENS.record } else { TOKENS.text_dim };
    p.text(peak_r.center(), Align2::CENTER_CENTER, &text, FontId::proportional(10.0), color);
    let on_meter = ui.interact(meter, ui.id().with("main_meter_click"), Sense::click());
    if resp.clicked() || on_meter.clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        if all {
            app.reset_meters(None);
        } else {
            app.reset_main_meter();
        }
    }
    let sl = StripLayout {
        peak_rect: r4(peak_r),
        peak_text: text,
        name: "Main".into(),
        rect: r4(r),
        meter: r4(meter),
        level: app.main_meter.level,
        ..StripLayout::default()
    };
    (sl, r4(add))
}

const STRIP_FIXED_H: f32 = 207.0;
/// Height of an insert row, and the most rows a strip has for them: three inserts and "+", or
/// two, "n more…" and "+" in a short pane (one row: label beside a reserved "+" cell).
const INSERT_ROW_H: f32 = 15.0;
const INSERT_ROWS: usize = 4;
const MIN_FADER: f32 = 80.0;

/// A strip's insert rows at `y`: one per occupied slot (click or double-click: the plug-in's editor; right-click:
/// bypass and remove), then a "+" row that adds an effect to this track. `rows` rows in all, so
/// every strip's faders line up; with more inserts than fit, the last row says how many more.
#[allow(clippy::too_many_arguments)]
fn inserts_block(
    app: &mut StudioApp,
    ui: &mut Ui,
    t: &Track,
    x0: f32,
    w: f32,
    y: f32,
    rows: usize,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let list: Vec<(usize, transit_model::Insert)> = t.mixer.inserts.iter().enumerate().filter_map(|(i, x)| x.clone().map(|x| (i, x))).collect();
    let free = list.len() < t.mixer.inserts.len();
    let row_r = |i: usize| Rect::from_min_size(pos2(x0, y + i as f32 * INSERT_ROW_H), vec2(w, INSERT_ROW_H - 2.0));
    // The last row is always "+" (while a slot is free); a list that does not fit above it gives
    // up its last visible row to "n more…". A single row shares the label and a reserved + cell.
    let room = rows.saturating_sub(usize::from(free));
    let fits = list.len() <= room;
    let shown = if fits { list.len() } else { room.saturating_sub(1) };
    let p = ui.painter().with_clip_rect(clip);
    let id = t.id;
    for (i, (slot, ins)) in list.iter().take(shown).enumerate() {
        let rr = row_r(i);
        let name = app.plugin_name(&ins.plugin, false);
        let resp = ui.interact(rr.intersect(clip), ui.id().with(("insert", id.0, *slot)), Sense::click());
        p.rect_filled(rr, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.bg });
        let mut text_r = rr.shrink2(vec2(4.0, 0.0));
        // While the plug-in is on its way: "Loading…", "Loading… (slow) 12 s", "Not responding"
        // with a Retry button at the row's end (issue #82).
        let load = app.player.as_ref().and_then(|pl| pl.slot_load(id, *slot));
        let stuck = matches!(load, Some(transit_playback::SlotLoad::NotResponding { .. }));
        let last = app.last_load_time(&ins.plugin);
        let status = load.map(|l| crate::devices::load_text(l, last)).unwrap_or_default();
        let shown = if status.is_empty() { name.clone() } else { format!("{name} ({status})") };
        let mut retry = R4::default();
        if stuck {
            let rb = Rect::from_center_size(pos2(rr.max.x - 9.0, rr.center().y), vec2(14.0, 14.0));
            text_r.max.x = rb.min.x - 2.0;
            crate::devices::retry_button(app, ui, &p, clip, rb, ui.id().with(("insert_retry", id.0, *slot)), id, *slot, &name);
            retry = r4(rb);
        }
        p.with_clip_rect(text_r.intersect(clip)).text(
            text_r.left_center(),
            Align2::LEFT_CENTER,
            &shown,
            FontId::proportional(10.5),
            if ins.bypass { TOKENS.text_dim } else { TOKENS.text },
        );
        // A built-in's editor is our own window: it needs no player (the card does the same).
        let builtin = app.builtin_at((id, *slot)).is_some();
        let enabled = builtin || app.player.as_ref().is_some_and(|pl| crate::devices::hosted(&ins.plugin) || pl.has_editor(id, *slot));
        if resp.clicked() && enabled {
            app.want_editor((id, *slot));
        }
        let state = if ins.bypass { " (bypassed)" } else { "" };
        let resp = resp.on_hover_text(format!("{name}{state}. Click for its window, right-click for more"));
        let slot = *slot;
        let bypass = ins.bypass;
        let editor_open = app.ui.plugin_windows.contains(&(id, slot));
        let retry_name = name.clone();
        egui::Popup::context_menu(&resp).id(resp.id.with("insert_ctx")).show(|ui| {
            let label = if editor_open { "Close editor" } else { "Open editor" };
            let b = ui.add_enabled(enabled, egui::Button::new(label));
            menu.push(MenuItem::at(ui, label.into(), "editor".into(), b.rect));
            if b.clicked() {
                app.toggle_editor((id, slot));
                ui.close();
            }
            if stuck {
                let b = ui.button("Retry").on_hover_text(format!("Retry loading {retry_name}"));
                menu.push(MenuItem::at(ui, "Retry".into(), "retry".into(), b.rect));
                if b.clicked() {
                    let _ = app.retry_plugin(id, slot);
                    ui.close();
                }
            }
            let label = if bypass { "Turn On" } else { "Bypass" };
            let b = ui.button(label);
            menu.push(MenuItem::at(ui, label.into(), "bypass".into(), b.rect));
            if b.clicked() {
                let _ = app.toggle_bypass(id, slot);
                ui.close();
            }
            let b = ui.button("Remove");
            menu.push(MenuItem::at(ui, "Remove".into(), "remove".into(), b.rect));
            if b.clicked() {
                let _ = app.remove_device(id, slot);
                ui.close();
            }
        });
        sl.inserts.push(InsertRow { slot, name, bypass, rect: r4(rr), status, retry });
    }
    if rows == 0 {
        return;
    }
    if !fits {
        let mut rr = row_r(shown);
        if room == 0 {
            rr.max.x -= 20.0;
        }
        let resp = ui.interact(rr.intersect(clip), ui.id().with(("more_inserts", id.0)), Sense::click());
        p.with_clip_rect(rr.intersect(clip)).text(
            pos2(rr.min.x + 4.0, rr.center().y),
            Align2::LEFT_CENTER,
            format!("{} more…", list.len() - shown),
            FontId::proportional(10.5),
            TOKENS.text_dim,
        );
        if resp.on_hover_text("Show all devices in the device chain").clicked() {
            app.focus(id);
            app.ui.show_devices = true;
        }
    }
    if free {
        let mut rr = row_r(rows - 1);
        if !fits && room == 0 {
            rr.min.x = rr.max.x - 18.0;
        }
        let resp = ui.interact(rr.intersect(clip), ui.id().with(("add_insert", id.0)), Sense::click());
        if resp.hovered() {
            p.rect_filled(rr, crate::theme::RADIUS_SM, TOKENS.button_hover);
        }
        crate::icons::paint(&p, pos2(rr.min.x + 9.0, rr.center().y), crate::icons::Icon::Plus, 12.0, TOKENS.text_dim);
        let resp = resp.on_hover_text("Add an effect to this track");
        let before = list.len();
        crate::devices::add_device_menu(app, &resp, id, menu);
        if app.engine.session().track(id).is_some_and(|x| x.mixer.inserts.iter().flatten().count() > before) {
            app.focus(id);
        }
        sl.add_insert = r4(rr);
    }
}

/// One channel strip.
#[allow(clippy::too_many_arguments)]
fn strip(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    focused: bool,
    (rows, ins_rows): (usize, usize),
    menu: &mut Vec<MenuItem>,
) -> StripLayout {
    let id = t.id;
    let m = &t.mixer;
    // Under Read automation the controls show the lane's value at the playhead (no command, no edit).
    let vol_db = app.followed(t, &crate::automation::Lane::Volume).unwrap_or(m.volume_db);
    let bal = app.followed(t, &crate::automation::Lane::Pan).unwrap_or_else(|| balance_of(&m.pan));
    let p = ui.painter().with_clip_rect(clip);
    let background = if focused { TOKENS.header_selected } else { TOKENS.panel };
    p.rect_filled(r, crate::theme::RADIUS, background);
    let mut sl = StripLayout {
        track: id.0,
        name: t.name.clone(),
        kind: Some(t.kind),
        rect: r4(r),
        volume_db: vol_db,
        balance: bal,
        pan: m.pan.clone(),
        level: app.meters.get(&id).map_or([0.0; 2], |d| d.level),
        fx: t.kind == TrackKind::Aux,
        focused,
        folder: t.folder.map(|f| f.0),
        ..StripLayout::default()
    };
    let x0 = r.min.x + 4.0;
    let w = r.width() - 8.0;
    // Name and color: click focuses the track.
    let name_r = Rect::from_min_size(pos2(r.min.x, r.min.y), vec2(r.width(), 22.0));
    p.rect_filled(Rect::from_min_size(name_r.min, vec2(r.width(), 4.0)), crate::theme::top_corners(), theme::rgb(t.color));
    let mut tr = name_r.shrink2(vec2(5.0, 0.0));
    if sl.fx {
        // FX channels: a small "FX" tag and a muted name.
        let tag = Rect::from_min_size(pos2(tr.max.x - 18.0, tr.center().y - 5.0), vec2(18.0, 13.0));
        p.rect_filled(tag, crate::theme::RADIUS_SM, TOKENS.button);
        p.text(tag.center(), Align2::CENTER_CENTER, "FX", FontId::proportional(9.5), TOKENS.text_dim);
        tr.max.x = tag.min.x - 3.0;
    }
    p.with_clip_rect(tr.intersect(clip)).text(
        pos2(tr.min.x, tr.center().y + 2.0),
        Align2::LEFT_CENTER,
        &t.name,
        FontId::proportional(13.0),
        if focused {
            TOKENS.accent
        } else if sl.fx {
            TOKENS.text_dim
        } else {
            TOKENS.text
        },
    );
    sl.name_rect = r4(name_r);
    // Anywhere on the strip focuses it; controls drawn after this take their own clicks.
    let resp = ui.interact(r.intersect(clip), ui.id().with(("strip_name", id.0)), Sense::click());
    if resp.clicked() {
        let shift = ui.input(|i| i.modifiers.shift);
        app.focus_header(id, shift);
    }
    // Slice 6: double-click the name to rename the track in place.
    if resp.double_clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| name_r.contains(p)) {
        app.start_rename(crate::finish::RenameTarget::Track(id), r4(name_r));
    }
    if t.kind == TrackKind::Audio {
        resp.context_menu(|ui| {
            crate::arrange::width_item(app, ui, id);
        });
    }
    let mut y = r.min.y + 26.0;
    // Hardware/bus input for audio tracks; device/channel input for MIDI and instruments.
    let in_r = Rect::from_min_size(pos2(x0, y), vec2(w, 16.0));
    if matches!(t.kind, TrackKind::Audio | TrackKind::Aux) {
        let cur = route_label(s, &m.input, true, if t.kind == TrackKind::Audio { t.channels() } else { 0 });
        let mut choices = input_choices(s, t, app.device_inputs(), app.engine.io_map());
        push_unused_entry(s, &mut choices);
        if let Some(v) = dropdown(ui, clip, in_r, ui.id().with(("input", id.0)), &cur, None, &choices, menu) {
            if v == REMOVE_UNUSED {
                let _ = app.remove_unused_busses();
            } else {
                let _ = app.set_input(id, &v);
            }
        }
        sl.input = r4(in_r);
        sl.input_label = cur;
    } else if t.kind.is_midi() {
        // The MIDI output destination (#61) is the small Out button at the row's end.
        let out_w = 22.0;
        let out_r = Rect::from_min_max(pos2(in_r.max.x - out_w, in_r.min.y), in_r.max);
        let in_r = Rect::from_min_max(in_r.min, pos2(in_r.max.x - out_w - 2.0, in_r.max.y));
        midi_output_button(app, ui, &p, clip, t, out_r, &mut sl);
        let view = crate::midi_input::InputView::read(app.player.as_ref());
        let label = view.label(&t.midi_input);
        let resp = ui.interact(in_r.intersect(clip), ui.id().with(("midi-input", id.0)), Sense::click());
        p.rect_filled(in_r, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.bg });
        let text_r = in_r.shrink2(vec2(4.0, 0.0));
        p.with_clip_rect(text_r.intersect(clip)).text(text_r.left_center(), Align2::LEFT_CENTER, &label, FontId::proportional(10.5), TOKENS.text);
        crate::icons::paint(&p, pos2(in_r.max.x - 7.0, in_r.center().y), crate::icons::Icon::ChevronDown, 10.0, TOKENS.text_dim);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "MIDI input"));
        if let Some(params) = view.popup(&resp, id, &t.midi_input) {
            let _ = app.run("midi.input", params);
        }
        sl.input = r4(in_r);
        sl.input_label = label;
    }
    y += 19.0;
    // Inserts (Logic: under the input), then sends.
    inserts_block(app, ui, t, x0, w, y, ins_rows, clip, &mut sl, menu);
    y += ins_rows as f32 * INSERT_ROW_H;
    // Sends (Logic: between the input and the output).
    sends_block(app, ui, s, t, x0, w, y, rows, clip, &mut sl, menu);
    y += rows as f32 * SEND_ROW_H;
    // Output.
    let out_r = Rect::from_min_size(pos2(x0, y), vec2(w, 16.0));
    let cur = route_label(s, &m.output, false, t.channels());
    let mut choices = output_choices(s, t, app.engine.io_map());
    push_unused_entry(s, &mut choices);
    let tip = unresolved_output(app.engine.io_map(), &m.output).then_some(NOT_HERE);
    if let Some(v) = dropdown(ui, clip, out_r, ui.id().with(("output", id.0)), &cur, tip, &choices, menu) {
        if v == REMOVE_UNUSED {
            let _ = app.remove_unused_busses();
        } else {
            let _ = app.set_output(id, &v);
        }
    }
    sl.output = r4(out_r);
    sl.output_label = cur;
    y += 20.0;
    y += 4.0;
    // Balance knob.
    let knob = Rect::from_center_size(pos2(r.center().x, y + 18.0), vec2(34.0, 34.0));
    sl.knob = r4(knob);
    let b = bal;
    let has_pan = !m.pan.is_empty();
    let pan = Target::Pan(id);
    let resp = ui.interact(knob.intersect(clip), ui.id().with(("knob", id.0)), Sense::click_and_drag());
    let hot = resp.hovered() || app.mixer.gesture.is_some_and(|g| g.target == pan);
    draw_knob(&p, knob, b, hot, background);
    if has_pan {
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        control(app, ui, &resp, pan, 1.0, true, true);
        resp.context_menu(|ui| app.midi_learn_menu(ui, id, transit_model::Control::Pan));
        crate::midi_learn::mark(app, &p, knob, id, transit_model::Control::Pan);
    }
    let pan_r = Rect::from_center_size(pos2(r.center().x, knob.max.y + 7.0), vec2(34.0, 12.0));
    let pan_text = fine::pan_text(b);
    if has_pan {
        readout(
            app,
            ui,
            &p,
            clip,
            Readout {
                target: pan,
                salt: 0,
                draw: pan_r,
                hit: pan_r,
                text: pan_text.clone(),
                edit_text: pan_text.clone(),
                font: theme::mono(ui.ctx(), 9.5),
                color: TOKENS.text_dim,
                align: Align2::CENTER_CENTER,
                drag: false,
            },
        );
    } else {
        p.text(pan_r.center(), Align2::CENTER_CENTER, &pan_text, theme::mono(ui.ctx(), 9.5), TOKENS.text_dim);
    }
    sl.pan_readout_rect = r4(pan_r);
    sl.pan_text = pan_text;
    y = knob.max.y + 16.0;
    // M S / R I.
    let bw = (w - 3.0) / 2.0;
    let cell = |i: usize, row: f32| Rect::from_min_size(pos2(x0 + (i as f32) * (bw + 3.0), row), vec2(bw, 16.0));
    let buttons: [(&str, bool, Color32, &str, bool, Rect); 4] = [
        ("M", m.mute, TOKENS.mute, "mix.mute", true, cell(0, y)),
        ("S", m.solo, TOKENS.solo, "mix.solo", true, cell(1, y)),
        ("R", m.record_arm, TOKENS.record, "mix.record_arm", t.kind.has_playlist(), cell(0, y + 19.0)),
        ("I", m.input_monitor, TOKENS.monitor, "mix.input_monitor", t.kind == TrackKind::Audio, cell(1, y + 19.0)),
    ];
    if t.kind == TrackKind::Master {
        // A master has no solo, record or input.
        let resp = small_button(ui, clip, cell(0, y), ui.id().with(("mix.mute", id.0)), "M", m.mute, TOKENS.mute, true);
        if resp.clicked() {
            let _ = app.run("mix.mute", json!({"tracks": [id.0], "value": !m.mute}));
        }
        resp.context_menu(|ui| app.midi_learn_menu(ui, id, transit_model::Control::Mute));
        crate::midi_learn::mark(app, &p, cell(0, y), id, transit_model::Control::Mute);
        sl.mute = r4(cell(0, y));
    } else {
        for (label, on, color, cmd, enabled, br) in buttons {
            let resp = small_button(ui, clip, br, ui.id().with((cmd, id.0, "strip")), label, on, color, enabled);
            if resp.clicked() && enabled {
                let _ = app.run(cmd, json!({"tracks": [id.0], "value": !on}));
            }
            let learnable = match label {
                "M" => Some(transit_model::Control::Mute),
                "S" => Some(transit_model::Control::Solo),
                "R" => Some(transit_model::Control::Arm),
                _ => None,
            };
            if let Some(control) = learnable.filter(|_| enabled) {
                resp.context_menu(|ui| app.midi_learn_menu(ui, id, control.clone()));
                crate::midi_learn::mark(app, &p, br, id, control);
            }
            match label {
                "M" => sl.mute = r4(br),
                "S" => sl.solo = r4(br),
                "R" => sl.arm = r4(br),
                _ => sl.monitor = r4(br),
            }
        }
    }
    y += 42.0;
    // The held peak above the meter.
    let peak_r = Rect::from_min_max(pos2(r.center().x + 2.0, y + 1.0), pos2(r.max.x - 6.0, y + 14.0));
    let held = app.meters.get(&id).map_or(0.0, |d| d.peak);
    let (peak_text, over) = peak_text(held);
    let over = over || app.meters.get(&id).is_some_and(|d| d.clip);
    let resp = ui.interact(peak_r.intersect(clip), ui.id().with(("peak", id.0)), Sense::click());
    p.rect_filled(peak_r, crate::theme::RADIUS_SM, TOKENS.bg);
    p.with_clip_rect(peak_r).text(
        peak_r.center(),
        Align2::CENTER_CENTER,
        &peak_text,
        FontId::proportional(10.0),
        if over {
            TOKENS.record
        } else if resp.hovered() {
            TOKENS.accent
        } else {
            TOKENS.text_dim
        },
    );
    if resp.on_hover_text("Peak hold. Click to reset, Ctrl-click to reset all").clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        app.reset_meters(if all { None } else { Some(id) });
    }
    sl.peak_rect = r4(peak_r);
    sl.peak_text = peak_text;
    y += 12.0;
    // Fader and meter.
    let bottom = r.max.y - 22.0;
    let fader = Rect::from_min_max(pos2(x0 + 4.0, y + 6.0), pos2(r.center().x + 4.0, bottom - 6.0));
    let meter = Rect::from_min_max(pos2(r.center().x + 12.0, y + 6.0), pos2(r.max.x - 10.0, bottom - 6.0));
    sl.fader = r4(fader);
    sl.meter = r4(meter);
    let d = app.meters.get(&id).copied().unwrap_or_default();
    draw_meter(&p, meter, d.level, d.clip);
    let resp = ui.interact(meter.intersect(clip), ui.id().with(("meter", id.0)), Sense::click());
    if resp.on_hover_text("Click to clear the clip latch and the peak hold, Ctrl-click for every strip").clicked() {
        let all = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        app.reset_meters(if all { None } else { Some(id) });
    }
    let ((cy, cs), (zy, zs)) = fader_widget(app, ui, &p, clip, id, vol_db, fader);
    (sl.cap_line_y, sl.cap_line_span, sl.zero_tick_y, sl.zero_tick_span) = (cy, cs, zy, zs);
    let db = vol_db;
    let text = fine::db_text(db);
    let ro = Rect::from_min_max(pos2(x0, bottom), pos2(r.max.x - 4.0, r.max.y - 4.0));
    p.rect_filled(ro, crate::theme::RADIUS_SM, TOKENS.bg);
    let fader_id = Target::Fader(id);
    let hot = app.mixer.gesture.is_some_and(|g| g.target == fader_id);
    readout(
        app,
        ui,
        &p,
        clip,
        Readout {
            target: fader_id,
            salt: 0,
            draw: ro,
            hit: ro,
            text: text.clone(),
            edit_text: fine::edit_text_db(db),
            font: theme::mono(ui.ctx(), 10.5),
            color: if hot { TOKENS.accent } else { TOKENS.text },
            align: Align2::CENTER_CENTER,
            drag: false,
        },
    );
    sl.readout_rect = r4(ro);
    sl.volume_text = text;
    sl
}

/// A held peak (linear) as the readout shows it: "-3.2", "0.0", "+1.4" (red from 0 dBFS), "-inf".
/// A MIDI strip's output destination button (#61): "Out" in the accent color when the track
/// sends to an external port, red with a warning when that port's device is missing.
fn midi_output_button(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, r: Rect, sl: &mut StripLayout) {
    let view = crate::midi_output::OutputView::read(app.player.as_ref());
    let missing = view.missing(t.id, &t.midi_output);
    let resp = ui.interact(r.intersect(clip), ui.id().with(("midi-output", t.id.0)), Sense::click());
    let fill = if missing {
        TOKENS.record
    } else if t.midi_output.is_external() {
        TOKENS.accent
    } else if resp.hovered() {
        TOKENS.button_hover
    } else {
        TOKENS.bg
    };
    p.rect_filled(r, crate::theme::RADIUS_SM, fill);
    let ink = if missing || t.midi_output.is_external() { theme::text_on(fill) } else { TOKENS.text };
    if missing {
        crate::icons::paint(p, r.center(), crate::icons::Icon::Warning, 11.0, ink);
    } else {
        p.with_clip_rect(r.intersect(clip)).text(r.center(), Align2::CENTER_CENTER, "Out", FontId::proportional(9.5), ink);
    }
    let label = view.header_badge(t.id, &t.midi_output).unwrap_or_else(|| view.label(t.id, &t.midi_output));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "MIDI output"));
    if let Some(params) = view.popup(&resp, t.id, &t.midi_output, t.external_instrument().is_some()) {
        let _ = app.run("midi.output", params);
    }
    sl.midi_output = r4(r);
    sl.midi_output_label = label;
    sl.midi_output_missing = missing;
}
