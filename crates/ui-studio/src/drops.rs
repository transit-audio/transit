//! Files dropped on the window from the file manager, routed once per frame (#368).
//!
//! winit 0.30 reports a drop as `WindowEvent::DroppedFile(PathBuf)` with no position, and on
//! Windows no cursor motion arrives during the OS drag, so egui's pointer position is stale or
//! gone when the drop lands. On Windows the binary asks the OS for the cursor at the drop's frame
//! and puts a `PointerMoved` there first in the frame (`apps/transit`, `drop_cursor`). Elsewhere a
//! drop is held until the pointer next reports where it is (the first `PointerMoved` in the drop's
//! frame or a later one). A `PointerGone` does not route it blind: the drop stays held, and only
//! after [`GONE_GRACE_FRAMES`] frames with no position does it go to the arrangement. Then the
//! editor window on top at that position (egui's own layer order) claims it, or the arrangement
//! imports it. Exactly one claimant takes a drop; a held drop never vanishes (a second drop while
//! one is held routes with it).

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
    /// Frames the pending files have waited since a `PointerGone` (none: no gone seen yet).
    gone_frames: Option<u32>,
    /// The editor windows drawn last frame: each one's layer and editor key.
    pub(crate) windows: Vec<(egui::LayerId, (TrackId, usize))>,
    /// This frame's drop, claimed by the editor window `key`; its body takes it.
    offer: Option<((TrackId, usize), DropOffer)>,
    /// A browser drag released on a Sampler view's empty well (#464): next frame's offer to that
    /// window, so the view takes it as it takes a dropped file.
    next: Option<((TrackId, usize), DropOffer)>,
}

/// Frames a held drop waits for a position after the pointer is reported gone before it goes to
/// the arrangement (about half a second at 60 frames a second; the router asks for a repaint each
/// held frame so the count advances without input). Counted in frames, never wall-clock time.
pub(crate) const GONE_GRACE_FRAMES: u32 = 30;

/// What a frame does with the held drop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Placement {
    /// Keep holding it.
    Hold,
    /// Route it now, at this position (`None`: the pointer is gone; the arrangement imports it).
    Route(Option<egui::Pos2>),
}

/// The gone rule: a position routes the drop there; a `PointerGone` starts a count of held frames,
/// and the drop goes to the arrangement only once that count passes [`GONE_GRACE_FRAMES`] with no
/// position; with neither the drop is held. `gone_frames` is the router's count, updated here.
pub(crate) fn placement(moved: Option<egui::Pos2>, gone: bool, gone_frames: &mut Option<u32>) -> Placement {
    if let Some(p) = moved {
        *gone_frames = None;
        return Placement::Route(Some(p));
    }
    if gone {
        gone_frames.get_or_insert(0);
    }
    let Some(n) = gone_frames.as_mut() else { return Placement::Hold };
    *n += 1;
    if *n > GONE_GRACE_FRAMES {
        *gone_frames = None;
        Placement::Route(None)
    } else {
        Placement::Hold
    }
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
    // An offer no editor took last frame (its window closed) is refused; a browser drag's offer
    // made since is this frame's.
    r.offer = r.next.take();
    r.pending.extend(files);
    if r.pending.is_empty() {
        r.gone_frames = None;
        return;
    }
    let pos = match placement(moved, gone, &mut r.gone_frames) {
        Placement::Route(pos) => pos,
        Placement::Hold => {
            if r.gone_frames.is_some() {
                ctx.request_repaint();
            }
            return;
        }
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

/// Offers `path` to the editor window `key` in the next frame, as a file dropped on it there (a
/// browser drag released on a Sampler view's empty well, #464).
pub(crate) fn offer_next(app: &mut StudioApp, key: (TrackId, usize), path: PathBuf) {
    app.builtin_editor.drops.next = Some((key, DropOffer { paths: vec![path], pos: None }));
}

/// The drop the editor window `key` claimed this frame, if any (taken once).
pub(crate) fn take(app: &mut StudioApp, key: (TrackId, usize)) -> Option<DropOffer> {
    let r = &mut app.builtin_editor.drops;
    if r.offer.as_ref().is_some_and(|(k, _)| *k == key) { r.offer.take().map(|(_, o)| o) } else { None }
}

/// The arrangement's claim: a `.transit` or `.dawproject` file opens as the session, a MIDI file
/// imports, any other file imports as audio at the cursor (on the cursor's track). Also used for
/// files named on the command line.
pub fn open_path(app: &mut StudioApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    if lower.ends_with(".transit") || lower.ends_with(".dawproject") {
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
