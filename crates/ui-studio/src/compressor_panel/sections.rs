//! The controls (README, Device anatomy C; the board's second section) and the Detector section.
//!
//! - **Controls panel:** the dynamics knobs in the board's order and 84 px slots 8 apart
//!   (Threshold, Ratio, Knee, Attack, Release, Makeup, Mix; size M with label and readout), then a
//!   1 px `line` divider and the sidechain column (the board's `flex: 1`, 12 px in from the
//!   divider): the `Sidechain` label (`label.control`) with the key dropdown (110 wide, the
//!   board's) at the right, and under it the mode as a segmented control spread over the column:
//!   Peak (compress) or Limit (the `limiter` parameter). The board's third segment, RMS, waits
//!   for a detector parameter the plug-in does not have.
//! - **Sidechain key:** the chrome's dropdown (`chrome_pick::Combo`) reading the key and its tap
//!   (`Kick · pre`, `None`), its list the device card's: None, Post-fader and Pre-fader, the other
//!   tracks and the busses; a pick runs one `mix.insert_key` (the routing unchanged).
//! - **Detector** (a section bar, README Section bar, without an enable switch): lookahead and the
//!   sidechain's high-pass, which the board does not place. Open while either is in use (a
//!   lookahead above zero, the high-pass on) until the user chooses (`UiState::device_sections`);
//!   the open body is a panel with the Lookahead knob, the SC HPF option toggle and the HPF Freq
//!   knob in the knob row's slots.

use super::{CompressorLayout, Dev, finish, knob_at, panel};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space};
use crate::widgets::{self, knob, r4, section_bar, segmented, toggle};
use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use transit_model::{KeyFrom, Tap};

/// The dynamics knobs in the board's order: parameter and label.
pub const DYNAMICS: [(&str, &str); 7] = [
    ("threshold", "Threshold"),
    ("ratio", "Ratio"),
    ("knee", "Knee"),
    ("attack", "Attack"),
    ("release", "Release"),
    ("makeup", "Makeup"),
    ("mix", "Mix"),
];
/// A knob's slot and the gap between slots (the board's 84 and 8).
pub const SLOT: f32 = 84.0;
pub const SLOT_GAP: f32 = 8.0;
/// The sidechain column: its padding from the divider, the key dropdown and the room between its
/// rows (the board's 12, 110 by 26 and 8).
pub const COLUMN_PAD: f32 = 12.0;
pub const KEY_W: f32 = 110.0;
pub const KEY_H: f32 = space::CONTROL_HEIGHT[0];
pub const ROW_GAP: f32 = 8.0;
/// The mode's segments: compress (the peak detector) or limit.
pub const MODES: [&str; 2] = ["Peak", "Limit"];
/// Room above the section bar (the Macro Oscillator's).
pub const BAR_TOP: f32 = 6.0;

/// The controls panel's height: its padding and a knob cell.
pub fn controls_height() -> f32 {
    2.0 * space::PANEL_PADDING[0] + knob::cell(knob::Size::M).y
}

/// The collapse key of the Detector section in `UiState::device_sections`.
pub fn section_key() -> String {
    format!("{}/detector", super::COMPRESSOR)
}

/// Whether the Detector section's parameters are in use: a lookahead above zero, or the high-pass on.
pub(crate) fn in_use(app: &StudioApp, dev: Dev) -> bool {
    dev.value(app, "lookahead") > 0.0 || dev.on(app, "sc_hpf_on")
}

/// Whether the Detector section is collapsed: the user's choice, else collapsed while at rest.
pub(crate) fn collapsed(app: &StudioApp, dev: Dev) -> bool {
    app.ui.device_sections.get(&section_key()).copied().unwrap_or(!in_use(app, dev))
}

/// The Detector section's height (from the controls panel's bottom): the bar, and the body when
/// open.
pub(crate) fn detector_height(app: &StudioApp, dev: Dev) -> f32 {
    BAR_TOP + section_bar::HEIGHT + if collapsed(app, dev) { 0.0 } else { space::PANEL_GAP + controls_height() }
}

/// The key dropdown's text: the source and its tap (`Kick · pre`), `None` without a key.
pub(crate) fn key_text(s: &transit_model::Session, ins: &transit_model::Insert) -> String {
    let Some(k) = ins.key else { return "None".to_string() };
    let (name, tap) = match k.from {
        KeyFrom::Track(id) => (s.track(id).map_or("?".to_string(), |t| t.name.clone()), if k.tap == Tap::Pre { "pre" } else { "post" }),
        KeyFrom::Bus(id) => (s.busses.iter().find(|b| b.id == id).map_or("?".to_string(), |b| b.name.clone()), "post"),
    };
    format!("{name} · {tap}")
}

