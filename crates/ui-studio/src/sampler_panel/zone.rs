//! The sections row (README, Device anatomy C): the selected pad's zone basics, which ADR 0041's
//! panel note puts in the Pads view ("the selected pad's waveform and six controls beside it";
//! a pad edits its bound zone). Two panels named from the README's Sampler row:
//!
//! - **Playback:** Level and Pan (the zone slot's `zoneNNN_level` and `zoneNNN_pan`, through
//!   `mix.sampler_control`), Tune and Fine (the zone's transpose and cents), size M in knob slots
//!   of up to 84 px 8 apart (the Compressor's), then a 1 px `line` divider with 8 either side and a
//!   column with the trigger mode as a segmented control (One-shot, Gate) and Reverse and Key track
//!   as option toggles (through `mix.sampler_zone_edit`).
//! - **Amp Envelope:** Attack, Decay, Sustain, Release (`mix.sampler_zone_edit`).
//!
//! The ranges are ADR 0041's; the time knobs take the legacy Sampler's log tapers (their floors,
//! 0.1 ms and 1 ms, are the knobs'; a zone's zero is set by the command). A knob's drag or wheel
//! gesture is one undo step. With an empty pad selected the row is at 40 % (`alpha.disabledSection`)
//! and takes no input: there is no zone to edit until a sample is dropped on the pad.
//!
//! The waveform with start and end handles (the README's Sampler hero) waits for its board; so do
//! the shaper's filter and LFOs (the slot controls behind `zoneNNN_`).

use super::{KnobRecord, PadData, SamplerLayout, behavior, panel, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::{Dev, bipolar, child, knob_value};
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space};
use crate::widgets::{knob, r4, segmented, toggle};
use egui::{Rect, Stroke, Ui, pos2, vec2};
use transit_dsp::{ParamInfo, Taper, Unit};
use transit_model::sampler_patch::{PatchV1, PlayMode, ZoneEntry};

/// A knob's slot at the design width and the gap between slots (the Compressor's 84 and 8).
pub const SLOT: f32 = 84.0;
pub const SLOT_GAP: f32 = 8.0;
/// The divider and the room either side of it (the Macro Oscillator's Filter card's).
pub const DIVIDER_GAP: f32 = 8.0;
/// The Playback column's least width (the trigger segments at their text and the two toggles).
pub const COLUMN_MIN: f32 = 128.0;
/// The label above a knob row and the gap under it (the Macro Oscillator's sections).
pub const LABEL_GAP: f32 = 10.0;
/// The column's rows' gap.
pub const ROW_GAP: f32 = 8.0;
/// The trigger modes in the order of the ADR's mockup.
pub const MODES: [&str; 2] = ["One-shot", "Gate"];

const fn param(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, taper: Taper) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper, choices: &[], group: "", note_labels: None }
}

/// The zone fields the knobs edit (ADR 0041's ranges and new-pad defaults).
pub static TRANSPOSE: ParamInfo = param("transpose", "Tune", -48.0, 48.0, 0.0, Unit::Semitones, Taper::Linear);
pub static CENTS: ParamInfo = param("cents", "Fine", -100.0, 100.0, 0.0, Unit::Cents, Taper::Linear);
pub static ATTACK: ParamInfo = param("attack_ms", "Attack", 0.1, 5000.0, 1.0, Unit::Ms, Taper::Log);
pub static DECAY: ParamInfo = param("decay_ms", "Decay", 1.0, 10_000.0, 300.0, Unit::Ms, Taper::Log);
pub static SUSTAIN: ParamInfo = param("sustain", "Sustain", 0.0, 100.0, 100.0, Unit::Percent, Taper::Linear);
pub static RELEASE: ParamInfo = param("release_ms", "Release", 1.0, 10_000.0, 150.0, Unit::Ms, Taper::Log);

/// Which store a knob writes.
#[derive(Clone, Copy)]
enum Store {
    /// A slot control (`zoneNNN_<id>`), by its dsp field.
    Slot(usize),
    /// A zone playback field.
    Playback,
}

struct Field {
    info: &'static ParamInfo,
    label: &'static str,
    store: Store,
}

fn playback_fields() -> [Field; 4] {
    use transit_dsp::plugins::sampler::{FIELD_LEVEL, FIELD_PAN, field_info};
    let level = field_info(FIELD_LEVEL).unwrap_or(&TRANSPOSE);
    let pan = field_info(FIELD_PAN).unwrap_or(&TRANSPOSE);
    [
        Field { info: level, label: "Level", store: Store::Slot(FIELD_LEVEL) },
        Field { info: pan, label: "Pan", store: Store::Slot(FIELD_PAN) },
        Field { info: &TRANSPOSE, label: "Tune", store: Store::Playback },
        Field { info: &CENTS, label: "Fine", store: Store::Playback },
    ]
}

