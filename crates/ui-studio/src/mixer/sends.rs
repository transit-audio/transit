use super::*;

/// Height of one send row, and of the strip without its sends block and fader.
pub const SEND_ROW_H: f32 = 17.0;
/// The send level bar along a row's bottom edge.
const BAR_H: f32 = 5.0;
/// The rows a track's sends block wants: its sends plus "+ Send" while a slot is free.
pub(super) fn send_rows_of(t: &Track) -> usize {
    let used = t.mixer.sends.iter().flatten().count();
    used + usize::from(used < t.mixer.sends.len())
}

/// How a send level reads: whole dB in a strip, tenths in the device chain.
fn level_text(db: f32, wide: bool) -> String {
    if db <= -144.0 {
        "-inf".into()
    } else if wide {
        format!("{db:.1} dB")
    } else if db.abs() < 0.5 {
        "0".into()
    } else {
        format!("{db:.0}")
    }
}

/// One send row: the target (click: send it somewhere else; right-click: pre/post and remove),
/// the level (drag like a fader, one undo step; double-click: 0 dB) and remove. `wide` (the
/// device chain) adds a Pre/Post toggle; a strip marks a pre-fader send with a "P".
#[allow(clippy::too_many_arguments)]
pub(crate) fn send_row(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    slot: usize,
    snd: &SendSlot,
    r: Rect,
    clip: Rect,
    wide: bool,
    menu: &mut Vec<MenuItem>,
) -> SendRow {
    let id = t.id;
    let salt = (id.0, slot, wide);
    // Under Read automation the row shows the lanes' values at the playhead.
    let mut shown = snd.clone();
    if let Ok(i) = u8::try_from(slot) {
        shown.level_db = app.followed(t, &crate::automation::Lane::SendLevel(i)).unwrap_or(snd.level_db);
        shown.pan = app.followed(t, &crate::automation::Lane::SendPan(i)).unwrap_or(snd.pan);
    }
    let snd = &shown;
    let p = ui.painter().with_clip_rect(clip);
    let target = match &snd.target {
        Route::Bus(b) => bus_label(s, *b),
        other => route_label(s, other, false, 0),
    };
    let not_here = unresolved_output(app.engine.io_map(), &snd.target);
    let mut row = SendRow { slot, target: target.clone(), level_db: snd.level_db, pre_fader: snd.pre_fader, rect: r4(r), ..SendRow::default() };
    p.rect_filled(r, crate::theme::RADIUS_SM, TOKENS.bg);
    // The level as a thin line along the bottom, so the row reads as a small fader.
    let k = db_to_k(snd.level_db);
    p.rect_filled(
        Rect::from_min_size(pos2(r.min.x, r.max.y - 2.0), vec2(r.width() * k, 2.0)),
        0.0,
        if snd.mute { TOKENS.text_dim } else { TOKENS.accent },
    );
    let xb = Rect::from_min_max(pos2(r.max.x - 14.0, r.min.y), r.max);
    let lr = Rect::from_min_max(pos2(xb.min.x - if wide { 50.0 } else { 24.0 }, r.min.y), pos2(xb.min.x, r.max.y));
    // The bar along the bottom is its own control; the text hit rects stop above it.
    let above = |q: Rect| Rect::from_min_max(q.min, pos2(q.max.x, q.max.y - BAR_H));
    // Slice 7: the send's mute, left of the level, and in the device chain its pan.
    let mb = Rect::from_min_max(pos2(lr.min.x - 14.0, r.min.y + 1.0), pos2(lr.min.x - 1.0, r.max.y - BAR_H));
    let pr = Rect::from_min_max(
        pos2(mb.min.x - if wide { 34.0 } else { 0.0 }, r.min.y + 2.0),
        pos2(mb.min.x - if wide { 4.0 } else { 0.0 }, r.max.y - 3.0),
    );
    let pnr = Rect::from_min_max(pos2(pr.min.x - if wide { 32.0 } else { 0.0 }, r.min.y), pos2(pr.min.x - if wide { 2.0 } else { 0.0 }, r.max.y));
    let mut tr = Rect::from_min_max(r.min, pos2(pnr.min.x - 1.0, r.max.y));
    if !wide && snd.pre_fader {
        p.text(pos2(tr.min.x + 7.0, tr.center().y), Align2::CENTER_CENTER, "P", FontId::proportional(9.5), TOKENS.text_dim);
        tr.min.x += 12.0;
    }
    // Target: another bus, or right-click for pre/post and remove.
    let th = above(tr);
    let mut resp = ui.interact(th.intersect(clip), ui.id().with(("send_target", salt)), Sense::click());
    if not_here {
        resp = resp.on_hover_text(NOT_HERE);
    }
    if resp.hovered() {
        p.rect_filled(th, crate::theme::RADIUS_SM, TOKENS.button_hover);
    }
    let mut text_r = tr.shrink2(vec2(4.0, 0.0));
    if not_here {
        crate::icons::paint(&p, pos2(text_r.min.x + 5.0, tr.center().y), crate::icons::Icon::Warning, 10.0, TOKENS.record);
        text_r.min.x += 12.0;
    }
    p.with_clip_rect(text_r.intersect(clip)).text(text_r.left_center(), Align2::LEFT_CENTER, &target, FontId::proportional(10.5), TOKENS.text);
    row.target_rect = r4(th);
    let mut choices = send_targets(s, t, Some(slot), app.engine.io_map());
    if !choices.iter().any(|choice| {
        matches!(choice, RoutingChoice::Entry { label, value } if *label == target || matches!(&snd.target, Route::Hardware(h) if h.eq_ignore_ascii_case(value)))
    }) {
        choices.insert(0, RoutingChoice::Entry { label: target.clone(), value: String::new() });
    }
    push_unused_entry(s, &mut choices);
    if let Some(v) = choice_menu(&resp, &target, &choices, menu)
        && !v.is_empty()
        && v != target
    {
        if v == REMOVE_UNUSED {
            let _ = app.remove_unused_busses();
        } else {
            let _ = app.retarget_send(id, slot, &v);
        }
    }
    // Its own popup id: `Popup::context_menu` closes the popup on a primary click, which would
    // otherwise be the retarget menu's (both default to the response's id).
    egui::Popup::context_menu(&resp).id(resp.id.with("send_ctx")).show(|ui| {
        let label = if snd.pre_fader { "Post-fader" } else { "Pre-fader" };
        let b = ui.button(label);
        menu.push(MenuItem::at(ui, label.into(), "pre".into(), b.rect));
        if b.clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
            ui.close();
        }
        let b = ui.button("Remove Send");
        menu.push(MenuItem::at(ui, "Remove Send".into(), "remove".into(), b.rect));
        if b.clicked() {
            let _ = app.remove_send(id, slot);
            ui.close();
        }
        if let Ok(n) = u8::try_from(slot) {
            ui.separator();
            app.midi_learn_menu(ui, id, transit_model::Control::Send(n));
        }
    });
    if let Ok(n) = u8::try_from(slot) {
        crate::midi_learn::mark(app, &p, r, id, transit_model::Control::Send(n));
    }
    // Pre/Post (device chain).
    if wide {
        let resp = ui.interact(pr.intersect(clip), ui.id().with(("send_pre", salt)), Sense::click());
        p.rect_filled(pr, crate::theme::RADIUS_SM, if snd.pre_fader || resp.hovered() { TOKENS.button_hover } else { TOKENS.button });
        p.text(
            pr.center(),
            Align2::CENTER_CENTER,
            if snd.pre_fader { "Pre" } else { "Post" },
            FontId::proportional(10.0),
            if snd.pre_fader { TOKENS.text } else { TOKENS.text_dim },
        );
        if resp.on_hover_text("Pre-fader or post-fader send").clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
        }
        row.pre = r4(pr);
    }
    // Mute: one undo step (`mix.send_level {mute}`).
    let resp = ui.interact(mb.intersect(clip), ui.id().with(("send_mute", salt)), Sense::click());
    let fill = if snd.mute {
        TOKENS.mute
    } else if resp.hovered() {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(mb, crate::theme::RADIUS_SM, fill);
    p.text(mb.center(), Align2::CENTER_CENTER, "M", FontId::proportional(9.0), if snd.mute { theme::text_on(TOKENS.mute) } else { TOKENS.text_dim });
    if resp.on_hover_text(if snd.mute { "Unmute this send" } else { "Mute this send" }).clicked() {
        let _ = app.set_send_mute(id, slot, !snd.mute);
    }
    row.mute = r4(mb);
    row.muted = snd.mute;
    // Pan (device chain): drag, wheel, double-click to type, Alt-click to center. The mix pans a
    // mono track's send only (a stereo source keeps its sides).
    if wide {
        let pt = Target::SendPan(id, slot);
        let text = fine::pan_text(snd.pan);
        let hot = app.mixer.gesture.is_some_and(|g| g.target == pt);
        p.rect_filled(above(pnr).shrink2(vec2(0.0, 1.0)), crate::theme::RADIUS_SM, TOKENS.panel_raised);
        readout(
            app,
            ui,
            &p,
            clip,
            Readout {
                target: pt,
                salt: 1,
                draw: above(pnr),
                hit: above(pnr),
                text: text.clone(),
                edit_text: text.clone(),
                font: theme::mono(ui.ctx(), 10.0),
                color: if hot { TOKENS.accent } else { TOKENS.text },
                align: Align2::CENTER_CENTER,
                drag: true,
            },
        );
        row.pan = r4(above(pnr));
        row.pan_text = text;
    }
    // Level. The thin bar along the bottom is the control: drag up or right to raise it, like the
    // fader (fourth-root law); double-click, Alt-click, Ctrl-click and Delete reset it to 0 dB;
    // a mark shows 0 dB. The number is a readout: double-click types a level; drag and wheel work
    // on it too.
    let me = Target::Send(id, slot);
    let bar = Rect::from_min_max(pos2(r.min.x, r.max.y - BAR_H), r.max);
    let resp = ui.interact(bar.intersect(clip), ui.id().with(("send_level", salt)), Sense::click_and_drag());
    let zero_x = r.min.x + r.width() * db_to_k(0.0);
    p.line_segment([pos2(zero_x, bar.min.y), pos2(zero_x, r.max.y)], Stroke::new(1.0, TOKENS.text_dim));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    control(app, ui, &resp, me, 1.0, true, true);
    let _ = resp.on_hover_text(format!(
        "{target}: {} ({}). Drag to change, double-click for 0 dB, wheel for 0.1 dB",
        level_text(snd.level_db, true),
        if snd.pre_fader { "pre-fader" } else { "post-fader" }
    ));
    row.level_rect = r4(bar);
    let text = level_text(snd.level_db, wide);
    let hot = app.mixer.gesture.is_some_and(|g| g.target == me);
    readout(
        app,
        ui,
        &p,
        clip,
        Readout {
            target: me,
            salt: wide.into(),
            draw: lr,
            hit: above(lr),
            text: text.clone(),
            edit_text: fine::edit_text_db(snd.level_db),
            font: theme::mono(ui.ctx(), 10.0),
            color: if hot { TOKENS.accent } else { TOKENS.text },
            align: Align2::RIGHT_CENTER,
            drag: true,
        },
    );
    row.readout_rect = r4(lr);
    row.level_text = text;
    // Remove.
    let resp = ui.interact(xb.intersect(clip), ui.id().with(("send_remove", salt)), Sense::click());
    crate::icons::paint(&p, xb.center(), crate::icons::Icon::Remove, 11.0, if resp.hovered() { TOKENS.record } else { TOKENS.text_dim });
    if resp.on_hover_text("Remove send").clicked() {
        let _ = app.remove_send(id, slot);
    }
    row.remove = r4(xb);
    row
}

