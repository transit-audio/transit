//! Files dropped on the window from the file manager, routed once per frame (#368).
//!
//! winit 0.30 reports a drop as `WindowEvent::DroppedFile(PathBuf)` with no position, and on
//! Windows no cursor motion arrives during the OS drag, so egui's pointer position is stale or
//! gone when the drop lands. A drop is therefore held until the pointer next reports where it
//! is (the first `PointerMoved` in the drop's frame or a later one; a `PointerGone` first sends it
//! to the arrangement). Then the editor window on top at that position (egui's own layer order)
//! claims it, or the arrangement imports it. Exactly one claimant takes a drop.

use crate::StudioApp;
use std::path::PathBuf;
use transit_model::TrackId;

/// Files offered to a claimant, and where they were dropped (`None`: the pointer left the window).
#[derive(Debug, Clone)]
pub struct DropOffer {
    pub paths: Vec<PathBuf>,
    pub pos: Option<egui::Pos2>,
}

#[derive(Debug, Default)]
pub(crate) struct Router {
    /// Dropped files waiting for the pointer to report a position.
    pending: Vec<PathBuf>,
    /// The editor windows drawn last frame: each one's layer and editor key.
    pub(crate) windows: Vec<(egui::LayerId, (TrackId, usize))>,
    /// This frame's drop, claimed by the editor window `key`; its body takes it.
    offer: Option<((TrackId, usize), DropOffer)>,
}

/// The frame's drop routing (called from the frame logic, before the editors are drawn).
pub(crate) fn step(app: &mut StudioApp, ctx: &egui::Context) {
    let (files, moved, gone) = ctx.input(|i| {
        let files: Vec<PathBuf> = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect();
        let moved = i.raw.events.iter().find_map(|e| if let egui::Event::PointerMoved(p) = e { Some(*p) } else { None });
        let gone = i.raw.events.iter().any(|e| matches!(e, egui::Event::PointerGone));
        (files, moved, gone)
    });
    let r = &mut app.builtin_editor.drops;
    // An offer no editor took last frame (its window closed) is refused.
    r.offer = None;
    r.pending.extend(files);
    if r.pending.is_empty() {
        return;
    }
    let pos = match (moved, gone) {
        (Some(p), _) => Some(p),
        (None, true) => None,
        (None, false) => return,
    };
    let offer = DropOffer { paths: std::mem::take(&mut r.pending), pos };
    route(app, ctx, offer);
}

/// Offers `offer` to the editor window on top at its position, else imports it to the arrangement.
pub(crate) fn route(app: &mut StudioApp, ctx: &egui::Context, offer: DropOffer) {
    let r = &mut app.builtin_editor.drops;
    let top = offer.pos.and_then(|p| ctx.layer_id_at(p));
    if let Some(key) = top.and_then(|l| r.windows.iter().find(|(id, _)| *id == l).map(|(_, k)| *k)) {
        r.offer = Some((key, offer));
        return;
    }
    for p in &offer.paths {
        open_path(app, &p.to_string_lossy());
    }
}

/// The drop the editor window `key` claimed this frame, if any (taken once).
pub(crate) fn take(app: &mut StudioApp, key: (TrackId, usize)) -> Option<DropOffer> {
    let r = &mut app.builtin_editor.drops;
    if r.offer.as_ref().is_some_and(|(k, _)| *k == key) { r.offer.take().map(|(_, o)| o) } else { None }
}

/// The arrangement's claim: a `.transit` file opens as the session, a MIDI file imports, any other
/// file imports as audio at the cursor (on the cursor's track). Also used for files named on the
/// command line.
pub fn open_path(app: &mut StudioApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    if lower.ends_with(".transit") {
        app.run_or_ask("session.open", serde_json::json!({"path": p}));
        return;
    }
    let r = if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", serde_json::json!({"path": p}))
    } else {
        let at = app.cursor();
        let mut params = serde_json::json!({"path": p, "at": at});
        if let Some(t) = app.cursor_track() {
            params["track"] = serde_json::json!(t.0);
        }
        app.run("file.import_audio", params)
    };
    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}
