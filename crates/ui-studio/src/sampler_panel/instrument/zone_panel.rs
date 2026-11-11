//! The Zone panel (README, Sampler views, Instrument view; the boards `SamplerInstrument` and
//! `SamplerRound2`, answer 7): the selected zone's name, file, rate and length; its keys,
//! velocities, root and fades in Mono (`fades none` when all are zero; the widths playback uses
//! beside them when the state reports others); the zone waveform ([`super::zone_wave`]); Loop as a
//! segmented `Off · Forward · Ping-pong` and Crossfade as a millisecond field marked "forward loops
//! only", dimmed under Ping-pong. The crossfade converts at the zone source's native rate
//! (`floor(m * r / 1000 + 1/2)` frames, ADR 0041 amendment 2026-11-08, item 2, through the model's
//! own conversion); the field shows the stored value to one decimal with its frames in the tooltip,
//! and a warning line says what is used when the state reports less.
//!
//! Every edit is one `mix.sampler_zone_edit` with only the field it changes; the engine decides
//! (a one-shot cannot loop, a ping-pong loop has no crossfade, a crossfade is at most half the
//! loop) and its refusal is the view's note.

use super::state::{State, Zone};
use super::zone_wave::{self, Handle, WaveDrag, WaveLayout};

use crate::theme::TOKENS;
use crate::tokens::{self, layout, radius, sampler as tk};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use transit_model::sampler_loops::{crossfade_frames, crossfade_ms_text, tenths_of_ms};
use transit_model::sampler_patch::LoopMode;

pub const LOOPS: [&str; 3] = ["Off", "Forward", "Ping-pong"];
pub const FORWARD_ONLY: &str = "forward loops only";

/// What the panel shows beside the state.
pub struct PanelInputs<'a> {
    /// The zone's file name, its source's length in frames and its audio (peaks).
    pub file: String,
    pub frames: u64,
    pub audio: Option<&'a transit_model::SourceAudio>,
    pub names: transit_midi::MiddleC,
}

/// What the panel asks for this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    LoopMode(LoopMode),
    /// Zone (from 1) and its crossfade in source frames.
    Crossfade(u16, u64),
    Region(Handle, u64),
    /// Typed text that is not a crossfade: the reason, for the note row.
    Invalid(String),
}

/// The crossfade field's typing: the zone it is typed for, the text, and whether the text was
/// refused as not a crossfade (the field's error ring).
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub zone: u16,
    pub text: String,
    pub error: bool,
}

/// The crossfade field's id on instance `uid` (the device window's Escape rule reads it).
pub fn crossfade_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_zone_crossfade", uid))
}

/// What the field shows for `z`'s stored crossfade (`12.0 ms`).
fn stored_text(z: &Zone) -> String {
    format!("{} ms", crossfade_ms_text(z.crossfade, z.rate))
}

/// The reason typed text is not a crossfade.
pub fn invalid(text: &str) -> String {
    format!("No crossfade {}: type milliseconds, such as 12.5.", text.trim())
}

/// What committing `d` to zone `z` sends: nothing when the text still reads the stored value (a
/// focus that comes and goes, or the shown value's rounding read back, never edits) or names the
/// same frames; else the frames at the zone source's rate. Text that is not a number of
/// milliseconds is refused with its reason (the engine checks the value's limits).
fn commit(d: &Draft, z: &Zone) -> Result<Option<Intent>, String> {
    if d.text.trim() == stored_text(z) {
        return Ok(None);
    }
    let typed = d.text.trim().trim_end_matches("ms").trim();
    let frames = typed.parse::<f64>().ok().and_then(tenths_of_ms).map(|t| crossfade_frames(t, z.rate)).ok_or_else(|| invalid(&d.text))?;
    Ok((frames != z.crossfade).then_some(Intent::Crossfade(z.zone, frames)))
}

/// The panel as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZonePanelLayout {
    pub panel: [f32; 4],
    pub label: [f32; 4],
    pub name: (String, [f32; 4]),
    /// `felt_E2_ff_rr1.wav · 44.1 kHz · 3.82 s`.
    pub file: (String, [f32; 4]),
    /// `keys D#2–F2`, `vel 64–127`, `root E2`, `fades none`: each item's text and rect.
    pub fields: Vec<(String, [f32; 4])>,
    pub wave: WaveLayout,
    pub loop_label: [f32; 4],
    pub loop_mode: Option<segmented::SegmentedLayout>,
    pub crossfade_label: [f32; 4],
    pub crossfade: [f32; 4],
    pub crossfade_text: String,
    /// The field rings as refused typing.
    pub crossfade_error: bool,
    /// The field has the keyboard or holds typing (the device window then keeps Escape).
    pub typing: bool,
    pub crossfade_tip: String,
    /// The crossfade field's opacity (dimmed under Ping-pong; still editable).
    pub crossfade_opacity: f32,
    pub forward_only: [f32; 4],
    /// `Zone E2 is 9.4 ms long now: 4.7 ms of crossfade is used.`
    pub warning: Option<(String, [f32; 4])>,
}

