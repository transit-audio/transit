//! The pure reducer `next(phase, event, facts)` of ADR 0062 (slice S2a): the transition matrix of
//! tables.md section A ([`super::matrix`]) applied to plain data. Unused by the app: the host
//! adopts it in slice S2d.
//!
//! Conventions, from the matrix's preambles:
//! - The host applies a receipt's own retirement (and an older ticket's terminal stage) to the
//!   facts before calling `next`: a receipt "first retires its fact in every column". A receipt
//!   with no live fact is not an event (it has no lifecycle effect), except an untracked refusal
//!   (R12).
//! - The facts are read, never written: what the transition does to them is in
//!   [`Step::effects`], in the order the cell names it. Submitting a request is an effect; the host
//!   tracks it (or owes a Stop it could not send, W1).
//! - The record start's ticket is prepared by the host, outside the reducer: Record (C7) and the
//!   Resume check (H2) carry it as [`RecordPrep`]. The preparation contract (record section 3):
//!   `from: None` is a punch into existing playback, `from: Some(at)` an explicit start, and a
//!   count-in is prepared only with `from: Some(at)`. The reducer leaves a pass whose payloads
//!   break the contract unchanged (R3 and O1 in RP).
//! - R6, R7 and R8 in StopPending Resume only schedule the Resume check ([`Effect::ResumeCheck`]):
//!   the host runs H2 at its H4 step with fresh facts and a prepared start, and H2 rechecks its
//!   whole precondition.
//! - Render result feedback: the host feeds a writer result a cancel returned through E3 before
//!   it allows E5 or another admission (record section 11).
//! - A transition out of StartPending or Playing into any other phase sets speed Normal (record
//!   section 3, the matrix legend): [`next`] adds `SetSpeed(Normal)` first when the cell did not.
//! - No clock: the owed-Stop deadline compares the monotonic time the event carries with the
//!   first-owed stamp in the facts.

use super::matrix::{self, Col, Op, Refusal, Row};
use super::phase::{AfterStop, CancelReason, ParkEnd, RenderJob, RenderOutcome, TransportPhase};
use std::time::Duration;
use transit_playback::CountIn;
use transit_playback::record::start::{Point, Ticket};
use transit_time::Samples;

/// Resume defers at most this many commands; one more is refused (record section 4, U-3).
pub const DEFERRED_LIMIT: usize = 32;
/// An owed Stop still unsent this long after it was first owed faults the runtime (U-1).
pub const OWED_STOP_DEADLINE: Duration = Duration::from_secs(2);

/// The transport speed (record section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Speed {
    #[default]
    Normal,
    Half,
}

/// A prepared record start: the ticket and what the start is submitted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordStart {
    pub ticket: Ticket,
    pub from: Option<Samples>,
    pub count_in: Option<CountIn>,
}

/// What preparing a record start gave: the start, or the local refusal (Ta, Te).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordPrep {
    Ready(RecordStart),
    NoArmedTrack,
    Cannot,
}

/// A start command, as submitted or held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartCommand {
    Play,
    PlayExact(Samples),
    PlaySelection,
    TogglePlay,
    Record,
    HalfSpeed,
}

/// Park's one held start, stamped with the reset generation current when it was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub start: StartCommand,
    pub generation: u64,
}

/// Which Play a Play receipt acknowledges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayOf {
    /// A start group's member; `current` when its group is the current one.
    Group { current: bool },
    /// The render's Play.
    Render,
}

/// An event: one row of the matrix each (see [`Event::row`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Play,
    PlayExact(Samples),
    PlaySelection,
    TogglePlay,
    Stop,
    Pause,
    Record(RecordPrep),
    Locate(Samples),
    HalfSpeed,
    Scrub(Samples),
    AllNotesOff,
    ResetSession,
    RulerClick(Samples),
    PlayApplied(PlayOf),
    PlayRefused(PlayOf),
    RecordAdmitted(Ticket),
    RecordStarted {
        ticket: Ticket,
        latch: Point,
    },
    /// `retired_by_stage`: a Failed or Cancelled stage already retired the Record.
    RecordRefused {
        ticket: Ticket,
        retired_by_stage: bool,
    },
    StopFrontier,
    RecordOffApplied,
    RecordOffRefused,
    LocateOrLoopReceipt {
        applied: bool,
    },
    ReleaseFrontier,
    SessionRejected,
    UntrackedRefusal,
    PlayerFaulted,
    StageCountingIn(Ticket),
    StageWaiting(Ticket),
    StageStarted {
        ticket: Ticket,
        latch: Point,
    },
    StageFailed(Ticket),
    StageCancelled(Ticket),
    StoppedByItself,
    PlaybackObserved,
    RenderStart,
    RenderCancel(CancelReason),
    RenderPoll,
    RenderResult(RenderOutcome),
    RenderSettled,
    /// The owed-Stop retry on a host tick, at the monotonic time `now`.
    OwedStop {
        now: Duration,
    },
    OutputTrouble,
    InputTrouble,
    DriverReset,
    ReplaceRequested,
    OldPlayerRetired,
    MidiTransportLost,
    InputLoss,
    LoopChanged,
    LapLimit,
    ResumeCheck(RecordPrep),
}

