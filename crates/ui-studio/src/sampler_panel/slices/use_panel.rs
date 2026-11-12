//! The Use slices panel (README, Slices view: Use slices panel; round 2's Slice to MIDI): both
//! actions consume the set showing. Root (`C3 · 60`, slice i to key root+i); **Slices to pads**
//! with the 16-cell preview of the bank it fills and what it replaces, from the command's own
//! `preview: true` plan; then Slice to MIDI inline: the timing `Fit length · Keep duration` with a
//! one-line help each, the beat length (`8.00 beats`, the state query's default), the placement
//! line, the summary from the `preview: true` answer, an overlap's warning line (it becomes a new
//! take), a refusal's error banner in the engine's words (with **Map slices to keys** beside a
//! stale mapping's), and **Create clip** as the default button, which stays clickable and repeats
//! the reason.
//!
//! The previews are queries that edit nothing, read again only when the document, the placement
//! or one of the panel's own choices changes. Each action sends only what the panel changed: a
//! root the user picked, a beat length the user typed.

use super::bench;
use super::detect_panel::{default_button, default_w};
use super::edit::{self, Panel};
use super::feedback;
use super::panels::Rows;
use super::placement;
use super::state::Loop;
use super::{SlicesView, segmented_at};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, layout, sampler as tk};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Color32, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};

pub const TIMINGS: [&str; 2] = ["Fit length", "Keep duration"];
pub const FIT_HELP: &str = "Fit length stretches the notes to the beat length.";
pub const KEEP_HELP: &str = "Keep duration keeps the loop's own timing.";
pub const OVERLAP: &str = "The clip overlaps existing material: the result becomes a new take; the old clips move to the alternate playlist.";
pub const TO_PADS: &str = "Slices to pads";
pub const TO_MIDI: &str = "Slice to MIDI";
pub const CREATE: &str = "Create clip";
pub const MAP: &str = "Map slices to keys";

/// What a preview was read for: the revision, the placement and the panel's choices.
type Key = (u64, i64, Option<u8>, bool, Option<u64>);

/// The panel's state (UI state).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UseState {
    /// A root the user picked (none: the root in use).
    pub root: Option<u8>,
    /// Keep duration (else Fit length).
    pub keep: bool,
    /// A beat length the user typed (none: the default).
    pub beat: Option<f64>,
    /// The beat field's text while it is typed.
    pub beat_text: Option<String>,
    pads: Option<(Key, Value)>,
    midi: Option<(Key, Result<Value, String>)>,
    /// Preview reads so far.
    pub previews: u64,
}

/// The panel as drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UseLayout {
    pub caption: String,
    pub root: [f32; 4],
    pub root_text: String,
    pub to_pads: [f32; 4],
    /// The 16 preview cells: rect, filled (the run fills this pad), the selected slice's.
    pub cells: Vec<([f32; 4], bool, bool)>,
    pub plan: String,
    pub timing: Option<segmented::SegmentedLayout>,
    pub beat: [f32; 4],
    pub beat_text: String,
    pub beat_enabled: bool,
    pub help: String,
    pub placement: String,
    pub summary: String,
    pub overlap: Option<[f32; 4]>,
    pub map: Option<[f32; 4]>,
    pub create: [f32; 4],
    /// Every text of the panel (caption, key, plan, help, placement, summary, map caption).
    pub texts: Vec<[f32; 4]>,
}

fn beat_bits(b: Option<f64>) -> Option<u64> {
    b.map(f64::to_bits)
}

/// Reads the previews again when their key changed.
fn previews(app: &mut StudioApp, dev: Dev, v: &mut SlicesView) {
    let u = &mut v.usage;
    let key: Key = (app.engine.revision, v.at, u.root, u.keep, beat_bits(u.beat));
    let track = dev.key.0.0;
    if u.pads.as_ref().is_none_or(|(k, _)| *k != key) {
        let mut p = json!({"track": track, "preview": true});
        if let Some(r) = u.root {
            p["root"] = json!(r);
        }
        u.previews += 1;
        let plan = app.engine.execute("mix.sampler_slices_to_pads", &p).unwrap_or_else(|e| json!({"ok": false, "reason": edit::words(&e)}));
        u.pads = Some((key, plan));
    }
    if u.midi.as_ref().is_none_or(|(k, _)| *k != key) {
        u.previews += 1;
        let answer = app.engine.execute("mix.slice_to_midi", &midi_params(track, v.at, u, true)).map_err(|e| edit::words(&e));
        u.midi = Some((key, answer));
    }
}