fn line_h(p: &egui::Painter, role: &tokens::TypeRole) -> f32 {
    widgets::role_size(p, role, "Ag").y
}

fn header_h(p: &egui::Painter) -> f32 {
    line_h(p, &tokens::LABEL_SECTION).max(line_h(p, &tokens::TITLE_SECTION_BAR)).max(line_h(p, &tokens::READOUT_DISPLAY))
}

fn warning_h(p: &egui::Painter) -> f32 {
    line_h(p, &tokens::LABEL_CONTROL).max(crate::chrome::glyph_size(tokens::LABEL_CONTROL.size))
}

/// The panel's height (with the warning line when `warn`).
pub fn height(p: &egui::Painter, warn: bool) -> f32 {
    let rows = header_h(p) + line_h(p, &tokens::VALUE) + tk::ZONE_WAVE_HEIGHT + layout::DENSE_CONTROL_HEIGHT;
    let warn = if warn { tk::ZONE_ROW_GAP + warning_h(p) } else { 0.0 };
    2.0 * layout::PANEL_PADDING[0] + rows + 3.0 * tk::ZONE_ROW_GAP + warn
}

/// The warning line when playback uses less crossfade than is stored.
pub fn warning(state: &State, z: &Zone, names: transit_midi::MiddleC) -> Option<String> {
    (z.crossfade_used < z.crossfade).then(|| {
        let loop_len = state.span(z).map_or(0, |s| s.loop_end.saturating_sub(s.loop_start));
        let label = super::key_map::label(state, z, names);
        format!(
            "Zone {label} is {} ms long now: {} ms of crossfade is used.",
            crossfade_ms_text(loop_len, z.rate),
            crossfade_ms_text(z.crossfade_used, z.rate)
        )
    })
}

/// The kHz text of `rate` (`44.1 kHz`).
pub fn khz(rate: u32) -> String {
    format!("{} kHz", f64::from(rate) / 1000.0)
}

fn fades_text(f: [u8; 4]) -> String {
    f.iter().map(u8::to_string).collect::<Vec<_>>().join(" · ")
}

/// The Mono fields: (label, value) pairs.
pub fn fields(state: &State, z: &Zone, names: transit_midi::MiddleC) -> Vec<(&'static str, String)> {
    let n = |k: u8| transit_midi::note_name(k, names);
    let keys = match z.pad.and_then(|p| state.pad_note(p).map(|k| (p, k))) {
        Some((p, k)) => format!("{} · pad {}", n(k), super::super::pad_id(p - 1)),
        None if z.keys[0] == z.keys[1] => n(z.keys[0]),
        None => format!("{}–{}", n(z.keys[0]), n(z.keys[1])),
    };
    let vel = if z.pad.is_some() { "1–127".to_string() } else { format!("{}–{}", z.velocities[0], z.velocities[1]) };
    let fades = if z.fades.all() == [0; 4] {
        "none".to_string()
    } else if z.fades_used != z.fades {
        format!("{} ({} used)", fades_text(z.fades.all()), fades_text(z.fades_used.all()))
    } else {
        fades_text(z.fades.all())
    };
    vec![("keys ", keys), ("vel ", vel), ("root ", n(z.root)), ("fades ", fades)]
}

/// One line of `role` from `at` (its left center), cut with an ellipsis at `right` (the names'
/// rule) so it never leaves its panel; returns where it sits.
fn clipped(p: &egui::Painter, role: &tokens::TypeRole, text: &str, ink: egui::Color32, at: egui::Pos2, right: f32) -> Rect {
    let mut job = role.job(p.ctx(), text, ink);
    job.wrap.max_width = (right - at.x).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = p.layout_job(job);
    let r = Rect::from_min_size(pos2(at.x, at.y - galley.size().y / 2.0), galley.size());
    p.galley(r.min, galley, ink);
    r
}

