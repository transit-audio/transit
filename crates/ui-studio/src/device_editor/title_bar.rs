//! The device title bar (README, Device anatomy A; the board's header): 52 px on `bg.titlebar`
//! with a 1 px `line` below. Left: the brand, a divider, the track and the device. Centered in
//! the free space between the two side groups: the preset browser (previous, the name field,
//! next, save) over the existing preset system. Right: the output meter and its readout, the
//! follow-selection toggle, bypass, the device menu, close.
//!
//! **Reflow** (326-X, [`preset_layout`]): the side groups keep their places; the field shrinks
//! from 300 toward 160 before anything moves; below that the device title truncates with an
//! ellipsis, never a control.
//!
//! The title bar's buttons are the board's: 32 square, `surface.panel` with a 1 px `line` and
//! radius 6, a 14 px glyph in `text.muted`; close is borderless. The bypass glyph keeps the insert
//! slot's meaning: `signal` while the device plays, `text.faint` while bypassed, and ghosted
//! (`ink.arrow`, no click) for an instrument, which has no bypass (plan 06, L17).

use super::Dev;
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::macro_osc_panel::MacroLayout;
use crate::presets;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, level_meter, r4, toggle};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The bar's buttons and the preset field.
pub const BUTTON: f32 = space::CONTROL_HEIGHT[1];
const BUTTON_RADIUS: f32 = radius::DISPLAY;
const GLYPH: f32 = tokens::chrome::ICON_GLYPH;
pub const FIELD_W: f32 = 300.0;
/// The narrowest the preset field gets before the device title gives way (326-X).
pub const FIELD_MIN: f32 = 160.0;
/// The output meter: a 4 px bar 140 wide, and its readout's slot.
pub const METER_W: f32 = 140.0;
const METER_H: f32 = 4.0;
const READOUT_W: f32 = 64.0;
const GAP: f32 = tokens::SPACE[2];

/// The title bar as last drawn.
#[derive(Debug, Clone, Default)]
pub struct TitleLayout {
    pub uid: u64,
    pub bar: [f32; 4],
    pub track: String,
    pub device: String,
    /// The left group: the brand to the track's "/" (326-X), and the device title as drawn.
    pub track_rect: [f32; 4],
    pub device_rect: [f32; 4],
    pub prev: [f32; 4],
    pub field: [f32; 4],
    /// The field's text: the list the preset is in ("Factory", "User"; empty for neither) and its name.
    pub preset_group: String,
    pub preset_name: String,
    pub next: [f32; 4],
    pub save: [f32; 4],
    pub meter: [f32; 4],
    pub meter_fill: [f32; 4],
    pub readout: String,
    pub readout_rect: [f32; 4],
    pub follow: [f32; 4],
    pub follow_on: bool,
    pub bypass: [f32; 4],
    pub bypass_available: bool,
    pub bypass_on: bool,
    pub menu: [f32; 4],
    pub close: [f32; 4],
    #[doc(hidden)]
    pub follow_fill: Color32,
}

/// What the bar asks of its window.
#[derive(Debug, Default)]
pub struct TitleOut {
    pub close: bool,
    pub toggle_follow: bool,
    pub close_all: bool,
    /// The bar dragged by this much (the window moves with it).
    pub drag: egui::Vec2,
}

/// The presets in browser order: the factory list, then the user's.
pub(crate) fn preset_names(app: &StudioApp, plugin: &str) -> Vec<(bool, String)> {
    let mut v: Vec<(bool, String)> = presets::factory_for(plugin).into_iter().map(|(n, _)| (true, n.to_string())).collect();
    if let Some(root) = app.services.config_dir.as_deref() {
        v.extend(presets::user_names(root, plugin).into_iter().map(|n| (false, n)));
    }
    v
}

/// The preset `step` places from the current one (wrapping); from no preset, next is the first
/// and previous the last.
pub fn stepped(names: &[(bool, String)], current: &str, step: isize) -> Option<usize> {
    if names.is_empty() {
        return None;
    }
    let n = names.len() as isize;
    Some(match names.iter().position(|(_, x)| x == current) {
        Some(i) => (i as isize + step).rem_euclid(n) as usize,
        None if step > 0 => 0,
        None => (n - 1) as usize,
    })
}

/// Recalls preset `at` of `names` on the device.
fn recall(app: &mut StudioApp, dev: Dev, names: &[(bool, String)], at: usize) {
    let Some((factory, name)) = names.get(at) else { return };
    let p = if *factory {
        presets::factory_for(dev.info.id).into_iter().find(|(n, _)| n == name).map(|(_, p)| Ok(p)).unwrap_or_else(|| Err("not found".into()))
    } else {
        app.services.config_dir.as_deref().map_or(Err("no config folder".into()), |r| presets::load_user(r, dev.info.id, name))
    };
    match p {
        Ok(p) => app.apply_preset(dev.key, dev.info, name, &p),
        Err(e) => app.status = format!("Preset: {e}"),
    }
}

