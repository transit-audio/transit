//! The Pads panel (the board's pad grid panel; README, Pad): `PADS` in `label.section`, the bank
//! buttons at the right (a segmented control of the banks the patch has, A to D, each at least the
//! board's 34 wide, a 5 px `text.muted` dot on a bank that holds a peer of the selection), 10 below
//! them the 4 by 4 grid of 56 px pads with 4 px gaps, pad 1 at the bottom left (MPC order), and 10
//! below the grid the bank's status line in `readout.display` (`Bank A · 3 of 16 loaded`).
//!
//! A press on a pad selects it and auditions it; a drag from one pad released on another swaps
//! them; an audio file dropped on a pad becomes its sample ([`super::behavior`]).

use super::{BANKS, PadData, PadRecord, SamplerLayout, behavior, panel, section_label, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space};
use crate::widgets::{self, pad, r4, segmented};
use egui::{Align2, Rect, Ui, pos2, vec2};

/// Pads across and up.
pub const COLUMNS: usize = 4;
/// The grid's side: four 56 px pads and three 4 px gaps.
pub const GRID: f32 = COLUMNS as f32 * pad::GRID + (COLUMNS as f32 - 1.0) * pad::GAP;
/// The panel's width: the grid and the panel padding.
pub const WIDTH: f32 = GRID + 2.0 * space::PANEL_PADDING[1];
/// The header row (the bank buttons' track) and the gaps under it and under the grid (the board's 10).
pub const HEADER_H: f32 = segmented::HEIGHT;
pub const GAP: f32 = 10.0;
/// A bank button's least width (the board's).
pub const BANK_W: f32 = 34.0;
/// The board's bottom padding (14 above, 16 below).
pub const BOTTOM: f32 = 16.0;

/// The panel's height.
pub fn height(p: &egui::Painter) -> f32 {
    space::PANEL_PADDING[0] + HEADER_H + GAP + GRID + GAP + super::line_h(p, &tokens::READOUT_DISPLAY) + BOTTOM
}

/// The top left of pad slot `slot` (0..16) in a grid at `origin`: slot 0 at the bottom left, then
/// across each row and upward.
pub fn slot_pos(origin: egui::Pos2, slot: usize) -> egui::Pos2 {
    let (col, row) = (slot % COLUMNS, slot / COLUMNS);
    let step = pad::GRID + pad::GAP;
    pos2(origin.x + col as f32 * step, origin.y + (COLUMNS - 1 - row) as f32 * step)
}

/// The groups the selected pad shares with pads in other banks: which banks hold a peer.
pub(crate) fn peer_banks(pads: &[PadData], selected: u8) -> [bool; BANKS] {
    let mut out = [false; BANKS];
    let Some(sel) = pads.get(usize::from(selected)) else { return out };
    if sel.entry.choke == 0 {
        return out;
    }
    for (i, p) in pads.iter().enumerate() {
        if i != usize::from(selected)
            && p.zone.is_some()
            && p.entry.choke == sel.entry.choke
            && let Some(b) = out.get_mut(i / 16)
        {
            *b = true;
        }
    }
    out
}

/// Draws the panel into `rect` (its height is the hero row's).
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, pads: &[PadData], rect: Rect, now: f64, out: &mut SamplerLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "pads", out);
    let (bank, selected) = {
        let v = view(app, dev.uid());
        (v.bank.min(BANKS - 1), v.selected)
    };
    out.bank = bank;
    out.selected = selected;
    let p = ui.painter().clone();
    section_label(&p, pos2(inner.min.x, inner.min.y + (HEADER_H - super::line_h(&p, &tokens::LABEL_SECTION)) / 2.0), "Pads");
    // The bank buttons, right-aligned.
    let names: Vec<String> = (0..BANKS).map(|b| char::from(b'A' + b as u8).to_string()).collect();
    let options: Vec<&str> = names.iter().map(String::as_str).collect();
    let bw = BANKS as f32 * BANK_W + 2.0 * segmented::PADDING;
    let bank_r = Rect::from_min_size(pos2(inner.max.x - bw, inner.min.y), vec2(bw, HEADER_H));
    let mut c = crate::device_editor::child(ui, bank_r);
    let seg = segmented::show(&mut c, egui::Id::new(("sampler_bank", dev.uid())), &options, bank, Some(bw));
    let peers = peer_banks(pads, selected);
    out.bank_dots.clear();
    for (b, r) in seg.layout.segments.iter().enumerate() {
        if b != bank && peers.get(b).copied().unwrap_or(false) {
            let d = pad::BANK_DOT;
            let center = pos2(r[2] - 3.0 - d / 2.0, r[1] + 2.0 + d / 2.0);
            p.circle_filled(center, d / 2.0, t.text_muted);
            out.bank_dots.push(r4(Rect::from_center_size(center, vec2(d, d))));
        }
    }
    if let Some(b) = seg.picked {
        view(app, dev.uid()).bank = b;
    }
    out.banks = Some(seg.layout);
    // The grid.
    let origin = pos2(inner.min.x, inner.min.y + HEADER_H + GAP);
    out.grid = r4(Rect::from_min_size(origin, vec2(GRID, GRID)));
    let sel = pads.get(usize::from(selected));
    let any_solo = pads.iter().any(|d| d.entry.solo);
    let mut loaded = 0;
    out.pads.clear();
    for slot in 0..16 {
        let index = (bank * 16 + slot) as u8;
        let Some(d) = pads.get(usize::from(index)) else { continue };
        let empty = d.zone.is_none();
        loaded += usize::from(!empty);
        let peer = sel.is_some_and(|s| s.entry.choke != 0 && s.entry.choke == d.entry.choke && index != selected && !empty);
        let (fill, choked) = view(app, dev.uid()).live.get(usize::from(index)).map_or((0.0, false), |l| l.look(now));
        let id = super::pad_id(index);
        let shown = pad::Pad {
            size: pad::Size::Grid,
            id: &id,
            name: &d.name,
            empty,
            choke: d.entry.choke,
            // Group colors wait for a model field and a picker (reported): no group has one.
            tag: None,
            mute: d.entry.mute,
            solo: d.entry.solo,
            excluded: any_solo && !d.entry.solo,
            selected: index == selected,
            peer,
            fill,
            choked,
        };
        let o = pad::show(ui, egui::Id::new(("sampler_pad", dev.uid(), index)), slot_pos(origin, slot), &shown);
        behavior::pad_input(app, ui, dev, index, d, &o.response);
        super::pad_actions::menu(app, &o.response, dev, index, d.entry.note.is_some(), out);
        out.pads.push(PadRecord { index, pad: o.layout, note: d.note, zone: d.entry.zone });
    }
    // The status line.
    let at = pos2(inner.min.x, origin.y + GRID + GAP);
    out.status = format!("Bank {} · {loaded} of 16 loaded", names.get(bank).map_or("A", String::as_str));
    out.status_rect = r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, &out.status, t.text_faint, Align2::LEFT_TOP, at));
}
