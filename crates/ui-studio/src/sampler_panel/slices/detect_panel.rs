//! The Detect panel (Auto; README, Slices view: Detect panel and round 2's pending detection; ADR
//! 0041 amendment 2026-11-08, items 5 and 7): Source `Cue points · Transients · Grid`,
//! Sensitivity (0 to 1, its tooltip the detector's 30 ms gap), and the Grid row (the division,
//! Bars `From tempo` or 1 to 16, and the BPM field), dimmed to the unused-control alpha unless the
//! source is Grid (dim: still editable).
//!
//! The panel's settings travel with Detect: a change makes the panel's own settings (only the
//! fields that differ from the loop's stored ones are sent) and, once they rest for
//! [`DEBOUNCE`] seconds of input time with no button held, one `mix.sampler_slice_detect` with
//! the placement `at`. The candidate it prepares on the engine's worker is read back
//! (`mix.sampler_slice_candidate`) while it is pending, every [`POLL`] seconds, and once per
//! document change after. It previews on the bench (Auto only); `Replace markers` accepts it (one
//! step, the settings stored), `Keep current` discards it and returns the panel to the stored
//! settings. An over-capacity, empty or refused result is the error banner, in the engine's
//! words (an empty result says the engine's note when it has one).

use super::bench::{self, segmented_at};
use super::edit::{self, Panel};
use super::state::Loop;
use super::{SlicesView, panels::Rows};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, chrome, fader as fd, layout, sampler as tk};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde::Deserialize;
use serde_json::{Value, json};

/// Seconds the panel's settings rest before Detect runs (a drag of Sensitivity sends one Detect).
pub const DEBOUNCE: f64 = 0.25;
/// Seconds between reads of a pending candidate.
pub const POLL: f64 = 0.1;

pub const SOURCES: [&str; 3] = ["Cue points", "Transients", "Grid"];
const SOURCE_KEYS: [&str; 3] = ["cues", "transients", "grid"];
pub const DIVISIONS: [&str; 4] = ["1/4", "1/8", "1/16", "1/32"];
pub const FROM_TEMPO: &str = "From tempo";
pub const SENSITIVITY_TIP: &str = "Detections keep at least 30 ms apart";
pub const NOTHING_FOUND: &str = "Detect found no slice markers.";

/// The detector's settings, as stored on the loop or as the panel holds them.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub source: String,
    pub sensitivity: f32,
    pub division: String,
    pub bars: Option<u8>,
    pub grid_bpm: Option<f64>,
}

impl Settings {
    pub fn stored(l: &Loop) -> Settings {
        Settings {
            source: l.detector.source.clone(),
            sensitivity: l.detector.sensitivity,
            division: l.division.clone(),
            bars: l.bars,
            grid_bpm: l.grid_bpm,
        }
    }

    /// Detect's params: the fields that differ from `stored` (each defaults to the stored one).
    pub fn changed(&self, stored: &Settings) -> Value {
        let mut p = json!({});
        if self.source != stored.source {
            p["source"] = json!(self.source);
        }
        if self.sensitivity != stored.sensitivity {
            // The fader steps by hundredths; sent as that decimal, not the f32's widening.
            p["sensitivity"] = json!((f64::from(self.sensitivity) * 100.0).round() / 100.0);
        }
        if self.division != stored.division {
            p["division"] = json!(self.division);
        }
        if self.bars != stored.bars {
            p["bars"] = json!(self.bars);
        }
        if self.grid_bpm != stored.grid_bpm {
            p["grid_bpm"] = json!(self.grid_bpm);
        }
        p
    }
}

/// A candidate marker.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CandidateMarker {
    pub frame: u64,
}

/// The track's detection candidate as `mix.sampler_slice_candidate` reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Candidate {
    pub candidate_id: u64,
    /// `pending`, `ready` or `refused`.
    pub state: String,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub current_count: usize,
    #[serde(default)]
    pub markers: Vec<CandidateMarker>,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub bpm: Option<f64>,
    #[serde(default)]
    pub tempo_origin: Option<String>,
}

