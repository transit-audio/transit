//! A strip's inserts: the full-height section (up to three insert slots and "+ Insert" in a fixed
//! 112 px body), the short variant's `FX n` button that opens the same slots in an area, and the
//! MIDI OUT button of a MIDI strip's input row. An insert slot is 22 high on `surface.raised` with
//! a 1 px `line` and radius 4: the device name on the left (Sans 500 11, ellipsis) and the 12 px
//! power glyph on the right, `signal` while the insert is on and `text.faint` while bypassed (the
//! name then `text.muted`). A click on the glyph toggles bypass; a click on the name opens the
//! device; a right-click offers the editor, Retry, bypass and Remove.

use super::anatomy;
use super::channel::{CONTROL, SMALL_VALUE, text_in};
use super::*;
use crate::tokens::channel_strip as cs;

/// The rows a full-height inserts body holds: three slots and "+ Insert" (past three the slots
/// scroll, [`super::overflow`]).
pub(super) const INSERT_ROWS: usize = 4;
/// The shortest fader travel a strip keeps; below it the short variant shows.
pub(super) const MIN_FADER: f32 = 80.0;

/// A dashed 1 px `line.strong` outline around `r` (the add rows' empty look).
pub(super) fn dashed(p: &egui::Painter, r: Rect) {
    let stroke = Stroke::new(1.0, TOKENS.line_strong);
    let r = r.shrink(0.5);
    let (dash, gap) = (3.0, 2.0);
    for seg in
        [[r.left_top(), r.right_top()], [r.right_top(), r.right_bottom()], [r.right_bottom(), r.left_bottom()], [r.left_bottom(), r.left_top()]]
    {
        p.extend(egui::Shape::dashed_line(&seg, stroke, dash, gap));
    }
}

/// An add row ("+ Insert", "+ Send"): transparent, dashed `line.strong`, the label in Sans 500
/// 11 `text.faint` (`text` on `button.hover` while hovered).
pub(super) fn add_row(p: &egui::Painter, r: Rect, label: &str, hovered: bool) {
    if hovered {
        p.rect_filled(r, cs::INSERT_SLOT_RADIUS, TOKENS.button_hover);
    }
    dashed(p, r);
    text_in(p, &CONTROL, label, if hovered { TOKENS.text } else { TOKENS.text_faint }, r, Align2::CENTER_CENTER);
}

/// Lucide's `power` glyph (a 24 unit box: the stem from (12, 2) to (12, 12), the arc of radius 9
/// about (12, 13) from 45 degrees right of the top round to 45 degrees left of it), drawn `size`
/// square with a `stroke` point line: the insert slot's indicator at 12 with the README's 1.5,
/// where the rasterized icon would scale Lucide's stroke of 2 down to 1.
pub(super) fn power_glyph(p: &egui::Painter, center: egui::Pos2, size: f32, stroke: f32, color: Color32) {
    let k = size / 24.0;
    let at = |x: f32, y: f32| center + vec2((x - 12.0) * k, (y - 12.0) * k);
    let line = Stroke::new(stroke, color);
    p.line_segment([at(12.0, 2.0), at(12.0, 12.0)], line);
    let n = 32;
    let arc: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = (-45.0 + 270.0 * i as f32 / n as f32).to_radians();
            at(12.0 + 9.0 * a.cos(), 13.0 + 9.0 * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(arc, line));
}

