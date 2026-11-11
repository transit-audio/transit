//! The Sampler's Slices view (ADR 0041; README, Sampler views, Slices view), pass G2: the bench,
//! read-only ([`bench`]). Everything it shows comes from `mix.sampler_slice_state` ([`state`]),
//! read again when the document changes; the source's peaks draw the waveform ([`wave`]). The
//! zoom, the scroll ([`viewport`]) and the selected slice are UI state: no command and no undo
//! step. A click on a slice's cell in the flag strip selects it; Space or Enter auditions the
//! selected slice's pad through the pads' audition path (the piano roll's velocity, held as long
//! as the key), while the Sampler window has the keyboard on this view.

pub mod bench;
pub mod state;
pub mod viewport;
mod wave;

#[cfg(test)]
mod bench_tests;
#[cfg(test)]
mod viewport_tests;

use super::{SamplerLayout, behavior, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use egui::{Rect, Ui};

pub use bench::BenchLayout;

/// One instance's Slices view: the viewport, the selected slice (an index into the showing set)
/// and the last state read.
#[derive(Debug, Clone, Default)]
pub struct SlicesView {
    pub viewport: viewport::Viewport,
    pub selected: usize,
    pub snapshot: state::Snapshot,
}

/// Reads the state again if the document changed.
fn refresh(app: &mut StudioApp, dev: Dev) {
    let mut snap = std::mem::take(&mut view(app, dev.uid()).slices.snapshot);
    snap.refresh(app, dev.key.0);
    view(app, dev.uid()).slices.snapshot = snap;
}

/// The Slices view bar's summary: the loop's file, its length and its rate (`loop.wav · 2.76 s ·
/// 48 kHz`); nothing without a loop.
pub(crate) fn summary(app: &mut StudioApp, dev: Dev) -> String {
    refresh(app, dev);
    let Some(l) = view(app, dev.uid()).slices.snapshot.data.clone() else { return String::new() };
    let s = app.engine.session();
    let name = l
        .source
        .and_then(|id| s.source(id))
        .map(|src| std::path::Path::new(&src.path).file_name().and_then(|n| n.to_str()).map_or_else(|| src.name.clone(), str::to_string));
    let rate = format!("{} kHz", f64::from(l.rate) / 1000.0);
    let length = format!("{:.2} s", l.seconds(l.end as f64));
    name.into_iter().chain([length, rate]).collect::<Vec<_>>().join(" · ")
}

/// Draws the view's body into `rect` (the bench at the top) and returns its height.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, out: &mut SamplerLayout) -> f32 {
    refresh(app, dev);
    let mc = app.ui.middle_c;
    let data = view(app, dev.uid()).slices.snapshot.data.clone();
    let audio = data.as_ref().and_then(|l| l.source).and_then(|id| app.engine.session().pool.get(id).cloned());
    let h = bench::height(ui.painter());
    let mut v = std::mem::take(&mut view(app, dev.uid()).slices);
    let layout = bench::show(ui, dev.uid(), &mut v, data.as_ref(), audio.as_deref(), mc, Rect::from_min_size(rect.min, egui::vec2(rect.width(), h)));
    view(app, dev.uid()).slices = v;
    out.slices = Some(layout);
    h
}

/// The body's height (for the window's size).
pub(crate) fn height(p: &egui::Painter) -> f32 {
    bench::height(p)
}

/// Space or Enter on the focused Sampler's Slices view: a press auditions the selected slice's
/// pad, the release ends it. Runs before the global shortcuts, so Space does not also play.
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || egui::Popup::is_any_open(ctx) {
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
    let edges = crate::shortcuts::edges(ctx, crate::shortcuts::Action::SamplerAuditionSlice);
    if edges.is_empty() {
        return;
    }
    let Some(ins) = app.builtin_at(key) else { return };
    let Some(info) = transit_dsp::plugin_info(&ins.plugin) else { return };
    let dev = Dev { key, ins: &ins, info };
    for pressed in edges {
        if pressed {
            let v = &view(app, uid).slices;
            let note = v.snapshot.data.as_ref().and_then(|l| l.showing().get(v.selected)).and_then(|s| s.key);
            if let Some(note) = note {
                behavior::audition(app, dev, note);
            }
        } else {
            behavior::release(app, dev);
        }
    }
}