impl Candidate {
    /// The frames to preview: a ready candidate that differs from the current Auto set.
    pub fn preview(&self, l: &Loop) -> Option<Vec<u64>> {
        let frames: Vec<u64> = self.markers.iter().map(|m| m.frame).collect();
        let current: Vec<u64> = l.auto.iter().map(|s| s.frame).collect();
        (self.state == "ready" && self.count > 1 && frames != current).then_some(frames)
    }
}

/// The panel's state (UI state; the candidate lives on the engine).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetectState {
    /// The panel's settings when a change made them differ from the stored ones.
    pub pending: Option<Settings>,
    /// When the last change was made (input time): Detect waits [`DEBOUNCE`] after it.
    pub changed_at: Option<f64>,
    pub candidate: Option<Candidate>,
    polled_at: f64,
    polled_revision: Option<u64>,
    /// Candidate reads so far.
    pub polls: u64,
    /// The BPM field's text while it is typed.
    pub bpm_text: Option<String>,
}

/// The Detect panel as drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DetectLayout {
    pub source: Option<segmented::SegmentedLayout>,
    pub fader: [f32; 4],
    pub cap: [f32; 4],
    pub sensitivity: String,
    pub division: Option<segmented::SegmentedLayout>,
    pub bars: [f32; 4],
    pub bars_text: String,
    pub bpm: [f32; 4],
    pub bpm_text: String,
    pub bpm_hint: String,
    pub bpm_enabled: bool,
    /// The Grid row's opacity (the unused-control alpha unless the source is Grid).
    pub grid_opacity: f32,
    /// `Candidate 8 → 9`, or `Analyzing…`.
    pub candidate: Option<String>,
    pub keep: Option<[f32; 4]>,
    pub replace: Option<[f32; 4]>,
}

/// Runs the debounce and the candidate reads; call once a frame before drawing.
pub(crate) fn tick(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, l: &Loop, now: f64, held: bool) {
    let d = &mut v.detect;
    if let (Some(at), Some(p)) = (d.changed_at, d.pending.clone())
        && now - at >= DEBOUNCE
        && !held
    {
        d.changed_at = None;
        let mut params = p.changed(&Settings::stored(l));
        params["at"] = json!(v.at);
        if let Some(a) = edit::send(app, dev, v, Panel::Left, "mix.sampler_slice_detect", params) {
            let id = a["candidate_id"].as_u64().unwrap_or(0);
            v.detect.candidate = Some(Candidate {
                candidate_id: id,
                state: "pending".into(),
                stale: false,
                current_count: l.auto.len(),
                markers: Vec::new(),
                count: 0,
                note: None,
                error: None,
                bpm: None,
                tempo_origin: None,
            });
            v.detect.polled_at = f64::NEG_INFINITY;
        }
    }
    let d = &mut v.detect;
    let Some(c) = &d.candidate else { return };
    let due = (c.state == "pending" && now - d.polled_at >= POLL) || d.polled_revision != Some(app.engine.revision);
    if !due {
        return;
    }
    d.polled_at = now;
    d.polled_revision = Some(app.engine.revision);
    d.polls += 1;
    let read = app.engine.execute("mix.sampler_slice_candidate", &json!({"track": dev.key.0.0}));
    d.candidate = read.ok().and_then(|r| serde_json::from_value::<Option<Candidate>>(r["candidate"].clone()).ok().flatten());
}

/// The source segment of `source` (none for `auto`, the first nonempty of the three).
fn source_index(source: &str) -> usize {
    SOURCE_KEYS.iter().position(|k| *k == source).unwrap_or(usize::MAX)
}

fn set(v: &mut SlicesView, l: &Loop, now: f64, f: impl FnOnce(&mut Settings)) {
    let mut s = v.detect.pending.clone().unwrap_or_else(|| Settings::stored(l));
    f(&mut s);
    v.detect.pending = (s != Settings::stored(l)).then_some(s);
    v.detect.changed_at = Some(now);
}

