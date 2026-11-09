use super::*;

/// Height of one send row, and of the strip without its sends block and fader.
pub const SEND_ROW_H: f32 = 17.0;
/// The send level bar along a row's bottom edge.
const BAR_H: f32 = 5.0;

/// How a send level reads: whole dB in a strip, tenths in the device chain; the typographic minus
/// ([`fine::minus`]).
fn level_text(db: f32, wide: bool) -> String {
    fine::minus(if db <= -144.0 {
        "-inf".into()
    } else if wide {
        format!("{db:.1} dB")
    } else if db.abs() < 0.5 {
        "0".into()
    } else {
        format!("{db:.0}")
    })
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
    if !wide {
        return strip_send_row(app, ui, s, t, slot, snd, r, clip, menu);
    }
    let p = ui.painter().with_clip_rect(clip);
    // The target by name: the FX channel listening to the bus, else the bus itself, never with a
    // "Bus:" prefix (the board's rows read "Reverb", "Delay").
    let target = match &snd.target {
        Route::Bus(b) => super::routing::bus_name(s, *b),
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
        crate::icons::paint(&p, pos2(text_r.min.x + 5.0, tr.center().y), crate::icons::Icon::Error, 10.0, TOKENS.record);
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
    crate::chrome::context_menu(&resp, true).id(resp.id.with("send_ctx")).show(|ui| {
        let label = if snd.pre_fader { "Post-fader" } else { "Pre-fader" };
        let b = ui.add(crate::chrome::MenuRow::new(label));
        menu.push(MenuItem::at(ui, label.into(), "pre".into(), b.rect));
        if b.clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
            ui.close();
        }
        let b = ui.add(crate::chrome::MenuRow::new("Remove Send"));
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
                click_to_type: false,
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
            click_to_type: false,
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

/// The retarget menu (a click on the target) and the row's right-click menu (pre/post, mute in
/// a strip, remove, MIDI Learn) of a send whose target reads `target`.
#[allow(clippy::too_many_arguments)]
fn send_menus(
    app: &mut StudioApp,
    s: &Session,
    t: &Track,
    slot: usize,
    snd: &SendSlot,
    resp: &egui::Response,
    target: &str,
    with_mute: bool,
    menu: &mut Vec<MenuItem>,
) {
    let id = t.id;
    let mut choices = send_targets(s, t, Some(slot), app.engine.io_map());
    if !choices.iter().any(|choice| {
        matches!(choice, RoutingChoice::Entry { label, value } if *label == target || matches!(&snd.target, Route::Hardware(h) if h.eq_ignore_ascii_case(value)))
    }) {
        choices.insert(0, RoutingChoice::Entry { label: target.to_string(), value: String::new() });
    }
    push_unused_entry(s, &mut choices);
    if let Some(v) = choice_menu(resp, target, &choices, menu)
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
    crate::chrome::context_menu(resp, true).id(resp.id.with("send_ctx")).show(|ui| {
        let label = if snd.pre_fader { "Post-fader" } else { "Pre-fader" };
        let b = ui.add(crate::chrome::MenuRow::new(label));
        menu.push(MenuItem::at(ui, label.into(), "pre".into(), b.rect));
        if b.clicked() {
            let _ = app.set_send_pre(id, slot, !snd.pre_fader);
            ui.close();
        }
        if with_mute {
            let label = if snd.mute { "Unmute Send" } else { "Mute Send" };
            let b = ui.add(crate::chrome::MenuRow::new(label));
            menu.push(MenuItem::at(ui, label.into(), "mute".into(), b.rect));
            if b.clicked() {
                let _ = app.set_send_mute(id, slot, !snd.mute);
                ui.close();
            }
        }
        let b = ui.add(crate::chrome::MenuRow::new("Remove Send"));
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
}

/// The strip's send row (Channel strip, sends): 22 high on `bg.titlebar` with a 1 px `line` and
/// radius 4; the target in Sans 500 11 (a click sends it elsewhere; right-click: pre/post, mute,
/// remove), a 28 px level bar on the fader's taper (4 px `line` track, 6 px `signal` fill,
/// `ink.ghost` while the send is muted) and the level in Mono 500 10 `text.muted`. The bar and the
/// number drag the level (one undo step); a double-click resets it (the bar) or types it (the
/// number). A pre-fader send leads with a faint "P".
#[allow(clippy::too_many_arguments)]
fn strip_send_row(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    slot: usize,
    snd: &SendSlot,
    r: Rect,
    clip: Rect,
    menu: &mut Vec<MenuItem>,
) -> SendRow {
    use super::channel::{CONTROL, SMALL_VALUE, text_in};
    use crate::tokens::channel_strip as cs;
    let id = t.id;
    let salt = (id.0, slot, false);
    let p = ui.painter().with_clip_rect(clip);
    // The target by name: the FX channel listening to the bus, else the bus itself, never with a
    // "Bus:" prefix (the board's rows read "Reverb", "Delay").
    let target = match &snd.target {
        Route::Bus(b) => super::routing::bus_name(s, *b),
        other => route_label(s, other, false, 0),
    };
    let not_here = unresolved_output(app.engine.io_map(), &snd.target);
    let mut row = SendRow {
        slot,
        target: target.clone(),
        level_db: snd.level_db,
        pre_fader: snd.pre_fader,
        rect: r4(r),
        muted: snd.mute,
        ..SendRow::default()
    };
    p.rect_filled(r, cs::INSERT_SLOT_RADIUS, TOKENS.bg_titlebar);
    p.rect_stroke(r, cs::INSERT_SLOT_RADIUS, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    let value_r = Rect::from_min_max(pos2(r.max.x - 6.0 - 22.0, r.min.y), pos2(r.max.x - 6.0, r.max.y));
    let bar_r = Rect::from_min_max(pos2(value_r.min.x - 6.0 - 28.0, r.center().y - 3.0), pos2(value_r.min.x - 6.0, r.center().y + 3.0));
    let level_hit = Rect::from_min_max(pos2(bar_r.min.x - 3.0, r.min.y), pos2(value_r.min.x - 1.0, r.max.y));
    let mut name_r = Rect::from_min_max(pos2(r.min.x + 6.0, r.min.y), pos2(level_hit.min.x - 3.0, r.max.y));
    let target_hit = Rect::from_min_max(r.min, pos2(level_hit.min.x, r.max.y));
    // Target: another bus, or right-click for pre/post, mute and remove.
    let mut resp = ui.interact(target_hit.intersect(clip), ui.id().with(("send_target", salt)), Sense::click());
    if not_here {
        resp = resp.on_hover_text(NOT_HERE);
    }
    if resp.hovered() {
        p.rect_filled(target_hit.shrink(1.0), cs::INSERT_SLOT_RADIUS, TOKENS.button_hover);
    }
    if snd.pre_fader {
        let at = text_in(&p, &SMALL_VALUE, "P", TOKENS.text_faint, name_r, Align2::LEFT_CENTER);
        name_r.min.x = at.max.x + 4.0;
    }
    if not_here {
        crate::icons::paint(&p, pos2(name_r.min.x + 5.0, r.center().y), crate::icons::Icon::Error, 10.0, TOKENS.meter_hot);
        name_r.min.x += 13.0;
    }
    text_in(&p, &CONTROL, &target, if snd.mute { TOKENS.text_muted } else { TOKENS.text }, name_r, Align2::LEFT_CENTER);
    row.target_rect = r4(target_hit);
    send_menus(app, s, t, slot, snd, &resp, &target, true, menu);
    if let Ok(n) = u8::try_from(slot) {
        crate::midi_learn::mark(app, &p, r, id, transit_model::Control::Send(n));
    }
    // The level bar on the fader's taper.
    let track = Rect::from_min_max(pos2(bar_r.min.x, bar_r.min.y + 1.0), pos2(bar_r.max.x, bar_r.max.y - 1.0));
    p.rect_filled(track, crate::tokens::fader::TRACK_RADIUS, TOKENS.line);
    let k = super::controls::fader_k(snd.level_db);
    let fill = Rect::from_min_max(bar_r.min, pos2(bar_r.min.x + k * bar_r.width(), bar_r.max.y));
    if k > 0.0 {
        p.rect_filled(fill, crate::tokens::fader::TRACK_RADIUS, if snd.mute { TOKENS.ink_ghost } else { TOKENS.signal });
    }
    row.level_fill = r4(fill);
    let me = Target::Send(id, slot);
    let resp = ui.interact(level_hit.intersect(clip), ui.id().with(("send_level", salt)), Sense::click_and_drag());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    control(app, ui, &resp, me, 1.0, true, true);
    let _ = resp.on_hover_text(format!(
        "{target}: {} ({}{}). Drag to change, double-click for 0 dB, wheel for 0.1 dB",
        level_text(snd.level_db, true),
        if snd.pre_fader { "pre-fader" } else { "post-fader" },
        if snd.mute { ", muted" } else { "" }
    ));
    row.level_rect = r4(level_hit);
    let text = level_text(snd.level_db, false);
    let hot = app.mixer.gesture.is_some_and(|g| g.target == me);
    readout(
        app,
        ui,
        &p,
        clip,
        Readout {
            target: me,
            salt: 0,
            draw: value_r,
            hit: value_r,
            text: text.clone(),
            edit_text: fine::edit_text_db(snd.level_db),
            font: SMALL_VALUE.font_id(ui.ctx()),
            color: if hot { TOKENS.signal } else { TOKENS.text_muted },
            align: Align2::RIGHT_CENTER,
            drag: true,
            click_to_type: false,
        },
    );
    row.readout_rect = r4(value_r);
    row.level_text = text;
    row
}

/// The occupied send slots of a track, in slot order.
fn occupied(t: &Track) -> Vec<(usize, SendSlot)> {
    t.mixer.sends.iter().enumerate().filter_map(|(i, x)| x.clone().map(|x| (i, x))).collect()
}

/// The send rows of a track from `row_r`, drawn and hit inside `clip`.
#[allow(clippy::too_many_arguments)]
fn send_rows(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    sends: &[(usize, SendSlot)],
    row_r: &dyn Fn(usize) -> Rect,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    for (i, (slot, snd)) in sends.iter().enumerate() {
        let row = send_row(app, ui, s, t, *slot, snd, row_r(i), clip, false, menu);
        sl.sends.push(row);
    }
}

/// "+ Send" in `r` while a slot is free; `hidden`, an overflowing section's count after its label.
/// Returns the count's text and rect when it drew one.
#[allow(clippy::too_many_arguments)]
fn add_send(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    hidden: Option<usize>,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) -> Option<(String, R4)> {
    let p = ui.painter().with_clip_rect(clip);
    if !t.mixer.sends.iter().any(Option::is_none) {
        // Every slot in use: the pinned row keeps the count alone.
        return hidden.map(|n| super::overflow::paint_count(&p, r, n));
    }
    let resp = ui.interact(r.intersect(clip), ui.id().with(("add_send", t.id.0, r.min.x as i32)), Sense::click());
    let count = match hidden {
        Some(n) => {
            super::inserts::add_row_left(&p, r, "+ Send", resp.hovered());
            Some(super::overflow::paint_count(&p, r, n))
        }
        None => {
            super::inserts::add_row(&p, r, "+ Send", resp.hovered());
            None
        }
    };
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
    sl.add_send = r4(r);
    count
}

/// The full-height sends section's rows in `body` (under its SENDS label): two sends and "+ Send";
/// past two the rows scroll in the two rows' view and "+ Send" stays pinned under it with the
/// hidden count (Overflow).
#[allow(clippy::too_many_arguments)]
pub(super) fn sends_block(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    body: Rect,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let rows = super::anatomy::slots_in(body);
    if rows == 0 || !takes_sends(t.kind) {
        return;
    }
    let visible = rows - 1;
    let sends = occupied(t);
    if sends.len() <= visible {
        send_rows(app, ui, s, t, &sends, &|i| super::anatomy::slot(body, i), clip, sl, menu);
        let _ = add_send(app, ui, s, t, super::anatomy::slot(body, sends.len()), clip, None, sl, menu);
        return;
    }
    let view = Rect::from_min_max(super::anatomy::slot(body, 0).min, super::anatomy::slot(body, visible - 1).max);
    let ov = super::overflow::Overflow::begin(app, ui, t.id, super::overflow::Section::Sends, view, clip, sends.len(), visible);
    send_rows(app, ui, s, t, &sends, &|i| ov.row(i), ov.clip(), sl, menu);
    let mut layout = ov.finish(ui.painter());
    if let Some((text, at)) = add_send(app, ui, s, t, super::anatomy::slot(body, visible), clip, Some(layout.hidden), sl, menu) {
        (layout.count_text, layout.count_rect) = (text, at);
    }
    sl.sends_overflow = Some(layout);
}

/// The short variant's `Snd n` button: it opens the track's send rows and "+ Send" in a list under
/// it (every send). A strip that takes no sends keeps the button so every strip keeps its layout.
#[allow(clippy::too_many_arguments)]
pub(super) fn snd_button(
    app: &mut StudioApp,
    ui: &mut Ui,
    s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let count = t.mixer.sends.iter().flatten().count();
    let p = ui.painter().with_clip_rect(clip);
    let (resp, text) = super::inserts::count_button(ui, &p, clip, r, ui.id().with(("snd_button", t.id.0)), "Snd", count);
    sl.snd_button = r4(r);
    sl.snd_text = text;
    if !takes_sends(t.kind) {
        return;
    }
    let resp = resp.on_hover_text(format!("Sends ({count}): click for the list"));
    let free = count < t.mixer.sends.len();
    let mut drawn = StripLayout::default();
    let list = super::short::list(app, ui, t.id, super::short::ShortList::Sends, &resp, count + usize::from(free), |app, ui, rows, clip| {
        let sends = occupied(t);
        send_rows(app, ui, s, t, &sends, &|i| rows[i], clip, &mut drawn, menu);
        if let Some(r) = rows.get(sends.len()) {
            let _ = add_send(app, ui, s, t, *r, clip, None, &mut drawn, menu);
        }
    });
    (sl.sends, sl.add_send) = (drawn.sends, drawn.add_send);
    sl.list_rect = list.map_or(sl.list_rect, r4);
}
