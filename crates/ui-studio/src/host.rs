//! The host loop: transport requests, recording, plug-in editor edits and state capture,
//! autosave, device trouble. None of it is UI, but the engine leaves it to the front end
//! (plan 06, section 3, "the host problem").
//!
//! **Copied** from `crates/ui-egui/src/lib.rs` (`SoundApp`) at `transit` 2af868e (file last changed
//! 04e1fe8). Every engine fix SoundCraft makes there must be ported here until the loop moves into
//! an egui-free host crate. Carried fixes, by escape number: E3 (instruments sound while stopped:
//! the player is never stopped for that), E5 (quit order: `close_input` before the player is
//! dropped, see `apps/transit` `on_exit`), E7 (`Player::idle` every frame and a 30 Hz repaint while
//! an editor is open, see `StudioApp::logic`), E9/E10 (the input is taken from the player with
//! `Player::open_input`, which opens it inside `with_output_paused` on ASIO, and given back with
//! `Player::close_input`).
//!
//! Adapted, and marked where: play and record start at the cursor, not at the edit selection
//! (plan 06, L6); the stop-time insertion and scroll edits that read `transit-ui-egui`'s view
//! state are left out. The automation write pass is in [`crate::automation::write_pass`] (slice 7).

mod autosave;
mod plugin_state;
mod reconnect;
mod recording;
mod transport;

use crate::{MeterDisplay, StudioApp};
use serde_json::json;
use transit_engine::TransportRequest;
use transit_model::TrackId;
use transit_playback::record::Trouble;
use transit_playback::reopen::ReopenCause;
use transit_playback::{CountIn, OutputTrouble, Player};
use transit_time::{Range, Samples};

/// What [`StudioApp::shut_down`] achieved (issue #104).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutDown {
    /// Plug-in hosting shut down cleanly ([`transit_engine::shutdown_plugin_hosts`]).
    pub clean: bool,
    /// An autosave of unsaved changes was written; otherwise the last save (or an earlier
    /// autosave) stands.
    pub autosaved: bool,
}

impl StudioApp {
    fn meters_live(&self) -> bool {
        self.engine.transport.playing || (self.recorder.is_some() && self.engine.session().needs_live_input())
    }

    /// Done with the input, if open: it goes back to the player (`Player::close_input`), which
    /// keeps an input it opened with the output (ASIO) and closes any other. Must run before the
    /// player is dropped (E5).
    pub fn close_input(&mut self) {
        if self.recording_active() {
            self.finish_recording();
        }
        if let Some(p) = &self.player {
            self.player_requests.result(&mut self.status, p.close_input(&mut self.recorder));
        } else {
            self.recorder = None;
        }
    }

    /// The quit order (E5, issue #104): close the session's autosave (unsaved changes stay in it
    /// unless the user chose Quit), finish any take and hand the input back to the player, release
    /// the player and its plug-in instances, then shut plug-in hosting down. Used by the quit, a
    /// lost graphics device and a panic on the UI thread, under a watchdog
    /// ([`transit_engine::emergency`]).
    pub fn shut_down(&mut self) -> ShutDown {
        let autosaved = self.close_autosave();
        self.close_input();
        self.player = None;
        ShutDown { clean: transit_engine::shutdown_plugin_hosts(), autosaved }
    }

    pub(crate) fn poll_player(&mut self) {
        let Some(p) = &self.player else { return };
        let polled = p.poll_notices(&mut self.player_requests.notices);
        let count = self.player_requests.result(&mut self.status, polled);
        let mut collect = false;
        for index in 0..count.unwrap_or(0).min(self.player_requests.notices.len()) {
            if let Some(notice) = self.player_requests.notices.get_mut(index).and_then(Option::take) {
                collect |= self.player_requests.notice(notice, &mut self.status, &mut self.engine.transport.recording);
            }
        }
        let stopped = self
            .player_requests
            .result(&mut self.status, p.recovery_progress())
            .is_some_and(|frontiers| self.player_requests.recovery(frontiers, &mut self.status));
        if stopped && !p.is_playing() {
            self.engine.transport.position = p.position();
            self.complete_stop();
        }
        if collect && self.recording_active() {
            self.finish_recording();
        }
        if let Some(point) = self.recorder.as_ref().and_then(transit_playback::record::Recorder::applied_start)
            && self.take_plan.as_ref().is_some_and(|plan| !plan.takes.is_empty() && plan.start != point.position.sample)
        {
            self.record_start = point.position.sample;
            if let Some(plan) = &mut self.take_plan {
                plan.start = self.record_start;
            }
            if let Some(root) = self.autosave_dir.clone() {
                self.queue_autosave(root);
            }
        }
    }

