//! The Sampler's editor in the design language (#326, job 326-S; the sampler is #269, ADR 0041):
//! the Pads view, the first of the Sampler's three views, in the shared device window
//! ([`crate::device_editor::window`], content-sized, no resize handle). The Slices and Instrument
//! views sit behind the shared view switch: Slices shows its bench ([`slices`]), Instrument its key
//! map and Zone panel ([`instrument`]).
//!
//! Anatomy (README, Device anatomy; the Pad component and the board "Pads with choke groups"):
//! A, the shared 52 px title bar; the view bar ([`bar`]: the summary and the kit's Start note
//! field); B, the hero row: the Pads panel ([`grid`]: the `PADS` label, the
//! bank buttons for the four banks the patch has, the 4 by 4 grid of 56 px pads with 4 px gaps,
//! pad 1 bottom-left, and the bank's status line) and beside it the Selected pad panel
//! ([`strip`]: the pad's name, id and trigger note, the choke picker as a segmented grid of Off and
//! 1 to 16, the peers line, the M and S toggles); a refused edit's note under them; C, the
//! sections row ([`zone`]): the selected pad's zone basics that ADR 0041's panel note puts in the
//! Pads view, Playback (Level, Pan, Tune, Fine, the trigger mode, Reverse, Key track) and Amp
//! Envelope (Attack, Decay, Sustain, Release); the footer with the load readout.
//!
//! Every Sampler is a kit (ADR 0041 amendment 2026-11-11, #455): an instance without a patch, or
//! with one this build cannot edit, shows the kept-as-saved note and plays silence.
//!
//! Every edit goes through the C2 commands ([`behavior`]), one undo step per gesture; the live
//! state (the hit fill, the cut) comes from what the instrument publishes through its tap
//! ([`live`]). The bank, the selection and the view are UI state, never playback switches.

pub mod bar;
mod behavior;
mod grid;
pub mod instrument;
mod kit_load;
pub mod kit_presets;
mod live;
pub(crate) mod pad_actions;
pub mod slices;
mod strip;
mod view_bar;
pub mod window;
mod zone;

#[cfg(test)]
mod bar_tests;
#[cfg(test)]
mod behavior_tests;
#[cfg(test)]
mod drop_routing_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod kit_load_tests;
#[cfg(test)]
mod kit_presets_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod new_sampler_tests;
#[cfg(test)]
mod pad_actions_tests;
#[cfg(test)]
mod test_rig;
#[cfg(test)]
mod view_bar_tests;
#[cfg(test)]
mod width_tests;
#[cfg(test)]
mod window_tests;