fn amp_fields() -> [Field; 4] {
    [
        Field { info: &ATTACK, label: "Attack", store: Store::Playback },
        Field { info: &DECAY, label: "Decay", store: Store::Playback },
        Field { info: &SUSTAIN, label: "Sustain", store: Store::Playback },
        Field { info: &RELEASE, label: "Release", store: Store::Playback },
    ]
}

/// The value a field reads from the zone (or its slot control in the patch).
fn read(patch: &PatchV1, zone: &ZoneEntry, f: &Field) -> f32 {
    let v = match f.store {
        Store::Slot(field) => {
            let id = transit_dsp::plugins::sampler::slot_id(usize::from(zone.id), field).unwrap_or_default();
            patch.controls.get(&id).copied().unwrap_or(f.info.default)
        }
        Store::Playback => match f.info.id {
            "transpose" => zone.transpose,
            "cents" => zone.cents,
            "attack_ms" => zone.attack_ms,
            "decay_ms" => zone.decay_ms,
            "sustain" => zone.sustain * 100.0,
            "release_ms" => zone.release_ms,
            _ => f.info.default,
        },
    };
    f.info.clamp(v)
}

/// The row's height.
pub fn height(p: &egui::Painter) -> f32 {
    2.0 * space::PANEL_PADDING[0] + super::line_h(p, &tokens::LABEL_SECTION) + LABEL_GAP + knob::cell(knob::Size::M).y
}

/// The knob slot at row width `w`: up to [`SLOT`], shrinking with the row once the column is at its
/// least.
pub fn slot_at(w: f32) -> f32 {
    let fixed = space::PANEL_GAP + 4.0 * space::PANEL_PADDING[1] + 2.0 * DIVIDER_GAP + 1.0 + COLUMN_MIN + 6.0 * SLOT_GAP;
    ((w - fixed) / 8.0).min(SLOT)
}

/// Draws the row into `rect`; returns its height.
pub(crate) fn show(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    patch: &PatchV1,
    pad: Option<&PadData>,
    rect: Rect,
    lay: &mut EditorLayout,
    out: &mut SamplerLayout,
) -> f32 {
    let t = &TOKENS;
    let zone = pad.and_then(|d| d.zone.clone());
    let slot = slot_at(rect.width());
    let knobs_w = 4.0 * slot + 3.0 * SLOT_GAP;
    let amp_w = knobs_w + 2.0 * space::PANEL_PADDING[1];
    let play_w = rect.width() - space::PANEL_GAP - amp_w;
    let play_r = Rect::from_min_size(rect.min, vec2(play_w, rect.height()));
    let amp_r = Rect::from_min_size(pos2(play_r.max.x + space::PANEL_GAP, rect.min.y), vec2(amp_w, rect.height()));
    let play_in = panel(ui, play_r, "playback", out);
    let amp_in = panel(ui, amp_r, "amp", out);
    let mut body = child(ui, rect);
    if zone.is_none() {
        body.multiply_opacity(tokens::alpha::DISABLED_SECTION);
        body.disable();
    }
    let p = body.painter().clone();
    let label_h = super::line_h(&p, &tokens::LABEL_SECTION);
    section_label(&p, play_in.min, "Playback");
    section_label(&p, amp_in.min, "Amp Envelope");
    let row_y = play_in.min.y + label_h + LABEL_GAP;
    let shown = zone.clone().unwrap_or_default();
    out.knobs.clear();
    for (row_x, fields) in [(play_in.min.x, playback_fields()), (amp_in.min.x, amp_fields())] {
        for (i, f) in fields.iter().enumerate() {
            let at = pos2(row_x + i as f32 * (slot + SLOT_GAP), row_y);
            knob_at(app, &mut body, dev, patch, &shown, zone.is_some(), f, at, slot, lay, out);
        }
    }
    // The divider and the column.
    let dx = play_in.min.x + knobs_w + DIVIDER_GAP;
    let cell_h = knob::cell(knob::Size::M).y;
    p.line_segment([pos2(dx + 0.5, row_y), pos2(dx + 0.5, row_y + cell_h)], Stroke::new(1.0, t.line));
    out.divider = r4(Rect::from_min_size(pos2(dx, row_y), vec2(1.0, cell_h)));
    let cx = dx + 1.0 + DIVIDER_GAP;
    let col_w = (play_in.max.x - cx).max(0.0);
    let mut c = child(&mut body, Rect::from_min_size(pos2(cx, row_y), vec2(col_w, segmented::HEIGHT)));
    let mode = segmented::show(&mut c, egui::Id::new(("sampler_mode", dev.uid())), &MODES, usize::from(shown.mode == PlayMode::Gate), Some(col_w));
    let ty = row_y + segmented::HEIGHT + ROW_GAP;
    let rw = toggle::width(&body, "Reverse");
    let mut c = child(&mut body, Rect::from_min_size(pos2(cx, ty), vec2(rw, tokens::toggle::HEIGHT)));
    let rev =
        toggle::show(&mut c, egui::Id::new(("sampler_reverse", dev.uid())), "Reverse", toggle::Role::Option, shown.reverse, zone.is_some(), None);
    let kw = toggle::width(&body, "Key track");
    let mut c = child(&mut body, Rect::from_min_size(pos2(cx + rw + SLOT_GAP, ty), vec2(kw, tokens::toggle::HEIGHT)));
    let key = toggle::show(
        &mut c,
        egui::Id::new(("sampler_key_track", dev.uid())),
        "Key track",
        toggle::Role::Option,
        shown.key_tracking,
        zone.is_some(),
        None,
    );
    out.mode = Some(mode.layout);
    out.reverse = Some(rev.layout);
    out.key_track = Some(key.layout);
    if let Some(z) = &zone {
        if let Some(m) = mode.picked {
            behavior::zone_once(app, dev, z.id, serde_json::json!({"playback": {"mode": if m == 1 { "gate" } else { "one_shot" }}}));
        }
        if rev.toggled {
            behavior::zone_once(app, dev, z.id, serde_json::json!({"playback": {"reverse": !z.reverse}}));
        }
        if key.toggled {
            behavior::zone_once(app, dev, z.id, serde_json::json!({"playback": {"key_tracking": !z.key_tracking}}));
        }
    }
    rect.height()
}

