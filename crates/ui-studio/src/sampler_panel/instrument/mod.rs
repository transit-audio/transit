//! The Sampler's Instrument view (ADR 0041 and its amendments; README, Sampler views, Instrument
//! view and its round 2 decisions; the boards `SamplerInstrument` and `SamplerRound2`): the key map
//! ([`key_map`], its gestures in [`key_map_gestures`]), a refused edit's note under it, then the
//! zone row (the Zone panel, [`zone_panel`], with its waveform, [`zone_wave`]; the Group panel's
//! place, not built yet), the Pads view's Playback and Amp Envelope sections editing the selected
//! zone, and the Shaper and Modulation section bars (collapsed; their open state is a later board,
//! so they are drawn unavailable).
//!
//! Everything it shows comes from `mix.sampler_state` ([`state`]), read again only when the
//! document changes. The selected zone is UI state (changing views keeps it and changes nothing).
//! Each gesture sends one command when it completes, one undo step, with only what it changes;
//! the engine decides, and a refusal is its text in the note row. The keyboard: Escape cancels a
//! zone or handle drag (nothing is sent), never while typing, a menu or a modal is open, or another
//! window has the keyboard, and it never falls through to the arrangement.

pub mod key_map;
pub mod key_map_gestures;
pub mod state;
pub mod zone_panel;
pub mod zone_wave;

#[cfg(test)]
mod hardening_tests;
#[cfg(test)]
mod key_map_tests;
#[cfg(test)]
mod rig;
#[cfg(test)]
mod zone_panel_tests;

use super::{SamplerLayout, view};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::Dev;
use crate::shortcuts::{self, Action};
use crate::theme::TOKENS;
use crate::tokens::{self, SPACE, layout as space, radius, sampler as tk};
use crate::widgets::{self, r4, section_bar};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use key_map_gestures::{Drag, ZoneEdit};
use serde_json::{Value, json};
use transit_model::sampler_patch::LoopMode;
use zone_wave::{Handle, WaveDrag};

/// The tooltip of what is drawn but not built yet.
pub const NOT_YET: &str = "Not available yet";
/// The collapsed section bars under the sections row.
pub const BARS: [&str; 2] = ["Shaper", "Modulation"];

/// One instance's Instrument view: the selected zone (from 1), the last state read, the gestures in
/// progress, the crossfade field's typing, the last refusal and what the view sent.
#[derive(Debug, Clone, Default)]
pub struct InstrumentView {
    pub selected: Option<u16>,
    pub snapshot: state::Snapshot,
    pub drag: Option<Drag>,
    /// The last conflict check: the edits it was made for and its reason.
    pub conflict: Option<(Vec<ZoneEdit>, Option<String>)>,
    pub wave_drag: Option<WaveDrag>,
    pub crossfade_text: Option<String>,
    /// The last refused edit, in the engine's words (until the next edit that lands).
    pub note: Option<String>,
    /// Every command this view sent, latest last (bounded): `id {params}`.
    pub sent: Vec<String>,
    /// The body's height as last drawn.
    pub height: Option<f32>,
}

/// The view as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstrumentLayout {
    pub key_map: key_map::KeyMapLayout,
    pub note: Option<(String, [f32; 4])>,
    pub zone: Option<zone_panel::ZonePanelLayout>,
    /// The Group panel's card and its opacity (not built yet).
    pub group: [f32; 4],
    pub group_opacity: f32,
    /// The sections row.
    pub sections: [f32; 4],
    pub bars: Vec<section_bar::SectionBarLayout>,
    pub bar_opacity: f32,
}

/// A color the view names by its token.
pub(crate) fn color(name: &str) -> Color32 {
    let t = &TOKENS;
    match name {
        "surface.panel" => t.surface_panel,
        "surface.raised" => t.surface_raised,
        "surface.inset" => t.surface_inset,
        "line" => t.line,
        "line.inset" => t.line_inset,
        "line.strong" => t.line_strong,
        "text" => t.text,
        "text.muted" => t.text_muted,
        "text.faint" => t.text_faint,
        "ink.arrow" => t.ink_arrow,
        "ink.ghost" => t.ink_ghost,
        "ink.dark" => t.ink_dark,
        "bg.titlebar" => t.bg_titlebar,
        "signal" => t.signal,
        "meter.hot" => t.meter_hot,
        _ => Color32::MAGENTA,
    }
}

