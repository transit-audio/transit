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
//!
//! States (README, Sampler views, States): with no zone the key map's well is the drop target
//! (`+ Drop a sample` and Load…, or a file dropped anywhere on the view: one
//! `mix.sampler_zone_source` into the lowest empty zone), the Zone and Group cards ghosted and the
//! sections at 40 percent; a refused file is the engine's text in the note row, nothing loaded;
//! the zone waveform reads `Analyzing…` while its source's peaks are not there, and hatches with
//! a card naming the file when its audio is missing (the zone, its id and its round-robin place
//! kept). A legacy one-sample patch shows the shared New Kit notice ([`super::legacy`]).

pub mod fade_fields;
pub mod group_actions;
pub mod group_panel;
pub mod key_map;
pub mod key_map_gestures;
pub mod keyboard;
pub mod state;
pub mod zone_panel;
pub mod zone_wave;

#[cfg(test)]
mod fades_tests;
#[cfg(test)]
mod followup_tests;
#[cfg(test)]
mod group_panel_tests;
#[cfg(test)]
mod hardening_tests;
#[cfg(test)]
mod key_map_tests;
#[cfg(test)]
mod keyboard_tests;
#[cfg(test)]
mod reach_tests;
#[cfg(test)]
mod rig;
#[cfg(test)]
mod states_tests;
#[cfg(test)]
mod typing_tests;
#[cfg(test)]
mod zone_panel_tests;

use super::{SamplerLayout, view};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::Dev;
use crate::shortcuts::{self, Action};
use crate::theme::TOKENS;
use crate::tokens::{SPACE, layout as space, radius, sampler as tk};
use crate::widgets::{r4, section_bar};
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use key_map_gestures::{Drag, ZoneEdit};
pub(crate) use keyboard::keys;
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
    /// The Zone panel's typing (the crossfade and fades fields).
    pub typed: zone_panel::Typing,
    /// The last refused edit, in the engine's words (until the next edit that lands).
    pub note: Option<String>,
    /// Every command this view sent, latest last (bounded): `id {params}` (the tests read it).
    #[cfg(test)]
    pub sent: Vec<String>,
    /// The body's height as last drawn.
    pub height: Option<f32>,
    /// The pad zone the add chip asked about (the Group panel's Notice shows until an answer or
    /// another zone is selected).
    pub ask: Option<u16>,
    /// The seed field's typing and the velocity fader's value while dragged.
    pub seed_text: Option<String>,
    pub fader: Option<f32>,
    /// The waveform handle the nudges move: the last one pressed (S until then).
    pub handle: Option<Handle>,
    /// The zone Delete asked to remove (the question shows until it is answered).
    pub confirm: Option<u16>,
    /// The seed reroll's entropy in tests (empty: the UI's own).
    #[cfg(test)]
    pub entropy: Vec<u32>,
}

/// The view as last drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstrumentLayout {
    pub key_map: key_map::KeyMapLayout,
    pub note: Option<(String, [f32; 4])>,
    pub zone: Option<zone_panel::ZonePanelLayout>,
    /// The Group panel's card and what it drew.
    pub group: [f32; 4],
    pub group_panel: Option<group_panel::GroupPanelLayout>,
    /// The sections row.
    pub sections: [f32; 4],
    pub bars: Vec<section_bar::SectionBarLayout>,
    pub bar_opacity: f32,
    /// The Zone and Group cards' opacity (ghosted with no zone).
    pub cards_opacity: f32,
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

