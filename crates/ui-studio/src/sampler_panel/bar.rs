//! The Pads view's view bar (README, Sampler views: View bar, and the round 2 "Pads view bar"):
//! a row `VIEW_BAR_HEIGHT` high under the title bar, before the hero row. Built so far: the
//! one-line summary at the left in `readout.display` (`Bank A · 3 of 16 loaded · 2 choke
//! groups`: the shown bank, the choke groups in use across banks) and, at the right, the kit's
//! Start note field (ADR 0041 amendment of 2026-11-09; ADR 0051 decision 9). The view switch
//! and the actions (`Load kit…`, `Expand to rack…`) wait for their slice: the switch goes before
//! the summary and the actions after the field, `START_NOTE_GAP_BEFORE_ACTIONS` from it, and
//! until then the field keeps the bar's right edge.
//!
//! The Start note field: `Start note` in `label.control`, `START_NOTE_LABEL_GAP`, then a
//! `START_NOTE_WIDTH` by `START_NOTE_HEIGHT` field (`surface.inset`, 1 px `line`, `line.strong` on
//! hover, the control radius) with the note name centred in `value` (Mono 500 11), named by the
//! one namer in the Middle C setting. Its value is the patch's `pad_start_note` (what
//! `mix.sampler_state` reports; a kit saved before the field reads 60).
//!
//! - **Drag** up or down: a semitone per `START_NOTE_DRAG_PX_PER_SEMITONE` (an octave with Shift),
//!   from the value at the press, kept within 0 to `MAX_PAD_START`.
//! - **Ctrl+wheel**: a semitone a notch (an octave with Shift); a plain wheel stays with the window.
//! - **Right-click** opens a menu with Reset to C1 (the new-kit start note, named in the Middle C
//!   setting), one step.
//! - **Double-click** types a name (`c#1`, `Db1`) or a MIDI number; Enter commits, Escape cancels.
//!   Text that is not a note from 0 to `MAX_PAD_START` changes nothing: the field takes its error
//!   ring and the panel's note says why.
//!
//! Each step is `mix.sampler_pad_start`, merged while the pointer is held or the wheel turns (one
//! undo step per gesture, the panel's knob convention); a typed value is one step. The engine's
//! refusal (a moved pad would take a stored pad's note, or start a ninth voice) becomes the
//! panel's note under the hero row, and the field keeps the engine's value.

use super::{PadData, SamplerLayout, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius, sampler as tk};
use crate::widgets::{self, r4};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use transit_midi::{MiddleC, note_name, parse_note_or_number};
use transit_model::pad_notes::{MAX_PAD_START, NEW_PAD_START};
use transit_model::sampler_patch::{PadEntry, PatchV1};

/// The bar's height: the view switch's (README: a 24 px segmented control).
pub const HEIGHT: f32 = tk::VIEW_BAR_HEIGHT;

/// The Start note field as last drawn.
#[derive(Debug, Clone, Default)]
pub struct StartNoteLayout {
    pub label_rect: [f32; 4],
    pub field: [f32; 4],
    /// The engine's value (MIDI number).
    pub value: u8,
    /// The name shown (the namer's, in the Middle C setting).
    pub text: String,
    pub hovered: bool,
    /// Dragging, or the value being typed: the 1.5 px `signal` ring.
    pub active: bool,
    pub editing: bool,
    /// Typed text that is not a start note: the 1.5 px `meter.hot` ring.
    pub error: bool,
    pub tooltip: String,
    /// The right-click menu's Reset row while the menu is open, and its text (`Reset to C1`).
    pub reset_row: Option<[f32; 4]>,
    pub reset_text: String,
}

/// The view bar as last drawn.
#[derive(Debug, Clone, Default)]
pub struct BarLayout {
    pub rect: [f32; 4],
    /// `Bank A · 3 of 16 loaded · 2 choke groups`.
    pub summary: String,
    pub summary_rect: [f32; 4],
    pub start: StartNoteLayout,
}

/// The id of the Start note field of instance `uid` (its typing state lives under `editing`).
pub fn field_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_start_note", uid))
}

/// The summary: the shown bank's loaded pads and the choke groups in use (a group with a loaded
/// pad, in any bank).
pub(crate) fn summary(pads: &[PadData], bank: usize) -> String {
    let loaded = pads.iter().skip(bank * 16).take(16).filter(|d| d.zone.is_some()).count();
    let mut groups: Vec<u8> = pads.iter().filter(|d| d.zone.is_some() && d.entry.choke != 0).map(|d| d.entry.choke).collect();
    groups.sort_unstable();
    groups.dedup();
    let chokes = match groups.len() {
        0 => "no choke groups".to_string(),
        1 => "1 choke group".to_string(),
        n => format!("{n} choke groups"),
    };
    format!("Bank {} · {loaded} of 16 loaded · {chokes}", char::from(b'A' + bank.min(25) as u8))
}