/// A control's tooltip naming its keys (`Move: drag; cancel: Escape`).
pub(crate) fn tip(what: &str, actions: &[Action]) -> String {
    let keys: Vec<String> = actions.iter().flat_map(|a| shortcuts::rows(*a)).map(shortcuts::text).collect();
    format!("{what}: {}", keys.join(" / "))
}

const SENT_KEPT: usize = 32;

fn refresh(app: &mut StudioApp, dev: Dev) {
    let mut snap = std::mem::take(&mut view(app, dev.uid()).instrument.snapshot);
    snap.refresh(app, dev.key.0);
    view(app, dev.uid()).instrument.snapshot = snap;
}

/// Runs `id` with `params` (the track added), logging it; a refusal becomes the note.
fn send(app: &mut StudioApp, dev: Dev, v: &mut InstrumentView, id: &str, mut params: Value) -> bool {
    if let Some(o) = params.as_object_mut() {
        o.insert("track".into(), json!(dev.key.0.0));
    }
    let mut logged = params.clone();
    if let Some(o) = logged.as_object_mut() {
        o.remove("track");
    }
    v.sent.push(format!("{id} {logged}"));
    if v.sent.len() > SENT_KEPT {
        v.sent.remove(0);
    }
    match app.execute_ui(id, &params) {
        Ok(_) => {
            v.note = None;
            true
        }
        Err(e) => {
            let text = super::slices::edit::words(&e);
            app.status = text.clone();
            v.note = Some(text);
            false
        }
    }
}

/// The Instrument view bar's summary (`24 zones · 12 groups · 2 layers · C1–B3 · 4 pad zones`):
/// the zones, the groups and the most layers one has, the keys the zones no pad binds cover, and
/// the pad zones; nothing without a state.
pub(crate) fn summary(app: &mut StudioApp, dev: Dev) -> String {
    refresh(app, dev);
    let mc = app.ui.middle_c;
    let Some(s) = view(app, dev.uid()).instrument.snapshot.data.as_ref() else { return String::new() };
    let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    let mut parts = vec![plural(s.zones.len(), "zone")];
    let groups: Vec<&state::Group> = s.groups.iter().filter(|g| !g.zones.is_empty()).collect();
    if !groups.is_empty() {
        parts.push(plural(groups.len(), "group"));
        let layers = groups
            .iter()
            .map(|g| {
                let mut l: Vec<u8> = s.zones.iter().filter_map(|z| z.membership.filter(|m| m.group == g.group).map(|m| m.layer)).collect();
                l.sort_unstable();
                l.dedup();
                l.len()
            })
            .max()
            .unwrap_or(0);
        parts.push(plural(layers, "layer"));
    }
    let unbound: Vec<&state::Zone> = s.zones.iter().filter(|z| z.pad.is_none()).collect();
    if let (Some(lo), Some(hi)) = (unbound.iter().map(|z| z.keys[0]).min(), unbound.iter().map(|z| z.keys[1]).max()) {
        parts.push(format!("{}–{}", transit_midi::note_name(lo, mc), transit_midi::note_name(hi, mc)));
    }
    let pads = s.zones.len() - unbound.len();
    if pads > 0 {
        parts.push(plural(pads, "pad zone"));
    }
    parts.join(" · ")
}

/// The body's height before it is first drawn (for the window's size).
pub(crate) fn height(p: &egui::Painter) -> f32 {
    key_map::height(p) + space::PANEL_GAP + zone_panel::height(p, false) + space::PANEL_GAP + super::zone::height(p) + bars_height()
}

fn bars_height() -> f32 {
    space::PANEL_GAP + 2.0 * section_bar::HEIGHT + SPACE[1]
}

/// The notes sounding now: the pads the live tap shows hit (the Pads view's fill).
fn held(app: &StudioApp, uid: u64, s: &state::State, now: f64) -> Vec<u8> {
    let Some(v) = app.builtin_editor.devices.sampler.views.get(&uid) else { return Vec::new() };
    v.live.iter().enumerate().filter(|(_, l)| l.look(now).0 > 0.0).filter_map(|(i, _)| s.pad_note(i as u8 + 1)).collect()
}