impl Event {
    /// The event's row in tables.md section A.
    pub fn row(&self) -> Row {
        match self {
            Event::Play => Row::C1,
            Event::PlayExact(_) => Row::C2,
            Event::PlaySelection => Row::C3,
            Event::TogglePlay => Row::C4,
            Event::Stop => Row::C5,
            Event::Pause => Row::C6,
            Event::Record(_) => Row::C7,
            Event::Locate(_) => Row::C8,
            Event::HalfSpeed => Row::C9,
            Event::Scrub(_) => Row::C10,
            Event::AllNotesOff => Row::C11,
            Event::ResetSession => Row::C12,
            Event::RulerClick(_) => Row::C13,
            Event::PlayApplied(_) => Row::R1,
            Event::PlayRefused(_) => Row::R2,
            Event::RecordAdmitted(_) => Row::R3,
            Event::RecordStarted { .. } => Row::R4,
            Event::RecordRefused { .. } => Row::R5,
            Event::StopFrontier => Row::R6,
            Event::RecordOffApplied => Row::R7,
            Event::RecordOffRefused => Row::R8,
            Event::LocateOrLoopReceipt { .. } => Row::R9,
            Event::ReleaseFrontier => Row::R10,
            Event::SessionRejected => Row::R11,
            Event::UntrackedRefusal => Row::R12,
            Event::PlayerFaulted => Row::R13,
            Event::StageCountingIn(_) => Row::O1,
            Event::StageWaiting(_) => Row::O2,
            Event::StageStarted { .. } => Row::O3,
            Event::StageFailed(_) => Row::O4,
            Event::StageCancelled(_) => Row::O5,
            Event::StoppedByItself => Row::O6,
            Event::PlaybackObserved => Row::O7,
            Event::RenderStart => Row::E1,
            Event::RenderCancel(_) => Row::E2,
            Event::RenderPoll => Row::E3a,
            Event::RenderResult(_) => Row::E3,
            Event::RenderSettled => Row::E5,
            Event::OwedStop { .. } => Row::W1,
            Event::OutputTrouble => Row::V1,
            Event::InputTrouble => Row::V2,
            Event::DriverReset => Row::V3,
            Event::ReplaceRequested => Row::V4a,
            Event::OldPlayerRetired => Row::V4b,
            Event::MidiTransportLost => Row::V5,
            Event::InputLoss => Row::V6,
            Event::LoopChanged => Row::L1,
            Event::LapLimit => Row::T1,
            Event::ResumeCheck(_) => Row::H2,
        }
    }

    /// The target position the event names.
    fn at(&self) -> Option<Samples> {
        match *self {
            Event::PlayExact(at) | Event::Locate(at) | Event::Scrub(at) | Event::RulerClick(at) => Some(at),
            _ => None,
        }
    }

    /// The record-start ticket a receipt or stage names.
    fn ticket(&self) -> Option<Ticket> {
        match *self {
            Event::RecordAdmitted(t)
            | Event::RecordStarted { ticket: t, .. }
            | Event::RecordRefused { ticket: t, .. }
            | Event::StageCountingIn(t)
            | Event::StageWaiting(t)
            | Event::StageStarted { ticket: t, .. }
            | Event::StageFailed(t)
            | Event::StageCancelled(t) => Some(t),
            _ => None,
        }
    }

    /// The start this command submits or holds.
    fn start(&self) -> Option<StartCommand> {
        match *self {
            Event::Play => Some(StartCommand::Play),
            Event::PlayExact(at) => Some(StartCommand::PlayExact(at)),
            Event::PlaySelection => Some(StartCommand::PlaySelection),
            Event::TogglePlay => Some(StartCommand::TogglePlay),
            Event::Record(_) => Some(StartCommand::Record),
            Event::HalfSpeed => Some(StartCommand::HalfSpeed),
            _ => None,
        }
    }