/// The field's tooltip: what it moves, and the shown bank's default notes.
fn tooltip(start: u8, bank: usize, mc: MiddleC) -> String {
    let first = (bank * 16) as u8;
    let lo = note_name(PadEntry::default_note(first, start), mc);
    let hi = note_name(PadEntry::default_note(first + 15, start), mc);
    format!("Pad A1 plays this note; pads without their own note follow it · Bank {}: {lo}–{hi}", char::from(b'A' + bank.min(25) as u8))
}

/// What typed text sets: a start note from 0 to `MAX_PAD_START`, as a name in the Middle C setting
/// or a MIDI number (ADR 0051 decision 9), else `None`.
pub(crate) fn typed_note(text: &str, mc: MiddleC) -> Option<u8> {
    parse_note_or_number(text, mc, None).filter(|n| *n <= MAX_PAD_START)
}

/// The note `delta` steps of `step` semitones from `base`, kept within 0 to `MAX_PAD_START`.
pub(crate) fn stepped(base: u8, delta: i32, step: i32) -> u8 {
    (i32::from(base) + delta * step).clamp(0, i32::from(MAX_PAD_START)) as u8
}

/// The error the panel's note shows for typed text that is not a start note.
fn typed_error(text: &str, mc: MiddleC) -> String {
    format!(
        "No start note {}: type a note from {} to {} or a number from 0 to {MAX_PAD_START}.",
        text.trim(),
        note_name(0, mc),
        note_name(MAX_PAD_START, mc)
    )
}

/// The drag in progress: the value at the press and the last value it asked for.
#[derive(Debug, Clone, Copy)]
struct Drag {
    base: u8,
    sent: u8,
}

/// Sends `mix.sampler_pad_start` merged into the gesture in progress (drag or wheel).
fn send_merged(app: &mut StudioApp, dev: Dev, note: u8) -> bool {
    let key = format!("sampler_pad_start:{}", dev.key.0.0);
    super::behavior::merged(app, dev, "mix.sampler_pad_start", json!({"note": note}), &key)
}

/// Sends a typed value as its own step.
fn send_once(app: &mut StudioApp, dev: Dev, note: u8) -> bool {
    super::behavior::run(app, dev, "mix.sampler_pad_start", json!({"note": note}))
}

