//! The Selected pad panel (README, Pad: the selected-pad strip; the board's middle panel): under
//! `SELECTED PAD`, the pad's name (Sans 600 16, the board's; a double-click renames it in place,
//! the app's rename convention) and `Pad A6 · note 66` in `readout.display`; 14 below, `Choke`
//! (`label.control`, the board's 54 px label column) with the group and its size in `value`
//! (`Group 1 · 3 pads`, `Off`), and the choke picker: a segmented grid of Off and 1 to 16 in nine
//! columns (the board's: a `surface.inset` track with a 1 px `line`, radius 6, padding and gaps
//! of 3, 22 px segments in Mono 500 10; the pad's group `surface.selected` with `text`, groups in
//! use `text.muted`, the others `ink.arrow`, a used group's color as a 2 px underline); then the
//! peers line (Sans 12 `text.muted`, line height 1.5); 14 below, `Pad` with the M and S toggles
//! (28 wide, the board's) and the board's hint in Sans 11 `text.faint`.

use super::{ChokeSegment, PadData, SamplerLayout, behavior, panel, section_label, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, Family, TypeRole, layout as space, radius};
use crate::widgets::{self, r4, toggle};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_model::sampler_patch::CHOKE_GROUPS;

/// The pad's name: Sans 600 16 (the board's).
pub const NAME: TypeRole = TypeRole { family: Family::Sans, weight: 600, size: 16.0, tracking: 0.0, uppercase: false, ink: None };
/// The peers line: Sans 12 `text.muted` at line height 1.5 (the board's).
pub const PEERS: TypeRole = tokens::LABEL_CONTROL;
pub const PEERS_LINE: f32 = 18.0;
/// The hint beside M and S: Sans 11 `text.faint` (the board's).
pub const HINT: TypeRole = TypeRole { family: Family::Sans, weight: 400, size: 11.0, tracking: 0.0, uppercase: false, ink: None };
pub const HINT_TEXT: &str = "muted or solo-excluded pads neither sound nor choke";
/// The label column (`Choke`, `Pad`), the gap after it, the choke segments' columns and their
/// track's padding and gaps, a segment's height, the toggles' width (all the board's).
pub const LABEL_W: f32 = 54.0;
pub const ITEM_GAP: f32 = 8.0;
pub const PICKER_COLUMNS: usize = 9;
pub const PICKER_PAD: f32 = 3.0;
pub const PICKER_GAP: f32 = 3.0;
pub const SEGMENT_H: f32 = 22.0;
pub const TOGGLE_W: f32 = 28.0;
/// The board's gaps: 2 inside the title group, 14 between groups, 8 inside the choke group.
pub const TITLE_GAP: f32 = 2.0;
pub const GROUP_GAP: f32 = 14.0;
/// The board's bottom padding.
pub const BOTTOM: f32 = 16.0;

/// The picker's rows and its track's height.
pub fn picker_rows() -> usize {
    (usize::from(CHOKE_GROUPS) + 1).div_ceil(PICKER_COLUMNS)
}

pub fn picker_h() -> f32 {
    let rows = picker_rows() as f32;
    rows * SEGMENT_H + (rows - 1.0) * PICKER_GAP + 2.0 * PICKER_PAD + 2.0
}

/// The peers line of pad `selected` (the board's sentences).
pub(crate) fn peers_line(pads: &[PadData], selected: u8) -> String {
    let Some(sel) = pads.get(usize::from(selected)) else { return String::new() };
    let g = sel.entry.choke;
    if g == 0 {
        return "Not in a choke group. Pick a number to join one; the same number in any bank is the same group.".to_string();
    }
    let peers = peers_of(pads, selected);
    if peers.is_empty() {
        return format!("Alone in group {g} so far.");
    }
    let names: Vec<String> =
        peers.iter().map(|i| format!("{} {}", super::pad_id(*i), pads.get(usize::from(*i)).map_or("", |d| d.name.as_str()))).collect();
    format!("Chokes with {}.", names.join(", "))
}

/// The pads sharing pad `selected`'s choke group, in pad order, across banks (loaded pads only).
pub(crate) fn peers_of(pads: &[PadData], selected: u8) -> Vec<u8> {
    let Some(sel) = pads.get(usize::from(selected)) else { return Vec::new() };
    if sel.entry.choke == 0 {
        return Vec::new();
    }
    (0..pads.len() as u8)
        .filter(|i| *i != selected && pads.get(usize::from(*i)).is_some_and(|d| d.zone.is_some() && d.entry.choke == sel.entry.choke))
        .collect()
}