/// The id of zone field `field`'s knob on the insert `uid` (its value field is `id.with("editing")`).
pub fn knob_id(uid: u64, field: &str) -> egui::Id {
    egui::Id::new(("sampler_knob", uid, field))
}

/// One knob (size M) centered in its slot at `at`.
#[allow(clippy::too_many_arguments)]
fn knob_at(
    app: &mut StudioApp,
    ui: &mut Ui,
    dev: Dev,
    patch: &PatchV1,
    zone: &ZoneEntry,
    live: bool,
    f: &Field,
    at: egui::Pos2,
    slot: f32,
    lay: &mut EditorLayout,
    out: &mut SamplerLayout,
) {
    let size = knob::Size::M;
    let cw = knob::cell(size).x;
    let cell = Rect::from_min_size(pos2(at.x + (slot - cw) / 2.0, at.y), knob::cell(size));
    let p = f.info;
    let v = read(patch, zone, f);
    let text = crate::middle_c::param_text(p, v, app.ui.middle_c);
    let k = knob::Knob {
        size,
        label: f.label,
        value_text: &text,
        value: p.to_normalized(v),
        default: p.to_normalized(p.default),
        bipolar: bipolar(p),
        modulation: None,
        disabled: !live,
    };
    let mut c = child(ui, cell);
    let shown = knob::show(&mut c, knob_id(dev.uid(), p.id), &k);
    let new = if live { knob_value(p, &shown, app.ui.middle_c) } else { None };
    if shown.wheeled {
        app.builtin_editor.devices.wheel_now = true;
    }
    let mut value = text.clone();
    if let Some(n) = new.map(|n| p.clamp(n)).filter(|n| (n - v).abs() > f32::EPSILON) {
        match f.store {
            Store::Slot(field) => {
                if let Some(id) = transit_dsp::plugins::sampler::slot_id(usize::from(zone.id), field) {
                    behavior::control(app, dev, &id, n);
                }
            }
            Store::Playback => {
                let x = if p.id == "sustain" { n / 100.0 } else { n };
                let mut fields = serde_json::Map::new();
                fields.insert(p.id.to_string(), serde_json::json!(x));
                behavior::zone_merged(app, dev, zone.id, p.id, serde_json::json!({ "playback": fields }));
            }
        }
        value = crate::middle_c::param_text(p, n, app.ui.middle_c);
    }
    lay.rows.push(crate::builtin_editor::RowLayout { param: format!("zone{:03}_{}", zone.id + 1, p.id), control: r4(cell), value: value.clone() });
    out.knobs.push(KnobRecord { field: p.id.to_string(), label: f.label.to_string(), value, knob: shown.layout });
}
