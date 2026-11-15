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

use super::plugin_state::STILL_UPDATING;
use super::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    named_tracks_where(s, params, |t| hosted(t).next().is_some())
}

/// Whether a track `params` names (as for [`named_tracks_host`]) passes `test`; with none named,
/// whether any track does.
fn named_tracks_where(s: &Session, params: &Value, test: impl Fn(&Track) -> bool) -> bool {
    let one = |v: &Value| match (v.as_u64(), v.as_str()) {
        (Some(n), _) => s.track(TrackId(n)),
        (_, Some(name)) => s.track_by_name(name),
        _ => None,
    };
    let named: Vec<&Value> = match (params.get("tracks").and_then(Value::as_array), params.get("track")) {
        (Some(list), _) => list.iter().collect(),
        (None, Some(v)) => vec![v],
        (None, None) => return s.tracks.iter().any(&test),
    };
    named.into_iter().filter_map(one).any(test)
}

/// Whether the insert slot `params` names (`track` and `slot`: an index or a letter) holds a
/// third-party device. A slot that cannot be read here counts as the whole track (the command
/// itself refuses what it cannot read).
fn named_slot_hosts(s: &Session, params: &Value) -> bool {
    let track = match params.get("track") {
        Some(v) => match (v.as_u64(), v.as_str()) {
            (Some(n), _) => s.track(TrackId(n)),
            (_, Some(name)) => s.track_by_name(name),
            _ => None,
        },
        None => None,
    };
    let slot = params.get("slot").and_then(|v| match (v.as_u64(), v.as_str().map(|x| x.trim().to_ascii_lowercase())) {
        (Some(n), _) => usize::try_from(n).ok(),
        (_, Some(x)) => match x.as_bytes() {
            [c @ b'a'..=b'z'] => Some(usize::from(c - b'a')),
            _ => x.parse().ok(),
        },
        _ => None,
    });
    match (track, slot) {
        (Some(t), Some(i)) => t.mixer.inserts.get(i).and_then(Option::as_ref).is_some_and(|ins| transit_mix::is_third_party(&ins.plugin)),
        _ => named_tracks_host(s, params),
    }
}

/// How long a capture before a drop waits for the audio side to apply the current document
/// before it refuses (round 2 of #574): about ten frames, well under the capture's own 500 ms
/// modal deadline. Nothing waits while a plug-in is being made or the device reopens.
pub(crate) const PLAN_WAIT: Duration = Duration::from_millis(150);
/// The timed autosave's read budget: the read runs on the UI thread every two minutes, so it is
/// held to about six frames; a plug-in read that is already running is never cut (one slow read
/// can run past it). The next read goes on where this one stopped, so every plug-in is read
/// within `ceil(total read time / budget)` intervals.
pub(crate) const TIMED_READ_BUDGET: Duration = Duration::from_millis(100);

/// The state of the protection (the tests change the durations).
pub(crate) struct Guard {
    /// The bounded wait before a refused capture ([`PLAN_WAIT`]).
    pub(crate) plan_wait: Duration,
    /// The timed autosave's read budget ([`TIMED_READ_BUDGET`]).
    pub(crate) timed_budget: Duration,
    /// An Undo History jump captured once for all its steps: `run` does not capture again.
    pub(crate) jump_captured: bool,
    /// The revision the last timed autosave wrote.
    pub(crate) timed_rev: u64,
    /// Timed autosaves written (the tests count them).
    pub(crate) timed_writes: u64,
}

impl Default for Guard {
    fn default() -> Self {
        Guard { plan_wait: PLAN_WAIT, timed_budget: TIMED_READ_BUDGET, jump_captured: false, timed_rev: 0, timed_writes: 0 }
    }
}

