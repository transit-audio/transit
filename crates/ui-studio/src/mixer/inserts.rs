use super::*;

/// Height of an insert row, and the most rows a strip has for them: three inserts and "+", or
/// two, "n more…" and "+" in a short pane (one row: label beside a reserved "+" cell).
pub(super) const INSERT_ROW_H: f32 = 15.0;
pub(super) const INSERT_ROWS: usize = 4;
pub(super) const MIN_FADER: f32 = 80.0;

/// A strip's insert rows at `y`: one per occupied slot (click or double-click: the plug-in's editor; right-click:
/// bypass and remove), then a "+" row that adds an effect to this track. `rows` rows in all, so
/// every strip's faders line up; with more inserts than fit, the last row says how many more.
#[allow(clippy::too_many_arguments)]
pub(super) fn inserts_block(
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
        crate::help::hover(ui, &resp, crate::help::Id::MixerStripInsert);
        p.rect_filled(rr, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.bg });
        let mut text_r = rr.shrink2(vec2(4.0, 0.0));
        // While the plug-in is on its way: "Loading…", "Loading… (slow) 12 s", "Not responding"
        // with a Retry button at the row's end (issue #82).
        let load = app.player.as_ref().and_then(|pl| pl.slot_load(id, *slot));
        let stuck = matches!(load, Some(transit_playback::SlotLoad::NotResponding { .. }));
        let last = app.last_load_time(&ins.plugin);
        let mut status = load.map(|l| crate::devices::load_text(l, last)).unwrap_or_default();
        // A loaded plug-in that does not answer (#213): the same marker as the device card.
        let hang = if load.is_none() { app.slot_hang(id, *slot, &ins.plugin) } else { None };
        if hang.is_some() {
            status = crate::devices::UNRESPONSIVE.to_string();
        }
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
            if ins.bypass || app.mode_bypassed(ins.uid.get()) { TOKENS.text_dim } else { TOKENS.text },
        );
        // A built-in's editor is our own window: it needs no player (the card does the same).
        let builtin = app.builtin_at((id, *slot)).is_some();
        let enabled = builtin || app.player.as_ref().is_some_and(|pl| crate::devices::hosted(&ins.plugin) || pl.has_editor(id, *slot));
        if resp.clicked() && enabled {
            // Selecting an insert opens its editor, or raises the one already open (#173).
            app.show_editor((id, *slot));
        }
        if let Some(tip) = app.mode_bypass_tip(ins.uid.get()) {
            crate::help::hover(ui, &resp, crate::help::Id::DeviceLowLatencyBypass);
            resp.clone().on_hover_text(tip);
        }
        let state = if ins.bypass { " (bypassed)" } else { "" };
        let why = hang.map_or(String::new(), |h| format!(" {}", app.hang_tip(h, &name)));
        let resp = resp.on_hover_text(format!("{name}{state}. Click for its window, right-click for more.{why}"));
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
        sl.inserts.push(InsertRow { low_latency_bypass: app.mode_bypassed(ins.uid.get()), slot, name, bypass, rect: r4(rr), status, retry });
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
        crate::help::hover(ui, &resp, crate::help::Id::MixerStripAddEffect);
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

/// A held peak (linear) as the readout shows it: "-3.2", "0.0", "+1.4" (red from 0 dBFS), "-inf".
/// A MIDI strip's output destination button (#61): "Out" in the accent color when the track
/// sends to an external port, red with a warning when that port's device is missing.
pub(super) fn midi_output_button(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, r: Rect, sl: &mut StripLayout) {
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
        crate::icons::paint(p, r.center(), crate::icons::Icon::Error, 11.0, ink);
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
