//! The Zone panel's fades item (README, Sampler views, round 2 "Edge fades"; ADR 0041 amendment
//! 2026-11-08, item 3): the four edge-fade widths, key low, key high, velocity low and velocity
//! high, as fields in Mono. `fades none` while all four are zero (a click on `none` opens the four
//! fields at 0); otherwise each width is a field, with the widths playback uses beside them when
//! the state reports others (`(5 · 5 · 0 · 0 used)`).
//!
//! When the fields let go (Enter, a click elsewhere) the four widths go in one
//! `mix.sampler_zone_edit {playback: {fades}}`, as typed: the engine checks them (integers 0 to
//! 127, `lo + hi` within the range on each axis, none on a pad's zone) and its refusal is the
//! view's note. Escape gives the typing up. Typing belongs to the zone it was typed for: when the
//! selection changes under it, it commits to that zone, never to the new one.

use super::state::{State, Zone};
use crate::theme::TOKENS;
use crate::tokens;
use crate::widgets;
use egui::{Align2, Rect, Ui, pos2, vec2};
use serde_json::{Value, json};

/// The fields' order and the command's keys.
pub const KEYS: [&str; 4] = ["key_lo", "key_hi", "vel_lo", "vel_hi"];
pub const TIP: &str = "Edge fades: key low, key high, velocity low, velocity high (0 to 127 steps)";

/// The four fields' typing and the zone it is for.
#[derive(Debug, Clone, PartialEq)]
pub struct FadeDraft {
    pub zone: u16,
    pub texts: [String; 4],
    /// The first field has been given the keyboard (after a click on `none`).
    pub opened: bool,
}

/// Field `i`'s id on instance `uid`.
pub fn field_id(uid: u64, i: usize) -> egui::Id {
    egui::Id::new(("sampler_zone_fade", uid, i))
}

/// What the item drew and asks for.
pub struct FadeOut {
    pub text: String,
    pub rect: Rect,
    pub focused: bool,
    pub intent: Option<super::zone_panel::Intent>,
    /// The four fields (none while `fades none` shows).
    pub fields: Vec<[f32; 4]>,
}

/// The command's `fades` for `d` against the stored widths `stored`: nothing when every field
/// still reads its stored width; each width as typed (a number when it reads as one).
pub fn params(d: &FadeDraft, stored: [u8; 4]) -> Option<Value> {
    if d.texts.iter().zip(stored).all(|(t, s)| t.trim() == s.to_string()) {
        return None;
    }
    let mut o = serde_json::Map::new();
    for (k, t) in KEYS.iter().zip(&d.texts) {
        let t = t.trim();
        o.insert((*k).to_string(), t.parse::<u64>().map_or_else(|_| json!(t), |n| json!(n)));
    }
    Some(Value::Object(o))
}

fn joined(w: [u8; 4]) -> String {
    w.iter().map(u8::to_string).collect::<Vec<_>>().join(" · ")
}

/// Draws the item from `at` (its left center) for zone `z`. `kept` says the panel's typing key
/// survived since the last frame (the device window's Escape removes it).
pub(crate) fn show(ui: &mut Ui, uid: u64, state: &State, z: &Zone, at: egui::Pos2, draft: &mut Option<FadeDraft>, kept: bool) -> FadeOut {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mut intent = None;
    if let Some(d) = draft.take_if(|d| d.zone != z.zone)
        && let Some(o) = state.zone(d.zone)
    {
        intent = params(&d, o.fades.all()).map(|v| super::zone_panel::Intent::Fades(d.zone, v));
    }
    let lr = widgets::role_text(&p, &tokens::READOUT_DISPLAY, "fades ", t.text_faint, Align2::LEFT_CENTER, at);
    let stored = z.fades.all();
    let fh = widgets::role_size(&p, &tokens::VALUE, "Ag").y;
    if draft.is_none() && stored == [0; 4] {
        let vr = widgets::role_text(&p, &tokens::VALUE, "none", t.text, Align2::LEFT_CENTER, pos2(lr.max.x, at.y));
        let resp = ui.interact(vr, egui::Id::new(("sampler_zone_fades_none", uid)), egui::Sense::click()).on_hover_text(TIP);
        if resp.clicked() {
            *draft = Some(FadeDraft { zone: z.zone, texts: [0u8; 4].map(|w| w.to_string()), opened: false });
        }
        return FadeOut { text: "fades none".into(), rect: lr.union(vr), focused: false, intent, fields: Vec::new() };
    }
    let had_id = egui::Id::new(("sampler_zone_fades_had", uid));
    let had = ui.data(|d| d.get_temp::<bool>(had_id).unwrap_or(false));
    let w = widgets::role_size(&p, &tokens::VALUE, "127").x;
    let sep = " · ";
    let mut x = lr.max.x;
    let mut focused = false;
    let mut right = x;
    let mut fields = Vec::new();
    for i in 0..4 {
        let mut s = draft.as_ref().map_or_else(|| stored[i].to_string(), |d| d.texts[i].clone());
        let r = Rect::from_min_size(pos2(x, at.y - fh / 2.0), vec2(w, fh));
        let edit = egui::TextEdit::singleline(&mut s)
            .id(field_id(uid, i))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .font(tokens::VALUE.font_id(ui.ctx()))
            .text_color(t.text)
            .horizontal_align(egui::Align::Center)
            .desired_width(w)
            .char_limit(3);
        let resp = ui.put(r, edit).on_hover_text(TIP);
        fields.push(widgets::r4(r));
        if resp.has_focus() || resp.changed() {
            let d = draft.get_or_insert_with(|| FadeDraft { zone: z.zone, texts: stored.map(|w| w.to_string()), opened: true });
            d.texts[i] = s;
        }
        if i == 0
            && let Some(d) = draft.as_mut().filter(|d| !d.opened)
        {
            resp.request_focus();
            d.opened = true;
        }
        focused |= resp.has_focus();
        x = r.max.x;
        right = x;
        if i < 3 {
            let sr = widgets::role_text(&p, &tokens::READOUT_DISPLAY, sep, t.text_faint, Align2::LEFT_CENTER, pos2(x, at.y));
            x = sr.max.x;
        }
    }
    let mut text = format!("fades {}", draft.as_ref().map_or_else(|| joined(stored), |d| d.texts.join(sep)));
    if draft.is_none() && z.fades_used != z.fades {
        let used = format!(" ({} used)", joined(z.fades_used.all()));
        let ur = widgets::role_text(&p, &tokens::VALUE, &used, t.text, Align2::LEFT_CENTER, pos2(right, at.y));
        right = ur.max.x;
        text.push_str(&used);
    }
    ui.data_mut(|d| d.insert_temp(had_id, focused));
    // All four let go: commit, or give the typing up on Escape.
    if had && !focused && draft.is_some() {
        if !kept || ui.input(|i| crate::shortcuts::hit(i, crate::shortcuts::Action::CloseEditor)) {
            *draft = None;
        } else if let Some(d) = draft.take() {
            intent = params(&d, stored).map(|v| super::zone_panel::Intent::Fades(z.zone, v)).or(intent);
        }
    }
    let rect = Rect::from_min_max(pos2(lr.min.x, lr.min.y.min(at.y - fh / 2.0)), pos2(right, lr.max.y.max(at.y + fh / 2.0)));
    FadeOut { text, rect, focused, intent, fields }
}