/// Draws the panel's rows into `rows` and handles its controls.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, l: &Loop, rows: &mut Rows, out: &mut DetectLayout) {
    let t = &TOKENS;
    let uid = dev.uid();
    let now = ui.input(|i| i.time);
    let s = v.detect.pending.clone().unwrap_or_else(|| Settings::stored(l));
    let p = ui.painter().clone();
    let control_x = rows.x + tk::DETECT_LABEL_WIDTH + tk::PANEL_ITEM_GAP;
    // Source.
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    rows.label(&p, r, "Source");
    let seg = segmented_at(ui, egui::Id::new(("sampler_detect_source", uid)), pos2(control_x, r.min.y), &SOURCES, source_index(&s.source), true);
    if let Some(i) = seg.picked {
        set(v, l, now, |s| s.source = SOURCE_KEYS[i].into());
    }
    out.source = Some(seg.layout);
    // Sensitivity: a linear horizontal fader, 0 to 1 (the fader's track, fill and cap).
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    rows.label(&p, r, "Sensitivity");
    let fader = Rect::from_min_size(pos2(control_x, r.min.y), vec2(tk::FADER_LENGTH, layout::DENSE_CONTROL_HEIGHT));
    let track = Rect::from_center_size(fader.center(), vec2(fader.width() - 2.0 * fd::INSET, fd::TRACK_WIDTH));
    let resp = ui.interact(fader, egui::Id::new(("sampler_detect_sensitivity", uid)), Sense::click_and_drag()).on_hover_text(SENSITIVITY_TIP);
    let mut value = s.sensitivity;
    if let Some(at) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let x = ((at.x - track.min.x) / track.width()).clamp(0.0, 1.0);
        let next = (x * 100.0).round() / 100.0;
        if next != value {
            value = next;
            set(v, l, now, |s| s.sensitivity = next);
        }
    }
    let cx = track.min.x + value * track.width();
    p.rect_filled(track, fd::TRACK_RADIUS, t.line);
    p.rect_filled(Rect::from_min_max(track.min, pos2(cx, track.max.y)), fd::TRACK_RADIUS, t.signal);
    let [cw, ch] = fd::CAP_HORIZONTAL;
    let cap = Rect::from_center_size(pos2(cx, track.center().y), vec2(cw, ch));
    p.rect_filled(cap, fd::CAP_RADIUS, t.knob_cap_active);
    p.rect_stroke(cap, fd::CAP_RADIUS, Stroke::new(1.0, t.ink_arrow), egui::StrokeKind::Inside);
    out.sensitivity = format!("{value:.2}");
    widgets::role_text(&p, &tokens::VALUE, &out.sensitivity, t.text, Align2::LEFT_CENTER, pos2(fader.max.x + tk::PANEL_ITEM_GAP, r.center().y));
    (out.fader, out.cap) = (r4(fader), r4(cap));
    // Grid: dimmed unless the source is Grid.
    let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
    let grid = s.source == "grid";
    out.grid_opacity = if grid { 1.0 } else { tk::UNUSED_CONTROL_ALPHA };
    ui.scope(|ui| {
        ui.multiply_opacity(out.grid_opacity);
        let p = ui.painter().clone();
        rows.label(&p, r, "Grid");
        let di = DIVISIONS.iter().position(|d| *d == s.division).unwrap_or(usize::MAX);
        let seg = segmented_at(ui, egui::Id::new(("sampler_detect_division", uid)), pos2(control_x, r.min.y), &DIVISIONS, di, true);
        if let Some(i) = seg.picked {
            set(v, l, now, |s| s.division = DIVISIONS[i].into());
        }
        let bars_x = seg.layout.track[2] + tk::PANEL_ITEM_GAP;
        out.division = Some(seg.layout);
        out.bars_text = s.bars.map_or_else(|| FROM_TEMPO.to_string(), |b| format!("{b} bars"));
        let bars_w = widgets::role_size(&p, &tokens::VALUE, FROM_TEMPO).x + 2.0 * chrome::FIELD_PADDING[1] + SPACE[2];
        let bars = Rect::from_min_size(pos2(bars_x, r.min.y), vec2(bars_w, layout::DENSE_CONTROL_HEIGHT));
        let mut pick = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bars), |ui| {
            ui.spacing_mut().interact_size.y = layout::DENSE_CONTROL_HEIGHT;
            crate::chrome::field(ui, |ui| {
                egui::ComboBox::from_id_salt(("sampler_detect_bars", uid)).width(bars_w).selected_text(out.bars_text.clone()).show_ui(ui, |ui| {
                    if crate::chrome_pick::choice_label(ui, s.bars.is_none(), FROM_TEMPO).clicked() {
                        pick = Some(None);
                    }
                    for b in 1..=16u8 {
                        if crate::chrome_pick::choice_label(ui, s.bars == Some(b), format!("{b} bars")).clicked() {
                            pick = Some(Some(b));
                        }
                    }
                })
            });
        });
        if let Some(b) = pick.filter(|b| *b != s.bars) {
            set(v, l, now, |s| s.bars = b);
        }
        out.bars = r4(bars);
        bpm_field(ui, uid, v, l, &s, now, pos2(bars.max.x + tk::PANEL_ITEM_GAP, r.min.y), out);
    });
    candidate_rows(app, ui, dev, v, l, rows, out);
}