fn wrapped(p: &egui::Painter, role: &TypeRole, text: &str, color: egui::Color32, width: f32, line: Option<f32>) -> std::sync::Arc<egui::Galley> {
    let mut job = role.job(p.ctx(), text, color);
    job.wrap.max_width = width.max(20.0);
    if let Some(l) = line
        && let Some(s) = job.sections.first_mut()
    {
        s.format.line_height = Some(l);
    }
    p.layout_job(job)
}

/// The hint's room: what the label and both toggles leave.
fn hint_w(inner_w: f32) -> f32 {
    inner_w - LABEL_W - ITEM_GAP - 2.0 * (TOGGLE_W + ITEM_GAP)
}

/// The choke row's height (its label and value).
fn row_h(p: &egui::Painter) -> f32 {
    super::line_h(p, &tokens::LABEL_CONTROL).max(super::line_h(p, &tokens::VALUE))
}

/// The panel's height at `width` with `peers` as its peers line.
pub fn height(p: &egui::Painter, width: f32, peers: &str) -> f32 {
    let inner = width - 2.0 * space::PANEL_PADDING[1];
    let t = &TOKENS;
    let title =
        super::line_h(p, &tokens::LABEL_SECTION) + TITLE_GAP + super::line_h(p, &NAME) + TITLE_GAP + super::line_h(p, &tokens::READOUT_DISPLAY);
    let peers_h = wrapped(p, &PEERS, peers, t.text_muted, inner, Some(PEERS_LINE)).size().y;
    let hint_h = wrapped(p, &HINT, HINT_TEXT, t.text_faint, hint_w(inner), None).size().y;
    space::PANEL_PADDING[0]
        + title
        + GROUP_GAP
        + row_h(p)
        + ITEM_GAP
        + picker_h()
        + ITEM_GAP
        + peers_h
        + GROUP_GAP
        + crate::tokens::toggle::HEIGHT.max(hint_h)
        + BOTTOM
}