use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::Dev;
use crate::device_editor::title_bar::TitleLayout;
use crate::theme::TOKENS;
use crate::tokens::{self, layout as space, radius};
use crate::widgets::{self, knob, pad, r4, segmented, toggle};
use egui::{Align2, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::collections::HashMap;
use transit_model::sampler_patch::{PADS, PadEntry, PatchV1, ZoneEntry};
use transit_model::{Session, TrackId};

pub use behavior::pad_name;
pub use live::Live;
pub use view_bar::View;

/// The Sampler's plug-in id.
pub const SAMPLER: &str = "sampler";

/// The content's width with every column at its natural size: the Pads panel, the strip and the
/// sections row at their knob slots (there is no device board for the Sampler; the window lays out
/// at the width its content asks for). Narrower windows shrink the strip and the knob slots down to
/// [`MIN_WIDTH`].
pub const DESIGN_WIDTH: f32 = 976.0;
/// The narrowest the window's content lays out; below it the window keeps this width. Set by the
/// content (`width_tests`, the 326-X method): the narrowest multiple of 8 at which, with every
/// knob at the widest value it prints, no label, readout or dial meets its neighbor or leaves its
/// panel, every choke and trigger segment's text fits, the strip's hint clears the toggles, and the
/// title bar's preset browser keeps clear of the brand and the track, and the view bar's widest
/// summary keeps a bar gap before the Start note label. The view bar sets it since Expand to
/// rack… joined its actions (the title bar alone last fit at 832, the knob readouts at 616).
pub const MIN_WIDTH: f32 = 856.0;
/// The footer under the content.
pub const FOOTER_H: f32 = 32.0;
/// Banks of sixteen pads.
pub const BANKS: usize = PADS / 16;

/// Whether `plugin`'s editor is this panel.
pub fn handles(plugin: &str) -> bool {
    plugin == SAMPLER
}

/// The pad id: bank and slot (`A6`; pad index 0 is A1, 16 is B1).
pub fn pad_id(index: u8) -> String {
    format!("{}{}", char::from(b'A' + (index / 16).min(25)), index % 16 + 1)
}

/// One instance's Pads view: the shown bank, the selected pad, each pad's live state, the note of
/// the last refused edit and the pad a drag started on.
#[derive(Debug, Clone)]
pub struct PadsView {
    pub view: View,
    pub kit_preview: Option<kit_load::Preview>,
    /// The kit presets: the last recall or save and a recall waiting for its question.
    pub kit: kit_presets::KitState,
    pub bank: usize,
    pub selected: u8,
    pub live: [Live; PADS],
    /// The last refused edit's message (shown until the next edit that lands).
    pub note: Option<String>,
    /// The pad a drag started on (a swap on release over another pad).
    pub drag: Option<u8>,
    /// The note an audition holds (released with the pointer, or the key on the Slices view) and
    /// the track it plays on.
    pub held: Option<u8>,
    pub held_track: Option<TrackId>,
    /// Auditions so far and the last one as (note, velocity): with or without a player.
    pub auditions: u64,
    pub last_audition: Option<(u8, u8)>,
    /// The Slices view's zoom, scroll, selected slice and last state read.
    pub slices: slices::SlicesView,
    /// The Instrument view's selected zone, last state read and gestures.
    pub instrument: instrument::InstrumentView,
}

impl Default for PadsView {
    fn default() -> Self {
        PadsView {
            view: View::Pads,
            kit_preview: None,
            kit: kit_presets::KitState::default(),
            bank: 0,
            selected: 0,
            live: [Live::default(); PADS],
            note: None,
            drag: None,
            held: None,
            held_track: None,
            auditions: 0,
            last_audition: None,
            slices: slices::SlicesView::default(),
            instrument: instrument::InstrumentView::default(),
        }
    }
}

/// Every open Sampler's view, by insert uid (UI state for the session).
#[derive(Debug, Default)]
pub struct ViewState {
    pub views: HashMap<u64, PadsView>,
    /// The document a run of kit preset steps left ([`kit_presets::stepping`]).
    pub step_run: Option<std::sync::Weak<Session>>,
}

/// One pad as last drawn: its index, the kit's record, its trigger note and bound zone.
#[derive(Debug, Clone)]
pub struct PadRecord {
    pub index: u8,
    pub pad: pad::PadLayout,
    pub note: u8,
    pub zone: Option<u16>,
    /// A pad whose sample is missing: its hover text (`Kick 01.wav is missing`) and the hatch over it.
    pub missing: Option<(String, widgets::hatch::HatchLayout)>,
}

/// One knob of the sections row as last drawn.
#[derive(Debug, Clone)]
pub struct KnobRecord {
    /// `level`, `pan`, `transpose`, `cents`, `attack_ms`, `decay_ms`, `sustain`, `release_ms`.
    pub field: String,
    pub label: String,
    pub value: String,
    pub knob: knob::KnobLayout,
}

/// One choke segment as last drawn: its rect, text, whether its group is in use and selected,
/// its ink and its underline (a used group with a color).
#[derive(Debug, Clone)]
pub struct ChokeSegment {
    pub rect: [f32; 4],
    pub text: String,
    pub group: u8,
    pub used: bool,
    pub selected: bool,
    pub ink: egui::Color32,
    pub underline: Option<[f32; 4]>,
}

/// The Selected pad panel as last drawn.
#[derive(Debug, Clone, Default)]
pub struct StripLayout {
    pub panel: [f32; 4],
    pub name: String,
    pub name_rect: [f32; 4],
    /// `Pad A6 · note 66`.
    pub id_line: String,
    pub id_line_rect: [f32; 4],
    pub choke_label: [f32; 4],
    /// `Group 1 · 3 pads` or `Off`.
    pub choke_value: String,
    pub choke_value_rect: [f32; 4],
    pub picker: [f32; 4],
    pub segments: Vec<ChokeSegment>,
    pub peers: String,
    pub peers_rect: [f32; 4],
    pub pad_label: [f32; 4],
    pub mute: Option<toggle::ToggleLayout>,
    pub solo: Option<toggle::ToggleLayout>,
    pub hint: String,
    pub hint_rect: [f32; 4],
}

/// The whole panel as last drawn, for `ui.inspect`-style checks and the tests.
#[derive(Debug, Clone, Default)]
pub struct SamplerLayout {
    pub window: [f32; 4],
    pub title: TitleLayout,
    /// The shared view bar.
    pub bar: bar::BarLayout,
    pub placeholder: Option<(String, [f32; 4])>,
    /// The Slices view's bench (`None` on the other views).
    pub slices: Option<slices::BenchLayout>,
    /// The Slices view's panels row under the bench.
    pub slice_panels: Option<slices::panels::PanelsLayout>,
    /// The Slices view's note row (a refused file) and where it sits.
    pub slice_note: Option<(String, [f32; 4])>,
    /// The Instrument view (`None` on the other views).
    pub instrument: Option<instrument::InstrumentLayout>,
    /// The body's top left as egui placed it (on the physical pixel grid, so at a fractional
    /// scale it can sit a fraction of a point below the title bar's bottom).
    pub body_origin: [f32; 2],
    /// Every panel card: its name (`pads`, `strip`, `playback`, `amp`) and rect.
    pub panels: Vec<(String, [f32; 4])>,
    pub banks: Option<segmented::SegmentedLayout>,
    /// The dots on bank buttons that hold a peer of the selection.
    pub bank_dots: Vec<[f32; 4]>,
    pub bank: usize,
    pub selected: u8,
    /// The shown bank's sixteen pads.
    pub pads: Vec<PadRecord>,
    /// Rows in the open pad context menu.
    pub pad_menu: Vec<(String, [f32; 4])>,
    pub grid: [f32; 4],
    /// `Bank A · 3 of 16 loaded`.
    pub status: String,
    pub status_rect: [f32; 4],
    pub strip: StripLayout,
    /// A refused edit's note and where it sits.
    pub note: Option<(String, [f32; 4])>,
    pub knobs: Vec<KnobRecord>,
    /// The trigger mode (One-shot, Gate), Reverse and Key track.
    pub mode: Option<segmented::SegmentedLayout>,
    pub reverse: Option<toggle::ToggleLayout>,
    pub key_track: Option<toggle::ToggleLayout>,
    /// The 1 px divider in the Playback panel.
    pub divider: [f32; 4],
    /// The sections row's caption when the selected pad is empty.
    pub empty_hint: Option<[f32; 4]>,
    pub load: String,
    pub footer: [f32; 4],
}

impl SamplerLayout {
    /// The pad with index `index` in the shown bank.
    pub fn pad(&self, index: u8) -> Option<&PadRecord> {
        self.pads.iter().find(|p| p.index == index)
    }

    /// The knob of zone field `field`.
    pub fn knob(&self, field: &str) -> Option<&KnobRecord> {
        self.knobs.iter().find(|k| k.field == field)
    }

    /// The panel named `name`.
    pub fn panel(&self, name: &str) -> Option<[f32; 4]> {
        self.panels.iter().find(|p| p.0 == name).map(|p| p.1)
    }
}

/// What the panel reads of a patch for one pad.
#[derive(Debug, Clone)]
pub(crate) struct PadData {
    pub entry: PadEntry,
    /// The note the pad plays: its stored note, else the pad start note plus its index.
    pub note: u8,
    pub zone: Option<ZoneEntry>,
    pub name: String,
}

/// The ready patch of the Sampler on `key`'s track (`None`: no patch, or a patch this build cannot
/// edit).
pub(crate) fn patch_at(s: &Session, key: (TrackId, usize)) -> Option<PatchV1> {
    let ins = if key.1 == transit_mix::INSTRUMENT_SLOT { s.track(key.0)?.instrument.as_ref() } else { None }?;
    ins.sampler_patch.as_ref()?.ready().cloned()
}

/// Whether the instance has a patch at all (ready or kept opaque).
pub(crate) fn has_patch(s: &Session, key: (TrackId, usize)) -> bool {
    key.1 == transit_mix::INSTRUMENT_SLOT && s.track(key.0).and_then(|t| t.instrument.as_ref()).is_some_and(|i| i.sampler_patch.is_some())
}

/// Every pad's data from `patch`.
pub(crate) fn pads_of(s: &Session, patch: &PatchV1) -> Vec<PadData> {
    (0..PADS as u8)
        .map(|i| {
            let entry = patch.pad(i);
            let zone = entry.zone.and_then(|z| patch.zone(z)).cloned();
            let name = pad_name(s, patch, &entry, zone.as_ref());
            PadData { entry, note: patch.pad_note(i), zone, name }
        })
        .collect()
}

/// The name pad `pad` (0-based) of the Sampler on track `t` shows (for its rename field).
pub fn shown_pad_name(s: &Session, t: TrackId, pad: u8) -> Option<String> {
    let patch = patch_at(s, (t, transit_mix::INSTRUMENT_SLOT))?;
    let entry = patch.pad(pad);
    let zone = entry.zone.and_then(|z| patch.zone(z));
    Some(pad_name(s, &patch, &entry, zone))
}

/// The view of the instance `uid`, made on first use.
pub(crate) fn view(app: &mut StudioApp, uid: u64) -> &mut PadsView {
    app.builtin_editor.devices.sampler.views.entry(uid).or_default()
}

/// A panel card: `surface.panel`, 1 px `line`, the panel radius. Returns the inner rect (the
/// panel padding, 14 by 16, inside).
pub(crate) fn panel(ui: &Ui, rect: Rect, name: &str, out: &mut SamplerLayout) -> Rect {
    let t = &TOKENS;
    ui.painter().rect(rect, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside);
    out.panels.push((name.to_string(), r4(rect)));
    rect.shrink2(vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]))
}

