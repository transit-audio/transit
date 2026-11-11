//! The Sampler's Slices view (ADR 0041 and its amendment 2026-11-08; README, Sampler views, Slices
//! view and its round 2 decisions): the bench ([`bench`], its gestures in [`bench_gestures`]),
//! then the panels row ([`panels`]: Detect or Manual, and Use slices), then the sections row with
//! the per-slice loop ([`loop_row`]; a looping slice on the bench, [`bench_loop`]). Everything it shows comes
//! from `mix.sampler_slice_state` ([`state`]), read again when the document or the placement
//! changes, and from the commands' own previews; the source's peaks draw the waveform ([`wave`]).
//! Zoom, scroll and the selected slice are UI state. Each gesture sends one command on completion
//! ([`edit`]), one undo step, with only the fields it changes; the placement `at` is the UI's
//! ([`placement`]).
//!
//! Keyboard, while the Sampler window has the keyboard on this view (Help › Keyboard Shortcuts,
//! Sampler and Sampler slices): Left and Right select the previous and next slice; Alt+Left and
//! Alt+Right nudge the selected slice's start marker one frame in Manual, Shift+Alt ten; Delete
//! removes it (never S); Space or Enter auditions the selected slice while held; Escape cancels a
//! marker drag. Never while typing, a menu or a modal is open, or another window has the
//! keyboard; the keys never fall through to the arrangement.

pub mod bench;
pub mod bench_gestures;
pub mod bench_loop;
pub mod detect_panel;
pub mod edit;
pub mod feedback;
pub mod loop_row;
pub mod panels;
pub mod placement;
pub mod state;
pub mod use_panel;
pub mod viewport;
mod wave;

#[cfg(test)]
mod bench_tests;
#[cfg(test)]
mod gesture_tests;
#[cfg(test)]
mod keyboard_tests;
#[cfg(test)]
mod loop_tests;
#[cfg(test)]
mod panel_tests;
#[cfg(test)]
pub(super) mod rig;
#[cfg(test)]
mod viewport_tests;

use super::{SamplerLayout, behavior, view};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::Dev;
use crate::shortcuts::{self, Action};
use crate::tokens::layout as space;
use bench_gestures::{Intent, Renaming};
use egui::{Rect, Ui, pos2, vec2};

pub use bench::BenchLayout;
pub(crate) use bench::{escaped, segmented_at};

/// One instance's Slices view: the viewport, the selected slice (an index into the showing set),
/// the last state read, the placement, the gestures, the panels' state and what the view said.
#[derive(Debug, Clone, Default)]
pub struct SlicesView {
    pub viewport: viewport::Viewport,
    pub selected: usize,
    pub snapshot: state::Snapshot,
    /// The placement tick the state was read at and the commands are sent with.
    pub at: i64,
    pub gesture: bench_gestures::Gestures,
    pub feedback: feedback::Feedback,
    pub detect: detect_panel::DetectState,
    pub usage: use_panel::UseState,
    /// The Playback section's loop row (the crossfade being typed).
    pub play: loop_row::PlayState,
    /// An audition held by Space or Enter (released with the key, or when the window closes, the
    /// view switches or the instance changes).
    pub key_held: bool,
    /// Every command this view sent, latest last (bounded): `id {params}`.
    pub sent: Vec<String>,
    /// The body's height as last drawn (the bench, the note row and the panels).
    pub height: Option<f32>,
}

/// Reads the state again if the document or the placement changed.
fn refresh(app: &mut StudioApp, dev: Dev) {
    let at = placement::of(app, dev.key.0).tick;
    let mut snap = std::mem::take(&mut view(app, dev.uid()).slices.snapshot);
    snap.refresh(app, dev.key.0, at);
    let v = &mut view(app, dev.uid()).slices;
    v.snapshot = snap;
    v.at = at;
}