/// A title bar button at `r`: hovered `button.hover`, else `surface.panel`, 1 px `line`.
fn button(ui: &mut Ui, id: egui::Id, r: Rect, enabled: bool) -> egui::Response {
    let t = &TOKENS;
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    let fill = if enabled && resp.hovered() { t.button_hover } else { t.surface_panel };
    ui.painter().rect(r, BUTTON_RADIUS, fill, Stroke::new(1.0, t.line), StrokeKind::Inside);
    resp
}

/// Points of the board's 16-unit glyphs scaled into a `GLYPH` box at `c`.
fn glyph_pts(c: Pos2, pts: &[[f32; 2]]) -> Vec<Pos2> {
    let s = GLYPH / 16.0;
    pts.iter().map(|[x, y]| c + vec2((x - 8.0) * s, (y - 8.0) * s)).collect()
}

fn stroke_glyph(p: &egui::Painter, c: Pos2, paths: &[&[[f32; 2]]], color: Color32) {
    for path in paths {
        p.add(egui::Shape::line(glyph_pts(c, path), Stroke::new(1.6, color)));
    }
}

/// The board's previous and next chevrons, save, the device menu's dots, and the link of the
/// follow toggle (not on the board: two interlocked links at 45 degrees).
fn paint_glyph(p: &egui::Painter, c: Pos2, which: &str, color: Color32) {
    match which {
        "prev" => stroke_glyph(p, c, &[&[[10.0, 3.0], [5.0, 8.0], [10.0, 13.0]]], color),
        "next" => stroke_glyph(p, c, &[&[[6.0, 3.0], [11.0, 8.0], [6.0, 13.0]]], color),
        "save" => stroke_glyph(
            p,
            c,
            &[
                &[[3.0, 2.5], [11.0, 2.5], [13.5, 5.0], [13.5, 13.5], [2.5, 13.5], [2.5, 2.5], [3.0, 2.5]],
                &[[5.0, 2.5], [5.0, 6.0], [10.0, 6.0], [10.0, 2.5]],
                &[[5.0, 13.5], [5.0, 9.5], [11.0, 9.5], [11.0, 13.5]],
            ],
            color,
        ),
        "menu" => {
            for x in [3.5, 8.0, 12.5] {
                p.circle_filled(glyph_pts(c, &[[x, 8.0]])[0], 1.3 * GLYPH / 16.0, color);
            }
        }
        _ => {
            // Two stadiums along the diagonal, overlapping in the middle.
            let s = GLYPH / 16.0;
            for off in [-2.2_f32, 2.2] {
                let center = c + vec2(off, -off) * s;
                let (along, across) = (vec2(1.0, -1.0).normalized(), vec2(1.0, 1.0).normalized());
                let (len, r) = (3.2 * s, 2.4 * s);
                let mut pts = Vec::new();
                for i in 0..=12 {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    pts.push(center + along * len + (across * a.cos() + along * a.sin()) * r);
                }
                for i in 0..=12 {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    pts.push(center - along * len - (across * a.cos() + along * a.sin()) * r);
                }
                pts.push(pts[0]);
                p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
            }
        }
    }
}

/// The newest peak of the tap's frames in dB (the last 1024 frames, about 21 ms at 48 kHz; the
/// fader's floor for silence).
pub fn newest_peak_db(frames: &[f32]) -> f32 {
    let peak = frames.iter().rev().take(1024).fold(0.0f32, |m, v| m.max(*v));
    if peak > 0.0 { (20.0 * peak.log10()).max(crate::widgets::fader::FLOOR_DB) } else { crate::widgets::fader::FLOOR_DB }
}

/// The meter bar's fill at `level_db` as fractions of its width: (to the level or 0 dBFS, the
/// part above 0 dBFS). The bar follows the fader's taper, as every meter does.
pub fn meter_fractions(level_db: f32) -> (f32, f32) {
    let frac = 1.0 - crate::widgets::fader::taper(level_db);
    let zero = 1.0 - crate::widgets::fader::taper(0.0);
    (frac.min(zero), (frac - zero).max(0.0))
}

/// The readout beside the meter: `-inf dB` at the floor.
pub fn readout(level_db: f32) -> String {
    if level_db <= crate::widgets::fader::FLOOR_DB { "-inf dB".to_string() } else { format!("{level_db:.1} dB") }
}

