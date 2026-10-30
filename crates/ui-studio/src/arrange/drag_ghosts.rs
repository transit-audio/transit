//! The move/copy drag preview of a whole selection (#218).
//!
//! Ableton and Logic both preview every selected clip while the pointer drags one of them. The
//! members come from the rule the commit uses (`copy_drag::drag_set`: the whole selection when the
//! dragged clip is part of it, else just that clip), so the preview never disagrees with the drop.
//! One clip may change track; several move or copy in time only, by the same offset.

use super::{ClipId, Gesture, StudioApp, TrackId, copy_drag};
use transit_time::Samples;

/// One clip of the drag as it would land.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DragGhost {
    pub clip: u64,
    pub track: u64,
    /// Where the clip would start and end, in timeline samples.
    pub start: Samples,
    pub end: Samples,
    /// The painted rectangle `[x0, y0, x1, y1]` (filled in when drawn).
    pub rect: [f32; 4],
}

/// Every clip the drag in progress carries, at its proposed position. `preview` is the dragged
/// clip's own landing (track, start, end); a trim drag has only that one. With two or more
/// clips every ghost stays on its own track; one clip may land on the row under the pointer.
pub(super) fn members(app: &StudioApp, preview: (ClipId, TrackId, Samples, Samples)) -> Vec<DragGhost> {
    let (clip, dest, a, b) = preview;
    let own = DragGhost { clip: clip.0, track: dest.0, start: a, end: b, rect: [0.0; 4] };
    let Some(Gesture::Clip { kind: super::DragKind::Move, start, .. }) = app.arrange.gesture else { return vec![own] };
    let set = copy_drag::drag_set(app, clip);
    if set.len() < 2 {
        return vec![own];
    }
    let by = a - start;
    let s = app.engine.session();
    set.into_iter()
        .filter_map(|id| {
            // Several clips move in time only (like `commit_drag`): each keeps its own track,
            // the grabbed one too, whatever row the pointer is over.
            let (t, c) = s.find_clip(id)?;
            Some(DragGhost { clip: id.0, track: t.0, start: c.start + by, end: c.end() + by, rect: [0.0; 4] })
        })
        .collect()
}
