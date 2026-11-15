//! Issue #574: a hosted plug-in keeps state the session cannot see, and the session stores it only
//! when a capture reads it ([`StudioApp::capture_plugin_states`]).
//!
//! - Before a change that drops a live hosted instance (removing or replacing its device, deleting
//!   its track or group, an undo or redo that takes it away), the live states are captured first,
//!   so the undo snapshot of that step holds the state the plug-in had at that moment, and Undo
//!   (or Redo) makes the instance again with it. A capture that cannot run now refuses the change
//!   with its reason, as a device copy does: the state is never dropped silently.
//! - Before the unsaved-changes question (Quit, New, Open, closing the window), the live states
//!   are captured too, so a change that no plug-in reported still counts as unsaved work.

use super::*;
use serde_json::Value;
use std::collections::HashSet;
use transit_model::{Session, Track};

/// The uids of `t`'s hosted (third-party) devices: inserts and instrument.
fn hosted(t: &Track) -> impl Iterator<Item = u64> + '_ {
    t.mixer.inserts.iter().flatten().chain(t.instrument.as_ref()).filter(|i| transit_mix::is_third_party(&i.plugin)).map(|i| i.uid.get())
}

/// Whether `after` lacks a hosted device that `before` has (an undo or redo that drops it).
pub(crate) fn drops_hosted(before: &Session, after: &Session) -> bool {
    let kept: HashSet<u64> = after.tracks.iter().flat_map(hosted).collect();
    before.tracks.iter().flat_map(hosted).any(|u| !kept.contains(&u))
}

/// Whether any track of `s` hosts a third-party device.
fn any_hosted(s: &Session) -> bool {
    s.tracks.iter().any(|t| hosted(t).next().is_some())
}

/// Whether the tracks `params` names (`track` or `tracks`, by id or name) host a third-party
/// device. A command that names none acts on the selection: then any hosted device counts.
fn named_tracks_host(s: &Session, params: &Value) -> bool {
    let one = |v: &Value| match (v.as_u64(), v.as_str()) {
        (Some(n), _) => s.track(TrackId(n)),
        (_, Some(name)) => s.track_by_name(name),
        _ => None,
    };
    let named: Vec<&Value> = match (params.get("tracks").and_then(Value::as_array), params.get("track")) {
        (Some(list), _) => list.iter().collect(),
        (None, Some(v)) => vec![v],
        (None, None) => return any_hosted(s),
    };
    named.into_iter().filter_map(one).any(|t| hosted(t).next().is_some())
}

impl StudioApp {
    /// Whether running `id` with `params` may drop a live hosted instance (see the module docs).
    pub(crate) fn drops_live_state(&self, id: &str, params: &Value) -> bool {
        let s = self.engine.session();
        match id {
            "edit.undo" => self.engine.undo_doc().is_some_and(|d| drops_hosted(s, d)),
            "edit.redo" => self.engine.redo_doc().is_some_and(|d| drops_hosted(s, d)),
            "track.delete" | "mix.insert_remove" | "mix.instrument" => named_tracks_host(s, params),
            // An insert into a named slot replaces what is there.
            "mix.insert" => params.get("slot").is_some() && named_tracks_host(s, params),
            _ => false,
        }
    }

    /// Captures the live plug-in states when `tracks` host a third-party device, before a change
    /// that deletes them (see the module docs). The error is the capture's reason, also shown in
    /// the status line.
    pub(crate) fn capture_before_dropping(&mut self, tracks: &[TrackId]) -> Result<(), String> {
        let s = self.engine.session();
        if !tracks.iter().filter_map(|t| s.track(*t)).any(|t| hosted(t).next().is_some()) {
            return Ok(());
        }
        self.capture_live_states()
    }

    /// [`Self::capture_plugin_states`], its refusal shown in the status line.
    pub(crate) fn capture_live_states(&mut self) -> Result<(), String> {
        self.capture_plugin_states().inspect_err(|e| self.status.clone_from(e))
    }

    /// Whether the session has unsaved work, for the save question (Quit, New, Open, closing the
    /// window). The hosted plug-ins' live states are read into the session first, so a change none
    /// of them reported still counts; when they cannot be read now, the session's own answer
    /// stands (its edits and the changes plug-ins reported).
    pub fn unsaved_changes(&mut self) -> bool {
        if self.player.is_some() && any_hosted(self.engine.session()) {
            let _ = self.capture_plugin_states();
        }
        self.engine.is_dirty()
    }
}

#[cfg(test)]
#[path = "state_guard_tests.rs"]
mod state_guard_tests;
