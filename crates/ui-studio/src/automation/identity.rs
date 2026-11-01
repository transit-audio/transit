//! Keep positional MIDI lane addresses attached to their effect across chain edits and undo.

use super::{AutoGesture, Lane, StudioApp};
use std::collections::HashMap;
use transit_model::{AutoParam, TrackId};

fn remap(id: &mut String, before: &[u64], after: &[u64]) -> bool {
    let Some(Lane::Plugin { slot, param }) = Lane::parse(id) else { return true };
    let Some(index) = AutoParam::midi_effect_index(slot) else { return true };
    let Some(uid) = before.get(index) else { return false };
    let Some(slot) = after.iter().position(|other| other == uid).and_then(AutoParam::midi_effect_slot) else { return false };
    *id = Lane::Plugin { slot, param }.id();
    true
}

impl StudioApp {
    /// Run before reading or editing selection, including commands issued between UI frames.
    pub(super) fn sync_midi_lanes(&mut self) {
        let current: HashMap<TrackId, Vec<u64>> = self
            .engine
            .session()
            .tracks
            .iter()
            .filter(|track| !track.midi_effects.is_empty())
            .map(|track| (track.id, track.midi_effects.iter().map(|effect| effect.uid.get()).collect()))
            .collect();
        let previous = std::mem::replace(&mut self.auto.midi_chains, current);
        for (track, before) in previous {
            let after = self.auto.midi_chains.get(&track).map_or(&[][..], Vec::as_slice);
            if before == after {
                continue;
            }
            self.ui.auto_lanes.retain_mut(|view| view.track != track || remap(&mut view.lane, &before, after));
            self.auto.selection.retain_mut(|point| point.track != track.0 || remap(&mut point.lane, &before, after));
            if let Some((id, lane)) = &mut self.auto.highlight
                && *id == track
                && !remap(lane, &before, after)
            {
                self.auto.highlight = None;
            }
            // A gesture's captured curve is invalid after a structural edit; stop the gesture.
            if self.auto.gesture.as_ref().is_some_and(|gesture| match gesture {
                AutoGesture::Point(drag) => drag.track == track,
                AutoGesture::Band { track: id, .. } | AutoGesture::Height { track: id, .. } => *id == track,
            }) {
                self.auto.gesture = None;
            }
        }
    }
}