/// Draws the panel into `rect` for instance `uid`. `text` is the crossfade field's typing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut Ui,
    uid: u64,
    state: &State,
    z: &Zone,
    inputs: &PanelInputs,
    wave_drag: &mut Option<WaveDrag>,
    text: &mut Option<Draft>,
    rect: Rect,
) -> (ZonePanelLayout, Option<Intent>) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let mut out = ZonePanelLayout { panel: r4(rect), crossfade_opacity: 1.0, ..ZonePanelLayout::default() };
    let mut intent = None;
    // Text typed for another zone (the selection changed as the field let go) commits to that
    // zone, never to this one, and never stays on screen; text that cannot commit is dropped.
    if let Some(d) = text.take_if(|d| d.zone != z.zone)
        && !d.error
        && let Some(o) = state.zone(d.zone)
    {
        intent = commit(&d, o).ok().flatten();
    }
    p.rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    let inner = rect.shrink2(vec2(layout::PANEL_PADDING[1], layout::PANEL_PADDING[0]));
    // Header: ZONE, the name, the file line.
    let hh = header_h(&p);
    let cy = inner.min.y + hh / 2.0;
    out.label = r4(widgets::role_text(&p, &tokens::LABEL_SECTION, "Zone", t.text_muted, Align2::LEFT_CENTER, pos2(inner.min.x, cy)));
    let name = super::key_map::label(state, z, inputs.names);
    let nr = widgets::role_text(&p, &tokens::TITLE_SECTION_BAR, &name, t.text, Align2::LEFT_CENTER, pos2(out.label[2] + tk::ZONE_HEADER_GAP, cy));
    out.name = (name, r4(nr));
    let file = [inputs.file.clone(), khz(z.rate), format!("{:.2} s", inputs.frames as f64 / f64::from(z.rate.max(1)))]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let fr = clipped(&p, &tokens::READOUT_DISPLAY, &file, t.text_faint, pos2(nr.max.x + tk::ZONE_HEADER_GAP, cy), inner.max.x);
    out.file = (file, r4(fr));
    // The Mono fields.
    let mut y = inner.min.y + hh + tk::ZONE_ROW_GAP;
    let fh = line_h(&p, &tokens::VALUE);
    let mut x = inner.min.x;
    for (label, value) in fields(state, z, inputs.names) {
        let lr = widgets::role_text(&p, &tokens::READOUT_DISPLAY, label, t.text_faint, Align2::LEFT_CENTER, pos2(x, y + fh / 2.0));
        let vr = widgets::role_text(&p, &tokens::VALUE, &value, t.text, Align2::LEFT_CENTER, pos2(lr.max.x, y + fh / 2.0));
        out.fields.push((format!("{label}{value}"), r4(lr.union(vr))));
        x = vr.max.x + tk::ZONE_FIELD_GAP;
    }
    y += fh + tk::ZONE_ROW_GAP;
    // The waveform.
    let well = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), tk::ZONE_WAVE_HEIGHT));
    let span = state.span(z).unwrap_or(super::state::Span { source: None, start: 0, end: 0, loop_start: 0, loop_end: 0 });
    let tip = if z.slice.is_some() {
        format!("Slice {}: its region and loop follow the slice (Slices view).", z.slice.as_ref().map_or(0, |s| s.slice))
    } else {
        super::tip("Move: drag; cancel", &[crate::shortcuts::Action::SamplerCancelZoneDrag])
    };
    let w = zone_wave::WaveInputs {
        zone: z.zone,
        span,
        frames: inputs.frames,
        rate: z.rate,
        loop_used: z.loop_used,
        crossfade_used: z.crossfade_used,
        locked: z.slice.is_some(),
        audio: inputs.audio,
        handle_tip: &tip,
    };
    let (wave, moved) = zone_wave::show(ui, uid, &w, wave_drag, well);
    out.wave = wave;
    if let Some((h, f)) = moved {
        intent = Some(Intent::Region(h, f));
    }
    y += tk::ZONE_WAVE_HEIGHT + tk::ZONE_ROW_GAP;
    // Loop and Crossfade.
    let row = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), layout::DENSE_CONTROL_HEIGHT));
    let cy = row.center().y;
    let ll = widgets::role_text(&p, &tokens::LABEL_CONTROL, "Loop", t.text_muted, Align2::LEFT_CENTER, pos2(row.min.x, cy));
    out.loop_label = r4(ll);
    let selected = match z.loop_mode {
        LoopMode::Off => 0,
        LoopMode::Forward => 1,
        LoopMode::PingPong => 2,
    };
    let seg = super::super::slices::segmented_at(
        ui,
        egui::Id::new(("sampler_zone_loop", uid)),
        pos2(row.min.x + tk::LOOP_LABEL_WIDTH + tk::ZONE_ROW_GAP, cy - segmented::HEIGHT / 2.0),
        &LOOPS,
        selected,
        true,
    );
    if let Some(i) = seg.picked {
        intent = Some(Intent::LoopMode([LoopMode::Off, LoopMode::Forward, LoopMode::PingPong][i]));
    }
    let seg_right = seg.layout.track[2];
    out.loop_mode = Some(seg.layout);
    let dim = z.loop_mode == LoopMode::PingPong;
    out.crossfade_opacity = if dim { tk::UNUSED_CONTROL_ALPHA } else { 1.0 };
    let fade = |c: egui::Color32| if dim { c.gamma_multiply(tk::UNUSED_CONTROL_ALPHA) } else { c };
    let cl = widgets::role_text(
        &p,
        &tokens::LABEL_CONTROL,
        "Crossfade",
        fade(t.text_muted),
        Align2::LEFT_CENTER,
        pos2(seg_right + tk::ZONE_ROW_GAP + tk::CROSSFADE_LABEL_GAP, cy),
    );
    out.crossfade_label = r4(cl);
    let field = Rect::from_min_size(
        pos2(cl.max.x + tk::ZONE_ROW_GAP, cy - layout::DENSE_CONTROL_HEIGHT / 2.0),
        vec2(tk::CROSSFADE_WIDTH, layout::DENSE_CONTROL_HEIGHT),
    );
    out.crossfade = r4(field);
    let stored = stored_text(z);
    out.crossfade_tip = format!("{} frames at {}", z.crossfade, khz(z.rate));
    let mut typing = text.as_ref().map_or_else(|| stored.clone(), |d| d.text.clone());
    let error = text.as_ref().is_some_and(|d| d.error);
    let resp = ui
        .scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
            if dim {
                ui.multiply_opacity(tk::UNUSED_CONTROL_ALPHA);
            }
            let edit = egui::TextEdit::singleline(&mut typing)
                .id(crossfade_id(uid))
                .font(tokens::VALUE.font_id(ui.ctx()))
                .horizontal_align(egui::Align::RIGHT)
                .desired_width(field.width())
                .min_size(field.size())
                .vertical_align(egui::Align::Center);
            crate::chrome::text_field_enabled(ui, true, edit)
        })
        .inner
        .on_hover_text(&out.crossfade_tip);
    // While the field has the keyboard its text is also kept where the device window's Escape
    // rule gives typing up (`<id>.editing`): gone when the field lets go, Escape was pressed.
    // Whether it had the keyboard last frame is kept beside it (egui can report the letting go on
    // two frames; only the first is the field's).
    let (edit_id, had_id) = (crossfade_id(uid).with("editing"), crossfade_id(uid).with("had"));
    let kept = ui.data(|d| d.get_temp::<String>(edit_id).is_some());
    let had = ui.data(|d| d.get_temp::<bool>(had_id).unwrap_or(false));
    let focused = resp.has_focus();
    ui.data_mut(|d| d.insert_temp(had_id, focused));
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(edit_id, typing.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(edit_id));
    }
    if resp.has_focus() || resp.changed() {
        // The draft belongs to the zone it is typed for; an edit clears the error.
        *text = Some(Draft { zone: z.zone, text: typing.clone(), error: error && !resp.changed() });
    }
    if resp.lost_focus() && had {
        if !kept || ui.input(|i| crate::shortcuts::hit(i, crate::shortcuts::Action::CloseEditor)) {
            // Escape gives the typing up: the field reads the stored value again.
            *text = None;
        } else if let Some(d) = text.take() {
            match commit(&d, z) {
                Ok(i) => intent = i,
                Err(reason) => {
                    *text = Some(Draft { error: true, ..d });
                    intent = Some(Intent::Invalid(reason));
                }
            }
        }
    }
    let error = text.as_ref().is_some_and(|d| d.error);
    if error {
        p.rect_stroke(field, crate::tokens::chrome::RADIUS_CONTROL, super::super::bar::field_stroke(super::super::bar::FieldState::Error), StrokeKind::Inside);
    }
    out.crossfade_error = error;
    out.typing = resp.has_focus() || text.as_ref().is_some_and(|d| !d.error);
    out.crossfade_text = text.as_ref().map_or(stored, |d| d.text.clone());
    let fo = clipped(&p, &tokens::LABEL_CONTROL, FORWARD_ONLY, fade(t.text_faint), pos2(field.max.x + tk::ZONE_ROW_GAP, cy), inner.max.x);
    out.forward_only = r4(fo);
    y += layout::DENSE_CONTROL_HEIGHT;
    if let Some(wtext) = warning(state, z, inputs.names) {
        y += tk::ZONE_ROW_GAP;
        let font = tokens::LABEL_CONTROL.font_id(ui.ctx());
        let r = crate::chrome::status_text(&p, pos2(inner.min.x, y + warning_h(&p) / 2.0), &wtext, font, crate::chrome::Severity::Warning);
        out.warning = Some((wtext, r4(r)));
    }

    (out, intent)
}