/// The Slices view bar's summary: the loop's file, its length and its rate (`loop.wav · 2.76 s ·
/// 48 kHz`); nothing without a loop.
pub(crate) fn summary(app: &mut StudioApp, dev: Dev) -> String {
    refresh(app, dev);
    let Some(l) = view(app, dev.uid()).slices.snapshot.data.clone() else { return String::new() };
    let rate = format!("{} kHz", f64::from(l.rate) / 1000.0);
    let length = format!("{:.2} s", l.seconds(l.end as f64));
    file_name(app, &l).into_iter().chain([length, rate]).collect::<Vec<_>>().join(" · ")
}

/// The loop source's file name (its display name without a path).
fn file_name(app: &StudioApp, l: &state::Loop) -> Option<String> {
    let s = app.engine.session();
    l.source
        .and_then(|id| s.source(id))
        .map(|src| std::path::Path::new(&src.path).file_name().and_then(|n| n.to_str()).map_or_else(|| src.name.clone(), str::to_string))
}

/// `Remove: click, or Delete` and the like: a control's tooltip naming its keys.
fn tip(what: &str, actions: &[Action]) -> String {
    let keys: Vec<String> = actions.iter().flat_map(|a| shortcuts::rows(*a)).map(shortcuts::text).collect();
    format!("{what}: {}", keys.join(" / "))
}

/// Draws the view's body into `rect` (the bench at the top, the note row, the panels) and returns
/// its height.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut SamplerLayout) -> f32 {
    refresh(app, dev);
    let uid = dev.uid();
    let mc = app.ui.middle_c;
    let mut v = std::mem::take(&mut view(app, uid).slices);
    if v.feedback.notice.as_ref().is_some_and(|(_, rev)| *rev != app.engine.revision) {
        v.feedback.notice = None;
    }
    // The document changed underneath the view: its banners and lines go, and a picked root
    // returns to the stored one.
    if v.feedback.follow(app.engine.revision) {
        v.usage.root = None;
    }
    let data = v.snapshot.data.clone();
    let s = app.engine.session_arc();
    let audio = data.as_ref().and_then(|l| l.source).and_then(|id| s.pool.get(id).cloned());
    let missing = data.as_ref().and_then(|l| l.source.filter(|id| !s.pool.contains(*id)).map(|_| file_name(app, l).unwrap_or_default()));
    let (now, held) = ui.input(|i| (i.time, i.pointer.any_down()));
    if let Some(l) = &data {
        detect_panel::tick(app, dev, &mut v, l, now, held);
    }
    let inputs = bench::Inputs {
        candidate: data.as_ref().and_then(|l| v.detect.candidate.as_ref().and_then(|c| c.preview(l))),
        stale: data.as_ref().is_some_and(|l| l.mapping == "stale"),
        missing,
        error: data.is_none().then(|| v.snapshot.error.clone()).flatten(),
        cell_tip: tip("Select: click; previous / next slice", &[Action::SamplerPreviousSlice, Action::SamplerNextSlice])
            + &tip("; audition", &[Action::SamplerAuditionSlice])
            + "; rename: double-click",
        flag_tip: tip("Remove: click, or", &[Action::SamplerRemoveMarker]),
        handle_tip: tip("Move: drag; cancel", &[Action::SamplerCancelDrag])
            + &tip("; nudge one frame", &[Action::SamplerNudgeEarlier, Action::SamplerNudgeLater])
            + &tip("; ten", &[Action::SamplerNudgeEarlierTen, Action::SamplerNudgeLaterTen]),
    };
    let h = bench::height(ui.painter());
    let bench_rect = Rect::from_min_size(rect.min, vec2(rect.width(), h));
    let (mut layout, intent) = bench::show(ui, uid, &mut v, data.as_ref(), audio.as_deref(), &inputs, mc, bench_rect);
    if let Some(i) = intent {
        apply(app, dev, &mut v, i);
    }
    let mut y = rect.min.y + h;
    out.slice_note = None;
    if let Some(n) = v.feedback.note.clone() {
        let p = ui.painter().clone();
        y += space::PANEL_GAP;
        let nh = super::note_height(&p, rect.width(), &n);
        let r = super::draw_note(ui, Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), nh)), &n);
        out.slice_note = Some((n, crate::widgets::r4(r)));
        y += nh;
    }
    y += space::PANEL_GAP;
    let data = v.snapshot.data.clone();
    let (mut panels, ph) = panels::show(app, ui, dev, &mut v, data.as_ref(), pos2(rect.min.x, y), rect.width());
    y += ph + space::PANEL_GAP;
    let (row, sh) = loop_row::show(app, ui, dev, &mut v, data.as_ref(), pos2(rect.min.x, y), rect.width(), lay, out);
    panels.loop_row = row;
    y += sh;
    if data.is_none()
        && let Some(offer) = crate::drops::take(app, dev.key)
        && let Some(path) = offer.paths.iter().find(|p| p.is_absolute() && crate::builtin_editor::is_audio_name(&p.to_string_lossy()))
    {
        edit::load(app, dev, &mut v, &path.to_string_lossy());
    }
    let pending = v.detect.changed_at.is_some() || v.detect.candidate.as_ref().is_some_and(|c| c.state == "pending");
    if pending {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(detect_panel::POLL));
    }
    v.height = Some(y - rect.min.y);
    view(app, uid).slices = v;
    layout.typing = typing(ui.ctx(), uid);
    out.slices = Some(layout);
    out.slice_panels = Some(panels);
    y - rect.min.y
}