/// A pick in the key list.
enum KeyPick {
    Clear,
    Tap(bool),
    Source(KeyFrom),
}

/// The sidechain key dropdown at `rect`.
fn key_dropdown(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, out: &mut CompressorLayout) {
    let (t, slot) = dev.key;
    let s = app.engine.session_arc();
    let text = key_text(&s, dev.ins);
    let (tracks, busses) = crate::devices::key_sources(&s, t);
    let current = dev.ins.key;
    let pre = current.map_or(app.mixer.key_pre, |k| k.tap == Tap::Pre);
    let mut pick: Option<KeyPick> = None;
    let mut rows: Vec<(String, [f32; 4])> = Vec::new();
    let mut c = super::child(ui, rect);
    let shown = crate::chrome_pick::Combo::new(("compressor_key", dev.uid()), &text).width(rect.width()).show(&mut c, |ui| {
        let mut row = |ui: &mut Ui, on: bool, label: &str, to: KeyPick| {
            let r = crate::chrome_pick::choice_label(ui, on, label);
            rows.push((label.to_string(), r4(r.rect)));
            if r.clicked() {
                pick = Some(to);
            }
        };
        row(ui, current.is_none(), "None", KeyPick::Clear);
        ui.separator();
        // A tap change keeps the source; with no key yet it is what the next pick will use.
        row(ui, !pre, "Post-fader", KeyPick::Tap(false));
        row(ui, pre, "Pre-fader", KeyPick::Tap(true));
        if !tracks.is_empty() || !busses.is_empty() {
            ui.separator();
        }
        for (id, name) in &tracks {
            row(ui, current.is_some_and(|k| k.from == KeyFrom::Track(*id)), name, KeyPick::Source(KeyFrom::Track(*id)));
        }
        for (id, name) in &busses {
            row(ui, current.is_some_and(|k| k.from == KeyFrom::Bus(*id)), name, KeyPick::Source(KeyFrom::Bus(*id)));
        }
    });
    out.key.list = rows;
    crate::help::hover(ui, &shown.response, crate::help::Id::DeviceChainSidechain);
    out.key.dropdown = r4(shown.response.rect);
    out.key.text = text;
    match pick {
        None => {}
        Some(KeyPick::Clear) => {
            let _ = app.set_insert_key(t, slot, None, pre);
        }
        Some(KeyPick::Tap(pre)) => {
            app.mixer.key_pre = pre;
            // With a key, the same source at the other tap; without one, only the choice is kept.
            if let Some(k) = current {
                let _ = app.set_insert_key(t, slot, Some(k.from), pre);
            }
        }
        Some(KeyPick::Source(from)) => {
            let _ = app.set_insert_key(t, slot, Some(from), pre);
        }
    }
}

/// The controls panel into `rect`.
pub(crate) fn controls(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut CompressorLayout) {
    let t = &TOKENS;
    let inner = panel(ui, rect, "controls", out);
    let cell = knob::cell(knob::Size::M);
    let mut x = inner.min.x;
    for (id, label) in DYNAMICS {
        knob_at(app, ui, dev, id, label, pos2(x + (SLOT - cell.x) / 2.0, inner.min.y), lay, out);
        x += SLOT + SLOT_GAP;
    }
    let knobs_end = x - SLOT_GAP;
    out.sections.push(("dynamics".to_string(), r4(Rect::from_min_max(inner.min, pos2(knobs_end, inner.max.y)))));
    // The divider and the sidechain column.
    let dx = knobs_end + SLOT_GAP;
    ui.painter().line_segment([pos2(dx + 0.5, inner.min.y), pos2(dx + 0.5, inner.max.y)], Stroke::new(1.0, t.line));
    out.divider = [dx, inner.min.y, dx + 1.0, inner.max.y];
    let col = Rect::from_min_max(pos2(dx + 1.0 + COLUMN_PAD, inner.min.y), inner.max);
    let col_h = KEY_H + ROW_GAP + segmented::HEIGHT;
    let y = col.center().y - col_h / 2.0;
    // Sidechain: the label, the key at the right.
    let key = Rect::from_min_size(pos2(col.max.x - KEY_W, y), vec2(KEY_W, KEY_H));
    let lr =
        widgets::role_text(ui.painter(), &tokens::LABEL_CONTROL, "Sidechain", t.text_muted, Align2::LEFT_CENTER, pos2(col.min.x, key.center().y));
    out.key.label = r4(lr);
    key_dropdown(app, ui, dev, key, out);
    out.sections.push(("sidechain".to_string(), r4(Rect::from_min_max(pos2(col.min.x, y), pos2(col.max.x, key.max.y)))));
    // The mode: Peak or Limit.
    let my = key.max.y + ROW_GAP;
    let mode_r = Rect::from_min_size(pos2(col.min.x, my), vec2(col.width(), segmented::HEIGHT));
    let limit = dev.on(app, "limiter");
    let mut c = super::child(ui, mode_r);
    let s = segmented::show(&mut c, egui::Id::new(("compressor_mode", dev.uid())), &MODES, usize::from(limit), Some(col.width()));
    let track = super::hero::rect_of(s.layout.track);
    let r = ui.interact(track, egui::Id::new(("compressor_mode_row", dev.uid())), Sense::hover());
    // The row's control is the Limit segment: a click there turns Limit on, as the older toggle did.
    let limit_seg = s.layout.segments.get(1).copied().map_or(track, super::hero::rect_of);
    finish(app, ui, dev, "limiter", &r, limit_seg, s.picked.map(|i| i as f32), lay);
    out.sections.push(("mode".to_string(), r4(track)));
    out.mode = Some(s.layout);
    out.mode_options = MODES.iter().map(|m| m.to_string()).collect();
}