#[cfg(test)]
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
    #[cfg(test)]
    {
        let mut logged = params.clone();
        if let Some(o) = logged.as_object_mut() {
            o.remove("track");
        }
        v.sent.push(format!("{id} {logged}"));
        if v.sent.len() > SENT_KEPT {
            v.sent.remove(0);
        }
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
    let mut lo = InstrumentLayout { bar_opacity: tk::UNUSED_CONTROL_ALPHA, cards_opacity: 1.0, ..InstrumentLayout::default() };
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
            Some(key_map::Intent::Ask(z)) => {
                v.selected = Some(z);
                v.ask = Some(z);
            }
            Some(key_map::Intent::Map(edits)) => {
                send(app, dev, &mut v, "mix.sampler_zone_map", key_map_gestures::params(&edits));
            }
            Some(key_map::Intent::Load) => {
                if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("mix.sampler_zone_source", None)) {
                    load(app, dev, &mut v, st, &path);
                }
            }
            None => {}
        }
        // The empty view is the drop target: a file dropped anywhere on it loads as a new zone.
        if st.zones.is_empty()
            && let Some(offer) = crate::drops::take(app, dev.key)
            && let Some(path) = offer.paths.iter().find(|p| p.is_absolute() && crate::builtin_editor::is_audio_name(&p.to_string_lossy()))
        {
            load(app, dev, &mut v, st, &path.to_string_lossy());
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
    // The zone row: the Zone panel and the Group panel, as tall as the taller.
    y += space::PANEL_GAP;
    let [a, b] = tk::ZONE_COLUMNS;
    let zone_w = ((rect.width() - space::PANEL_GAP) * a / (a + b)).round();
    let group_w = rect.width() - zone_w - space::PANEL_GAP;
    let warn = matches!((&data, &selected), (Some(st), Some(z)) if zone_panel::warning(st, z, mc).is_some());
    let asking = selected.as_ref().is_some_and(|z| v.ask == Some(z.zone) && z.pad.is_some());
    // The question goes with its zone: another selected zone drops it.
    if v.ask != v.selected {
        v.ask = None;
    }
    let ask_text = selected.as_ref().and_then(|z| z.pad.filter(|_| asking)).map(|pad| group_panel::ask_text(&super::pad_id(pad - 1)));
    let rows = match (&data, &selected) {
        (Some(st), Some(z)) => group_panel::lines(&p, group_w, &group_panel::layers(st, z)),
        _ => 1,
    };
    let zh = zone_panel::height(&p, warn).max(group_panel::height(&p, group_w, rows, ask_text.as_deref()));
    let zone_rect = Rect::from_min_size(pos2(rect.min.x, y), vec2(zone_w, zh));
    let group = Rect::from_min_size(pos2(zone_rect.max.x + space::PANEL_GAP, y), vec2(group_w, zh));
    lo.group = r4(group);
    // The view's typing key: set while a field has the keyboard, where the device window's Escape
    // rule gives typing up (`<crossfade id>.editing`), so a field that lets go finds it gone then.
    let edit_id = zone_panel::crossfade_id(uid).with("editing");
    let kept = ui.data(|d| d.get_temp::<String>(edit_id).is_some());
    let mut focused = false;
    match (&data, &selected) {
        (Some(st), Some(z)) => {
            let (file, frames, audio, missing) = source_of(&s, st, z);
            let inputs = zone_panel::PanelInputs { file, frames, audio: audio.as_deref(), names: mc, missing };
            let (zl, intent) = zone_panel::show(ui, uid, st, z, &inputs, &mut v.wave_drag, &mut v.typed, kept, zone_rect);
            focused |= zl.focused;
            if let Some(h) = zl.wave.touched {
                v.handle = Some(h);
            }
            lo.zone = Some(zl);
            if let Some(i) = intent {
                apply(app, dev, &mut v, z.zone, i);
            }
            let gi = group_panel::GroupInputs { names: mc, asking, seed_text: &mut v.seed_text, fader: &mut v.fader, kept };
            let (gl, intent) = group_panel::show(ui, uid, st, z, gi, group);
            focused |= ui.memory(|m| m.has_focus(group_panel::seed_id(uid)));
            if let Some(zl) = lo.zone.as_mut() {
                zl.typing |= gl.typing;
            }
            lo.group_panel = Some(gl);
            if let Some(i) = intent {
                group_actions::apply(app, dev, &mut v, st, z, i);
            }
        }
        _ => {
            // No zone to edit: the cards ghosted (README, States, "empty": controls ghosted).
            lo.cards_opacity = tk::UNUSED_CONTROL_ALPHA;
            let mut gp = p.clone();
            gp.multiply_opacity(lo.cards_opacity);
            for (r, name) in [(zone_rect, "Zone"), (group, "Group")] {
                gp.rect(r, radius::PANEL, TOKENS.surface_panel, Stroke::new(1.0, TOKENS.line), StrokeKind::Inside);
                let inner = r.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]));
                super::section_label(&gp, inner.min, name);
            }
        }
    }
    if focused {
        ui.data_mut(|d| d.insert_temp(edit_id, String::new()));
    } else {
        ui.data_mut(|d| d.remove::<String>(edit_id));
    }
    y += zh + space::PANEL_GAP;
    // The sections row, editing the selected zone.
    let sh = super::zone::height(&p);
    let entry = selected.as_ref().and_then(|z| data.as_ref().and_then(|st| st.entry(z.zone)).cloned());
    let patch = data.as_ref().map(|st| st.patch.clone()).unwrap_or_default();
    let sections = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), sh));
    // A knob's value typed for the zone shown before is dropped by the shared knob itself (it
    // remembers the zone it was typed for), never committed to this one.
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