/// A section label (`label.section`) with its top left at `at`; returns its rect.
pub(crate) fn section_label(p: &egui::Painter, at: Pos2, text: &str) -> Rect {
    widgets::role_text(p, &tokens::LABEL_SECTION, text, TOKENS.text_muted, Align2::LEFT_TOP, at)
}

/// An audio file dragged from the browser onto pad `pad` of the Sampler `key`: the pad's own drop
/// (`behavior::drop_file`, one undo step; a refusal shows as the panel's note).
pub(crate) fn drop_file_on_pad(app: &mut StudioApp, key: (TrackId, usize), pad: u8, path: &str) -> bool {
    let Some(ins) = app.builtin_at(key) else { return false };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return false };
    behavior::drop_file(app, Dev { key, ins: &ins, info }, pad, path)
}

/// A layout rect as an egui rect.
pub(crate) fn grid_rect(r: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}

/// A painter for measuring text outside a frame's drawing.
pub(crate) fn measurer(ctx: &egui::Context) -> egui::Painter {
    egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::EVERYTHING)
}

/// The height of a line of `role`.
pub(crate) fn line_h(p: &egui::Painter, role: &tokens::TypeRole) -> f32 {
    widgets::role_size(p, role, "Ag").y
}

/// The window's inside width the body lays out at: its width less the frame's edge, never under the
/// minimum's.
fn inside_width(app: &StudioApp, avail: f32) -> f32 {
    avail.max(window::min_width(app) - crate::device_window::edge().x)
}