/// The Detector section's collapsed summary.
pub(crate) fn summary(app: &StudioApp, dev: Dev) -> String {
    let hpf = if dev.on(app, "sc_hpf_on") { format!("HPF {}", dev.text(app, "sc_hpf_freq")) } else { "HPF Off".to_string() };
    format!("Lookahead {}   ·   {hpf}", dev.text(app, "lookahead"))
}

/// The Detector section bar and, when open, its body, from `rect`'s top. Returns the height drawn.
pub(crate) fn detector(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut CompressorLayout) -> f32 {
    let collapsed = collapsed(app, dev);
    let bar_r = Rect::from_min_size(pos2(rect.min.x + 2.0, rect.min.y + BAR_TOP), vec2(rect.width() - 4.0, section_bar::HEIGHT));
    let text = summary(app, dev);
    let bar = section_bar::Bar {
        title: "Detector",
        tag: None,
        description: Some("Lookahead and the sidechain high-pass"),
        summary: Some(&text),
        collapsed,
        enabled: None,
    };
    let mut c = super::child(ui, bar_r);
    let b = section_bar::show(&mut c, egui::Id::new(("compressor_detector", dev.uid())), bar_r.width(), &bar);
    if b.toggle_collapse {
        app.ui.device_sections.insert(section_key(), !collapsed);
    }
    out.detector_bar = Some(b.layout);
    if collapsed {
        out.detector_body = None;
        return BAR_TOP + section_bar::HEIGHT;
    }
    let body = Rect::from_min_size(pos2(rect.min.x, bar_r.max.y + space::PANEL_GAP), vec2(rect.width(), controls_height()));
    out.detector_body = Some(r4(body));
    out.sections.push(("detector".to_string(), r4(body)));
    let inner = panel(ui, body, "detector", out);
    let cell = knob::cell(knob::Size::M);
    let at = |i: f32| pos2(inner.min.x + i * (SLOT + SLOT_GAP) + (SLOT - cell.x) / 2.0, inner.min.y);
    knob_at(app, ui, dev, "lookahead", "Lookahead", at(0.0), lay, out);
    // SC HPF: an option toggle in the second slot, centered on the dials.
    let on = dev.on(app, "sc_hpf_on");
    let w = toggle::width(ui, "SC HPF");
    let dial_cy = inner.min.y + knob::Size::M.diameter() / 2.0;
    let tr = Rect::from_min_size(
        pos2(inner.min.x + SLOT + SLOT_GAP + (SLOT - w) / 2.0, dial_cy - tokens::toggle::HEIGHT / 2.0),
        vec2(w, tokens::toggle::HEIGHT),
    );
    let mut c = super::child(ui, tr);
    let tg = toggle::show(&mut c, egui::Id::new(("compressor_hpf", dev.uid())), "SC HPF", toggle::Role::Option, on, true, Some(w));
    finish(app, ui, dev, "sc_hpf_on", &tg.response, tr, tg.toggled.then_some(if on { 0.0 } else { 1.0 }), lay);
    out.hpf_toggle = Some(tg.layout);
    knob_at(app, ui, dev, "sc_hpf_freq", "HPF Freq", at(2.0), lay, out);
    BAR_TOP + section_bar::HEIGHT + space::PANEL_GAP + controls_height()
}
