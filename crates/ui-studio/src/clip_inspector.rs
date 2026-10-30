//! The clip inspector's fade section (#182): Fade In, Fade Out and Crossfade lengths in the
//! session's time format, and the curve of each fade as a small popup.
//!
//! Convention: Logic's region inspector carries the same fields (Fade In, Fade Out, Fade
//! Curve), and Ableton's clip view puts its fade lengths in numeric boxes too; both type a
//! length and press Return, and the clip follows. A strip under the arrangement shows while one
//! audio clip is selected. Lengths are durations, so they read in the main counter's format
//! counted from zero (Bars|Beats as `0| 1| 000`: zero bars, one beat, no ticks), never offset by
//! the session's timecode start.
//!
//! Every edit goes through the same commands as the handles: `edit.fades_create` for a fade's
//! length and curve (via [`crate::fades`]), one undo step per edit. Typing zero removes a fade.

use crate::StudioApp;
use crate::arrange::R4;
use crate::fades::{self, Live, Plan, Side};
use crate::theme::TOKENS;
use egui::{Align, Layout, Ui};
use transit_model::{ClipContent, ClipId, Fade, FadeShape, Session};
use transit_time::{BarBeat, Samples, TimeFormat};

/// Height of the inspector strip (points).
pub const HEIGHT: f32 = 30.0;
/// Width of a length field (points).
const FIELD_W: f32 = 96.0;
/// What a crossfade field shows when its two ends differ in length.
pub const CUSTOM: &str = "Custom";

/// A numeric field of the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Field {
    FadeIn,
    FadeOut,
    Crossfade,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::FadeIn => "Fade In",
            Field::FadeOut => "Fade Out",
            Field::Crossfade => "Crossfade",
        }
    }

    fn side(self) -> Option<Side> {
        match self {
            Field::FadeIn => Some(Side::In),
            Field::FadeOut => Some(Side::Out),
            Field::Crossfade => None,
        }
    }
}

/// A field as drawn last frame (tests and `ui.inspect` read these).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldView {
    pub field: Field,
    /// What the field shows: the length in the session's time format, or what is being typed.
    pub text: String,
    pub rect: R4,
    /// The fade's curve, for the fields that have one.
    pub curve: Option<&'static str>,
    pub curve_rect: R4,
}

/// The inspector's state: the field being typed in, and last frame's drawing.
#[derive(Debug, Default)]
pub struct State {
    /// The field being typed in and its text.
    pub draft: Option<(Field, String)>,
    /// The clip the strip shows (none: hidden) and its fields.
    pub clip: Option<ClipId>,
    pub fields: Vec<FieldView>,
}

/// The clip the inspector shows: the one audio clip selected.
pub fn target(s: &Session) -> Option<ClipId> {
    match s.edit.selected_clips.as_slice() {
        [id] => matches!(s.find_clip(*id)?.1.content, ClipContent::Audio { .. }).then_some(*id),
        _ => None,
    }
}

/// A duration in `fmt`, counted from zero.
pub fn format_len(s: &Session, len: Samples, fmt: TimeFormat) -> String {
    if fmt == TimeFormat::BarsBeats {
        let bb = s.tempo.bar_beat_at(len, s.sample_rate);
        return format!("{}| {}| {:03}", bb.bar - 1, bb.beat - 1, bb.tick);
    }
    transit_time::format_position(len, fmt, s.sample_rate, &s.tempo, s.frame_rate, 0, 0)
}

/// A duration typed in `fmt`, counted from zero (the inverse of [`format_len`]).
pub fn parse_len(s: &Session, text: &str, fmt: TimeFormat) -> Result<Samples, String> {
    let bad = || format!("can't read \"{}\" as {}", text.trim(), fmt.label());
    if fmt == TimeFormat::BarsBeats {
        let parts: Vec<i64> = text.split('|').map(|p| p.trim().parse::<i64>()).collect::<Result<_, _>>().map_err(|_| bad())?;
        let [bars, beats, ticks] = parts[..] else { return Err(bad()) };
        if bars < 0 || beats < 0 || ticks < 0 {
            return Err(bad());
        }
        return Ok(s.tempo.samples_at_bar_beat(BarBeat { bar: bars + 1, beat: beats + 1, tick: ticks }, s.sample_rate));
    }
    transit_time::parse_position(text, fmt, s.sample_rate, &s.tempo, s.frame_rate, 0, 0)
        .map_err(|_| bad())
        .and_then(|v| if v < 0 { Err(bad()) } else { Ok(v) })
}

/// A field's length: a fade's, or the crossfade's when both ends agree (`None` when they differ
/// or the clip overlaps nothing).
pub fn length(s: &Session, clip: ClipId, field: Field) -> Option<Samples> {
    let c = s.find_clip(clip)?.1;
    match field.side() {
        Some(side) => Some(side.of(c).len),
        None => {
            let o = fades::overlap(s, clip)?;
            let (l, r) = (s.find_clip(o.left)?.1, s.find_clip(o.right)?.1);
            (l.fade_out.len == r.fade_in.len).then_some(l.fade_out.len)
        }
    }
}

