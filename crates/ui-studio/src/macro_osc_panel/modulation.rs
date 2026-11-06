//! The Modulation section (transit's addition, #243; README, Device anatomy D, Source chip,
//! Routing row; the board's Row D) behind its section bar: the formula line, the source chips, the
//! routing rows of the matrix's sixteen and the dashed Add route row.
//!
//! - **Bar:** collapse only (a per-user view setting); the matrix has no enable parameter, so the
//!   bar has no switch. It says `n / 16 routes`, and while collapsed names the first three routes.
//! - **Source chips:** one per source, the dot `mod` when a row uses it (its badge counts them)
//!   and `ink.arrow` when none does. Drag a chip onto a knob with a destination: the first free row
//!   takes that source and destination (one undo step), at amount 0 until its bar is dragged.
//! - **Routing rows:** only the rows in use (a source or a destination set) and the rows Add route
//!   revealed this session. Row fill `bg.titlebar`, 1 px `line.inset`, radius 6, 40 high: number,
//!   source (dot and name), the arrow, destination with its group, via with `×`, curve icon and
//!   name, the bipolar amount bar with its value, remove (the row back to its defaults).

use super::{Dev, Lead, MacroLayout, active, dropdown, finish, panel, route};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, bipolar_amount, r4, section_bar};
use egui::{Align2, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_dsp::plugins::macro_osc::matrix::{CURVES, DESTINATIONS, ROWS, SOURCES, fade_gain};
use transit_dsp::plugins::macro_osc::row_param;

/// A source chip being dragged: the source's index into the matrix's sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chip(pub usize);

/// One chip as last drawn.
#[derive(Debug, Clone)]
pub struct ChipRecord {
    pub source: usize,
    pub name: String,
    pub rect: [f32; 4],
    /// `×n` when `n` rows use the source, empty otherwise.
    pub badge: String,
    pub routed: bool,
}

/// One routing row as last drawn.
#[derive(Debug, Clone)]
pub struct RouteRecord {
    pub row: usize,
    pub rect: [f32; 4],
    pub source: [f32; 4],
    pub dest: [f32; 4],
    pub via: [f32; 4],
    pub curve: [f32; 4],
    pub amount: bipolar_amount::AmountLayout,
    pub remove: [f32; 4],
    /// The group, destination, via and curve as printed, and the amount's text.
    pub texts: [String; 5],
}

const ROW_H: f32 = 40.0;
const ROW_GAP: f32 = 4.0;
const CHIP_H: f32 = 26.0;
const CHIP_GAP: f32 = 6.0;
const HEADER_H: f32 = 16.0;
const FORMULA: &str = "out = amount × curve(source) × via · per voice";
const CHIPS_LABEL: &str = "Sources — drag onto any knob";
/// The routing row's columns at the board's width: #, source, arrow, destination (the rest), via,
/// curve, amount, remove; they shrink together below it.
const COLUMNS: [f32; 8] = [32.0, 200.0, 24.0, 0.0, 180.0, 140.0, 300.0, 64.0];
const COLUMN_GAP: f32 = 8.0;
/// The narrowest the destination column gets before the fixed columns shrink.
const DEST_MIN: f32 = 160.0;

/// A destination's group (the board's prefix) and its name without the group.
pub fn group_of(dest: usize) -> (&'static str, &'static str) {
    let name = DESTINATIONS.get(dest).copied().unwrap_or("Off");
    match dest {
        1 => ("Pitch", name),
        2..=4 => ("Tone", name),
        5 => ("Gate", name),
        6 | 14 => ("Output", name),
        7 | 8 => ("Filter", name),
        9 => ("Filter", "Env amount"),
        10 | 12 => ("LFO 1", name.strip_prefix("LFO 1 ").unwrap_or(name)),
        11 | 13 => ("LFO 2", name.strip_prefix("LFO 2 ").unwrap_or(name)),
        _ => ("", name),
    }
}

/// The column widths at `width` (inside the row's 8 px padding).
pub fn columns(width: f32) -> [f32; 8] {
    let fixed: f32 = COLUMNS.iter().sum();
    let room = width - 2.0 * 8.0 - 7.0 * COLUMN_GAP;
    let scale = ((room - DEST_MIN) / fixed).min(1.0);
    let mut c = COLUMNS.map(|w| w * scale);
    c[3] = room - c.iter().sum::<f32>();
    c
}

/// The collapse state: the user's choice, else collapsed until a row is in use.
pub(crate) fn collapsed(app: &StudioApp, dev: Dev) -> bool {
    let key = super::shaper::section_key("modulation");
    app.ui.device_sections.get(&key).copied().unwrap_or_else(|| !(0..ROWS).any(|r| active(&route(app, dev, r))))
}

/// The rows shown: in use, or revealed by Add route.
pub(crate) fn shown_rows(app: &StudioApp, dev: Dev) -> Vec<usize> {
    let pending = app.builtin_editor.macro_osc.pending.get(&dev.uid());
    (0..ROWS).filter(|r| active(&route(app, dev, *r)) || pending.is_some_and(|p| p.contains(r))).collect()
}

/// The chips' rects across `width` from `origin`, wrapping (the label first).
fn chip_rects(p: &egui::Painter, origin: Pos2, width: f32) -> (Rect, Vec<Rect>) {
    let label = Rect::from_min_size(origin, vec2(widgets::role_size(p, &tokens::LABEL_CONTROL, CHIPS_LABEL).x + 4.0, CHIP_H));
    let mut x = label.max.x + CHIP_GAP;
    let mut y = origin.y;
    let mut out = Vec::new();
    for name in SOURCES.iter().skip(1) {
        let w = 6.0 + 10.0 + 6.0 + widgets::role_size(p, &tokens::chrome::BUTTON_FONT, name).x + 6.0 + 18.0 + 8.0;
        if x + w > origin.x + width {
            x = origin.x;
            y += CHIP_H + CHIP_GAP;
        }
        out.push(Rect::from_min_size(pos2(x, y), vec2(w, CHIP_H)));
        x += w + CHIP_GAP;
    }
    (label, out)
}

/// The section's height (chips estimated at two lines when no painter is at hand).
pub(crate) fn height(app: &StudioApp, dev: Dev, _width: f32) -> f32 {
    let bar = super::shaper::BAR_TOP + section_bar::HEIGHT;
    if collapsed(app, dev) {
        return bar;
    }
    bar + space::PANEL_GAP + body_h(shown_rows(app, dev).len(), 2.0 * CHIP_H + CHIP_GAP)
}

fn body_h(rows: usize, chips: f32) -> f32 {
    let p = space::PANEL_PADDING[0];
    p + 14.0 + space::PANEL_GAP + chips + space::PANEL_GAP + HEADER_H + ROW_GAP + rows as f32 * (ROW_H + ROW_GAP) + ROW_H + p
}

/// The collapsed summary: the first three routes, then how many more.
pub(crate) fn summary(app: &StudioApp, dev: Dev) -> String {
    let routes: Vec<String> = (0..ROWS)
        .map(|r| route(app, dev, r))
        .filter(|r| r.source != 0 && r.dest != 0)
        .map(|r| format!("{} → {}", SOURCES.get(r.source).copied().unwrap_or(""), DESTINATIONS.get(r.dest).copied().unwrap_or("")))
        .collect();
    let mut s: Vec<String> = routes.iter().take(3).cloned().collect();
    if routes.len() > 3 {
        s.push(format!("+{} more", routes.len() - 3));
    }
    s.join("   ·   ")
}

/// Makes a route from `source` to `dest` in the first free row (source and destination Off).
pub(crate) fn drop_route(app: &mut StudioApp, dev: Dev, source: usize, dest: usize) {
    let Some(row) = (0..ROWS).find(|r| {
        let x = route(app, dev, *r);
        x.source == 0 && x.dest == 0
    }) else {
        app.status = format!("All {ROWS} modulation routes are in use");
        return;
    };
    let (Some(s), Some(d)) = (row_param(row, 0), row_param(row, 2)) else { return };
    super::set_many(app, dev, &[(s, source as f32), (d, dest as f32)]);
}

/// Clears row `row` back to its defaults (one undo step) and forgets it was revealed.
pub(crate) fn remove_route(app: &mut StudioApp, dev: Dev, row: usize) {
    let values: Vec<(&str, f32)> = (0..5).filter_map(|f| row_param(row, f)).filter_map(|id| dev.param(id).map(|p| (id, p.default))).collect();
    super::set_many(app, dev, &values);
    if let Some(p) = app.builtin_editor.macro_osc.pending.get_mut(&dev.uid()) {
        p.retain(|r| *r != row);
    }
}

/// Add route: reveals the first row that is neither in use nor shown.
pub(crate) fn add_route(app: &mut StudioApp, dev: Dev) -> Option<usize> {
    let shown = shown_rows(app, dev);
    let row = (0..ROWS).find(|r| !shown.contains(r))?;
    app.builtin_editor.macro_osc.pending.entry(dev.uid()).or_default().push(row);
    Some(row)
}

/// The bar and, when open, the body, from `rect`'s top. Returns the height drawn.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut MacroLayout) -> f32 {
    let t = &TOKENS;
    let collapsed = collapsed(app, dev);
    let in_use = (0..ROWS).filter(|r| route(app, dev, *r).source != 0).count();
    let count = format!("{in_use} / {ROWS} routes");
    let sum = summary(app, dev);
    let closed = if sum.is_empty() { count.clone() } else { format!("{count}   ·   {sum}") };
    let bar_r = Rect::from_min_size(pos2(rect.min.x + 2.0, rect.min.y + super::shaper::BAR_TOP), vec2(rect.width() - 4.0, section_bar::HEIGHT));
    let id = egui::Id::new(("macro_osc_modulation", dev.uid()));
    let bar = section_bar::Bar {
        title: "Modulation",
        tag: Some("TRANSIT EXTENSION"),
        description: Some(&count),
        summary: Some(&closed),
        collapsed,
        enabled: None,
    };
    let mut c = super::child(ui, bar_r);
    let b = section_bar::show(&mut c, id, bar_r.width(), &bar);
    if b.toggle_collapse {
        app.ui.device_sections.insert(super::shaper::section_key("modulation"), !collapsed);
    }
    if let Some(tag) = b.layout.tag {
        let r = ui.interact(super::shaper::rect_of(tag), id.with("tag"), Sense::hover());
        crate::help::hover(ui, &r, crate::help::Id::MacroOscAddition);
    }
    out.modulation_bar = Some(b.layout);
    let top = super::shaper::BAR_TOP + section_bar::HEIGHT;
    if collapsed {
        out.modulation_body = None;
        return top;
    }
    let y = bar_r.max.y + space::PANEL_GAP;
    let p = ui.painter().clone();
    let inner_w = rect.width() - 2.0 * space::PANEL_PADDING[1];
    let (label, chips) = chip_rects(&p, pos2(rect.min.x + space::PANEL_PADDING[1], y + space::PANEL_PADDING[0] + 14.0 + space::PANEL_GAP), inner_w);
    let chips_h = chips.last().map_or(CHIP_H, |r| r.max.y - label.min.y);
    let rows = shown_rows(app, dev);
    let body = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), body_h(rows.len(), chips_h)));
    out.modulation_body = Some(r4(body));
    let inner = panel(ui, body, "modulation", out);
    // The formula, right-aligned.
    let fr = widgets::role_text(&p, &tokens::READOUT_DISPLAY, FORMULA, t.text_faint, Align2::RIGHT_TOP, inner.right_top());
    let fresp = ui.interact(fr, id.with("formula"), Sense::hover());
    crate::help::hover(ui, &fresp, crate::help::Id::MacroOscMatrix);
    out.formula = FORMULA.to_string();
    // The chips.
    widgets::role_text(&p, &tokens::LABEL_CONTROL, CHIPS_LABEL, t.text_faint, Align2::LEFT_CENTER, label.left_center());
    let routes: Vec<super::Route> = (0..ROWS).map(|r| route(app, dev, r)).collect();
    for (i, r) in chips.iter().enumerate() {
        let source = i + 1;
        let uses = routes.iter().filter(|x| x.source == source).count();
        chip(ui, dev, *r, source, uses, out);
    }
    // The header and the rows.
    let cols = columns(inner.width());
    let hy = label.min.y + chips_h + space::PANEL_GAP;
    let mut x = inner.min.x + 8.0;
    for (i, h) in ["#", "Source", "", "Destination", "Via", "Curve", "Amount", ""].iter().enumerate() {
        if !h.is_empty() {
            widgets::role_text(&p, &tokens::LABEL_SECTION, h, t.text_faint, Align2::LEFT_CENTER, pos2(x, hy + HEADER_H / 2.0));
        }
        x += cols[i] + COLUMN_GAP;
    }
    let mut ry = hy + HEADER_H + ROW_GAP;
    for row in rows {
        route_row(app, ui, dev, Rect::from_min_size(pos2(inner.min.x, ry), vec2(inner.width(), ROW_H)), row, &cols, lay, out);
        ry += ROW_H + ROW_GAP;
    }
    // Add route.
    let add = Rect::from_min_size(pos2(inner.min.x, ry), vec2(inner.width(), ROW_H));
    let resp = ui.interact(add, id.with("add"), Sense::click());
    if resp.hovered() {
        p.rect_filled(add, radius::DISPLAY, t.button_hover);
    }
    dashed_rect(&p, add, t.line_strong);
    let tw = widgets::role_size(&p, &tokens::chrome::BUTTON_FONT, "Add route").x;
    let cx = add.center().x - (tw + 20.0) / 2.0;
    crate::icons::paint(&p, pos2(cx + 6.0, add.center().y), crate::icons::Icon::Plus, 14.0, t.text_muted);
    widgets::role_text(&p, &tokens::chrome::BUTTON_FONT, "Add route", t.text_muted, Align2::LEFT_CENTER, pos2(cx + 20.0, add.center().y));
    if resp.clicked() {
        add_route(app, dev);
    }
    out.add_route = Some(r4(add));
    top + space::PANEL_GAP + body.height()
}