/// The stored states of `s`'s hosted devices, by uid.
fn hosted_states(s: &Session) -> HashMap<u64, Vec<u8>> {
    s.tracks
        .iter()
        .flat_map(|t| t.mixer.inserts.iter().flatten().chain(t.instrument.as_ref()))
        .filter(|i| transit_mix::is_third_party(&i.plugin))
        .filter_map(|i| Some((i.uid.get(), i.state_bytes()?)))
        .collect()
}

/// Whether a track holds a third-party instrument.
fn hosted_instrument(t: &Track) -> bool {
    t.instrument.as_ref().is_some_and(|i| transit_mix::is_third_party(&i.plugin))
}

impl StudioApp {
    /// Whether running `id` with `params` may drop a live hosted instance (see the module docs).
    pub(crate) fn drops_live_state(&self, id: &str, params: &Value) -> bool {
        if self.state_guard.jump_captured {
            return false;
        }
        let s = self.engine.session();
        match id {
            "edit.undo" => self.engine.undo_doc().is_some_and(|d| drops_hosted(s, d)),
            "edit.redo" => self.engine.redo_doc().is_some_and(|d| drops_hosted(s, d)),
            // A width change makes its hosted instances again at once, from the stored state
            // (the instance's identity includes its channel count).
            "track.delete" | "track.change_width" | "track.load_preset" => named_tracks_host(s, params),
            "mix.instrument" | "instrument.external" | "mix.sampler_new_patch" => named_tracks_where(s, params, hosted_instrument),
            "mix.insert_remove" => named_slot_hosts(s, params),
            // An insert into a named slot replaces what is there.
            "mix.insert" => params.get("slot").is_some() && named_slot_hosts(s, params),
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

    /// [`Self::capture_plugin_states`] before a drop. When the audio side has not applied the
    /// current document yet, it first waits up to [`Guard::plan_wait`] for it (servicing the
    /// player and publishing the document about every 2 ms), unless a plug-in is being made or the
    /// device is reopening. A refusal is shown in the status line in the drop's own words.
    pub(crate) fn capture_live_states(&mut self) -> Result<(), String> {
        self.wait_for_plan();
        self.capture_plugin_states().map_err(|e| {
            let reopening = self.player.as_ref().is_some_and(|p| p.reopen_cause().is_some());
            let pending = e == STILL_UPDATING || e == transit_playback::PlayerError::SessionPending.to_string();
            let said = if reopening {
                "Can't do that yet: the plug-ins' current settings can't be read while the audio device reopens. Try again once it is back."
                    .to_string()
            } else if pending {
                "Can't do that yet: the plug-ins' current settings could not be read yet. Try again in a moment.".to_string()
            } else {
                e
            };
            self.status.clone_from(&said);
            said
        })
    }

    /// The bounded wait of [`Self::capture_live_states`]: returns once the player plays the
    /// current document, or when waiting cannot help or the time is up. No lock is held; the audio
    /// side never waits on this thread.
    fn wait_for_plan(&mut self) {
        let Some(p) = &self.player else { return };
        if p.reopen_cause().is_some() || p.any_loading() {
            return;
        }
        let start = Instant::now();
        loop {
            let session = self.engine.session_arc();
            let Some(p) = &self.player else { return };
            if p.plays(&session) || start.elapsed() >= self.state_guard.plan_wait {
                return;
            }
            if p.service().is_err() {
                return;
            }
            self.poll_player();
            if !self.sync_player_session() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Whether the session has unsaved work, for the save question (Quit, New, Open, closing the
    /// window). The hosted plug-ins' live states are read into the session first, so a change none
    /// of them reported still counts; when they cannot be read now, the session's own answer
    /// stands (its edits and the changes plug-ins reported).
    pub fn unsaved_changes(&mut self) -> bool {
        if self.hosts_live_plugins() {
            let _ = self.capture_plugin_states();
        }
        self.engine.is_dirty()
    }

    /// Undo until `keep` steps of the history remain (Edit > Undo History). When any step on the
    /// way drops a hosted plug-in, the live states are captured once first and then every step
    /// runs; when that capture is refused, nothing is undone and the status says why. A step
    /// refused for another reason stops the jump, and the status says how far it got and why.
    pub fn undo_steps(&mut self, keep: usize) -> usize {
        let want = self.engine.undo_history().len().saturating_sub(keep);
        if want == 0 {
            return 0;
        }
        let mut prev = self.engine.session_arc();
        let mut drops = false;
        for back in 0..want {
            let Some(doc) = self.engine.undo_doc_back(back) else { break };
            drops |= drops_hosted(&prev, doc);
            prev = Arc::clone(doc);
        }
        if drops && let Err(e) = self.capture_live_states() {
            self.status = format!("Nothing undone: {e}");
            return 0;
        }
        // Each step's redo snapshot is the document before it: the captured states are carried
        // into it (no frame passes, so the live states are still the captured ones).
        let captured = if drops { hosted_states(self.engine.session()) } else { HashMap::new() };
        self.state_guard.jump_captured = drops;
        let mut n = 0;
        let mut stopped = None;
        while n < want && self.engine.undo_history().len() > keep {
            if n > 0
                && let Err(e) = self.carry_states(&captured)
            {
                stopped = Some(e);
                break;
            }
            if let Err(e) = self.run("edit.undo", json!({})) {
                stopped = Some(e);
                break;
            }
            n += 1;
        }
        self.state_guard.jump_captured = false;
        let steps = |k: usize| if k == 1 { "step" } else { "steps" };
        self.status = match stopped {
            Some(e) => format!("Undid {n} of {want} {}: {e}", steps(want)),
            None => format!("Undid {n} {}", steps(n)),
        };
        n
    }

    /// Stores `captured` (uid to state) into the current document's hosted devices whose stored
    /// state differs (`mix.insert_state`, not an undo step).
    fn carry_states(&mut self, captured: &HashMap<u64, Vec<u8>>) -> Result<(), String> {
        let mut writes = Vec::new();
        for t in &self.engine.session().tracks {
            let slots = t.mixer.inserts.iter().enumerate().filter_map(|(i, ins)| ins.as_ref().map(|ins| (json!(i), ins)));
            for (slot, ins) in slots.chain(t.instrument.as_ref().map(|ins| (json!("instrument"), ins))) {
                if let Some(state) = captured.get(&ins.uid.get())
                    && transit_mix::is_third_party(&ins.plugin)
                    && ins.state_bytes().as_deref() != Some(&state[..])
                {
                    writes.push(json!({"track": t.id.0, "slot": slot, "state": transit_model::b64::encode(state)}));
                }
            }
        }
        for w in writes {
            self.execute_ui("mix.insert_state", &w).map_err(|e| format!("Cannot store plugin state: {e}"))?;
        }
        Ok(())
    }

    /// The timed autosave's read (round 2 of #574): the live hosted states, read within
    /// [`Guard::timed_budget`] (see [`TIMED_READ_BUDGET`]) and stored where they changed. Refused,
    /// with no change, when the audio side has not applied the current document. A plug-in left
    /// out this time keeps its stored state and is read next time; nothing is said.
    pub(crate) fn read_states_timed(&mut self) -> Result<(), String> {
        let action_pending = self.player_requests.play.is_some() || self.player_requests.locate.is_some() || self.player_requests.recording.is_some();
        if action_pending || !self.sync_player_session() {
            return Err(STILL_UPDATING.into());
        }
        let Some(p) = &self.player else { return Ok(()) };
        let read = p.capture_states_timed(&self.engine.session_arc(), self.state_guard.timed_budget).map(|(states, _all)| states);
        self.store_plugin_states(read)
    }

    /// Whether the session holds a hosted device and a player runs it.
    pub(crate) fn hosts_live_plugins(&self) -> bool {
        self.player.is_some() && any_hosted(self.engine.session())
    }
}

#[cfg(test)]
#[path = "state_guard_tests.rs"]
mod state_guard_tests;