    /// The prepared record start the event carries.
    fn prep(&self) -> Option<RecordPrep> {
        match *self {
            Event::Record(prep) | Event::ResumeCheck(prep) => Some(prep),
            _ => None,
        }
    }

    /// Why a render cancelled by this event was cancelled.
    fn cancel_reason(&self) -> CancelReason {
        match *self {
            Event::RenderCancel(reason) => reason,
            Event::AllNotesOff => CancelReason::AllNotesOff,
            Event::ResetSession => CancelReason::SessionReplaced,
            Event::OutputTrouble => CancelReason::DeviceLost,
            // V4a cites the reopen text (`finish/realtime.rs:26`).
            Event::DriverReset | Event::ReplaceRequested => CancelReason::Reopen,
            Event::PlayerFaulted | Event::OwedStop { .. } => CancelReason::Fault,
            _ => CancelReason::Dialog,
        }
    }

    /// The message a `msg` cell shows for this event.
    fn message(&self) -> Option<Message> {
        match *self {
            Event::SessionRejected => Some(Message::SessionRejected),
            Event::PlayerFaulted | Event::OwedStop { .. } => Some(Message::Fault),
            Event::OutputTrouble => Some(Message::OutputTrouble),
            Event::InputTrouble => Some(Message::InputTrouble),
            Event::DriverReset => Some(Message::DriverReset),
            Event::MidiTransportLost => Some(Message::MidiTransportLost),
            Event::InputLoss => Some(Message::InputLoss),
            _ => None,
        }
    }
}

/// The tracked requests (record section 4), counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tracked {
    /// Play-family starts, in every group.
    pub starts: u32,
    pub record: bool,
    pub record_off: bool,
    pub stops: u32,
    pub locates: u32,
    pub release: bool,
    pub set_loop: bool,
    pub render_play: bool,
    pub render_stops: u32,
}

impl Tracked {
    /// Nothing is tracked.
    pub fn none(&self) -> bool {
        *self == Tracked::default()
    }
}

/// The current start group: its live members and its `applied` bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartGroup {
    pub members: u32,
    pub applied: bool,
}

/// The facts beside the phase that the matrix reads (record section 3), after the event's own
/// retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    pub tracked: Tracked,
    pub group: StartGroup,
    /// When the owed Stop was first owed (monotonic), if one is owed.
    pub owed_stop_since: Option<Duration>,
    pub held: Option<Held>,
    /// How many commands Resume has deferred.
    pub deferred: usize,
    pub reset_generation: u64,
    pub speed: Speed,
    /// Observed playback and position, sampled together (fresh at admission step 3).
    pub playing: bool,
    pub position: Samples,
    /// The working cursor.
    pub cursor: Samples,
    pub faulted: bool,
    /// A take plan, a recorder take pending, a MIDI pass prepared, or record-off tracked.
    pub finalizing: bool,
    /// A device reopen is pending (refusal Tr).
    pub reopen_pending: bool,
}

impl Facts {
    /// `exclusive_ok` (record section 8), read in Stopped: nothing tracked, owed, held or
    /// deferred, not finalizing, not faulted.
    pub fn exclusive_ok(&self) -> bool {
        self.tracked.none() && self.owed_stop_since.is_none() && self.held.is_none() && self.deferred == 0 && !self.finalizing && !self.faulted
    }
}

/// A status outcome (record section 12); the display arbitration is slice S2f's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Playing,
    Recording,
    Stopped,
    AllNotesOff,
}

/// A message a transition shows. The texts stay with the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// Tp, once per request identity.
    PlayerRefusal,
    /// Tf, once per ticket.
    StageFailure,
    Fault,
    OutputTrouble,
    InputTrouble,
    DriverReset,
    MidiTransportLost,
    InputLoss,
    SessionRejected,
    /// The lap limit's own Note (tables.md section E).
    LapLimit,
    /// The result's or the cancellation reason's message (E5).
    RenderSettled {
        result: Option<RenderOutcome>,
        reason: Option<CancelReason>,
    },
    /// The Resume's record start was refused locally (H2): the reason shown.
    ResumeRefused(Refusal),
}