/// The panel's content height at `width` (the window's inside), for the window's size (every frame).
pub fn content_height(app: &StudioApp, ctx: &egui::Context, key: (TrackId, usize), width: f32) -> f32 {
    let Some(ins) = app.builtin_at(key) else { return 0.0 };
    let s = app.engine.session();
    let width = inside_width(app, width);
    let w = width - 2.0 * space::WINDOW_PADDING;
    let p = measurer(ctx);
    let v = app.builtin_editor.devices.sampler.views.get(&ins.uid.get());
    if let Some(v) = v.filter(|v| v.view == View::Slices) {
        return 2.0 * space::WINDOW_PADDING + bar::HEIGHT_WITH_GAP + v.slices.height.unwrap_or_else(|| slices::height(&p));
    }
    if let Some(v) = v.filter(|v| v.view == View::Instrument) {
        return 2.0 * space::WINDOW_PADDING + bar::HEIGHT_WITH_GAP + v.instrument.height.unwrap_or_else(|| instrument::height(&p));
    }
    if v.is_some_and(|v| v.view != View::Pads) {
        return 2.0 * space::WINDOW_PADDING + bar::HEIGHT_WITH_GAP + grid::height(&p);
    }
    let selected = v.map_or(0, |v| v.selected);
    let peers = patch_at(s, key).map(|patch| strip::peers_line(&pads_of(s, &patch), selected)).unwrap_or_default();
    let hero = grid::height(&p).max(strip::height(&p, w - grid::WIDTH - space::PANEL_GAP, &peers));
    let note = v.and_then(|v| v.note.as_deref()).map_or(0.0, |n| space::PANEL_GAP + note_height(&p, w, n));
    2.0 * space::WINDOW_PADDING + bar::HEIGHT_WITH_GAP + hero + note + space::PANEL_GAP + zone::height(&p)
}