/// Apply a typed length to a field: a fade's own length (the other end of a crossfade stays), or
/// both ends of the crossfade. Zero removes a fade. Refused with a message in the status line.
pub fn apply(app: &mut StudioApp, clip: ClipId, field: Field, text: &str) -> Result<(), String> {
    let fmt = app.engine.session().edit.main_counter;
    let len = parse_len(app.engine.session(), text, fmt).map_err(|e| format!("{}: {e}", field.label()));
    let result = len.and_then(|len| set(app, clip, field, len));
    if let Err(e) = &result {
        app.status = e.clone();
    }
    result
}

fn set(app: &mut StudioApp, clip: ClipId, field: Field, len: Samples) -> Result<(), String> {
    let s = app.engine.session();
    let Some(side) = field.side() else {
        let o = fades::overlap(s, clip).ok_or("Crossfade: this clip overlaps no other")?;
        if len <= 0 {
            return Err("Crossfade: type a length above zero (the fades stay as they are)".into());
        }
        fades::crossfade_len(app, o, len);
        return Ok(());
    };
    let base = s.find_clip(clip).map(|c| side.of(c.1)).ok_or("clip not found")?;
    // One side only: the inspector edits the number in front of it.
    let plan = fades::plan(s, clip, side, len, true);
    if len <= 0 && base.len > 0 && matches!(plan, Plan::None) {
        return Err(format!("{}: a fade in an overlap stays; shorten it, or move the clips apart first", field.label()));
    }
    let fade = match &plan {
        Plan::Set(v) => v.first().map_or(base, |x| x.2),
        _ => Fade { len: 0, ..base },
    };
    fades::commit(app, &Live { clip, side, fade, plan, curve: false });
    Ok(())
}

/// The strip under the arrangement, while one audio clip is selected. Call before the central
/// panel, with `roll` set while the piano roll covers the bottom.
pub fn panel(app: &mut StudioApp, ui: &mut Ui, roll: bool) {
    app.inspector.clip = None;
    app.inspector.fields.clear();
    if roll || app.ui.mixer_full {
        return;
    }
    let s = app.engine.session_arc();
    let Some(clip) = target(&s) else {
        app.inspector.draft = None;
        return;
    };
    app.inspector.clip = Some(clip);
    egui::Panel::bottom("clip_inspector").exact_size(HEIGHT).frame(egui::Frame::NONE.fill(TOKENS.panel)).show(ui, |ui| row(app, ui, &s, clip));
}

fn row(app: &mut StudioApp, ui: &mut Ui, s: &Session, clip: ClipId) {
    let r = ui.max_rect();
    ui.painter().line_segment([r.left_top(), r.right_top()], egui::Stroke::new(1.0, TOKENS.line));
    let fmt = s.edit.main_counter;
    let Some(c) = s.find_clip(clip).map(|x| x.1.clone()) else { return };
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(&c.name).color(TOKENS.text));
        for field in [Field::FadeIn, Field::FadeOut, Field::Crossfade] {
            ui.add_space(14.0);
            let value = length(s, clip, field);
            let enabled = field != Field::Crossfade || fades::overlap(s, clip).is_some();
            ui.label(egui::RichText::new(field.label()).color(TOKENS.text_dim));
            let editing = app.inspector.draft.as_ref().is_some_and(|d| d.0 == field);
            let mut text = match (&app.inspector.draft, value) {
                (Some((f, t)), _) if *f == field => t.clone(),
                (_, Some(len)) => format_len(s, len, fmt),
                (_, None) if enabled => CUSTOM.to_string(),
                _ => "-".to_string(),
            };
            let resp =
                ui.add_enabled(enabled, egui::TextEdit::singleline(&mut text).id(ui.id().with(("fade_len", field.label()))).desired_width(FIELD_W));
            if resp.gained_focus() || resp.changed() {
                app.inspector.draft = Some((field, text.clone()));
            }
            if editing && resp.lost_focus() {
                // Enter or a click elsewhere commits what was typed (if it differs from what the
                // field showed); Escape gives it up.
                let typed = app.inspector.draft.take().map(|d| d.1);
                let shown = value.map(|len| format_len(s, len, fmt));
                if let Some(t) = typed.filter(|t| !ui.input(|i| i.key_pressed(egui::Key::Escape)) && shown.as_deref() != Some(t.trim())) {
                    let _ = apply(app, clip, field, &t);
                }
            }
            let mut view = FieldView { field, text: text.clone(), rect: crate::arrange::r4(resp.rect), curve: None, curve_rect: [0.0; 4] };
            if let Some(side) = field.side() {
                let fade = side.of(&c);
                let mut shape = fade.shape;
                let combo = ui.add_enabled_ui(fade.len > 0, |ui| {
                    egui::ComboBox::from_id_salt(("fade_curve", side.label())).width(96.0).selected_text(shape.label()).show_ui(ui, |ui| {
                        for option in FadeShape::ALL {
                            ui.selectable_value(&mut shape, option, option.label());
                        }
                    })
                });
                view.curve = Some(fade.shape.label());
                view.curve_rect = crate::arrange::r4(combo.inner.response.rect);
                if shape != fade.shape {
                    fades::set_shape(app, clip, side, shape);
                }
            }
            app.inspector.fields.push(view);
        }
    });
}