/// The view's text field that has the keyboard (the rename, the BPM, the beat length or the
/// crossfade), so that the device window leaves Escape to it.
fn typing(ctx: &egui::Context, uid: u64) -> Option<egui::Id> {
    let ids = [bench::rename_id(uid), detect_panel::bpm_id(uid), use_panel::beat_id(uid), loop_row::crossfade_id(uid)];
    ctx.memory(|m| ids.into_iter().find(|id| m.has_focus(*id)))
}

/// Sends what a completed bench gesture asks for.
fn apply(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, intent: Intent) {
    match intent {
        Intent::Add(f) => edit::add(app, dev, v, f),
        Intent::Move { id, frame } => edit::move_to(app, dev, v, id, frame, None),
        Intent::Remove { id, index } => edit::remove(app, dev, v, id, index),
        Intent::Rename(index) => {
            let s = v.snapshot.data.as_ref().and_then(|l| l.showing().get(index).cloned());
            if let Some(s) = s
                && let Some(id) = s.id
            {
                v.gesture.renaming = Some(Renaming { index, id, text: s.label.unwrap_or_default(), focused: false });
            }
        }
        Intent::Label { id, text } => edit::label(app, dev, v, id, &text),
        Intent::Mode(m) => edit::mode(app, dev, v, m),
        Intent::Snap(on) => edit::snap(app, dev, v, on),
        Intent::Load => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|pick| pick("mix.sampler_load", None)) {
                edit::load(app, dev, v, &path);
            }
        }
    }
}

/// The body's height before it is first drawn (for the window's size).
pub(crate) fn height(p: &egui::Painter) -> f32 {
    bench::height(p) + space::PANEL_GAP + crate::tokens::sampler::PANEL_MIN_HEIGHT + space::PANEL_GAP + loop_row::height(p)
}

/// Releases held auditions that lost their key or window: a keyboard audition when its window
/// closed, its view switched, another window took the keyboard or neither audition key is down;
/// a pointer audition when its window closed, its view switched or no button is down. The note-off
/// goes to the track the note went to.
fn release_stale(app: &mut StudioApp, ctx: &egui::Context) {
    let (keys_down, button_down) = ctx.input(|i| {
        let down = shortcuts::rows(Action::SamplerAuditionSlice).any(|s| i.key_down(s.key));
        (down, i.pointer.any_down())
    });
    let focused = app.builtin_editor.devices.focused;
    let open: Vec<u64> = app.ui.plugin_windows.iter().filter_map(|k| app.builtin_at(*k).map(|i| i.uid.get())).collect();
    let stale: Vec<u64> = app
        .builtin_editor
        .devices
        .sampler
        .views
        .iter()
        .filter(|(_, v)| v.held.is_some())
        .filter(|(uid, v)| {
            let shown = open.contains(uid);
            if v.slices.key_held {
                !shown || v.view != super::View::Slices || focused != Some(**uid) || !keys_down
            } else {
                !shown || v.view != super::View::Pads || !button_down
            }
        })
        .map(|(uid, _)| *uid)
        .collect();
    for uid in stale {
        behavior::release_uid(app, uid);
    }
}