/// The refused edit's note: Lucide's circle-alert in `meter.hot` and the message in `text`
/// (README, Warnings and errors), wrapped to the row.
const NOTE_ROLE: tokens::TypeRole = tokens::LABEL_CONTROL;
const NOTE_GLYPH: f32 = 15.0;

fn note_job(p: &egui::Painter, width: f32, text: &str) -> egui::text::LayoutJob {
    let mut job = NOTE_ROLE.job(p.ctx(), text, TOKENS.text);
    job.wrap.max_width = (width - NOTE_GLYPH - 6.0).max(40.0);
    job
}

fn note_height(p: &egui::Painter, width: f32, text: &str) -> f32 {
    p.layout_job(note_job(p, width, text)).size().y.max(NOTE_GLYPH)
}

fn draw_note(ui: &Ui, rect: Rect, text: &str) -> Rect {
    let p = ui.painter();
    let g = NOTE_GLYPH;
    crate::icons::paint(p, pos2(rect.min.x + g / 2.0, rect.min.y + g / 2.0), crate::chrome::Severity::Error.icon(), g, TOKENS.meter_hot);
    let galley = p.layout_job(note_job(p, rect.width(), text));
    let size = galley.size();
    p.galley(pos2(rect.min.x + g + 6.0, rect.min.y), galley, TOKENS.text);
    Rect::from_min_size(rect.min, vec2(g + 6.0 + size.x, size.y.max(g)))
}