/// "+ Send": the busses a send from `t` can go to, then "New FX Channel…". Returns its rect.
pub(crate) fn add_send_row(app: &mut StudioApp, ui: &mut Ui, s: &Session, t: &Track, r: Rect, clip: Rect, menu: &mut Vec<MenuItem>) -> R4 {
    let resp = ui.interact(r.intersect(clip), ui.id().with(("add_send", t.id.0, r.min.x as i32)), Sense::click());
    let p = ui.painter().with_clip_rect(clip);
    if resp.hovered() {
        p.rect_filled(r, crate::theme::RADIUS_SM, TOKENS.button_hover);
    }
    crate::icons::paint_labeled(&p, r, true, crate::icons::Icon::Plus, 11.0, "Send", FontId::proportional(10.5), TOKENS.text_dim);
    let mut choices = send_targets(s, t, None, app.engine.io_map());
    choices.push(RoutingChoice::Entry { label: "New FX Channel…".into(), value: NEW_FX.into() });
    push_unused_entry(s, &mut choices);
    if let Some(v) = choice_menu(&resp, "", &choices, menu) {
        if v == REMOVE_UNUSED {
            let _ = app.remove_unused_busses();
        } else {
            let _ = app.add_send(t.id, &v);
        }
    }
    r4(r)
}