/// The empty view's Load… or a dropped file: one `mix.sampler_zone_source` with the file's path
/// into the lowest empty zone (the UI's default, ADR 0041 amendment 2026-11-08); the engine sets
/// the rest. The new zone is selected; a refusal is the note row and nothing is loaded.
fn load(app: &mut StudioApp, dev: Dev, v: &mut InstrumentView, st: &state::State, path: &str) {
    let zone = group_actions::free_zone(st).unwrap_or(transit_model::sampler_patch::ZONES as u16 + 1);
    if send(app, dev, v, "mix.sampler_zone_source", json!({"zone": zone, "path": path})) {
        v.selected = Some(zone);
    }
}

/// The zone's file name, its source's length in frames, its audio and whether its audio is
/// missing from the session (a source the session lists with no audio in its pool).
type SourceOf = (String, u64, Option<std::sync::Arc<transit_model::SourceAudio>>, bool);

fn source_of(s: &transit_model::Session, st: &state::State, z: &state::Zone) -> SourceOf {
    let Some(id) = st.span(z).and_then(|sp| sp.source) else { return (String::new(), 0, None, false) };
    let audio = s.pool.get(id).cloned();
    let src = s.source(id);
    let name = src
        .map(|x| std::path::Path::new(&x.path).file_name().and_then(|n| n.to_str()).map_or_else(|| x.name.clone(), str::to_string))
        .unwrap_or_default();
    let frames = audio.as_ref().map(|a| a.frames() as u64).or_else(|| src.map(|x| x.frames)).unwrap_or(0);
    let missing = !s.pool.contains(id);
    (name, frames, audio, missing)
}

/// Sends what a completed Zone panel gesture asks for (one `mix.sampler_zone_edit`, only its field).
fn apply(app: &mut StudioApp, dev: Dev, v: &mut InstrumentView, zone: u16, intent: zone_panel::Intent) {
    let region = match intent {
        zone_panel::Intent::LoopMode(m) => json!({"loop_mode": match m {
            LoopMode::Off => "off",
            LoopMode::Forward => "forward",
            LoopMode::PingPong => "ping_pong",
        }}),
        zone_panel::Intent::Crossfade(n, x) => {
            send(app, dev, v, "mix.sampler_zone_edit", json!({"zone": n, "region": {"crossfade": x}}));
            return;
        }
        zone_panel::Intent::Fades(n, f) => {
            send(app, dev, v, "mix.sampler_zone_edit", json!({"zone": n, "playback": {"fades": f}}));
            return;
        }
        zone_panel::Intent::Invalid(reason) => {
            v.note = Some(reason);
            return;
        }
        zone_panel::Intent::Region(Handle::Start, f) => json!({"start": f}),
        zone_panel::Intent::Region(Handle::End, f) => json!({"end": f}),
    };
    send(app, dev, v, "mix.sampler_zone_edit", json!({"zone": zone, "region": region}));
}