fn dashed_rect(p: &egui::Painter, r: Rect, color: egui::Color32) {
    let r = r.shrink(0.5);
    let s = Stroke::new(1.0, color);
    for seg in
        [[r.left_top(), r.right_top()], [r.right_top(), r.right_bottom()], [r.right_bottom(), r.left_bottom()], [r.left_bottom(), r.left_top()]]
    {
        p.extend(Shape::dashed_line(&seg, s, 4.0, 3.0));
    }
}

/// One source chip: a pill on `surface.raised` with a `line.strong` edge, the dot, the name, the
/// badge. Dragging it carries the source to a knob.
fn chip(ui: &mut Ui, dev: Dev, r: Rect, source: usize, uses: usize, out: &mut MacroLayout) {
    let t = &TOKENS;
    let resp = ui.interact(r, egui::Id::new(("macro_osc_chip", dev.uid(), source)), Sense::drag());
    resp.dnd_set_drag_payload(Chip(source));
    let p = ui.painter();
    p.rect(r, radius::CHIP, if resp.hovered() { t.button_hover } else { t.surface_raised }, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
    let routed = uses > 0;
    p.circle_filled(pos2(r.min.x + 6.0 + 5.0, r.center().y), 4.0, if routed { t.modulation } else { t.ink_arrow });
    let name = SOURCES.get(source).copied().unwrap_or("");
    let nr = widgets::role_text(p, &tokens::chrome::BUTTON_FONT, name, t.text, Align2::LEFT_CENTER, pos2(r.min.x + 22.0, r.center().y));
    let badge = if routed { format!("×{uses}") } else { String::new() };
    if routed {
        widgets::role_text(p, &tokens::TAG, &badge, t.text_faint, Align2::LEFT_CENTER, pos2(nr.max.x + 6.0, r.center().y));
    }
    if resp.dragged()
        && let Some(at) = ui.ctx().pointer_latest_pos()
    {
        // The chip follows the pointer while it is carried.
        let fp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("macro_osc_chip_drag")));
        let g = Rect::from_min_size(at + vec2(8.0, 8.0), r.size());
        fp.rect(g, radius::CHIP, t.surface_raised, Stroke::new(1.0, t.modulation), StrokeKind::Inside);
        widgets::role_text(&fp, &tokens::chrome::BUTTON_FONT, name, t.text, Align2::LEFT_CENTER, pos2(g.min.x + 22.0, g.center().y));
        fp.circle_filled(pos2(g.min.x + 11.0, g.center().y), 4.0, t.modulation);
    }
    out.chips.push(ChipRecord { source, name: name.to_string(), rect: r4(r), badge, routed });
}