/// Draws the panel's body (below the title bar, above the footer) into `ui`.
pub(crate) fn body(app: &mut StudioApp, ui: &mut Ui, dev: Dev, lay: &mut EditorLayout, out: &mut SamplerLayout) {
    let width = inside_width(app, ui.available_width());
    let origin = ui.cursor().min;
    let o = r4(Rect::from_min_size(origin, egui::Vec2::ZERO));
    out.body_origin = [o[0], o[1]];
    let Some(patch) = patch_at(app.engine.session(), dev.key) else {
        // A patch this build cannot edit, or none (a Sampler only a hand-made session can hold):
        // it is kept as saved and plays silence.
        let note =
            app.engine.session().track(dev.key.0).and_then(|t| t.instrument.as_ref()).and_then(|i| i.sampler_patch.as_ref()).and_then(|p| p.note(""));
        let r = Rect::from_min_size(origin + vec2(space::WINDOW_PADDING, space::WINDOW_PADDING), vec2(width - 2.0 * space::WINDOW_PADDING, 40.0));
        let text = note.unwrap_or_else(|| "This version cannot edit this Sampler patch; it is kept as saved.".to_string());
        out.note = Some((text.clone(), r4(draw_note(ui, r, &text))));
        ui.allocate_rect(Rect::from_min_max(origin, pos2(origin.x + width, r.max.y + space::WINDOW_PADDING)), Sense::hover());
        return;
    };
    let s = app.engine.session_arc();
    let pads = pads_of(&s, &patch);
    let now = ui.ctx().input(|i| i.time);
    live::update(app, dev, now);
    let p = ui.painter().clone();
    let w = width - 2.0 * space::WINDOW_PADDING;
    let x = origin.x + space::WINDOW_PADDING;
    let mut y = origin.y + space::WINDOW_PADDING;
    bar::show(app, ui, dev, &patch, &pads, Rect::from_min_size(pos2(x, y), vec2(w, bar::HEIGHT)), out);
    y += bar::HEIGHT_WITH_GAP;
    if view(app, dev.uid()).view == View::Slices {
        let h = slices::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, 0.0)), lay, out);
        behavior::end_drag(app, ui.ctx(), dev, out);
        ui.allocate_rect(Rect::from_min_size(origin, vec2(width, y + h + space::WINDOW_PADDING - origin.y)), Sense::hover());
        return;
    }
    if view(app, dev.uid()).view == View::Instrument {
        let h = instrument::show(app, ui, dev, Rect::from_min_size(pos2(x, y), vec2(w, 0.0)), lay, out);
        behavior::end_drag(app, ui.ctx(), dev, out);
        if live::moving(app, dev.uid(), now) {
            ui.ctx().request_repaint();
        }
        ui.allocate_rect(Rect::from_min_size(origin, vec2(width, y + h + space::WINDOW_PADDING - origin.y)), Sense::hover());
        return;
    }
    if !view_bar::is_pads(app, dev.uid()) {
        let h = grid::height(&p);
        view_bar::placeholder(app, ui, dev.uid(), Rect::from_min_size(pos2(x, y), vec2(w, h)), out);
        behavior::end_drag(app, ui.ctx(), dev, out);
        ui.allocate_rect(Rect::from_min_size(origin, vec2(width, y + h + space::WINDOW_PADDING - origin.y)), Sense::hover());
        return;
    }
    let selected = view(app, dev.uid()).selected;
    let peers = strip::peers_line(&pads, selected);
    let strip_w = w - grid::WIDTH - space::PANEL_GAP;
    let hero_h = grid::height(&p).max(strip::height(&p, strip_w, &peers));
    grid::show(app, ui, dev, (&patch, &pads), Rect::from_min_size(pos2(x, y), vec2(grid::WIDTH, hero_h)), now, out);
    strip::show(app, ui, dev, &pads, Rect::from_min_size(pos2(x + grid::WIDTH + space::PANEL_GAP, y), vec2(strip_w, hero_h)), out);
    y += hero_h;
    if let Some(n) = view(app, dev.uid()).note.clone() {
        y += space::PANEL_GAP;
        let h = note_height(&p, w, &n);
        let r = draw_note(ui, Rect::from_min_size(pos2(x, y), vec2(w, h)), &n);
        out.note = Some((n, r4(r)));
        y += h;
    }
    y += space::PANEL_GAP;
    let selected = view(app, dev.uid()).selected;
    let data = pads.get(usize::from(selected)).cloned();
    y += zone::show(app, ui, dev, &patch, data.as_ref(), Rect::from_min_size(pos2(x, y), vec2(w, zone::height(&p))), lay, out);
    y += space::WINDOW_PADDING;
    kit_load::show(app, ui.ctx(), dev);
    behavior::drops(app, dev, out);
    behavior::end_drag(app, ui.ctx(), dev, out);
    if live::moving(app, dev.uid(), now) {
        ui.ctx().request_repaint();
    }
    ui.allocate_rect(Rect::from_min_max(origin, pos2(origin.x + width, y)), Sense::hover());
}

/// The footer: the audio load on the right (`text.faint`, 11). The Sampler is transit's own, so
/// the footer carries no credit.
pub(crate) fn footer(app: &StudioApp, ui: &mut Ui, rect: Rect, _lay: &mut EditorLayout, out: &mut SamplerLayout) {
    let t = &TOKENS;
    let p = ui.painter();
    p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.line));
    out.footer = r4(rect);
    out.load = format!("Audio load {}", crate::load_readout::percent_text(app.telemetry.snap.current_load_ppm));
    widgets::role_text(
        p,
        &tokens::READOUT_DISPLAY,
        &out.load,
        t.text_faint,
        Align2::RIGHT_CENTER,
        pos2(rect.max.x - space::WINDOW_PADDING, rect.center().y),
    );
}