/// The BPM field after Bars: empty, it shows the tempo in use and its origin as its hint; typing
/// sets `grid_bpm` and clearing returns to none; while Bars is explicit it shows the derived BPM,
/// read-only.
#[allow(clippy::too_many_arguments)]
fn bpm_field(ui: &mut Ui, _uid: u64, v: &mut SlicesView, l: &Loop, s: &Settings, now: f64, at: egui::Pos2, out: &mut DetectLayout) {
    let r = Rect::from_min_size(at, vec2(tk::BEAT_WIDTH, layout::DENSE_CONTROL_HEIGHT));
    let tempo = l.grid_tempo.as_ref();
    out.bpm_hint = tempo.filter(|g| g.origin != "bars").map(|g| format!("{} {:.2}", g.origin, g.bpm)).unwrap_or_default();
    out.bpm_enabled = s.bars.is_none();
    let shown = if let Some(text) = &v.detect.bpm_text {
        text.clone()
    } else if s.bars.is_some() {
        let derived = v.detect.candidate.as_ref().filter(|c| c.tempo_origin.as_deref() == Some("bars")).and_then(|c| c.bpm);
        derived.or(tempo.filter(|g| g.origin == "bars").map(|g| g.bpm)).map(|b| format!("{b:.2}")).unwrap_or_default()
    } else {
        s.grid_bpm.map(|b| format!("{b:.2}")).unwrap_or_default()
    };
    let mut text = shown.clone();
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            let edit = egui::TextEdit::singleline(&mut text)
                .hint_text(out.bpm_hint.clone())
                .font(tokens::VALUE.font_id(ui.ctx()))
                .desired_width(r.width())
                .min_size(r.size())
                .vertical_align(egui::Align::Center);
            crate::chrome::text_field_enabled(ui, out.bpm_enabled, edit)
        })
        .inner;
    out.bpm = r4(r);
    out.bpm_text = text.clone();
    if resp.has_focus() || resp.changed() {
        v.detect.bpm_text = Some(text.clone());
    }
    if resp.lost_focus() {
        v.detect.bpm_text = None;
        let typed = text.trim();
        let next = if typed.is_empty() { Some(None) } else { typed.parse::<f64>().ok().map(Some) };
        if let Some(b) = next.filter(|b| *b != s.grid_bpm) {
            set(v, l, now, |s| s.grid_bpm = b);
        }
    }
}