    /// Take the input once (for recording and input monitoring) from the player and feed the
    /// player: on ASIO the one it opened with the output, elsewhere a newly opened device.
    pub(crate) fn ensure_input(&mut self) {
        use crate::audio_connection::Connection;
        if let Some(f) = &mut self.audio_devices.fake {
            if f.input.is_none()
                && let Some(p) = &f.connection
            {
                f.input = p.monitored_input(&self.ui.audio).ok();
            }
            return;
        }
        if self.recorder.is_some() || self.recorder_failed {
            return;
        }
        let Some(p) = &self.player else { return };
        match p.monitored_input(&self.ui.audio) {
            Ok(r) => {
                self.recorder = Some(r);
            }
            Err(e) => {
                self.recorder_failed = true;
                self.player_requests.fail(&mut self.status, format!("No audio input: {e}"));
            }
        }
    }
}

/// Instant attack, ~26 dB/s release, 2 s peak hold. Copied from `transit-ui-egui`.
pub fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], gr: f32, dt: f32) {
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    for c in 0..2 {
        let v = peak.get(c).copied().unwrap_or(0.0);
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        if v >= d.hold.get(c).copied().unwrap_or(0.0) {
            if let Some(h) = d.hold.get_mut(c) {
                *h = v;
            }
            d.hold_age = 0.0;
        }
        if v >= 1.0 {
            d.clip = true;
        }
        d.peak = d.peak.max(v);
    }
    d.hold_age += dt;
    if d.hold_age > 2.0 {
        d.hold = d.level;
    }
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// The status line naming the plugin in `slot` that the engine took out of the signal path.
fn failed_plugin_status(s: &transit_model::Session, track: TrackId, slot: usize) -> Option<String> {
    let t = s.track(track)?;
    let ins = if slot == transit_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(slot)?.as_ref() }?;
    Some(format!("{} on {} put out invalid audio, so it is bypassed until playback stops or starts again.", ins.plugin, t.name))
}

/// A refused audio action as a sentence for the status line; the raw variants go to the log only.
pub(crate) fn refusal_text(outcome: transit_playback::action_lane::Outcome, result: Option<transit_playback::ActionResult>) -> String {
    use transit_playback::{ActionResult, action_lane::Outcome, device_clock::Unavailable};
    match result {
        Some(ActionResult::RecordingClock(why)) => {
            let reason = match why {
                Unavailable::WarmingUp => "the audio clock is still warming up; try again in a moment",
                Unavailable::Stale | Unavailable::OutsideWindow => "the audio clock lost track of the device; try again in a moment",
                Unavailable::WrongClock => "no audio input is open on this device",
                Unavailable::Invalid | Unavailable::Reordered | Unavailable::Discontinuity | Unavailable::Arithmetic => {
                    "the audio device's clock is not usable right now"
                }
            };
            format!("Recording can't start: {reason}.")
        }
        Some(ActionResult::RecordingNotPlaying) => "Recording can't start: the transport is not playing.".into(),
        Some(ActionResult::RecordingCancelled) => "Recording was canceled before it could start.".into(),
        Some(ActionResult::InvalidPosition) => "The audio engine can't go there: the position is out of range.".into(),
        Some(ActionResult::Timeline(_) | ActionResult::MidiTimeline(_)) => {
            "The audio engine couldn't follow the timeline change; playback may need to be restarted.".into()
        }
        Some(ActionResult::MidiClock(_)) => "A MIDI note was dropped: the audio clock is not ready yet.".into(),
        Some(ActionResult::MidiFull) => "A MIDI note was dropped: the audio engine is busy.".into(),
        _ => match outcome {
            Outcome::PlanSuperseded | Outcome::PlanRejected => "The audio engine rejected that change because the session changed under it.".into(),
            Outcome::Faulted | Outcome::Shutdown => "The audio engine stopped before it could apply that.".into(),
            _ => "The audio engine couldn't apply that action.".into(),
        },
    }
}

/// Admission is separate from callback application. Storage is reused every frame,
/// including idle frames, so terminal results always return admission credits.
/// Said once at Stop when the audio take started without a validated device clock.
const APPROXIMATE_TAKE: &str = "Recorded with approximate alignment: the audio device clock did not validate";

/// A finished pass's MIDI journal, kept until the autosaver has the clips.
pub(crate) struct PlacedJournal {
    journal: transit_engine::midi_journal::Writer,
    /// The recording plan the autosave carries meanwhile: the clips are already placed.
    plan: transit_engine::io::Recording,
    step: JournalStep,
}

/// Where a held journal is on its way to deletion.
#[derive(Clone, Copy)]
pub(crate) enum JournalStep {
    /// No autosave carrying the placed plan has been handed over yet.
    Queue,
    /// That autosave is handed over (landed once the autosaver says so).
    Placed(transit_engine::autosave::Ticket),
    /// It landed: hand over one that no longer carries the recording.
    Release,
    /// That one is handed over; once it lands the journal goes.
    Cleared(transit_engine::autosave::Ticket),
}