/// The output meter's displayed level, stepped with peak fall-off from the tap's newest peak.
fn meter_level(app: &mut StudioApp, ui: &Ui, dev: Dev) -> f32 {
    let uid = crate::scope_strip::hub_uid(dev.key, dev.ins);
    let hub = app.scope_hub();
    let mut frames = std::mem::take(&mut app.builtin_editor.frames);
    frames.resize(transit_mix::SCOPE_FRAMES, 0.0);
    if let Some(h) = hub.as_ref() {
        h.frames(dev.key.0, uid, &mut frames);
    }
    let target = newest_peak_db(&frames);
    app.builtin_editor.frames = frames;
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
    let prev = app.builtin_editor.macro_osc.meters.get(&dev.uid()).copied().unwrap_or(crate::widgets::fader::FLOOR_DB);
    let shown = level_meter::meter_step(prev, target, dt, level_meter::Ballistics::PEAK_FALLOFF);
    app.builtin_editor.macro_osc.meters.insert(dev.uid(), shown);
    shown
}

/// The preset browser's width with a field `field` wide: previous, the field, next, save.
pub fn preset_width(field: f32) -> f32 {
    BUTTON + 4.0 + field + 4.0 + BUTTON + 8.0 + BUTTON
}

/// Where the preset browser goes between the left group (ending at `left_end` with the device
/// title at full width) and the right group (starting at `right_start`): its left edge, the
/// field's width, and the widest the device title may draw from `device_x` (`None`: in full).
/// The browser centers in the free space with the widest field up to [`FIELD_W`] that fits;
/// below [`FIELD_MIN`] it keeps the floor against the right group and the device title gives way.
pub fn preset_layout(left_end: f32, right_start: f32, device_x: f32) -> (f32, f32, Option<f32>) {
    let (from, to) = (left_end + GAP, right_start - GAP);
    let field = (to - from - preset_width(0.0)).clamp(FIELD_MIN, FIELD_W);
    let w = preset_width(field);
    if to - from >= w {
        (from + (to - from - w) / 2.0, field, None)
    } else {
        let x = to - w;
        (x, field, Some((x - GAP - device_x).max(0.0)))
    }
}