/// The candidate's line (`Candidate 8 → 9` with Keep current and Replace markers, or
/// `Analyzing…`), its refusal, and the panel's error banner and last edit.
fn candidate_rows(app: &mut StudioApp, ui: &mut Ui, dev: Dev, v: &mut SlicesView, l: &Loop, rows: &mut Rows, out: &mut DetectLayout) {
    let t = &TOKENS;
    let uid = dev.uid();
    let p = ui.painter().clone();
    let c = v.detect.candidate.clone();
    let mut error = v.feedback.left_error.clone();
    if let Some(c) = &c {
        rows.separator(&p);
        match c.state.as_str() {
            "pending" => {
                let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
                out.candidate = Some(bench::ANALYZING.to_string());
                widgets::role_text(&p, &tokens::VALUE, bench::ANALYZING, t.text_faint, Align2::LEFT_CENTER, r.left_center());
            }
            "refused" => error = error.or(c.error.clone()),
            _ if c.count <= 1 => error = error.or(Some(c.note.clone().unwrap_or_else(|| NOTHING_FOUND.to_string()))),
            _ => {
                let r = rows.row(layout::DENSE_CONTROL_HEIGHT);
                let text = format!("Candidate {} → {}", c.current_count, c.count);
                widgets::role_text(&p, &tokens::VALUE, &text, t.text, Align2::LEFT_CENTER, r.left_center());
                out.candidate = Some(text);
                let rw = default_w(&p, "Replace markers");
                let replace = Rect::from_min_size(pos2(r.max.x - rw, r.min.y), vec2(rw, r.height()));
                let kw = bench::button_w(&p, "Keep current");
                let keep = Rect::from_min_size(pos2(replace.min.x - SPACE[1] - kw, r.min.y), vec2(kw, r.height()));
                let keep_clicked = bench::dense_button(ui, egui::Id::new(("sampler_detect_keep", uid)), keep, "Keep current", true).clicked();
                let replace_clicked = default_button(ui, egui::Id::new(("sampler_detect_replace", uid)), replace, "Replace markers").clicked();
                (out.keep, out.replace) = (Some(r4(keep)), Some(r4(replace)));
                if keep_clicked {
                    edit::send(app, dev, v, Panel::Left, "mix.sampler_slice_discard", json!({"candidate_id": c.candidate_id}));
                    v.detect = super::detect_panel::DetectState::default();
                } else if replace_clicked {
                    let was = l.auto.len();
                    if let Some(a) = edit::send(app, dev, v, Panel::Left, "mix.sampler_slice_replace", json!({"candidate_id": c.candidate_id})) {
                        let n = a["markers"].as_array().map_or(c.count, Vec::len);
                        v.detect = super::detect_panel::DetectState::default();
                        v.feedback.left_success = Some(format!("Replaced the Auto set: {n} markers (was {was})"));
                    }
                }
                error = v.feedback.left_error.clone();
            }
        }
    }
    rows.feedback(ui, egui::Id::new(("sampler_detect_feedback", uid)), error.as_deref(), v.feedback.left_success.as_deref(), None);
}

/// The default button's width at the dense height (its text plus the default padding a side).
pub(crate) fn default_w(p: &egui::Painter, text: &str) -> f32 {
    (widgets::role_size(p, &chrome::DEFAULT_BUTTON_FONT, text).x + 2.0 * chrome::DEFAULT_BUTTON_PADDING[1]).ceil()
}

/// The default button (`signal` fill, `surface.inset` ink, Sans 600 12) at the dense height the
/// panels' boards draw (the chrome's default button is 26 high).
pub(crate) fn default_button(ui: &mut Ui, id: egui::Id, rect: Rect, text: &str) -> egui::Response {
    let t = &TOKENS;
    let resp = ui.interact(rect, id, Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.default_button_pressed
    } else if resp.hovered() {
        t.default_button_hover
    } else {
        t.signal
    };
    ui.painter().rect_filled(rect, chrome::RADIUS_CONTROL, fill);
    widgets::role_text(ui.painter(), &chrome::DEFAULT_BUTTON_FONT, text, t.surface_inset, Align2::CENTER_CENTER, rect.center());
    resp
}