/// The Slices view's keys on the focused Sampler (see the module's notes). Runs before the global
/// shortcuts, so Space does not also play and the arrows do not reach the arrangement.
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    release_stale(app, ctx);
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    let modal = ctx.memory(|m| m.top_modal_layer().is_some());
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || modal {
        return;
    }
    let Some(key) =
        app.ui.plugin_windows.iter().copied().find(|key| app.builtin_at(*key).is_some_and(|i| i.uid.get() == uid && super::handles(&i.plugin)))
    else {
        return;
    };
    if !super::has_patch(app.engine.session(), key) || view(app, uid).view != super::View::Slices {
        return;
    }
    let Some(ins) = app.builtin_at(key) else { return };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return };
    let dev = Dev { key, ins: &ins, info };
    if view(app, uid).slices.gesture.drag.is_some() && shortcuts::pressed(ctx, Action::SamplerCancelDrag) {
        if let Some(d) = view(app, uid).slices.gesture.drag.as_mut() {
            d.cancelled = true;
        }
        return;
    }
    let edges = shortcuts::edges(ctx, Action::SamplerAuditionSlice);
    let previous = shortcuts::pressed(ctx, Action::SamplerPreviousSlice);
    let next = shortcuts::pressed(ctx, Action::SamplerNextSlice);
    let nudge = [
        (Action::SamplerNudgeEarlier, -1i64),
        (Action::SamplerNudgeLater, 1),
        (Action::SamplerNudgeEarlierTen, -10),
        (Action::SamplerNudgeLaterTen, 10),
    ]
    .into_iter()
    .fold(0, |n, (a, step)| if shortcuts::pressed(ctx, a) { n + step } else { n });
    let delete = shortcuts::pressed(ctx, Action::SamplerRemoveMarker);
    // An open menu or list (a panel's dropdown, any popup) holds the keyboard, as system menus
    // do: the keys above are taken and do nothing, so they never reach the arrangement.
    if egui::Popup::is_any_open(ctx) {
        return;
    }
    refresh(app, dev);
    let mut v = std::mem::take(&mut view(app, uid).slices);
    if let Some(l) = v.snapshot.data.clone() {
        let set = l.showing();
        let last = set.len().saturating_sub(1);
        if previous || next {
            v.selected = if previous { v.selected.saturating_sub(1) } else { (v.selected + 1).min(last) };
            if let Some(s) = set.get(v.selected) {
                v.viewport.select(s.frame, s.end);
            }
        }
        let selected = set.get(v.selected).cloned().filter(|_| v.selected > 0);
        if let Some(s) = selected.as_ref().filter(|_| nudge != 0 && l.mode == state::Mode::Manual)
            && let Some(id) = s.id
        {
            let frame = (s.frame as i64 + nudge).max(0) as u64;
            edit::move_to(app, dev, &mut v, id, frame, Some(false));
        }
        if let Some(s) = selected.filter(|_| delete)
            && let Some(id) = s.id
        {
            let index = v.selected;
            edit::remove(app, dev, &mut v, id, index);
        }
    }
    let note = v.snapshot.data.as_ref().and_then(|l| l.showing().get(v.selected)).and_then(|s| s.key);
    view(app, uid).slices = v;
    for pressed in edges {
        if pressed {
            if let Some(note) = note {
                behavior::audition(app, dev, note);
                view(app, uid).slices.key_held = view(app, uid).held.is_some();
            }
        } else {
            behavior::release(app, dev);
        }
    }
}