/// Slice to MIDI's params: the timing and the placement always, the beat length and the root only
/// when the user set them.
fn midi_params(track: u64, at: i64, u: &UseState, preview: bool) -> Value {
    let mut p = json!({"track": track, "timing": if u.keep { "keep" } else { "fit" }, "at": at});
    if preview {
        p["preview"] = json!(true);
    }
    if let (Some(b), false) = (u.beat, u.keep) {
        p["beat_length"] = json!(b);
    }
    if let Some(r) = u.root {
        p["root"] = json!(r);
    }
    p
}

/// Draws the panel's rows; returns the label's rect.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, l: &Loop, rows: &mut Rows, out: &mut UseLayout) -> [f32; 4] {
    let t = &TOKENS;
    let uid = dev.uid();
    let mc = app.ui.middle_c;
    let live = l.source.is_some();
    if live {
        previews(app, dev, v);
    }
    let p = ui.painter().clone();
    out.texts.clear();
    // The label and the set it uses.
    let r = rows.row(super::super::line_h(&p, &tokens::LABEL_SECTION));
    let label = super::super::section_label(&p, r.min, "Use slices");
    out.caption = format!("the {} set, as shown", if l.mode == super::state::Mode::Auto { "Auto" } else { "Manual" });
    let r = widgets::role_text(
        &p,
        &tokens::READOUT_DISPLAY,
        &out.caption,
        t.text_faint,
        Align2::LEFT_CENTER,
        pos2(label.max.x + tk::PANEL_ITEM_GAP, label.center().y),
    );
    out.texts.push(r4(r));
    // Root.
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    rows.label(&p, r, "Root");
    let root = v.usage.root.or(l.root).unwrap_or(60);
    let name = |n: u8| format!("{} · {n}", transit_midi::note_name(n, mc));
    out.root_text = name(root);
    let rr = Rect::from_min_size(pos2(r.min.x + tk::USE_LABEL_WIDTH + tk::PANEL_ITEM_GAP, r.min.y), vec2(tk::ROOT_WIDTH, r.height()));
    let mut pick = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(rr), |ui| {
        ui.spacing_mut().interact_size.y = layout::DENSE_CONTROL_HEIGHT;
        crate::chrome::field(ui, |ui| {
            egui::ComboBox::from_id_salt(("sampler_use_root", uid)).width(rr.width()).selected_text(out.root_text.clone()).show_ui(ui, |ui| {
                for n in 0..=127u8 {
                    if crate::chrome_pick::choice_label(ui, n == root, name(n)).clicked() {
                        pick = Some(n);
                    }
                }
            })
        });
    });
    if let Some(n) = pick {
        v.usage.root = (Some(n) != l.root).then_some(n);
    }
    out.root = r4(rr);
    let key_text = widgets::role_text(
        &p,
        &tokens::READOUT_DISPLAY,
        &format!("slice i → key {root}+i"),
        t.text_faint,
        Align2::LEFT_CENTER,
        pos2(rr.max.x + tk::PANEL_ITEM_GAP, r.center().y),
    );
    out.texts.push(r4(key_text));
    // Slices to pads, with the bank it fills.
    let cell = tk::PREVIEW_CELL;
    let grid_h = 2.0 * cell + tk::PREVIEW_GAP;
    let r = rows.row(grid_h.max(layout::DENSE_CONTROL_HEIGHT));
    let b = Rect::from_min_size(
        pos2(r.min.x, r.center().y - layout::DENSE_CONTROL_HEIGHT / 2.0),
        vec2(tk::PANEL_ACTION_WIDTH, layout::DENSE_CONTROL_HEIGHT),
    );
    if bench::dense_button(ui, egui::Id::new(("sampler_use_to_pads", uid)), b, TO_PADS, true).clicked() {
        let mut params = json!({});
        if let Some(n) = v.usage.root {
            params["root"] = json!(n);
        }
        edit::send(app, dev, v, Panel::Use, "mix.sampler_slices_to_pads", params);
        previews(app, dev, v);
    }
    out.to_pads = r4(b);
    let plan = v.usage.pads.as_ref().map(|(_, p)| p.clone()).unwrap_or(Value::Null);
    let cells: Vec<(u64, u64)> =
        plan["cells"].as_array().into_iter().flatten().filter_map(|c| Some((c["pad"].as_u64()?, c["slice"].as_u64()?))).collect();
    let bank = cells.first().map_or(0, |c| (c.0 - 1) / 16);
    let gx = b.max.x + tk::PANEL_ITEM_GAP;
    let gy = r.center().y - grid_h / 2.0;
    out.cells.clear();
    for k in 0..16u64 {
        let (col, row) = ((k % tk::PREVIEW_COLUMNS as u64) as f32, (k / tk::PREVIEW_COLUMNS as u64) as f32);
        let c = Rect::from_min_size(pos2(gx + col * (cell + tk::PREVIEW_GAP), gy + row * (cell + tk::PREVIEW_GAP)), vec2(cell, cell));
        let hit = cells.iter().find(|x| x.0 == bank * 16 + k + 1);
        let selected = hit.is_some_and(|x| x.1 == v.selected as u64 + 1);
        let (fill, edge) = match (hit, selected) {
            (_, true) => (t.surface_raised, t.signal),
            (Some(_), false) => (t.surface_raised, t.line_strong),
            (None, _) => (Color32::TRANSPARENT, t.line),
        };
        p.rect(c, tk::PREVIEW_RADIUS, fill, Stroke::new(chrome::FIELD_EDGE_WIDTH, edge), StrokeKind::Inside);
        out.cells.push((r4(c), hit.is_some(), selected));
    }
    let grid_right = gx + tk::PREVIEW_COLUMNS * (cell + tk::PREVIEW_GAP) - tk::PREVIEW_GAP;
    out.plan = match (plan["ok"].as_bool(), cells.first()) {
        (Some(true), Some(first)) => {
            let replaced = plan["cells"].as_array().into_iter().flatten().filter(|c| !c["replaces"].is_null()).count();
            let replaces = if replaced == 0 { "replaces none".to_string() } else { format!("replaces {replaced}") };
            format!("bank {} from {} · {replaces}", char::from(b'A' + bank as u8), super::super::pad_id((first.0 - 1) as u8))
        }
        _ => plan["reason"].as_str().unwrap_or_default().to_string(),
    };
    // The plan wraps across the room the preview leaves (an engine refusal is a sentence); a
    // taller text grows the row.
    let room = (rows.x + rows.w - grid_right - tk::PANEL_ITEM_GAP).max(40.0);
    let mut job = tokens::READOUT_DISPLAY.job(ui.ctx(), &out.plan, t.text_faint);
    job.wrap.max_width = room;
    let galley = p.layout_job(job);
    let top = (r.center().y - galley.size().y / 2.0).max(r.min.y);
    out.texts.push(r4(Rect::from_min_size(pos2(grid_right + tk::PANEL_ITEM_GAP, top), galley.size())));
    rows.y = rows.y.max(top + galley.size().y);
    p.galley(pos2(grid_right + tk::PANEL_ITEM_GAP, top), galley, t.text_faint);
    midi(app, ui, dev, v, l, rows, out);
    r4(label)
}