/// Draws the panel into `rect`.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, pads: &[PadData], rect: Rect, out: &mut SamplerLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "strip", out);
    let p = ui.painter().clone();
    let selected = view(app, dev.uid()).selected;
    let Some(sel) = pads.get(usize::from(selected)).cloned() else { return };
    let s = &mut out.strip;
    *s = super::StripLayout { panel: r4(rect), ..Default::default() };
    let mut y = inner.min.y;
    let label = section_label(&p, pos2(inner.min.x, y), "Selected pad");
    y = label.max.y + TITLE_GAP;
    // The name: one line, cut with an ellipsis; a double-click renames the pad in place.
    s.name = if sel.zone.is_none() { "Empty pad".to_string() } else { sel.name.clone() };
    let mut job = NAME.job(p.ctx(), &s.name, t.text);
    job.wrap.max_width = inner.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = p.layout_job(job);
    let name_r = Rect::from_min_size(pos2(inner.min.x, y), vec2(galley.size().x.max(40.0), galley.size().y));
    p.galley(name_r.min, galley, t.text);
    s.name_rect = r4(name_r);
    let resp = ui.interact(name_r, egui::Id::new(("sampler_pad_name", dev.uid())), Sense::click());
    if sel.zone.is_some() && widgets::reset_click(&resp) {
        app.start_rename(crate::finish::RenameTarget::Pad(dev.key.0, selected), r4(name_r.expand2(vec2(4.0, 2.0))));
    }
    let s = &mut out.strip;
    y = name_r.max.y + TITLE_GAP;
    s.id_line = format!("Pad {} · note {}", super::pad_id(selected), sel.note);
    s.id_line_rect = r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, &s.id_line, t.text_faint, Align2::LEFT_TOP, pos2(inner.min.x, y)));
    y += super::line_h(&p, &tokens::READOUT_DISPLAY) + GROUP_GAP;
    // The choke row and the picker.
    let rh = row_h(&p);
    s.choke_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Choke", t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, y + rh / 2.0)));
    let peers = peers_of(pads, selected);
    s.choke_value = if sel.entry.choke == 0 { "Off".to_string() } else { format!("Group {} · {} pads", sel.entry.choke, peers.len() + 1) };
    s.choke_value_rect =
        r4(widgets::role_text(&p, &tokens::VALUE, &s.choke_value, t.text, Align2::LEFT_CENTER, pos2(inner.min.x + LABEL_W + ITEM_GAP, y + rh / 2.0)));
    y += rh + ITEM_GAP;
    let track = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), picker_h()));
    p.rect(track, radius::DISPLAY, t.surface_inset, Stroke::new(1.0, t.line), StrokeKind::Inside);
    s.picker = r4(track);
    let used: Vec<bool> = (0..=CHOKE_GROUPS).map(|g| g != 0 && pads.iter().any(|d| d.zone.is_some() && d.entry.choke == g)).collect();
    let cells = track.shrink(1.0 + PICKER_PAD);
    let cw = (cells.width() - (PICKER_COLUMNS as f32 - 1.0) * PICKER_GAP) / PICKER_COLUMNS as f32;
    let role = TypeRole { family: Family::Mono, weight: 500, size: 10.0, tracking: 0.0, uppercase: false, ink: None };
    let mut pick = None;
    for g in 0..=CHOKE_GROUPS {
        let i = usize::from(g);
        let (col, row) = (i % PICKER_COLUMNS, i / PICKER_COLUMNS);
        let r = Rect::from_min_size(
            pos2(cells.min.x + col as f32 * (cw + PICKER_GAP), cells.min.y + row as f32 * (SEGMENT_H + PICKER_GAP)),
            vec2(cw, SEGMENT_H),
        );
        let on = sel.entry.choke == g;
        let in_use = used.get(i).copied().unwrap_or(false);
        let ink = if on {
            t.text
        } else if in_use {
            t.text_muted
        } else {
            t.ink_arrow
        };
        if on {
            p.rect_filled(r, radius::SEGMENT, t.surface_selected);
        }
        let text = if g == 0 { "Off".to_string() } else { g.to_string() };
        widgets::role_text(&p, &role, &text, ink, Align2::CENTER_CENTER, r.center());
        let resp = ui.interact(r, egui::Id::new(("sampler_choke", dev.uid(), g)), Sense::click());
        if resp.clicked() && !on {
            pick = Some(g);
        }
        // A used group's color underline waits for group colors (none can be picked yet).
        out.strip.segments.push(ChokeSegment { rect: r4(r), text, group: g, used: in_use, selected: on, ink, underline: None });
    }
    let s = &mut out.strip;
    y = track.max.y + ITEM_GAP;
    s.peers = peers_line(pads, selected);
    let g = wrapped(&p, &PEERS, &s.peers, t.text_muted, inner.width(), Some(PEERS_LINE));
    let pr = Rect::from_min_size(pos2(inner.min.x, y), g.size());
    p.galley(pr.min, g, t.text_muted);
    s.peers_rect = r4(pr);
    y = pr.max.y + GROUP_GAP;
    // Pad: M and S, and the hint.
    let th = crate::tokens::toggle::HEIGHT;
    s.pad_label = r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, "Pad", t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, y + th / 2.0)));
    let mx = inner.min.x + LABEL_W + ITEM_GAP;
    let mut c = crate::device_editor::child(ui, Rect::from_min_size(pos2(mx, y), vec2(TOGGLE_W, th)));
    let m = toggle::show(&mut c, egui::Id::new(("sampler_mute", dev.uid())), "M", toggle::Role::Mute, sel.entry.mute, true, Some(TOGGLE_W));
    let mut c = crate::device_editor::child(ui, Rect::from_min_size(pos2(mx + TOGGLE_W + ITEM_GAP, y), vec2(TOGGLE_W, th)));
    let so = toggle::show(&mut c, egui::Id::new(("sampler_solo", dev.uid())), "S", toggle::Role::Solo, sel.entry.solo, true, Some(TOGGLE_W));
    let hx = mx + 2.0 * (TOGGLE_W + ITEM_GAP);
    let hg = wrapped(&p, &HINT, HINT_TEXT, t.text_faint, hint_w(inner.width()), None);
    let hr = Rect::from_min_size(pos2(hx, y + (th - hg.size().y).max(0.0) / 2.0), hg.size());
    p.galley(hr.min, hg, t.text_faint);
    let s = &mut out.strip;
    s.hint = HINT_TEXT.to_string();
    s.hint_rect = r4(hr);
    s.mute = Some(m.layout);
    s.solo = Some(so.layout);
    if m.toggled {
        behavior::set_pad(app, dev, selected, serde_json::json!({"mute": !sel.entry.mute}));
    }
    if so.toggled {
        behavior::set_pad(app, dev, selected, serde_json::json!({"solo": !sel.entry.solo}));
    }
    if let Some(g) = pick {
        behavior::set_pad(app, dev, selected, serde_json::json!({"choke": g}));
    }
}