/// The curve's icon: the curve sampled into a 26 by 18 box.
fn curve_icon(p: &egui::Painter, at: Pos2, curve: usize, color: egui::Color32) {
    let pts: Vec<Pos2> = (0..=16)
        .map(|i| {
            let x = i as f32 / 16.0;
            pos2(at.x + 3.0 + 20.0 * x, at.y + 15.0 - 12.0 * fade_gain(curve, x))
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(1.6, color)));
}

/// One routing row.
#[allow(clippy::too_many_arguments)]
fn route_row(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, row: usize, cols: &[f32; 8], lay: &mut EditorLayout, out: &mut MacroLayout) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    p.rect(rect, radius::DISPLAY, t.bg_titlebar, Stroke::new(1.0, t.line_inset), StrokeKind::Inside);
    let r = route(app, dev, row);
    let mut x = rect.min.x + 8.0;
    let mut cell = |i: usize| {
        let c = Rect::from_min_size(pos2(x, rect.center().y - 14.0), vec2(cols[i], 28.0));
        x += cols[i] + COLUMN_GAP;
        c
    };
    let (n, src, arrow, dstr, via, cur, amt, rem) = (cell(0), cell(1), cell(2), cell(3), cell(4), cell(5), cell(6), cell(7));
    widgets::role_text(&p, &tokens::VALUE, &format!("{}", row + 1), t.text_faint, Align2::LEFT_CENTER, n.left_center());
    let id = |f: &str| egui::Id::new(("macro_osc_route", dev.uid(), row, f.to_string()));
    let ids: Vec<&'static str> = (0..5).filter_map(|f| row_param(row, f)).collect();
    let [sid, vid, did, aid, cid] = ids[..] else { return };
    // Source.
    let sname = SOURCES.get(r.source).copied().unwrap_or("Off");
    let (resp, pick) = dropdown(ui, id("source"), src, t.surface_raised, Lead::Dot(t.modulation), sname, false, &SOURCES, r.source);
    finish(app, ui, dev, sid, &resp, pick.map(|i| i as f32), lay);
    // The arrow.
    let (a0, a1) = (pos2(arrow.min.x + 2.0, arrow.center().y), pos2(arrow.max.x - 4.0, arrow.center().y));
    p.line_segment([a0, a1], Stroke::new(1.5, t.ink_arrow));
    p.add(Shape::line(vec![a1 + vec2(-4.0, -4.0), a1, a1 + vec2(-4.0, 4.0)], Stroke::new(1.5, t.ink_arrow)));
    // Destination with its group.
    let (group, dname) = group_of(r.dest);
    let lead = if group.is_empty() { Lead::None } else { Lead::Text(group) };
    let (resp, pick) = dropdown(ui, id("dest"), dstr, t.surface_raised, lead, dname, false, &DESTINATIONS, r.dest);
    finish(app, ui, dev, did, &resp, pick.map(|i| i as f32), lay);
    // Via.
    let vname = if r.via == 0 { "None" } else { SOURCES.get(r.via).copied().unwrap_or("") };
    let (resp, pick) = dropdown(ui, id("via"), via, t.surface_panel, Lead::Text("×"), vname, false, &SOURCES, r.via);
    finish(app, ui, dev, vid, &resp, pick.map(|i| i as f32), lay);
    // Curve.
    let cname = CURVES.get(r.curve).copied().unwrap_or("Linear");
    let (resp, pick) = dropdown(ui, id("curve"), cur, t.surface_panel, Lead::Gap(26.0), cname, false, &CURVES, r.curve);
    curve_icon(&p, pos2(cur.min.x + 6.0, cur.center().y - 9.0), r.curve, t.text);
    finish(app, ui, dev, cid, &resp, pick.map(|i| i as f32), lay);
    // Amount.
    let bar_w = (amt.width() - 58.0).max(40.0);
    let mut c =
        super::child(ui, Rect::from_min_size(pos2(amt.min.x, amt.center().y - bipolar_amount::HEIGHT / 2.0), vec2(bar_w, bipolar_amount::HEIGHT)));
    let a = bipolar_amount::show(&mut c, id("amount"), bar_w, r.amount / 100.0);
    let amount_text = format!("{:+.0}%", a.amount.unwrap_or(r.amount / 100.0) * 100.0);
    widgets::role_text(&p, &tokens::VALUE, &amount_text, t.text, Align2::RIGHT_CENTER, pos2(amt.min.x + bar_w + 10.0 + 48.0, amt.center().y));
    finish(app, ui, dev, aid, &a.response, a.amount.map(|v| v * 100.0), lay);
    // Remove.
    let rr = Rect::from_center_size(pos2(rem.max.x - 13.0, rem.center().y), vec2(26.0, 26.0));
    let resp = ui.interact(rr, id("remove"), Sense::click());
    if resp.hovered() {
        p.rect_filled(rr, radius::SEGMENT, t.button_hover);
    }
    crate::icons::paint(&p, rr.center(), crate::icons::Icon::Remove, 16.0, t.text_faint);
    resp.clone().on_hover_text("Remove route");
    if resp.clicked() {
        remove_route(app, dev, row);
    }
    out.routes.push(RouteRecord {
        row,
        rect: r4(rect),
        source: r4(src),
        dest: r4(dstr),
        via: r4(via),
        curve: r4(cur),
        amount: a.layout,
        remove: r4(rr),
        texts: [group.to_string(), dname.to_string(), vname.to_string(), cname.to_string(), amount_text],
    });
}