/// One insert slot in `rr`.
#[allow(clippy::too_many_arguments)]
fn insert_slot(
    app: &mut StudioApp,
    ui: &mut Ui,
    p: &egui::Painter,
    clip: Rect,
    t: &Track,
    slot: usize,
    ins: &transit_model::Insert,
    rr: Rect,
    menu: &mut Vec<MenuItem>,
) -> InsertRow {
    let id = t.id;
    let name = app.plugin_name(&ins.plugin, false);
    let power = Rect::from_center_size(pos2(rr.max.x - 6.0 - cs::INDICATOR_SIZE / 2.0, rr.center().y), vec2(cs::INDICATOR_SIZE + 6.0, rr.height()));
    let name_r = Rect::from_min_max(rr.min, pos2(power.min.x, rr.max.y));
    let resp = ui.interact(name_r.intersect(clip), ui.id().with(("insert", id.0, slot)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripInsert);
    let glyph = ui.interact(power.intersect(clip), ui.id().with(("insert_power", id.0, slot)), Sense::click());
    let fill = if resp.hovered() || glyph.hovered() { TOKENS.button_hover } else { TOKENS.surface_raised };
    p.rect_filled(rr, cs::INSERT_SLOT_RADIUS, fill);
    p.rect_stroke(rr, cs::INSERT_SLOT_RADIUS, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    // While the plug-in is on its way: "Loading…", "Loading… (slow) 12 s", "Not responding"
    // with a Retry button before the glyph (issue #82).
    let load = app.player.as_ref().and_then(|pl| pl.slot_load(id, slot));
    let stuck = matches!(load, Some(transit_playback::SlotLoad::NotResponding { .. }));
    let last = app.last_load_time(&ins.plugin);
    let mut status = load.map(|l| crate::devices::load_text(l, last)).unwrap_or_default();
    // A loaded plug-in that does not answer (#213): the same marker as the device card.
    let hang = if load.is_none() { app.slot_hang(id, slot, &ins.plugin) } else { None };
    if hang.is_some() {
        status = crate::devices::UNRESPONSIVE.to_string();
    }
    // A plug-in that does not answer reads by its glyph (the circle-alert in the power glyph's
    // place, Strip details 13) and its tooltip; a load in progress still says so after the name.
    let shown = if status.is_empty() || hang.is_some() { name.clone() } else { format!("{name} ({status})") };
    let mut text_r = Rect::from_min_max(pos2(rr.min.x + 8.0, rr.min.y), pos2(power.min.x, rr.max.y));
    let mut retry = R4::default();
    if stuck {
        let rb = Rect::from_center_size(pos2(power.min.x - 8.0, rr.center().y), vec2(14.0, 14.0));
        text_r.max.x = rb.min.x - 2.0;
        crate::devices::retry_button(app, ui, p, clip, rb, ui.id().with(("insert_retry", id.0, slot)), id, slot, &name);
        retry = r4(rb);
    }
    // The glyph is the state: `signal` on, `text.faint` bypassed (by the user or the Low
    // Latency mode), the name dimmed to `text.muted` with it.
    let mode = app.mode_bypassed(ins.uid.get());
    let on = !(ins.bypass || mode);
    let (glyph_ink, name_ink) = if hang.is_some() {
        (TOKENS.meter_hot, TOKENS.text_muted)
    } else if on {
        (TOKENS.signal, TOKENS.text)
    } else {
        (TOKENS.text_faint, TOKENS.text_muted)
    };
    text_in(p, &CONTROL, &shown, name_ink, text_r, Align2::LEFT_CENTER);
    if hang.is_some() {
        crate::icons::paint(p, power.center(), crate::icons::Icon::Error, cs::INDICATOR_SIZE, glyph_ink);
        // The glyph names the trouble; a click on it opens the device card, as the name does.
        let tip = hang.map(|h| app.hang_tip(h, &name)).unwrap_or_default();
        if glyph.on_hover_text(tip).clicked() {
            open_card(app, id);
        }
    } else if glyph.on_hover_text(if ins.bypass { "Turn on" } else { "Bypass" }).clicked() {
        let _ = app.toggle_bypass(id, slot);
    }
    if hang.is_none() {
        power_glyph(p, power.center(), cs::INDICATOR_SIZE, cs::INDICATOR_STROKE, glyph_ink);
    }
    // A built-in's editor is our own window: it needs no player (the card does the same).
    let builtin = app.builtin_at((id, slot)).is_some();
    let enabled = builtin || app.player.as_ref().is_some_and(|pl| crate::devices::hosted(&ins.plugin) || pl.has_editor(id, slot));
    if resp.clicked() && hang.is_some() {
        // A plug-in that does not answer: its device card, hatched with the reason and Resume.
        open_card(app, id);
    } else if resp.clicked() && enabled {
        // Selecting an insert opens its editor, or raises the one already open (#173).
        app.show_editor((id, slot));
    }
    if let Some(tip) = app.mode_bypass_tip(ins.uid.get()) {
        crate::help::hover(ui, &resp, crate::help::Id::DeviceLowLatencyBypass);
        resp.clone().on_hover_text(tip);
    }
    let state = if ins.bypass { " (bypassed)" } else { "" };
    let why = hang.map_or(String::new(), |h| format!(" {}", app.hang_tip(h, &name)));
    let resp = resp.on_hover_text(format!("{name}{state}. Click for its window, right-click for more.{why}"));
    let bypass = ins.bypass;
    let editor_open = app.ui.plugin_windows.contains(&(id, slot));
    let retry_name = name.clone();
    crate::chrome::context_menu(&resp).id(resp.id.with("insert_ctx")).show(|ui| {
        let label = if editor_open { "Close editor" } else { "Open editor" };
        let b = ui.add_enabled(enabled, crate::chrome::MenuRow::new(label));
        menu.push(MenuItem::at(ui, label.into(), "editor".into(), b.rect));
        if b.clicked() {
            app.toggle_editor((id, slot));
            ui.close();
        }
        if stuck {
            let b = ui.add(crate::chrome::MenuRow::new("Retry")).on_hover_text(format!("Retry loading {retry_name}"));
            menu.push(MenuItem::at(ui, "Retry".into(), "retry".into(), b.rect));
            if b.clicked() {
                let _ = app.retry_plugin(id, slot);
                ui.close();
            }
        }
        let label = if bypass { "Turn On" } else { "Bypass" };
        let b = ui.add(crate::chrome::MenuRow::new(label));
        menu.push(MenuItem::at(ui, label.into(), "bypass".into(), b.rect));
        if b.clicked() {
            let _ = app.toggle_bypass(id, slot);
            ui.close();
        }
        let b = ui.add(crate::chrome::MenuRow::new("Remove"));
        menu.push(MenuItem::at(ui, "Remove".into(), "remove".into(), b.rect));
        if b.clicked() {
            let _ = app.remove_device(id, slot);
            ui.close();
        }
    });
    InsertRow {
        low_latency_bypass: mode,
        slot,
        name,
        bypass,
        rect: r4(rr),
        name_rect: r4(name_r),
        power: r4(power),
        on,
        glyph_color: glyph_ink.to_array(),
        name_color: name_ink.to_array(),
        status,
        retry,
        alert: hang.is_some(),
    }
}

/// Opens the device chain on `track`, where an unresponsive insert's card is hatched with the
/// reason and Resume.
pub(crate) fn open_card(app: &mut StudioApp, track: TrackId) {
    app.focus(track);
    app.ui.show_devices = true;
}

/// The names of the inserts on `t` that do not answer, in slot order.
fn hung_inserts(app: &mut StudioApp, t: &Track) -> Vec<String> {
    let hung: Vec<&str> = t
        .mixer
        .inserts
        .iter()
        .enumerate()
        .filter_map(|(slot, ins)| ins.as_ref().filter(|ins| app.slot_hang(t.id, slot, &ins.plugin).is_some()).map(|ins| ins.plugin.as_str()))
        .collect();
    hung.into_iter().map(|id| app.plugin_name(id, false)).collect()
}

/// The FX button's tooltip for the inserts that do not answer: "Room Reverb is not responding",
/// or the list ("Room Reverb and Bite are not responding").
pub(crate) fn not_responding_tip(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => format!("{one} is not responding"),
        [rest @ .., last] => format!("{} and {last} are not responding", rest.join(", ")),
    }
}

/// The 10 px circle-alert riding the FX button's top-right corner (the board: 4 past its right
/// and top edges), on a disc of the panel fill that rings it. Returns the glyph's rect.
fn paint_alert(p: &egui::Painter, button: Rect) -> Rect {
    let size = cs::UNRESPONSIVE_SIZE;
    let r = Rect::from_min_size(pos2(button.max.x + 4.0 - size, button.min.y - 4.0), vec2(size, size));
    p.circle_filled(r.center(), size / 2.0 + 1.0, TOKENS.surface_panel);
    crate::icons::paint(p, r.center(), crate::icons::Icon::Error, size, TOKENS.meter_hot);
    r
}

/// The occupied insert slots of a track, in slot order.
fn occupied(t: &Track) -> Vec<(usize, transit_model::Insert)> {
    t.mixer.inserts.iter().enumerate().filter_map(|(i, x)| x.clone().map(|x| (i, x))).collect()
}

/// The insert rows of a track from `row_r`, drawn and hit inside `clip`: one slot per occupied
/// insert.
#[allow(clippy::too_many_arguments)]
fn insert_rows(
    app: &mut StudioApp,
    ui: &mut Ui,
    t: &Track,
    list: &[(usize, transit_model::Insert)],
    row_r: &dyn Fn(usize) -> Rect,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let p = ui.painter().with_clip_rect(clip);
    for (i, (slot, ins)) in list.iter().enumerate() {
        let row = insert_slot(app, ui, &p, clip, t, *slot, ins, row_r(i), menu);
        sl.inserts.push(row);
    }
}

/// "+ Insert" in `rr` while a slot is free; `hidden`, an overflowing section's count after its
/// label. Returns the count's text and rect when it drew one.
#[allow(clippy::too_many_arguments)]
fn add_insert(
    app: &mut StudioApp,
    ui: &mut Ui,
    t: &Track,
    rr: Rect,
    clip: Rect,
    hidden: Option<usize>,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) -> Option<(String, R4)> {
    let id = t.id;
    let p = ui.painter().with_clip_rect(clip);
    let free = t.mixer.inserts.iter().any(Option::is_none);
    if !free {
        // Every slot in use: the pinned row keeps the count alone.
        return hidden.map(|n| super::overflow::paint_count(&p, rr, n));
    }
    let resp = ui.interact(rr.intersect(clip), ui.id().with(("add_insert", id.0)), Sense::click());
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripAddEffect);
    let count = match hidden {
        Some(n) => {
            add_row_left(&p, rr, "+ Insert", resp.hovered());
            Some(super::overflow::paint_count(&p, rr, n))
        }
        None => {
            add_row(&p, rr, "+ Insert", resp.hovered());
            None
        }
    };
    let resp = resp.on_hover_text("Add an effect to this track");
    let before = t.mixer.inserts.iter().flatten().count();
    crate::devices::add_device_menu(app, &resp, id, menu);
    if app.engine.session().track(id).is_some_and(|x| x.mixer.inserts.iter().flatten().count() > before) {
        app.focus(id);
    }
    sl.add_insert = r4(rr);
    count
}