/// What a transition does besides the phase, in the order its cell names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    SubmitStart(StartCommand),
    NewGroup,
    CloseGroup,
    GroupApplied,
    SetSpeed(Speed),
    SetRetained(Samples),
    SetCursor(Samples),
    SetPosition(Samples),
    /// Held until the stop completes, replacing any held start.
    Hold(Held),
    DropHeld,
    Defer,
    DiscardDeferred,
    ReleaseDeferred,
    /// `finish_recording`: a take with content is placed, an empty one discarded.
    FinishPass,
    /// Submit a Stop; the host owes it (W1) if the Player refuses it without faulting.
    SubmitStop,
    SubmitRecord {
        punch: bool,
    },
    SubmitLocate(Samples),
    SubmitRelease,
    SubmitSetLoop,
    MarkLoopSeen,
    KeepWantedLoop,
    CancelRender(CancelReason),
    SubmitRenderPlay,
    TrackRenderObligations,
    DisposeRenderWithoutStop,
    /// Return to Zero, with its nested Locate, inside the Park completion.
    ReturnToZero,
    /// Admit the held start (current reset generation) as a nested admission.
    AdmitHeld,
    /// Schedule the Resume check (H2) at H4; it rechecks its full precondition with fresh facts
    /// before preparation and submission.
    ResumeCheck,
    /// Perform the Reset after the render settled.
    ResetContinuation,
    /// Retire every fact and owed Stop, close every group, discard held and deferred work,
    /// finalize the prepared resources.
    RetireAll,
    SetFaulted,
    ClearFaulted,
    FaultRuntime,
    RetryOwedStop,
    CancelOffers,
    SetRecovering,
    LogAdopted,
    Message(Message),
    Outcome(Outcome),
}

/// What a command returns when it is not refused (record section 7, results).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Submitted,
    Held,
    Deferred,
    NoEffect,
}

/// One transition: the phase entered, its effects, and the command's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub phase: TransportPhase,
    pub effects: Vec<Effect>,
    pub result: Result<Disposition, Refusal>,
}

/// The cell of `row` in `col` (tables.md section A).
pub fn cell(row: Row, col: Col) -> &'static [Op] {
    matrix::row(row)[col.index()]
}

/// The transition for `event` in `phase` with `facts` (ADR 0062, tables.md section A). Total: every
/// phase and event has a cell, and a refusal keeps the phase.
pub fn next(phase: &TransportPhase, event: &Event, facts: &Facts) -> Step {
    let mut run = Run { col: Col::of(phase), phase: *phase, effects: Vec::new(), event, facts };
    let result = if run.holds() { run.row_ops(event.row()) } else { Ok(false) };
    match result {
        Err(refusal) => Step { phase: *phase, effects: Vec::new(), result: Err(refusal) },
        Ok(_) => {
            let left_start_or_play = matches!(phase, TransportPhase::StartPending | TransportPhase::Playing)
                && !matches!(run.phase, TransportPhase::StartPending | TransportPhase::Playing);
            if left_start_or_play && !run.effects.contains(&Effect::SetSpeed(Speed::Normal)) {
                run.effects.insert(0, Effect::SetSpeed(Speed::Normal));
            }
            let has = |f: fn(&Effect) -> bool| run.effects.iter().any(f);
            let disposition = if has(|e| matches!(e, Effect::Hold(_))) {
                Disposition::Held
            } else if has(|e| matches!(e, Effect::Defer)) {
                Disposition::Deferred
            } else if has(|e| {
                matches!(
                    e,
                    Effect::SubmitStart(_)
                        | Effect::SubmitStop
                        | Effect::SubmitRecord { .. }
                        | Effect::SubmitLocate(_)
                        | Effect::SubmitRelease
                        | Effect::SubmitSetLoop
                        | Effect::SubmitRenderPlay
                )
            }) {
                Disposition::Submitted
            } else {
                Disposition::NoEffect
            };
            Step { phase: run.phase, effects: run.effects, result: Ok(disposition) }
        }
    }
}

/// The ticket of the current pass, in the phases that carry one.
fn pass_ticket(phase: &TransportPhase) -> Option<Ticket> {
    match *phase {
        TransportPhase::RecordPending { ticket, .. } | TransportPhase::CountIn { ticket, .. } | TransportPhase::WaitingForClock { ticket, .. } => {
            Some(ticket)
        }
        _ => None,
    }
}

/// A Rendering phase whose job is retired and whose Play and Stops are all retired, none owed,
/// playback not observed (E5's condition).
fn settled(phase: &TransportPhase, f: &Facts) -> bool {
    matches!(phase, TransportPhase::Rendering { job: RenderJob::Retired { .. } })
        && !f.tracked.render_play
        && f.tracked.render_stops == 0
        && f.owed_stop_since.is_none()
        && !f.playing
}

