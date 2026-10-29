//! The copy branch of a clip drag (Ctrl- or Alt-drag, #214): a selection copies as a whole.

use super::{ClipId, StudioApp, TrackId};
use transit_time::Samples;

/// The clips a drag of `clip` carries: the whole selection when `clip` is part of one.
pub(super) fn drag_set(app: &StudioApp, clip: ClipId) -> Vec<ClipId> {
    let sel: Vec<ClipId> = app.selected_clips().into_iter().map(|(_, c, _)| c).collect();
    if sel.contains(&clip) && sel.len() > 1 { sel } else { vec![clip] }
}

/// Copy `set` by `by` samples: one clip lands at `at` on `dest` (a track change is allowed);
/// several copy in time only, like the move, as one undo step with the copies selected.
pub(super) fn copy(app: &mut StudioApp, set: &[ClipId], at: Samples, by: Samples, dest: TrackId) {
    if let [clip] = set {
        let _ = app.copy_clip_to(*clip, at, dest);
    } else {
        let _ = app.copy_clips(set, by);
    }
}