/// Draws the view's body into `rect` and returns its height.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut SamplerLayout) -> f32 {
    refresh(app, dev);
    let uid = dev.uid();
    let mc = app.ui.middle_c;
    let now = ui.input(|i| i.time);
    let mut v = std::mem::take(&mut view(app, uid).instrument);
    let mut lo = InstrumentLayout { bar_opacity: tk::UNUSED_CONTROL_ALPHA, group_opacity: tk::UNUSED_CONTROL_ALPHA, ..InstrumentLayout::default() };
    let data = v.snapshot.data.clone();
    let s = app.engine.session_arc();
    let p = ui.painter().clone();
    let mut y = rect.min.y;
    // The selected zone: the view's, else the lowest.
    if let Some(st) = &data
        && v.selected.is_none_or(|z| st.zone(z).is_none())
    {
        v.selected = st.zones.first().map(|z| z.zone);
    }
    let selected = data.as_ref().and_then(|st| v.selected.and_then(|n| st.zone(n)).cloned());
    // The key map.
    let kh = key_map::height(&p);
    if let Some(st) = &data {
        let inputs = key_map::Inputs { selected: v.selected, held: &held(app, uid, st, now), names: mc, session: &s };
        let (km, intent) = key_map::show(ui, uid, st, &inputs, &mut v.drag, &mut v.conflict, Rect::from_min_size(rect.min, vec2(rect.width(), kh)));
        lo.key_map = km;
        match intent {
            Some(key_map::Intent::Select(z)) => v.selected = Some(z),
            Some(key_map::Intent::Map(edits)) => {
                send(app, dev, &mut v, "mix.sampler_zone_map", key_map_gestures::params(&edits));
            }
            None => {}
        }
    }
    y += kh;
    let note = v.note.clone().or_else(|| data.is_none().then(|| v.snapshot.error.clone()).flatten());
    if let Some(n) = note {
        y += space::PANEL_GAP;
        let nh = super::note_height(&p, rect.width(), &n);
        let r = super::draw_note(ui, Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), nh)), &n);
        lo.note = Some((n, r4(r)));
        y += nh;
    }
    // The zone row: the Zone panel and the Group panel's place.
    y += space::PANEL_GAP;
    let [a, b] = tk::ZONE_COLUMNS;
    let zone_w = ((rect.width() - space::PANEL_GAP) * a / (a + b)).round();
    let warn = matches!((&data, &selected), (Some(st), Some(z)) if zone_panel::warning(st, z, mc).is_some());
    let zh = zone_panel::height(&p, warn);
    let zone_rect = Rect::from_min_size(pos2(rect.min.x, y), vec2(zone_w, zh));
    match (&data, &selected) {
        (Some(st), Some(z)) => {
            let (file, frames, audio) = source_of(&s, st, z);
            let inputs = zone_panel::PanelInputs { file, frames, audio: audio.as_deref(), names: mc };
            let (zl, intent) = zone_panel::show(ui, uid, st, z, &inputs, &mut v.wave_drag, &mut v.crossfade_text, zone_rect);
            lo.zone = Some(zl);
            if let Some(i) = intent {
                apply(app, dev, &mut v, z.zone, i);
            }
        }
        _ => {
            p.rect(zone_rect, radius::PANEL, TOKENS.surface_panel, Stroke::new(1.0, TOKENS.line), StrokeKind::Inside);
            let inner = zone_rect.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]));
            super::section_label(&p, inner.min, "Zone");
        }
    }
    let group = Rect::from_min_max(pos2(zone_rect.max.x + space::PANEL_GAP, y), pos2(rect.max.x, y + zh));
    lo.group = r4(group);
    {
        let a = tk::UNUSED_CONTROL_ALPHA;
        p.rect(group, radius::PANEL, TOKENS.surface_panel.gamma_multiply(a), Stroke::new(1.0, TOKENS.line.gamma_multiply(a)), StrokeKind::Inside);
        let inner = group.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]));
        widgets::role_text(&p, &tokens::LABEL_SECTION, "Group", TOKENS.text_muted.gamma_multiply(a), Align2::LEFT_TOP, inner.min);
        ui.interact(group, egui::Id::new(("sampler_group_panel", uid)), Sense::hover()).on_hover_text(NOT_YET);
    }
    y += zh + space::PANEL_GAP;
    // The sections row, editing the selected zone.
    let sh = super::zone::height(&p);
    let entry = selected.as_ref().and_then(|z| data.as_ref().and_then(|st| st.entry(z.zone)).cloned());
    let patch = data.as_ref().map(|st| st.patch.clone()).unwrap_or_default();
    let sections = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), sh));
    super::zone::show_zone(app, ui, dev, &patch, entry.as_ref(), sections, lay, out);
    lo.sections = r4(sections);
    y += sh + space::PANEL_GAP;
    // Shaper and Modulation: collapsed bars, not built yet.
    for (i, title) in BARS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), section_bar::HEIGHT));
        let id = egui::Id::new(("sampler_instrument_bar", uid, i));
        let bar = ui
            .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                ui.visuals_mut().disabled_alpha = tk::UNUSED_CONTROL_ALPHA;
                ui.disable();
                let b = section_bar::Bar { title, tag: None, description: None, summary: None, collapsed: true, enabled: None };
                section_bar::show(ui, id, r.width(), &b).layout
            })
            .inner;
        ui.interact(r, id.with("tip"), Sense::hover()).on_hover_text(NOT_YET);
        lo.bars.push(bar);
        y += section_bar::HEIGHT + if i == 0 { SPACE[1] } else { 0.0 };
    }
    v.height = Some(y - rect.min.y);
    // A drag lives only while a button is down: one whose release this view did not see (its
    // widget was not drawn that frame) is dropped here, never kept to take Escape or send later.
    if !ui.input(|i| i.pointer.any_down()) {
        v.drag = None;
        v.wave_drag = None;
    }
    view(app, uid).instrument = v;
    out.instrument = Some(lo);
    y - rect.min.y
}