/// `text` in `role` cut to `max_w` with an ellipsis (the names' rule: insert slots, pads).
fn fitted(painter: &egui::Painter, role: &tokens::TypeRole, text: &str, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = role.job(painter.ctx(), text, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// Draws the bar into `rect` (52 high) and records it.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, follow: bool, lay: &mut EditorLayout, out: &mut MacroLayout) -> TitleOut {
    let t = &TOKENS;
    let mut res = TitleOut::default();
    let uid = dev.uid();
    let p = ui.painter().clone();
    p.rect_filled(rect, crate::theme::top_corners_of(radius::PANEL), t.bg_titlebar);
    p.line_segment([pos2(rect.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)], Stroke::new(1.0, t.line));
    // Dragging the bar moves the window (its controls take their own clicks).
    let drag = ui.interact(rect, egui::Id::new(("macro_osc_title", uid)), Sense::drag());
    if drag.dragged() {
        res.drag = drag.drag_delta();
    }
    let cy = rect.center().y;
    let id = |s: &str| egui::Id::new(("macro_osc_title", uid, s.to_string()));
    let mut tl = TitleLayout { uid, bar: r4(rect), ..TitleLayout::default() };

    // Left: brand, divider, track / device.
    let mut x = rect.min.x + space::WINDOW_PADDING;
    let r = widgets::role_text(&p, &tokens::TITLE_BRAND, "transit", t.text, Align2::LEFT_CENTER, pos2(x, cy));
    x = r.max.x + GAP;
    p.line_segment([pos2(x, cy - 9.0), pos2(x, cy + 9.0)], Stroke::new(1.0, t.line_strong));
    x += 1.0 + GAP;
    tl.track = app.engine.session().track(dev.key.0).map(|tr| tr.name.clone()).unwrap_or_default();
    let r = widgets::role_text(&p, &tokens::LABEL_CONTROL, &tl.track, t.text_faint, Align2::LEFT_CENTER, pos2(x, cy));
    let r = widgets::role_text(&p, &tokens::LABEL_CONTROL, "/", t.text_faint, Align2::LEFT_CENTER, pos2(r.max.x + 8.0, cy));
    tl.device = dev.info.name.to_string();
    let device_x = r.max.x + 8.0;
    let left_end = device_x + widgets::role_size(&p, &tokens::TITLE_DEVICE, &tl.device).x;
    let right_start = rect.max.x - space::WINDOW_PADDING - 4.0 * BUTTON - 4.0 * GAP - READOUT_W - GAP - METER_W;
    let (px, field_w, device_max) = preset_layout(left_end, right_start, device_x);
    let galley = fitted(&p, &tokens::TITLE_DEVICE, &tl.device, t.text, device_max.unwrap_or(f32::INFINITY));
    let device_r = Align2::LEFT_CENTER.anchor_size(pos2(device_x, cy), galley.size());
    p.galley(device_r.min, galley, t.text);
    tl.device_rect = r4(device_r);
    tl.track_rect = r4(Rect::from_min_max(pos2(rect.min.x + space::WINDOW_PADDING, r.min.y), r.max));

    // Centered in the free space: previous, the field, next, save.
    let mut x = px;
    let sq = |x: f32| Rect::from_min_size(pos2(x, cy - BUTTON / 2.0), vec2(BUTTON, BUTTON));
    let names = preset_names(app, dev.info.id);
    let prev = sq(x);
    if button(ui, id("prev"), prev, !names.is_empty()).clicked()
        && let Some(i) = stepped(&names, &dev.ins.preset, -1)
    {
        recall(app, dev, &names, i);
    }
    paint_glyph(&p, prev.center(), "prev", t.text_muted);
    x += BUTTON + 4.0;
    let field = Rect::from_min_size(pos2(x, cy - BUTTON / 2.0), vec2(field_w, BUTTON));
    field_ui(app, ui, dev, field, &names, &mut tl);
    lay.preset = r4(field);
    x += field_w + 4.0;
    let next = sq(x);
    if button(ui, id("next"), next, !names.is_empty()).clicked()
        && let Some(i) = stepped(&names, &dev.ins.preset, 1)
    {
        recall(app, dev, &names, i);
    }
    paint_glyph(&p, next.center(), "next", t.text_muted);
    x += BUTTON + 8.0;
    let save = sq(x);
    if button(ui, id("save"), save, app.services.config_dir.is_some()).clicked() {
        save_current(app, ui, dev, &names);
    }
    paint_glyph(&p, save.center(), "save", t.text_muted);
    (tl.prev, tl.field, tl.next, tl.save) = (r4(prev), r4(field), r4(next), r4(save));

    // Right, from the edge in: close, menu, bypass, follow, readout, meter.
    let mut x = rect.max.x - space::WINDOW_PADDING - BUTTON;
    let close = sq(x);
    let resp = ui.interact(close, id("close"), Sense::click());
    if resp.hovered() {
        p.rect_filled(close, BUTTON_RADIUS, t.button_hover);
    }
    crate::icons::paint(&p, close.center(), crate::icons::Icon::Remove, GLYPH + 4.0, t.text_muted);
    res.close = resp.clicked();
    x -= GAP + BUTTON;
    let menu = sq(x);
    let mresp = button(ui, id("menu"), menu, true);
    paint_glyph(&p, menu.center(), "menu", t.text_muted);
    egui::Popup::menu(&mresp).show(|ui| {
        crate::chrome::menu_style(ui.style_mut());
        if crate::menus::check_button(ui, follow, "Follow Selection").clicked() {
            res.toggle_follow = true;
        }
        if crate::menus::check_button(ui, false, "Close All Editors").clicked() {
            res.close_all = true;
        }
    });
    x -= GAP + BUTTON;
    let bypass = sq(x);
    let insert = dev.key.1 != transit_mix::INSTRUMENT_SLOT;
    let bresp = button(ui, id("bypass"), bypass, insert);
    if bresp.clicked() {
        let _ = app.toggle_bypass(dev.key.0, dev.key.1);
    }
    let glyph = if !insert {
        t.ink_arrow
    } else if dev.ins.bypass {
        t.text_faint
    } else {
        t.signal
    };
    crate::icons::paint(&p, bypass.center(), crate::icons::Icon::Power, GLYPH + 4.0, glyph);
    (tl.bypass, tl.bypass_available, tl.bypass_on) = (r4(bypass), insert, insert && !dev.ins.bypass);
    x -= GAP + BUTTON;
    let link = sq(x);
    let fresp = ui.interact(link, id("follow"), Sense::click());
    let (fill, ink, border) = toggle::colors(toggle::Role::Option, follow, true);
    p.rect(link, BUTTON_RADIUS, if !follow && fresp.hovered() { t.button_hover } else { fill }, Stroke::new(1.0, border), StrokeKind::Inside);
    paint_glyph(&p, link.center(), "link", ink);
    fresp.clone().on_hover_text("Follow Selection: show the device of the focused track");
    res.toggle_follow |= fresp.clicked();
    (tl.follow, tl.follow_on, tl.follow_fill) = (r4(link), follow, fill);
    x -= GAP + READOUT_W;
    let level = meter_level(app, ui, dev);
    tl.readout = readout(level);
    let rr = widgets::role_text(&p, &tokens::VALUE, &tl.readout, t.text_muted, Align2::LEFT_CENTER, pos2(x, cy));
    tl.readout_rect = r4(rr);
    x -= GAP + METER_W;
    let meter = Rect::from_min_size(pos2(x, cy - METER_H / 2.0), vec2(METER_W, METER_H));
    p.rect_filled(meter, tokens::fader::TRACK_RADIUS, t.line);
    let (ok, over) = meter_fractions(level);
    let fill = Rect::from_min_size(meter.min, vec2(METER_W * ok, METER_H));
    if fill.width() > 0.0 {
        p.rect_filled(fill, tokens::fader::TRACK_RADIUS, t.meter_ok);
    }
    if over > 0.0 {
        let hot = Rect::from_min_size(pos2(fill.max.x, meter.min.y), vec2(METER_W * over, METER_H));
        p.rect_filled(hot, 0.0, t.meter_hot);
    }
    (tl.meter, tl.meter_fill, tl.menu, tl.close) = (r4(meter), r4(fill), r4(menu), r4(close));
    out.title = tl;
    res
}

/// The preset name field: `surface.inset`, the list's name in `text.faint` before the preset's,
/// the chevron; a click opens the preset list (the dropdown's: factory, user, Save As, Delete).
fn field_ui(app: &mut StudioApp, ui: &mut Ui, dev: Dev, field: Rect, names: &[(bool, String)], tl: &mut TitleLayout) {
    let t = &TOKENS;
    let resp = ui.interact(field, egui::Id::new(("macro_osc_preset", dev.uid())), Sense::click());
    let p = ui.painter().clone();
    p.rect(field, BUTTON_RADIUS, t.surface_inset, Stroke::new(1.0, if resp.hovered() { t.line_strong } else { t.line }), StrokeKind::Inside);
    let current = if dev.ins.preset.is_empty() { "Preset" } else { dev.ins.preset.as_str() };
    let group = match names.iter().find(|(_, n)| n == current) {
        Some((true, _)) => "Factory",
        Some((false, _)) => "User",
        None => "",
    };
    let mut x = field.min.x + 12.0;
    if !group.is_empty() {
        let r = widgets::role_text(
            &p,
            &tokens::chrome::BUTTON_FONT,
            &format!("{group} · "),
            t.text_faint,
            Align2::LEFT_CENTER,
            pos2(x, field.center().y),
        );
        x = r.max.x;
    }
    widgets::role_text(&p, &tokens::chrome::BUTTON_FONT, current, t.text, Align2::LEFT_CENTER, pos2(x, field.center().y));
    super::chevron(&p, pos2(field.max.x - 16.0, field.center().y), 9.0, t.text_faint);
    (tl.preset_group, tl.preset_name) = (group.to_string(), current.to_string());
    let root = app.services.config_dir.clone();
    let user = root.as_deref().map(|r| presets::user_names(r, dev.info.id)).unwrap_or_default();
    let name_id = egui::Id::new(("macro_osc_preset_name", dev.uid()));
    let mut name: String = ui.data_mut(|d| d.get_temp(name_id)).unwrap_or_default();
    let mut picked = presets::Picked::default();
    crate::chrome_pick::dropdown_with_fields(&resp)
        .width(FIELD_W)
        .show(|ui| picked = presets::list(ui, dev.ins, dev.info, root.as_deref(), &user, &mut name));
    ui.data_mut(|d| d.insert_temp(name_id, name));
    presets::act(app, ui, dev.key, dev.ins, dev.info, picked, name_id);
}

/// Save: a user preset is written over with the current values (what Save As with its own name
/// did); a factory preset or none opens the list, where Save As names a new one.
fn save_current(app: &mut StudioApp, ui: &Ui, dev: Dev, names: &[(bool, String)]) {
    let user = names.iter().any(|(factory, n)| !factory && *n == dev.ins.preset);
    if user {
        let picked = presets::Picked { save: Some(dev.ins.preset.clone()), ..presets::Picked::default() };
        presets::act(app, ui, dev.key, dev.ins, dev.info, picked, egui::Id::new(("macro_osc_preset_name", dev.uid())));
    } else {
        // The field's popup id (`Popup::default_response_id` of the field's response).
        egui::Popup::open_id(ui.ctx(), egui::Id::new(("macro_osc_preset", dev.uid())).with("popup"));
    }
}