/// An add row with its label at the left (6 px in), as an overflowing section's pinned row draws
/// it with the hidden count at the right (the Strip details board).
pub(super) fn add_row_left(p: &egui::Painter, r: Rect, label: &str, hovered: bool) {
    if hovered {
        p.rect_filled(r, cs::INSERT_SLOT_RADIUS, TOKENS.button_hover);
    }
    dashed(p, r);
    text_in(p, &CONTROL, label, if hovered { TOKENS.text } else { TOKENS.text_faint }, r.shrink2(vec2(6.0, 0.0)), Align2::LEFT_CENTER);
}

/// The full-height inserts section's slots in `body` (under its INSERTS label): three slots and
/// "+ Insert"; past three the slots scroll in the three slots' view and "+ Insert" stays pinned
/// under it with the hidden count (Overflow).
pub(super) fn inserts_block(app: &mut StudioApp, ui: &mut Ui, t: &Track, body: Rect, clip: Rect, sl: &mut StripLayout, menu: &mut Vec<MenuItem>) {
    let rows = anatomy::slots_in(body).min(INSERT_ROWS);
    let visible = rows.saturating_sub(1);
    let list = occupied(t);
    if list.len() <= visible {
        insert_rows(app, ui, t, &list, &|i| anatomy::slot(body, i), clip, sl, menu);
        let _ = add_insert(app, ui, t, anatomy::slot(body, list.len()), clip, None, sl, menu);
        return;
    }
    let view = Rect::from_min_max(anatomy::slot(body, 0).min, anatomy::slot(body, visible - 1).max);
    let ov = super::overflow::Overflow::begin(app, ui, t.id, super::overflow::Section::Inserts, view, clip, list.len(), visible);
    insert_rows(app, ui, t, &list, &|i| ov.row(i), ov.clip(), sl, menu);
    let mut layout = ov.finish(ui.painter());
    if let Some((text, at)) = add_insert(app, ui, t, anatomy::slot(body, visible), clip, Some(layout.hidden), sl, menu) {
        (layout.count_text, layout.count_rect) = (text, at);
    }
    sl.inserts_overflow = Some(layout);
}