/// A strip's sends block: `rows` rows at `y`, the sends in slot order, then "+ Send". When the
/// sends don't all fit, the last row says how many more there are (the device chain lists all).
#[allow(clippy::too_many_arguments)]
pub(super) fn sends_block(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    x0: f32,
    w: f32,
    y: f32,
    rows: usize,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    if rows == 0 || !takes_sends(t.kind) {
        return;
    }
    let sends: Vec<(usize, SendSlot)> = t.mixer.sends.iter().enumerate().filter_map(|(i, x)| x.clone().map(|x| (i, x))).collect();
    let free = sends.len() < t.mixer.sends.len();
    let row_r = |i: usize| Rect::from_min_size(pos2(x0, y + i as f32 * SEND_ROW_H), vec2(w, SEND_ROW_H - 2.0));
    let fits = sends.len() + usize::from(free) <= rows;
    let shown = if fits { sends.len() } else { rows.saturating_sub(1) };
    for (i, (slot, snd)) in sends.iter().take(shown).enumerate() {
        let row = send_row(app, ui, s, t, *slot, snd, row_r(i), clip, false, menu);
        sl.sends.push(row);
    }
    if fits {
        if free {
            sl.add_send = add_send_row(app, ui, s, t, row_r(shown), clip, menu);
        }
    } else {
        let r = row_r(shown);
        let resp = ui.interact(r.intersect(clip), ui.id().with(("more_sends", t.id.0)), Sense::click());
        ui.painter().with_clip_rect(clip).text(
            pos2(r.min.x + 4.0, r.center().y),
            Align2::LEFT_CENTER,
            format!("{} more…", sends.len() - shown),
            FontId::proportional(10.5),
            TOKENS.text_dim,
        );
        if resp.on_hover_text("Show all sends in the device chain").clicked() {
            app.focus(t.id);
            app.ui.show_devices = true;
        }
    }
}