/// One transition in progress.
struct Run<'a> {
    col: Col,
    phase: TransportPhase,
    effects: Vec<Effect>,
    event: &'a Event,
    facts: &'a Facts,
}

impl Run<'_> {
    fn push(&mut self, effect: Effect) {
        if !self.effects.contains(&effect) {
            self.effects.push(effect);
        }
    }

    /// The conditions an event's name states, and the current-identity rule of the receipt and
    /// observation preambles.
    fn holds(&self) -> bool {
        let f = self.facts;
        if let (Some(event), Some(pass)) = (self.event.ticket(), pass_ticket(&self.phase))
            && event != pass
        {
            return false;
        }
        match self.event {
            Event::StopFrontier => !f.playing,
            Event::StoppedByItself => !f.playing && f.tracked.stops == 0 && f.owed_stop_since.is_none(),
            Event::PlaybackObserved => f.tracked.none() && f.owed_stop_since.is_none() && !f.faulted,
            Event::RenderSettled => settled(&self.phase, f),
            Event::OwedStop { .. } => f.owed_stop_since.is_some(),
            Event::ResumeCheck(_) => f.tracked.stops == 0 && f.owed_stop_since.is_none() && !f.tracked.record_off && !f.playing,
            _ => true,
        }
    }

    /// The row's every-column header, then its cell. D-6: in Rendering, where the header retires
    /// every fact (R13, V3), the cell's render cancel comes first, so the retirement covers what
    /// the cancel returned (its Stops are retired, neither tracked nor owed).
    fn row_ops(&mut self, row: Row) -> Result<bool, Refusal> {
        let header = matrix::header(row);
        let cell = cell(row, self.col);
        if self.col == Col::Rnd
            && header.contains(&Op::RetireAll)
            && let Some((&Op::CancelRender, rest)) = cell.split_first()
        {
            return Ok(self.ops(&[Op::CancelRender])? && self.ops(header)? && self.ops(rest)?);
        }
        Ok(self.ops(header)? && self.ops(cell)?)
    }

    /// Apply `ops` in order; `Ok(false)` when a guard stopped the cell.
    fn ops(&mut self, ops: &[Op]) -> Result<bool, Refusal> {
        for &op in ops {
            if !self.op(op)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Every start refuses while `faulted` (the matrix's global rule).
    fn start_allowed(&self) -> Result<(), Refusal> {
        if self.facts.faulted { Err(Refusal::Faulted) } else { Ok(()) }
    }

    /// Enter StopPending with `then` and submit its Stop; a Stop closes the current group.
    fn stop_into(&mut self, then: AfterStop) {
        self.phase = TransportPhase::StopPending { then };
        self.push(Effect::SubmitStop);
        if self.facts.group.members > 0 {
            self.push(Effect::CloseGroup);
        }
    }

    fn submit_start(&mut self) -> Result<(), Refusal> {
        self.start_allowed()?;
        if let Some(start) = self.event.start() {
            self.push(Effect::SubmitStart(start));
            self.phase = TransportPhase::StartPending;
        }
        Ok(())
    }

    fn record(&mut self, punch: bool) -> Result<(), Refusal> {
        self.start_allowed()?;
        // The cells name Ta and Te before RP; an unprepared start reaching here (an event without
        // a preparation) refuses defensively, since RecordPending needs a prepared ticket.
        match self.event.prep() {
            Some(RecordPrep::Ready(s)) => {
                self.phase = TransportPhase::RecordPending { ticket: s.ticket, from: s.from, count_in: s.count_in };
                self.push(Effect::SubmitRecord { punch });
                Ok(())
            }
            Some(RecordPrep::NoArmedTrack) => Err(Refusal::Ta),
            Some(RecordPrep::Cannot) | None => Err(Refusal::Te),
        }
    }

    fn cancel_render(&mut self) {
        if let TransportPhase::Rendering { job: RenderJob::Live } = self.phase {
            let reason = self.event.cancel_reason();
            self.phase = TransportPhase::Rendering { job: RenderJob::Retired { result: None, reason: Some(reason), then_reset: false } };
            self.push(Effect::CancelRender(reason));
        }
    }

    /// The destination after a failed, refused or cancelled start (record section 4, D-4).
    fn by_four(&self) -> TransportPhase {
        // D-4: an outstanding member of the current group decides first, whatever its `applied`
        // bit; a tracked member counts as outstanding until its receipt retires it.
        let f = self.facts;
        if f.group.members > 0 {
            TransportPhase::StartPending
        } else if f.playing {
            TransportPhase::Playing
        } else {
            TransportPhase::Stopped
        }
    }

    fn latch(&self) -> Option<Point> {
        match *self.event {
            Event::RecordStarted { latch, .. } | Event::StageStarted { latch, .. } => Some(latch),
            _ => None,
        }
    }

    /// The count-in the pass phase carries.
    fn pass_count_in(&self) -> Option<CountIn> {
        match self.phase {
            TransportPhase::RecordPending { count_in, .. } | TransportPhase::WaitingForClock { count_in, .. } => count_in,
            TransportPhase::CountIn { plan, .. } => Some(plan),
            _ => None,
        }
    }

    fn op(&mut self, op: Op) -> Result<bool, Refusal> {
        let f = self.facts;
        let at = self.event.at();
        match op {
            Op::Dash | Op::Same | Op::Retired => {}
            Op::Tr if f.reopen_pending => return Err(Refusal::Tr),
            Op::Ta if self.event.prep() == Some(RecordPrep::NoArmedTrack) => return Err(Refusal::Ta),
            Op::Te if self.event.prep() == Some(RecordPrep::Cannot) => return Err(Refusal::Te),
            Op::Ts if f.speed == Speed::Half => return Err(Refusal::Ts),
            Op::Tr | Op::Ta | Op::Te | Op::Ts => {}
            Op::Refuse(refusal) => return Err(refusal),
            Op::Sp => {
                self.start_allowed()?;
                self.push(Effect::NewGroup);
                self.submit_start()?;
            }
            Op::Join => self.submit_start()?,
            Op::NewGroup => {
                self.start_allowed()?;
                self.push(Effect::CloseGroup);
                self.push(Effect::NewGroup);
                self.submit_start()?;
            }
            Op::Half => self.push(Effect::SetSpeed(Speed::Half)),
            Op::Retained => at.into_iter().for_each(|at| self.push(Effect::SetRetained(at))),
            Op::Cursor => at.into_iter().for_each(|at| self.push(Effect::SetCursor(at))),
            Op::Position => at.into_iter().for_each(|at| self.push(Effect::SetPosition(at))),
            Op::Hold => {
                self.start_allowed()?;
                if let Some(start) = self.event.start() {
                    self.push(Effect::Hold(Held { start, generation: f.reset_generation }));
                }
            }
            Op::Defer => {
                if self.event.start().is_some() {
                    self.start_allowed()?;
                }
                if f.deferred >= DEFERRED_LIMIT {
                    return Err(Refusal::DeferredFull);
                }
                self.push(Effect::Defer);
            }
            Op::Finish => self.push(Effect::FinishPass),
            Op::Park => self.stop_into(AfterStop::Park { end: ParkEnd::AtCursor }),
            Op::ParkRtz => self.stop_into(AfterStop::Park { end: ParkEnd::ReturnToZero }),
            Op::CloseGroup => self.push(Effect::CloseGroup),
            Op::EndRtz => self.phase = TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::ReturnToZero } },
            Op::EndTarget => {
                if let Some(at) = at {
                    self.phase = TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::Target(at) } };
                }
            }
            Op::DropHeld => self.push(Effect::DropHeld),
            Op::DiscardDeferred => self.push(Effect::DiscardDeferred),
            Op::ResendStop => self.push(Effect::SubmitStop),
            Op::Rp => self.record(f.playing)?,
            Op::RpPunch => self.record(true)?,
            Op::LocatePlayer => at.into_iter().for_each(|at| self.push(Effect::SubmitLocate(at))),
            Op::TakeSafe => {
                if let Some(at) = at {
                    self.stop_into(AfterStop::Resume(at));
                }
            }
            Op::Retarget => {
                if let Some(at) = at {
                    self.phase = TransportPhase::StopPending { then: AfterStop::Resume(at) };
                }
            }
            Op::Release => self.push(Effect::SubmitRelease),
            Op::CancelRender => self.cancel_render(),
            Op::ThenReset => {
                if let TransportPhase::Rendering { job: RenderJob::Retired { result, reason, .. } } = self.phase {
                    // C12 in RND: "job Retired: keep result and reason; in either case
                    // then_reset := true".
                    self.phase = TransportPhase::Rendering { job: RenderJob::Retired { result, reason, then_reset: true } };
                }
            }
            Op::Reset => self.stop_into(AfterStop::Reset),
            Op::AsC8 => return self.ops(cell(Row::C8, self.col)),
            Op::GroupApplied => {
                if let Event::PlayApplied(PlayOf::Group { current: true }) = self.event {
                    self.push(Effect::GroupApplied);
                    if f.group.members == 0 {
                        self.phase = TransportPhase::Playing;
                        self.push(Effect::Outcome(Outcome::Playing));
                    }
                }
            }
            Op::GroupRefused => {
                if let Event::PlayRefused(PlayOf::Group { current: true }) = self.event
                    && f.group.members == 0
                {
                    if f.group.applied || f.playing {
                        self.phase = TransportPhase::Playing;
                    } else {
                        self.phase = TransportPhase::Stopped;
                        self.push(Effect::SetSpeed(Speed::Normal));
                    }
                }
            }
            Op::DisposeRender => {
                // R2 in RND: a live job is disposed without Stop and retired empty (E5 settles it);
                // an already retired job keeps its entire payload.
                if let (Event::PlayRefused(PlayOf::Render), TransportPhase::Rendering { job: RenderJob::Live }) = (self.event, self.phase) {
                    self.phase = TransportPhase::Rendering { job: RenderJob::Retired { result: None, reason: None, then_reset: false } };
                    self.push(Effect::DisposeRenderWithoutStop);
                }
            }
            Op::Tp => self.push(Effect::Message(Message::PlayerRefusal)),
            Op::Tf => self.push(Effect::Message(Message::StageFailure)),
            Op::Admitted => match self.phase {
                TransportPhase::RecordPending { ticket, from: Some(from), count_in: Some(plan) } => {
                    self.phase = TransportPhase::CountIn { ticket, from, plan };
                }
                TransportPhase::RecordPending { ticket, from, count_in: None } => {
                    self.phase = TransportPhase::WaitingForClock { ticket, from, count_in: None };
                }
                _ => return Ok(false),
            },
            Op::CountIn => match self.phase {
                TransportPhase::RecordPending { ticket, from: Some(from), count_in: Some(plan) } => {
                    self.phase = TransportPhase::CountIn { ticket, from, plan };
                }
                _ => return Ok(false),
            },
            Op::Wait => match self.phase {
                TransportPhase::RecordPending { ticket, from, count_in } | TransportPhase::WaitingForClock { ticket, from, count_in } => {
                    self.phase = TransportPhase::WaitingForClock { ticket, from, count_in };
                }
                TransportPhase::CountIn { ticket, from, plan } => {
                    self.phase = TransportPhase::WaitingForClock { ticket, from: Some(from), count_in: Some(plan) };
                }
                _ => return Ok(false),
            },
            Op::Latch => {
                if let Some(latch) = self.latch() {
                    self.phase = TransportPhase::Recording { latch, count_in: self.pass_count_in() };
                    self.push(Effect::Outcome(Outcome::Recording));
                }
            }
            Op::ReleaseDeferred => self.push(Effect::ReleaseDeferred),
            Op::Punch => {
                if !matches!(self.phase, TransportPhase::WaitingForClock { from: None, .. }) {
                    return Ok(false);
                }
            }
            Op::UnlessStageRetired => {
                if let Event::RecordRefused { retired_by_stage: true, .. } = self.event {
                    return Ok(false);
                }
            }
            Op::ByFour => self.phase = self.by_four(),
            Op::StopsSettled => {
                if f.tracked.stops > 0 || f.owed_stop_since.is_some() || f.playing {
                    return Ok(false);
                }
            }
            Op::ApplyEnd => {
                if let TransportPhase::StopPending { then: AfterStop::Park { end } } = self.phase {
                    match end {
                        ParkEnd::Target(at) => {
                            self.push(Effect::SetRetained(at));
                            self.push(Effect::SetPosition(at));
                            self.push(Effect::SetCursor(at));
                        }
                        ParkEnd::ReturnToZero => {
                            self.push(Effect::ReturnToZero);
                            self.push(Effect::SetRetained(f.position));
                        }
                        ParkEnd::AtCursor => {
                            self.push(Effect::SetRetained(f.position));
                            self.push(Effect::SetPosition(f.cursor));
                        }
                    }
                }
            }
            Op::ToStopped => self.phase = TransportPhase::Stopped,
            Op::Stopped => {
                // "Unless a state is live": Stopped has no state text of its own; a tracked
                // release keeps "All MIDI notes off pending" live.
                if !f.tracked.release {
                    self.push(Effect::Outcome(Outcome::Stopped));
                }
            }
            Op::AdmitHeld => match f.held {
                Some(held) if held.generation == f.reset_generation => self.push(Effect::AdmitHeld),
                Some(_) => self.push(Effect::DropHeld),
                None => {}
            },
            Op::ResumeCheck => self.push(Effect::ResumeCheck),
            Op::Settle => {
                if !settled(&self.phase, f) {
                    return Ok(false);
                }
                if let TransportPhase::Rendering { job: RenderJob::Retired { result, reason, then_reset } } = self.phase {
                    self.phase = TransportPhase::Stopped;
                    self.push(Effect::Message(Message::RenderSettled { result, reason }));
                    if then_reset {
                        self.push(Effect::ResetContinuation);
                    }
                }
            }
            Op::NotesOff => self.push(Effect::Outcome(Outcome::AllNotesOff)),
            Op::Msg => {
                if let Some(message) = self.event.message() {
                    self.push(Effect::Message(message));
                }
            }
            Op::RetireAll => self.push(Effect::RetireAll),
            Op::Faulted => self.push(Effect::SetFaulted),
            Op::NewPlayer => {
                self.push(Effect::RetireAll);
                self.push(Effect::ClearFaulted);
                self.phase = TransportPhase::Stopped;
            }
            Op::Normal => self.push(Effect::SetSpeed(Speed::Normal)),
            Op::Adopt => {
                self.phase = TransportPhase::Playing;
                self.push(Effect::LogAdopted);
            }
            Op::RenderStart => {
                if !f.exclusive_ok() {
                    return Err(Refusal::Tx);
                }
                self.phase = TransportPhase::Rendering { job: RenderJob::Live };
                self.push(Effect::SubmitRenderPlay);
            }
            Op::RenderPoll => self.push(Effect::TrackRenderObligations),
            Op::RenderResult => {
                // E3: Live becomes Retired with the result; a Retired job with no result gains it,
                // keeping its reason and Reset continuation; a collected result stands.
                if let (Event::RenderResult(result), TransportPhase::Rendering { job }) = (self.event, self.phase) {
                    let job = match job {
                        RenderJob::Live => RenderJob::Retired { result: Some(*result), reason: None, then_reset: false },
                        RenderJob::Retired { result: None, reason, then_reset } => RenderJob::Retired { result: Some(*result), reason, then_reset },
                        retired @ RenderJob::Retired { result: Some(_), .. } => retired,
                    };
                    self.phase = TransportPhase::Rendering { job };
                }
            }
            Op::RetryOwed => {
                if let (Event::OwedStop { now }, Some(since)) = (self.event, f.owed_stop_since) {
                    if now.saturating_sub(since) >= OWED_STOP_DEADLINE {
                        // The expired deadline is checked first: fault_runtime(), then R13.
                        self.push(Effect::FaultRuntime);
                        return self.row_ops(Row::R13);
                    }
                    self.push(Effect::RetryOwedStop);
                }
            }
            Op::CancelOffers => self.push(Effect::CancelOffers),
            Op::Recovering => self.push(Effect::SetRecovering),
            Op::LoopSeen => self.push(Effect::MarkLoopSeen),
            Op::SetLoop => self.push(Effect::SubmitSetLoop),
            Op::KeepLoop => self.push(Effect::KeepWantedLoop),
            Op::LapNote => self.push(Effect::Message(Message::LapLimit)),
            Op::ResumeStart => match self.event.prep() {
                Some(RecordPrep::Ready(s)) => {
                    self.phase = TransportPhase::RecordPending { ticket: s.ticket, from: s.from, count_in: s.count_in };
                    self.push(Effect::SubmitRecord { punch: false });
                }
                Some(refused) => {
                    let reason = if refused == RecordPrep::NoArmedTrack { Refusal::Ta } else { Refusal::Te };
                    self.phase = TransportPhase::Stopped;
                    self.push(Effect::Message(Message::ResumeRefused(reason)));
                    self.push(Effect::ReleaseDeferred);
                }
                None => {}
            },
            Op::LocateReceipt => {
                if let Event::LocateOrLoopReceipt { applied: false } = self.event {
                    self.push(Effect::Message(Message::PlayerRefusal));
                }
            }
        }
        Ok(true)
    }
}