/// Draws the bar into `rect` and handles the field's gestures.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, patch: &PatchV1, pads: &[PadData], rect: Rect, out: &mut SamplerLayout) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mc = app.ui.middle_c;
    let bank = view(app, dev.uid()).bank.min(super::BANKS - 1);
    let bar = &mut out.bar;
    bar.rect = r4(rect);
    // The summary, left.
    bar.summary = summary(pads, bank);
    bar.summary_rect = r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, &bar.summary, t.text_faint, Align2::LEFT_CENTER, rect.left_center()));
    // The Start note field, right (where the actions will follow it).
    let field = Rect::from_min_size(
        pos2(rect.max.x - tk::START_NOTE_WIDTH, rect.center().y - tk::START_NOTE_HEIGHT / 2.0),
        vec2(tk::START_NOTE_WIDTH, tk::START_NOTE_HEIGHT),
    );
    let label = widgets::role_text(
        &p,
        &tokens::LABEL_CONTROL,
        tk::START_NOTE_LABEL,
        tokens::LABEL_CONTROL.color(),
        Align2::RIGHT_CENTER,
        pos2(field.min.x - tk::START_NOTE_LABEL_GAP, rect.center().y),
    );
    let value = patch.pad_start_note;
    let id = field_id(dev.uid());
    let (edit_id, drag_id, error_id) = (id.with("editing"), id.with("drag"), id.with("error"));
    let resp = ui.interact(field, id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let mut editing = ui.data(|d| d.get_temp::<String>(edit_id));
    let mut error = ui.data(|d| d.get_temp::<bool>(error_id)).unwrap_or(false) && editing.is_some();
    // Drag: a semitone (an octave with Shift) per step of travel from the press, from the value
    // at the press.
    if editing.is_none() && resp.dragged() {
        let shift = ui.input(|i| i.modifiers.shift);
        let travel = ui.input(|i| i.pointer.press_origin().zip(i.pointer.interact_pos()).map_or(0.0, |(a, b)| b.y - a.y));
        let start = Drag { base: value, sent: value };
        let mut d = if resp.drag_started() { start } else { ui.data(|m| m.get_temp::<Drag>(drag_id)).unwrap_or(start) };
        let steps = (-travel / tk::START_NOTE_DRAG_PX_PER_SEMITONE).trunc() as i32;
        let target = stepped(d.base, steps, if shift { 12 } else { 1 });
        if target != d.sent {
            d.sent = target;
            if target != value {
                send_merged(app, dev, target);
            }
        }
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    } else {
        ui.data_mut(|m| m.remove::<Drag>(drag_id));
    }
    // Ctrl+wheel: a semitone a notch (an octave with Shift).
    if editing.is_none() && resp.hovered() {
        // Fractions of a notch (smooth scrolling) add up across frames until they make one.
        let (notches, shift) = widgets::command_wheel(ui);
        let wheel_id = id.with("wheel");
        let acc = ui.data(|d| d.get_temp::<f32>(wheel_id)).unwrap_or(0.0) + notches;
        let n = acc.trunc() as i32;
        ui.data_mut(|d| d.insert_temp(wheel_id, acc - n as f32));
        if n != 0 {
            // Holds the frame-level merge end off while the wheel turns, as the knobs do: the
            // notches of one turn are one undo step.
            app.builtin_editor.devices.wheel_now = true;
            let target = stepped(value, n, if shift { 12 } else { 1 });
            if target != value {
                send_merged(app, dev, target);
            }
        }
    }
    // Double-click: type a name or a number. A double-click that follows another click within
    // egui's triple-click window counts as a triple, so both open the typing (the knob's reset
    // and the strip's rename read clicks the same way, `widgets::reset_click`).
    if editing.is_none() && widgets::reset_click(&resp) {
        editing = Some(note_name(value, mc));
        error = false;
    }
    // The field's face, under the typing.
    let hovered = resp.hovered();
    let edge = if hovered || resp.dragged() || editing.is_some() { t.line_strong } else { t.line };
    p.rect(field, radius::CONTROL, t.surface_inset, Stroke::new(1.0, edge), StrokeKind::Inside);
    if let Some(text) = editing.as_mut() {
        let edit = egui::TextEdit::singleline(text)
            .font(tokens::VALUE.font_id(ui.ctx()))
            .horizontal_align(egui::Align::Center)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO);
        // Token gap: no token names a field's text inset; 2 keeps the typed text inside the 1.5 px
        // ring (reported with the job, not invented as a token).
        let r = ui.put(field.shrink(2.0), edit);
        r.request_focus();
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter {
            match typed_note(text, mc) {
                Some(n) => {
                    if n != value {
                        send_once(app, dev, n);
                    }
                    editing = None;
                    error = false;
                }
                None => {
                    let msg = typed_error(text, mc);
                    app.status = msg.clone();
                    view(app, dev.uid()).note = Some(msg);
                    error = true;
                }
            }
        } else if escape {
            editing = None;
            error = false;
        }
    }
    ui.data_mut(|d| {
        if let Some(text) = &editing {
            d.insert_temp(edit_id, text.clone());
        } else {
            d.remove::<String>(edit_id);
        }
        d.insert_temp(error_id, error);
    });
    // The ring: the error's, else dragging's or typing's (the focus ring's width).
    let active = resp.dragged() || editing.is_some();
    let ring = tokens::chrome::FOCUS_RING_WIDTH;
    if error {
        p.rect_stroke(field, radius::CONTROL, Stroke::new(ring, t.meter_hot), StrokeKind::Inside);
    } else if active {
        p.rect_stroke(field, radius::CONTROL, Stroke::new(ring, t.signal), StrokeKind::Inside);
    }
    let text = note_name(value, mc);
    if editing.is_none() {
        widgets::role_text(&p, &tokens::VALUE, &text, tokens::VALUE.color(), Align2::CENTER_CENTER, field.center());
    }
    // Right-click: Reset to the new-kit start note (C1 under C3), one step.
    let mut reset_row = None;
    let mut reset = false;
    let reset_text = format!("Reset to {}", note_name(NEW_PAD_START, mc));
    crate::chrome::context_menu(&resp, false).show(|ui| {
        let row = ui.add(crate::chrome::MenuRow::new(reset_text.as_str()));
        reset_row = Some(r4(row.rect));
        reset = row.clicked();
    });
    if reset && value != NEW_PAD_START {
        send_once(app, dev, NEW_PAD_START);
    }
    let tip = tooltip(value, bank, mc);
    if editing.is_none() {
        resp.on_hover_text(tip.as_str());
    }
    out.bar.start = StartNoteLayout {
        label_rect: r4(label),
        field: r4(field),
        value,
        text,
        hovered,
        active,
        editing: editing.is_some(),
        error,
        tooltip: tip,
        reset_row,
        reset_text,
    };
}

/// The bar's height and the panel gap under it (the body's rows above the hero row).
pub const HEIGHT_WITH_GAP: f32 = HEIGHT + space::PANEL_GAP;
