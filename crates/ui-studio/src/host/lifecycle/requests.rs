//! What the host keeps about its tracked Player requests beyond their sequences (ADR 0062 slice
//! S1), and the classification of receipts and frontiers that the status-text wrappers
//! (`PlayerRequests::notice`, `PlayerRequests::recovery`) apply today's rules to.

use super::phase::StopReason;
use crate::host::PlayerRequests;
use transit_playback::action_lane::{Outcome, Receipt, Sequence};
use transit_playback::record::start::Reader;
use transit_playback::{ActionResult, CountIn, PlayerCommand, PlayerNotice};
use transit_time::Samples;

/// The request metadata the phase is derived from. Each entry is read only while the request it
/// describes is still tracked (its sequence matches), so nothing here has to be cleared when a
/// request retires.
#[derive(Default)]
pub(crate) struct RequestMeta {
    /// The last Play-family start submitted: its sequence and where it starts.
    pub(crate) play_start: Option<(Sequence, Samples)>,
    /// Why the last Stop was required. A Reset stays while a Stop is still tracked: a later Stop
    /// or take-safe locate does not turn a replaced session's Stop into a Park.
    pub(crate) stop_reason: StopReason,
    /// The last record-off (`set_recording(false)`) submitted.
    pub(crate) record_off: Option<Sequence>,
    /// The record start of the planned take, once submitted.
    pub(crate) pass: Option<PassMeta>,
}

/// A submitted record start: the ticket-owned reader of its start stage, its request, and what it
/// was submitted with.
pub(crate) struct PassMeta {
    pub(crate) reader: Reader,
    pub(crate) request: Sequence,
    pub(crate) from: Option<Samples>,
    pub(crate) count_in: Option<CountIn>,
}

impl RequestMeta {
    /// A Stop is required for `reason`; `stop_tracked` says whether one is tracked already.
    pub(crate) fn stop_required(&mut self, reason: StopReason, stop_tracked: bool) {
        if !(stop_tracked && self.stop_reason == StopReason::Reset) {
            self.stop_reason = reason;
        }
    }
}

/// A notice classified, the tracked request it acknowledges already retired.
pub(crate) enum Classified {
    /// A session notice; the text when the Player rejected the session.
    Session(Option<String>),
    /// An action receipt.
    Action {
        receipt: Receipt<PlayerCommand, ActionResult>,
        /// Applied, with a handler result that counts as success.
        applied: bool,
        /// It acknowledged the tracked Play.
        play: bool,
        /// It acknowledged the tracked recording request (a Record or a record-off).
        record: bool,
    },
}

/// Which tracked requests the acknowledgment frontiers retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frontiers {
    pub(crate) stop: bool,
    pub(crate) release: bool,
}

impl PlayerRequests {
    /// Classify `notice` and retire the tracked request it acknowledges. No status text or flag
    /// is written here.
    pub(crate) fn classify(&mut self, notice: PlayerNotice) -> Classified {
        use transit_playback::player::SessionOutcome;
        match notice {
            PlayerNotice::Session(n) => Classified::Session(match n.outcome {
                SessionOutcome::Rejected(reason) => Some(format!("Audio session {} rejected: {reason:?}", n.request.get())),
                _ => None,
            }),
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
                }
                if record {
                    self.recording = None;
                }
                Classified::Action { receipt: r, applied, play, record }
            }
        }
    }

    /// Retire the tracked Stop and release the acknowledgment `frontiers` (stop, release) reached.
    pub(crate) fn frontiers(&mut self, frontiers: (Sequence, Sequence)) -> Frontiers {
        let stop = self.stop.is_some_and(|s| frontiers.0 >= s);
        if stop {
            self.stop = None;
        }
        let release = self.release.is_some_and(|s| frontiers.1 >= s);
        if release {
            self.release = None;
        }
        Frontiers { stop, release }
    }
}