/// The beat length field's id on the insert `uid`.
pub fn beat_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_use_beat", uid))
}

/// Slice to MIDI's rows.
fn midi(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, l: &Loop, rows: &mut Rows, out: &mut UseLayout) {
    let t = &TOKENS;
    let uid = dev.uid();
    let mc = app.ui.middle_c;
    let p = ui.painter().clone();
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    rows.label(&p, r, TO_MIDI);
    let seg = segmented_at(
        ui,
        egui::Id::new(("sampler_use_timing", uid)),
        pos2(r.min.x + tk::PANEL_ACTION_WIDTH + tk::PANEL_ITEM_GAP, r.min.y),
        &TIMINGS,
        usize::from(v.usage.keep),
        true,
    );
    if let Some(i) = seg.picked {
        v.usage.keep = i == 1;
    }
    let mut br = Rect::from_min_size(pos2(seg.layout.track[2] + tk::PANEL_ITEM_GAP, r.min.y), vec2(tk::BEAT_WIDTH, r.height()));
    if br.max.x > rows.x + rows.w {
        // Too narrow for the field beside the timing: it wraps to its own row under it, at the
        // timing's left edge.
        let next = rows.row(layout::DENSE_CONTROL_HEIGHT);
        br = Rect::from_min_size(pos2(seg.layout.track[0], next.min.y), br.size());
    }
    out.timing = Some(seg.layout);
    out.beat_enabled = !v.usage.keep;
    let beats = v.usage.beat.or(l.beats());
    let resting = beats.map(|b| format!("{b:.2} beats")).unwrap_or_default();
    let mut text = v.usage.beat_text.clone().unwrap_or_else(|| resting.clone());
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(br), |ui| {
            if !out.beat_enabled {
                ui.multiply_opacity(tk::UNUSED_CONTROL_ALPHA);
            }
            let edit = egui::TextEdit::singleline(&mut text)
                .font(tokens::VALUE.font_id(ui.ctx()))
                .desired_width(br.width())
                .min_size(br.size())
                .horizontal_align(egui::Align::Max)
                .vertical_align(egui::Align::Center)
                .id(beat_id(uid));
            crate::chrome::text_field_enabled(ui, out.beat_enabled, edit)
        })
        .inner;
    out.beat = r4(br);
    out.beat_text = text.clone();
    if resp.has_focus() || resp.changed() {
        v.usage.beat_text = Some(text.clone());
    }
    // The typed text commits once (the toolkit reports a lost focus on two frames running: the
    // second finds no typed text) and only when it changed; Escape cancels and the field returns
    // to the beat length in use.
    if resp.lost_focus()
        && let Some(typed) = v.usage.beat_text.take()
        && !super::escaped(ui)
        && typed.trim() != resting
    {
        let typed = typed.trim().trim_end_matches("beats").trim();
        if typed.is_empty() {
            v.usage.beat = None;
        } else if let Ok(b) = typed.parse::<f64>() {
            v.usage.beat = Some(b);
        }
    }
    // The help line, the placement and the summary.
    out.help = if v.usage.keep { KEEP_HELP } else { FIT_HELP }.to_string();
    let r = rows.row(super::super::line_h(&p, &tokens::LABEL_CONTROL));
    out.texts.push(r4(widgets::role_text(&p, &tokens::LABEL_CONTROL, &out.help, t.text_faint, Align2::LEFT_CENTER, r.left_center())));
    let place = placement::of(app, dev.key.0);
    out.placement = placement::line(app, place);
    let r = rows.row(super::super::line_h(&p, &tokens::VALUE));
    out.texts.push(r4(widgets::role_text(&p, &tokens::VALUE, &out.placement, t.text, Align2::LEFT_CENTER, r.left_center())));
    let preview = v.usage.midi.as_ref().map(|(_, a)| a.clone());
    if let Some(Ok(a)) = &preview {
        let key = |k: &str| a[k].as_u64().and_then(|n| u8::try_from(n).ok()).map(|n| transit_midi::note_name(n, mc)).unwrap_or_default();
        out.summary = format!(
            "{} notes · {} to {} · velocity {} · channel {}",
            a["notes"].as_u64().unwrap_or(0),
            key("first_key"),
            key("last_key"),
            a["velocity"].as_u64().unwrap_or(0),
            a["channel"].as_u64().unwrap_or(0)
        );
        let r = rows.row(super::super::line_h(&p, &tokens::READOUT_DISPLAY));
        out.texts.push(r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, &out.summary, t.text_muted, Align2::LEFT_CENTER, r.left_center())));
        if a["overlap"] == true {
            // The warning's triangle and its sentence, wrapped to the panel (the status line
            // helper draws one line).
            let sev = crate::chrome::Severity::Warning;
            let g = crate::chrome::glyph_size(tokens::LABEL_CONTROL.size);
            let mut job = tokens::LABEL_CONTROL.job(ui.ctx(), OVERLAP, sev.ink());
            job.wrap.max_width = (rows.w - g - 3.0).max(40.0);
            let galley = p.layout_job(job);
            let r = rows.row(galley.size().y.max(g));
            crate::icons::paint(&p, pos2(r.min.x + g / 2.0, r.min.y + g / 2.0), sev.icon(), g, sev.glyph());
            let text_at = pos2(r.min.x + g + 3.0, r.min.y);
            out.overlap = Some(r4(Rect::from_min_size(r.min, vec2(g + 3.0 + galley.size().x, r.height()))));
            p.galley(text_at, galley, sev.ink());
        }
    }
    // A refusal: the click's, else the preview's, in the engine's words; a stale mapping offers
    // Map slices to keys beside it.
    let refusal = v.feedback.use_error.clone().or(match &preview {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    });
    if let Some(e) = &refusal {
        let h = feedback::card_height(ui, e, rows.w, true, None);
        let r = rows.row(h);
        rows.error = Some(feedback::card(ui, egui::Id::new(("sampler_use_error", uid)), r, e, true, None).0);
        if l.mapping == "stale" {
            let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
            let mb = Rect::from_min_size(r.min, vec2(bench::button_w(&p, MAP), r.height()));
            let root = v.usage.root.or(l.root).unwrap_or(60);
            if bench::dense_button(ui, egui::Id::new(("sampler_use_map", uid)), mb, MAP, true).clicked()
                && edit::send(app, dev, v, Panel::Use, "mix.sampler_map_slices", json!({"root": root})).is_some()
            {
                previews(app, dev, v);
            }
            let caption = format!("from {}, one undo step; existing MIDI unchanged", transit_midi::note_name(root, mc));
            // Wrapped to the room beside the button; a taller caption grows the row.
            let cx = mb.max.x + SPACE[1];
            let mut job = tokens::LABEL_CONTROL.job(ui.ctx(), &caption, t.text_faint);
            job.wrap.max_width = (rows.x + rows.w - cx).max(40.0);
            let galley = p.layout_job(job);
            let top = (r.center().y - galley.size().y / 2.0).max(r.min.y);
            out.texts.push(r4(Rect::from_min_size(pos2(cx, top), galley.size())));
            rows.y = rows.y.max(top + galley.size().y);
            p.galley(pos2(cx, top), galley, t.text_faint);
            out.map = Some(r4(mb));
        }
    }
    // Create clip, the default button, with the last edit at its left.
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    let cw = default_w(&p, CREATE);
    let cb = Rect::from_min_size(pos2(r.max.x - cw, r.min.y), vec2(cw, r.height()));
    if let Some(s) = v.feedback.use_success.clone().filter(|_| refusal.is_none()) {
        // Beside the button: wrapped to the room it leaves, centered on the row.
        let room = cb.min.x - tk::PANEL_ITEM_GAP - r.min.x;
        let h = feedback::last_edit_height(ui, &s, room, true);
        let top = (r.center().y - h / 2.0).max(r.min.y);
        rows.line = Some(feedback::last_edit(ui, pos2(r.min.x, top), room, &s, true));
        rows.y = rows.y.max(top + h);
    }
    if default_button(ui, egui::Id::new(("sampler_use_create", uid)), cb, CREATE).clicked() {
        let params = midi_params(dev.key.0.0, v.at, &v.usage, false);
        if let Some(a) = edit::send(app, dev, v, Panel::Use, "mix.slice_to_midi", params) {
            let at = placement::position(app, a["at_tick"].as_i64().unwrap_or(v.at));
            v.feedback.use_success = Some(format!("Created clip at {at} · {} notes", a["notes"].as_u64().unwrap_or(0)));
        } else {
            v.feedback.use_success = None;
        }
    }
    out.create = r4(cb);
}