pub(crate) struct PlayerRequests {
    notices: [Option<transit_playback::PlayerNotice>; 64],
    pub(crate) play: Option<transit_playback::action_lane::Sequence>,
    pub(crate) locate: Option<transit_playback::action_lane::Sequence>,
    stop: Option<transit_playback::action_lane::Sequence>,
    release: Option<transit_playback::action_lane::Sequence>,
    pub(crate) recording: Option<transit_playback::action_lane::Sequence>,
    error: Option<String>,
}

impl Default for PlayerRequests {
    fn default() -> Self {
        Self { notices: [None; 64], play: None, locate: None, stop: None, release: None, recording: None, error: None }
    }
}

impl PlayerRequests {
    pub(crate) fn reset(&mut self) {
        let error = self.error.take();
        *self = Self { error, ..Self::default() };
    }

    pub(crate) fn pending(&self) -> bool {
        self.play.is_some() || self.locate.is_some() || self.stop.is_some() || self.release.is_some() || self.recording.is_some()
    }

    pub(crate) fn fail(&mut self, status: &mut String, message: String) {
        log::warn!("{message}");
        let error = self.error.get_or_insert_with(String::new);
        if !error.is_empty() {
            error.push_str("; ");
        }
        error.push_str(&message);
        status.clone_from(error);
    }

    pub(crate) fn result<T>(&mut self, status: &mut String, result: Result<T, transit_playback::PlayerError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                if error == transit_playback::PlayerError::Faulted {
                    self.reset();
                }
                self.fail(status, error.to_string());
                None
            }
        }
    }

    /// Returns true when a refused start requires collecting an armed take.
    pub(crate) fn notice(&mut self, notice: transit_playback::PlayerNotice, status: &mut String, recording: &mut bool) -> bool {
        use transit_playback::{ActionResult, PlayerCommand, PlayerNotice, action_lane::Outcome, player::SessionOutcome};
        match notice {
            PlayerNotice::Session(n) => {
                if let SessionOutcome::Rejected(reason) = n.outcome {
                    self.fail(status, format!("Audio session {} rejected: {reason:?}", n.request.get()));
                }
            }
            PlayerNotice::Action(r) => {
                let applied = r.outcome == Outcome::Applied
                    && matches!(
                        r.handler_result,
                        Some(
                            ActionResult::Applied
                                | ActionResult::RecordingStarted(_)
                                | ActionResult::Event(
                                    transit_mix::EventOutcome::Queued
                                        | transit_mix::EventOutcome::QueuedLate { .. }
                                        | transit_mix::EventOutcome::ReleaseAdvanced { .. }
                                        | transit_mix::EventOutcome::Coalesced
                                )
                        )
                    );
                if self.locate == Some(r.sequence) {
                    self.locate = None;
                }
                let play = self.play == Some(r.sequence);
                let record = self.recording == Some(r.sequence);
                if play {
                    self.play = None;
                    if applied && status == "Play pending" {
                        *status = "Playing".into();
                    }
                }
                if record {
                    self.recording = None;
                    if applied {
                        match r.payload {
                            PlayerCommand::Recording(on) => *recording = on,
                            PlayerCommand::Record { .. } => {
                                *recording = true;
                                if status == "Record pending" {
                                    *status = "Recording".into();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if !applied && !matches!(r.outcome, Outcome::Stopped | Outcome::NotesReleased) {
                    log::warn!("Audio action {} refused: {:?} ({:?})", r.sequence.get(), r.outcome, r.handler_result);
                    self.fail(status, refusal_text(r.outcome, r.handler_result));
                }
                return (play || record) && !applied && self.stop.is_none();
            }
        }
        false
    }

    pub(crate) fn recovery(
        &mut self,
        frontiers: (transit_playback::action_lane::Sequence, transit_playback::action_lane::Sequence),
        status: &mut String,
    ) -> bool {
        let stopped = self.stop.is_some_and(|s| frontiers.0 >= s);
        if stopped {
            self.stop = None;
            if status == "Stop pending" {
                *status = "Stopped".into();
            }
        }
        if self.release.is_some_and(|s| frontiers.1 >= s) {
            self.release = None;
            *status = "All MIDI notes off".into();
        }
        stopped
    }

    pub(crate) fn show_error(&mut self, status: &mut String) {
        if let Some(error) = self.error.take() {
            *status = error;
        }
    }
}

#[cfg(test)]
#[path = "player_tests.rs"]
mod player_tests;

/// How far a recording pass has run on the transport: the last position seen and the cycle laps
/// it has wrapped over since, so `last + laps` only ever grows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordClock {
    pub(crate) last: Samples,
    pub(crate) laps: Samples,
}

/// A MIDI pass under way, for drawing: the armed tracks, where it started and the playhead, and
/// the notes played so far (open notes closed at the playhead).
#[derive(Debug, Clone)]
pub(crate) struct MidiLive {
    pub tracks: Vec<TrackId>,
    pub start: Samples,
    pub end: Samples,
    pub built: Vec<transit_engine::midi_take::Built>,
}