/// A short count button (`FX n`, `Snd n`): `surface.raised`, 1 px `line`, radius 4, the label in
/// Sans 500 11 `text` and the count in Mono 500 10, `signal` when there is one and a `text.faint`
/// "+" when there is none. Returns its response and what it reads.
pub(super) fn count_button(ui: &mut Ui, p: &egui::Painter, clip: Rect, r: Rect, id: egui::Id, label: &str, count: usize) -> (egui::Response, String) {
    let resp = ui.interact(r.intersect(clip), id, Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let fill = if resp.hovered() || open { TOKENS.button_hover } else { TOKENS.surface_raised };
    p.rect_filled(r, cs::INSERT_SLOT_RADIUS, fill);
    p.rect_stroke(r, cs::INSERT_SLOT_RADIUS, Stroke::new(1.0, TOKENS.line), egui::StrokeKind::Inside);
    let (n, ink) = if count > 0 { (count.to_string(), TOKENS.signal) } else { ("+".to_string(), TOKENS.text_faint) };
    let lw = crate::widgets::role_size(p, &CONTROL, label).x;
    let nw = crate::widgets::role_size(p, &SMALL_VALUE, &n).x;
    let x = r.center().x - (lw + 5.0 + nw) / 2.0;
    crate::widgets::role_text(p, &CONTROL, label, TOKENS.text, Align2::LEFT_CENTER, pos2(x, r.center().y));
    crate::widgets::role_text(p, &SMALL_VALUE, &n, ink, Align2::LEFT_CENTER, pos2(x + lw + 5.0, r.center().y));
    (resp, format!("{label} {n}"))
}

/// The short variant's `FX n` button: it opens the track's insert slots and "+ Insert" in a list
/// under it, as the full-height section draws them (every insert, not just three).
#[allow(clippy::too_many_arguments)]
pub(super) fn fx_button(
    app: &mut StudioApp,
    ui: &mut Ui,
    _s: &Session,
    t: &Track,
    r: Rect,
    clip: Rect,
    sl: &mut StripLayout,
    menu: &mut Vec<MenuItem>,
) {
    let count = t.mixer.inserts.iter().flatten().count();
    let p = ui.painter().with_clip_rect(clip);
    let (resp, text) = count_button(ui, &p, clip, r, ui.id().with(("fx_button", t.id.0)), "FX", count);
    crate::help::hover(ui, &resp, crate::help::Id::MixerStripInsert);
    let hung = hung_inserts(app, t);
    let resp = if hung.is_empty() {
        resp.on_hover_text(format!("Inserts ({count}): click for the list"))
    } else {
        // One glyph however many do not answer; the tooltip names them (Strip details 13).
        let tip = not_responding_tip(&hung);
        sl.fx_alert = r4(paint_alert(&ui.painter().with_clip_rect(clip.expand(6.0)), r));
        sl.fx_alert_tip = tip.clone();
        resp.on_hover_text(tip)
    };
    sl.fx_button = r4(r);
    sl.fx_text = text;
    let free = count < t.mixer.inserts.len();
    let mut drawn = StripLayout::default();
    let list = super::short::list(app, ui, t.id, super::short::ShortList::Inserts, &resp, count + usize::from(free), |app, ui, rows, clip| {
        let list = occupied(t);
        insert_rows(app, ui, t, &list, &|i| rows[i], clip, &mut drawn, menu);
        if let Some(rr) = rows.get(list.len()) {
            let _ = add_insert(app, ui, t, *rr, clip, None, &mut drawn, menu);
        }
    });
    (sl.inserts, sl.add_insert) = (drawn.inserts, drawn.add_insert);
    sl.list_rect = list.map_or([0.0; 4], r4);
}

/// A MIDI strip's output destination button (#61) beside its input: "OUT" in Mono 500 10 on
/// `surface.raised` with a 1 px `line`; the option toggle's on look while the track sends to an
/// external port; the error glyph and a `meter.hot` edge while that port's device is missing.
pub(super) fn midi_output_button(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, t: &Track, r: Rect, sl: &mut StripLayout) {
    let view = crate::midi_output::OutputView::read(app.player.as_ref());
    let missing = view.missing(t.id, &t.midi_output);
    let resp = ui.interact(r.intersect(clip), ui.id().with(("midi-output", t.id.0)), Sense::click());
    let external = t.midi_output.is_external();
    let (fill, ink, edge) = if missing {
        (TOKENS.surface_raised, TOKENS.meter_hot, TOKENS.meter_hot)
    } else if external {
        (TOKENS.toggle_option_fill, TOKENS.signal, TOKENS.signal)
    } else if resp.hovered() {
        (TOKENS.button_hover, TOKENS.text_muted, TOKENS.line)
    } else {
        (TOKENS.surface_raised, TOKENS.text_muted, TOKENS.line)
    };
    p.rect_filled(r, crate::tokens::radius::CONTROL, fill);
    p.rect_stroke(r, crate::tokens::radius::CONTROL, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    if missing {
        crate::icons::paint(p, r.center(), crate::icons::Icon::Error, 11.0, ink);
    } else {
        text_in(p, &SMALL_VALUE, "OUT", ink, r, Align2::CENTER_CENTER);
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
