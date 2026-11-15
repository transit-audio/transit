//! The timed autosave and its read of hosted plug-in states (issue #574, rounds 2 and 3).
//!
//! Every two minutes the hosted plug-ins' live states are read into the session, also when it is
//! clean, so a plug-in that never says its state changed is still protected, and the session is
//! copied to the autosave folder when it changed since the last timed copy.
//!
//! - **Deferred, not dropped:** the read waits while the transport plays (a slow state read would
//!   stall the playing UI) and while a gesture is open (the automation write pass, a held control,
//!   a plug-in parameter being moved): storing a state ends the gesture's merge and would split its
//!   undo step in two. It runs on the first frame after Stop or after the gesture closes. While
//!   recording nothing runs.
//! - **Clean** here means: nothing changed the session since the last timed read except that read
//!   (no edit, no undo or redo, no plug-in notice), or it is saved and unmodified.
//! - **Noisy** plug-in: one whose state bytes changed on two timed reads in a row while the session
//!   was clean in that sense, so with no notice and no parameter edit (the bytes carry a timestamp,
//!   an id, a window position). The timed read skips it for the rest of the session, which bounds
//!   the false "modified" it causes to one per plug-in. It is still read before saves, Quit, New,
//!   Open and every drop (the full capture).

use super::plugin_state::STILL_UPDATING;
use super::*;
use std::time::{Duration, Instant};

/// How long a refused timed read waits before it tries again.
const RETRY: Duration = Duration::from_secs(10);
/// How often the timed autosave runs.
const INTERVAL: Duration = Duration::from_secs(120);
/// Changes in a row on a clean session that make a plug-in noisy.
const NOISY_AFTER: u8 = 2;

/// The hosted device at `slot` of track `t` (`INSTRUMENT_SLOT`: its instrument).
fn device_at(s: &transit_model::Session, t: TrackId, slot: usize) -> Option<&transit_model::Insert> {
    let tr = s.track(t)?;
    if slot == transit_mix::INSTRUMENT_SLOT { tr.instrument.as_ref() } else { tr.mixer.inserts.get(slot)?.as_ref() }
}

impl StudioApp {
    /// The timed autosave (see the module docs). Timed by the wall clock: egui's stands still while
    /// the window is hidden.
    pub(crate) fn autosave(&mut self) {
        let Some(dir) = self.autosave_dir.clone() else { return };
        let now = Instant::now();
        let last = *self.last_autosave.get_or_insert(now);
        if self.engine.transport.recording {
            return;
        }
        if now.duration_since(last) >= INTERVAL {
            self.last_autosave = Some(now);
            self.state_guard.read_due = self.hosts_live_plugins();
            self.state_guard.write_due = true;
        }
        if self.state_guard.read_due {
            if self.is_playing() || self.gesture_open() || self.state_guard.retry_at.is_some_and(|at| now < at) {
                return;
            }
            if self.read_states_timed().is_err() {
                self.state_guard.retry_at = Some(now + RETRY);
                return;
            }
            self.state_guard.read_due = false;
            self.state_guard.retry_at = None;
        }
        if std::mem::take(&mut self.state_guard.write_due)
            && self.engine.is_dirty()
            && self.engine.revision != self.state_guard.timed_rev
            && self.autosave_to(&dir)
        {
            self.state_guard.timed_rev = self.engine.revision;
            self.state_guard.timed_writes += 1;
        }
    }

    /// Whether a gesture is open that a state store would split: the automation write pass, the
    /// pointer held down, or a plug-in parameter being moved.
    fn gesture_open(&self) -> bool {
        self.state_guard.pointer_down || self.engine.merge_held() || !self.auto.touches.is_empty()
    }

    /// The timed read: the live hosted states but the noisy ones, read within
    /// [`Guard::timed_budget`](super::state_guard::Guard) (see [`super::state_guard::TIMED_READ_BUDGET`]) and stored where they changed. Refused,
    /// with no change, while Play, Locate or Record is pending or the audio side has not applied
    /// the current document. A plug-in left out keeps its stored state and is read next time.
    pub(crate) fn read_states_timed(&mut self) -> Result<(), String> {
        let action_pending = self.player_requests.play.is_some() || self.player_requests.locate.is_some() || self.player_requests.recording.is_some();
        if action_pending || !self.sync_player_session() {
            return Err(STILL_UPDATING.into());
        }
        let session = self.engine.session_arc();
        let g = &self.state_guard;
        let clean = self.engine.plugin_notices() == g.notices_seen && (g.read_rev == Some(self.engine.revision) || !self.engine.is_dirty());
        let skip: Vec<(TrackId, usize)> = session
            .tracks
            .iter()
            .flat_map(|t| {
                let inserts = t.mixer.inserts.iter().enumerate().filter_map(|(i, ins)| ins.as_ref().map(|ins| (i, ins)));
                inserts
                    .chain(t.instrument.as_ref().map(|ins| (transit_mix::INSTRUMENT_SLOT, ins)))
                    .map(move |(slot, ins)| (t.id, slot, ins.uid.get()))
            })
            .filter(|(_, _, uid)| g.noisy.contains(uid))
            .map(|(t, slot, _)| (t, slot))
            .collect();
        let Some(p) = &self.player else { return Ok(()) };
        let read = p.capture_states_timed(&session, self.state_guard.timed_budget, &skip).map(|(states, _all)| states);
        if let Ok(states) = &read {
            for (t, slot, data) in states {
                let Some(ins) = device_at(&session, *t, *slot) else { continue };
                let uid = ins.uid.get();
                let changed = ins.state_bytes().as_deref() != Some(&data[..]);
                let streak = self.state_guard.streak.entry(uid).or_insert(0);
                *streak = if clean && changed { streak.saturating_add(1) } else { 0 };
                if *streak >= NOISY_AFTER {
                    log::info!("{}: its state changes on every read; the timed read skips it from now on", ins.plugin);
                    self.state_guard.noisy.insert(uid);
                }
            }
        }
        self.store_plugin_states(read)?;
        self.state_guard.read_rev = Some(self.engine.revision);
        self.state_guard.notices_seen = self.engine.plugin_notices();
        Ok(())
    }
}