/// The zone's file name, its source's length in frames and its audio.
fn source_of(s: &transit_model::Session, st: &state::State, z: &state::Zone) -> (String, u64, Option<std::sync::Arc<transit_model::SourceAudio>>) {
    let Some(id) = st.span(z).and_then(|sp| sp.source) else { return (String::new(), 0, None) };
    let audio = s.pool.get(id).cloned();
    let src = s.source(id);
    let name = src
        .map(|x| std::path::Path::new(&x.path).file_name().and_then(|n| n.to_str()).map_or_else(|| x.name.clone(), str::to_string))
        .unwrap_or_default();
    let frames = audio.as_ref().map(|a| a.frames() as u64).or_else(|| src.map(|x| x.frames)).unwrap_or(0);
    (name, frames, audio)
}

/// Sends what a completed Zone panel gesture asks for (one `mix.sampler_zone_edit`, only its field).
fn apply(app: &mut StudioApp, dev: Dev, v: &mut InstrumentView, zone: u16, intent: zone_panel::Intent) {
    let region = match intent {
        zone_panel::Intent::LoopMode(m) => json!({"loop_mode": match m {
            LoopMode::Off => "off",
            LoopMode::Forward => "forward",
            LoopMode::PingPong => "ping_pong",
        }}),
        zone_panel::Intent::Crossfade(x) => json!({"crossfade": x}),
        zone_panel::Intent::Region(Handle::Start, f) => json!({"start": f}),
        zone_panel::Intent::Region(Handle::End, f) => json!({"end": f}),
    };
    send(app, dev, v, "mix.sampler_zone_edit", json!({"zone": zone, "region": region}));
}

/// The Instrument view's keys on the focused Sampler: Escape cancels a zone or handle drag (see
/// the module notes). Runs before the device window's Escape, so the window stays.
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    let modal = ctx.memory(|m| m.top_modal_layer().is_some());
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || modal || egui::Popup::is_any_open(ctx) {
        return;
    }
    let shown = app.ui.plugin_windows.iter().copied().any(|key| app.builtin_at(key).is_some_and(|i| i.uid.get() == uid && super::handles(&i.plugin)));
    let Some(v) = app.builtin_editor.devices.sampler.views.get_mut(&uid) else { return };
    if !shown || v.view != super::View::Instrument {
        return;
    }
    let iv = &mut v.instrument;
    // A drag is live only while a button is down (a stale one never takes Escape from the window).
    let down = ctx.input(|i| i.pointer.any_down());
    if down && (iv.drag.is_some() || iv.wave_drag.is_some()) && shortcuts::pressed(ctx, Action::SamplerCancelZoneDrag) {
        if let Some(d) = iv.drag.as_mut() {
            d.cancelled = true;
        }
        if let Some(d) = iv.wave_drag.as_mut() {
            d.cancelled = true;
        }
    }
}
