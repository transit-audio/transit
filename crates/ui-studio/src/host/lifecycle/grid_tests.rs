//! ADR 0062 slice S2a: every cell of tables.md section A pinned in full. `GRID` is written by
//! hand from tables.md, independently of `row()`, in the notation below; `transport_phase_matrix`
//! renders what `next` did for each cell (the phase entered, every effect in order, a refusal) in
//! the same notation and compares the two, so dropping, adding or reordering any op in `row()` or
//! the interpreter fails it.
//!
//! Notation of a cell: a refusal is its code (`Tn`, `Th`, `Tx`, `Td`, `RenderRunning`); `—` is no
//! phase change and no effect; otherwise the phase entered (`S`, `SP`, `P`, `RP`, `CI`, `W`, `R`,
//! `STp` with its end `rtz` or `target`, `STr`, `STz`, `RND` or `RND retired`) and, after a colon,
//! every effect in order:
//!
//! | Effect | Written |
//! |---|---|
//! | NewGroup, CloseGroup, GroupApplied | new group, group closed, applied |
//! | SubmitStart, SubmitStop, SubmitRecord, SubmitLocate, SubmitRelease, SubmitSetLoop, SubmitRenderPlay | start, Stop, Record, Locate, release, set_loop, render Play |
//! | SetSpeed Half / Normal | ½, speed normal |
//! | SetRetained, SetCursor, SetPosition | retained:, cursor:, position: with `at` (the event's target), `pos` (the observed position) or `cur` (the working cursor) |
//! | Hold, DropHeld, Defer, DiscardDeferred, ReleaseDeferred | hold, held dropped, def, deferred discarded, deferred released |
//! | FinishPass | ! |
//! | MarkLoopSeen, KeepWantedLoop | loop_seen, keep loop |
//! | CancelRender, TrackRenderObligations, DisposeRenderWithoutStop | cancel, obligations, disposed |
//! | ReturnToZero, AdmitHeld, ResumeCheck, ResetContinuation | RTZ, admit held, Resume check, then Reset |
//! | RetireAll, SetFaulted, ClearFaulted, FaultRuntime, RetryOwedStop | retire all, faulted, fault cleared, fault_runtime, retry |
//! | CancelOffers, SetRecovering, LogAdopted | offers cancelled, recovering, logged |
//! | Message: Tp, Tf, the lap-limit Note, any other | Tp, Tf, note, msg |
//! | Outcome | "Playing", "Recording", "Stopped", "All MIDI notes off" |
//!
//! Each column is pinned in one phase (RP with a count-in, W as an explicit start, RND live except
//! for R6 and E5), with empty facts (an owed Stop for W1), the event of its row (the render's Play in
//! RND for R1 and R2), position 70 and cursor 40.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::time::Duration;
use transit_playback::CountIn;
use transit_playback::record::start::{Point, Ticket};
use transit_playback::timeline_map::Position;

const PLAN: CountIn = CountIn { frames: 96_000, beat: 24_000, per_bar: 4 };
const AT: i64 = 1_000;

fn tickets() -> (Ticket, Ticket) {
    let (mut control, _publisher) = transit_playback::record::start::pair(transit_mix::EngineEpoch::FIRST);
    let older = control.prepare().unwrap().ticket();
    (control.prepare().unwrap().ticket(), older)
}

fn point() -> Point {
    Point { host_ns: 1, output_frame: 2, position: Position { sample: 3, fraction: 0, denominator: 1 } }
}

fn ready(ticket: Ticket) -> RecordPrep {
    RecordPrep::Ready(RecordStart { ticket, from: Some(0), count_in: Some(PLAN) })
}

const PARK: TransportPhase = TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } };
const RETIRED: RenderJob = RenderJob::Retired { result: Some(RenderOutcome::Completed), reason: None, then_reset: false };

/// The phase each column is pinned in; RND is a retired job for the rows that settle it.
fn column_phase(col: Col, row: Row) -> TransportPhase {
    let (t, _) = tickets();
    match col {
        Col::S => TransportPhase::Stopped,
        Col::Sp => TransportPhase::StartPending,
        Col::P => TransportPhase::Playing,
        Col::Rp => TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: Some(PLAN) },
        Col::Ci => TransportPhase::CountIn { ticket: t, from: 0, plan: PLAN },
        Col::W => TransportPhase::WaitingForClock { ticket: t, from: Some(0), count_in: None },
        Col::R => TransportPhase::Recording { latch: point(), count_in: Some(PLAN) },
        Col::Stp => PARK,
        Col::Str => TransportPhase::StopPending { then: AfterStop::Resume(500) },
        Col::Stz => TransportPhase::StopPending { then: AfterStop::Reset },
        Col::Rnd if matches!(row, Row::R6 | Row::E5) => TransportPhase::Rendering { job: RETIRED },
        Col::Rnd => TransportPhase::Rendering { job: RenderJob::Live },
    }
}

/// The event each row is pinned with (the render's Play in RND for R1 and R2).
fn row_event(row: Row, col: Col) -> Event {
    let (t, _) = tickets();
    let play_of = if col == Col::Rnd { PlayOf::Render } else { PlayOf::Group { current: true } };
    match row {
        Row::C1 => Event::Play,
        Row::C2 => Event::PlayExact(AT),
        Row::C3 => Event::PlaySelection,
        Row::C4 => Event::TogglePlay,
        Row::C5 => Event::Stop,
        Row::C6 => Event::Pause,
        Row::C7 => Event::Record(ready(t)),
        Row::C8 => Event::Locate(AT),
        Row::C9 => Event::HalfSpeed,
        Row::C10 => Event::Scrub(AT),
        Row::C11 => Event::AllNotesOff,
        Row::C12 => Event::ResetSession,
        Row::C13 => Event::RulerClick(AT),
        Row::R1 => Event::PlayApplied(play_of),
        Row::R2 => Event::PlayRefused(play_of),
        Row::R3 => Event::RecordAdmitted(t),
        Row::R4 => Event::RecordStarted { ticket: t, latch: point() },
        Row::R5 => Event::RecordRefused { ticket: t, retired_by_stage: false },
        Row::R6 => Event::StopFrontier,
        Row::R7 => Event::RecordOffApplied,
        Row::R8 => Event::RecordOffRefused,
        Row::R9 => Event::LocateOrLoopReceipt { applied: false },
        Row::R10 => Event::ReleaseFrontier,
        Row::R11 => Event::SessionRejected,
        Row::R12 => Event::UntrackedRefusal,
        Row::R13 => Event::PlayerFaulted,
        Row::O1 => Event::StageCountingIn(t),
        Row::O2 => Event::StageWaiting(t),
        Row::O3 => Event::StageStarted { ticket: t, latch: point() },
        Row::O4 => Event::StageFailed(t),
        Row::O5 => Event::StageCancelled(t),
        Row::O6 => Event::StoppedByItself,
        Row::O7 => Event::PlaybackObserved,
        Row::E1 => Event::RenderStart,
        Row::E2 => Event::RenderCancel(CancelReason::Dialog),
        Row::E3a => Event::RenderPoll,
        Row::E3 => Event::RenderResult(RenderOutcome::Completed),
        Row::E5 => Event::RenderSettled,
        Row::W1 => Event::OwedStop { now: Duration::from_secs(1) },
        Row::V1 => Event::OutputTrouble,
        Row::V2 => Event::InputTrouble,
        Row::V3 => Event::DriverReset,
        Row::V4a => Event::ReplaceRequested,
        Row::V4b => Event::OldPlayerRetired,
        Row::V5 => Event::MidiTransportLost,
        Row::V6 => Event::InputLoss,
        Row::L1 => Event::LoopChanged,
        Row::T1 => Event::LapLimit,
        Row::H2 => Event::ResumeCheck(ready(t)),
    }
}

/// The facts each row is pinned with: nothing tracked, normal speed; an owed Stop for W1.
fn row_facts(row: Row) -> Facts {
    let owed_stop_since = (row == Row::W1).then_some(Duration::ZERO);
    Facts { owed_stop_since, position: 70, cursor: 40, ..Facts::default() }
}

fn phase_name(phase: &TransportPhase) -> &'static str {
    match phase {
        TransportPhase::Stopped => "S",
        TransportPhase::StartPending => "SP",
        TransportPhase::Playing => "P",
        TransportPhase::RecordPending { .. } => "RP",
        TransportPhase::CountIn { .. } => "CI",
        TransportPhase::WaitingForClock { .. } => "W",
        TransportPhase::Recording { .. } => "R",
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } } => "STp",
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::ReturnToZero } } => "STp rtz",
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::Target(_) } } => "STp target",
        TransportPhase::StopPending { then: AfterStop::Resume(_) } => "STr",
        TransportPhase::StopPending { then: AfterStop::Reset } => "STz",
        TransportPhase::Rendering { job: RenderJob::Live } => "RND",
        TransportPhase::Rendering { job: RenderJob::Retired { .. } } => "RND retired",
    }
}

/// A position as the notation names it: the event's target, the observed position or the cursor.
fn at(samples: i64) -> &'static str {
    match samples {
        AT => "at",
        70 => "pos",
        40 => "cur",
        _ => "?",
    }
}

/// One effect in the notation; exhaustive, so a new effect does not compile until it is named.
fn token(effect: &Effect) -> String {
    match effect {
        Effect::SubmitStart(_) => "start".into(),
        Effect::NewGroup => "new group".into(),
        Effect::CloseGroup => "group closed".into(),
        Effect::GroupApplied => "applied".into(),
        Effect::SetSpeed(Speed::Half) => "½".into(),
        Effect::SetSpeed(Speed::Normal) => "speed normal".into(),
        Effect::SetRetained(s) => format!("retained:{}", at(*s)),
        Effect::SetCursor(s) => format!("cursor:{}", at(*s)),
        Effect::SetPosition(s) => format!("position:{}", at(*s)),
        Effect::Hold(_) => "hold".into(),
        Effect::DropHeld => "held dropped".into(),
        Effect::Defer => "def".into(),
        Effect::DiscardDeferred => "deferred discarded".into(),
        Effect::ReleaseDeferred => "deferred released".into(),
        Effect::FinishPass => "!".into(),
        Effect::SubmitStop => "Stop".into(),
        Effect::SubmitRecord(_) => "Record".into(),
        Effect::SubmitLocate(_) => "Locate".into(),
        Effect::SubmitRelease => "release".into(),
        Effect::SubmitSetLoop => "set_loop".into(),
        Effect::MarkLoopSeen => "loop_seen".into(),
        Effect::KeepWantedLoop => "keep loop".into(),
        Effect::CancelRender(_) => "cancel".into(),
        Effect::SubmitRenderPlay => "render Play".into(),
        Effect::TrackRenderObligations => "obligations".into(),
        Effect::DisposeRenderWithoutStop => "disposed".into(),
        Effect::ReturnToZero => "RTZ".into(),
        Effect::AdmitHeld => "admit held".into(),
        Effect::ResumeCheck => "Resume check".into(),
        Effect::ResetContinuation => "then Reset".into(),
        Effect::RetireAll => "retire all".into(),
        Effect::SetFaulted => "faulted".into(),
        Effect::ClearFaulted => "fault cleared".into(),
        Effect::FaultRuntime => "fault_runtime".into(),
        Effect::RetryOwedStop => "retry".into(),
        Effect::CancelOffers => "offers cancelled".into(),
        Effect::SetRecovering => "recovering".into(),
        Effect::LogAdopted => "logged".into(),
        Effect::Message(Message::PlayerRefusal) => "Tp".into(),
        Effect::Message(Message::StageFailure) => "Tf".into(),
        Effect::Message(Message::LapLimit) => "note".into(),
        Effect::Message(_) => "msg".into(),
        Effect::Outcome(Outcome::Playing) => "\"Playing\"".into(),
        Effect::Outcome(Outcome::Recording) => "\"Recording\"".into(),
        Effect::Outcome(Outcome::Stopped) => "\"Stopped\"".into(),
        Effect::Outcome(Outcome::AllNotesOff) => "\"All MIDI notes off\"".into(),
    }
}

/// What `next` did, in the notation of `GRID`.
fn render(before: &TransportPhase, step: &Step) -> String {
    if let Err(refusal) = step.result {
        return format!("{refusal:?}");
    }
    if step.effects.is_empty() {
        return if step.phase == *before { "—".into() } else { phase_name(&step.phase).into() };
    }
    let effects: Vec<String> = step.effects.iter().map(token).collect();
    format!("{}: {}", phase_name(&step.phase), effects.join(", "))
}

/// tables.md section A, cell by cell, in the notation of the module doc.
#[rustfmt::skip]
const GRID: [(Row, [&str; 11]); 49] = [
    //          S, SP, P, RP, CI, W, R, STp, STr, STz, RND
    (Row::C1, ["SP: new group, start", "—", "—", "—", "STp: !, Stop, deferred discarded", "—", "—", "STp: hold", "STr: def", "—", "Tn"]),
    (Row::C2, ["SP: new group, start, retained:at", "SP: start, retained:at", "SP: group closed, new group, start, retained:at", "—", "STp: !, Stop, deferred discarded", "—", "—", "STp: hold", "STr: def", "—", "Tn"]),
    (Row::C3, ["SP: new group, start", "SP: start", "SP: group closed, new group, start", "—", "STp: !, Stop, deferred discarded", "—", "—", "STp: hold", "STr: def", "—", "Tn"]),
    (Row::C4, ["SP: new group, start", "STp: speed normal, Stop, group closed", "STp: speed normal, Stop", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: hold", "STr: def", "—", "Tn"]),
    (Row::C5, ["STp rtz: Stop", "STp: speed normal, Stop, group closed", "STp: speed normal, Stop", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp rtz: held dropped", "STp: Stop", "STz: Stop", "Tn"]),
    (Row::C6, ["—", "STp: speed normal, Stop, group closed", "STp: speed normal, Stop", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: held dropped", "STp: Stop", "—", "Tn"]),
    (Row::C7, ["RP: Record", "RP: speed normal, Record", "RP: speed normal, Record", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: !, Stop, deferred discarded", "STp: hold", "STp: Stop, deferred discarded", "—", "Tn"]),
    (Row::C8, ["S: cursor:at, position:at, retained:at", "SP: cursor:at, retained:at", "P: Locate, cursor:at, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STp target: cursor:at", "STr: retained:at", "—", "Tn"]),
    (Row::C9, ["SP: ½, new group, start", "SP: ½, start", "SP: ½, group closed, new group, start", "Th", "STp: !, Stop, deferred discarded", "Th", "Th", "STp: hold", "Th", "—", "Tn"]),
    (Row::C10, ["S: position:at", "SP: position:at", "P: position:at", "RP: position:at", "CI: position:at", "W: position:at", "R: position:at", "STp: position:at", "STr: position:at", "STz: position:at", "RND: position:at"]),
    (Row::C11, ["S: release", "SP: release", "P: release", "RP: release", "CI: release", "W: release", "R: release", "STp: release", "STr: release", "STz: release", "RND retired: cancel, release"]),
    (Row::C12, ["STz: Stop", "STz: speed normal, Stop, group closed", "STz: speed normal, Stop", "STz: !, Stop", "STz: !, Stop", "STz: !, Stop", "STz: !, Stop", "STz: Stop, held dropped", "STz: Stop, deferred discarded", "—", "RND retired: cancel"]),
    (Row::C13, ["S: cursor:at, position:at, retained:at", "SP: cursor:at, retained:at", "P: Locate, cursor:at, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STr: !, Stop, retained:at", "STp target: cursor:at", "STr: retained:at", "—", "Tn"]),
    (Row::R1, ["—", "P: applied, \"Playing\"", "—", "—", "—", "—", "—", "—", "—", "—", "—"]),
    (Row::R2, ["S: Tp", "S: Tp, speed normal", "P: Tp", "RP: Tp", "CI: Tp", "W: Tp", "R: Tp", "STp: Tp", "STr: Tp", "STz: Tp", "RND retired: Tp, disposed"]),
    (Row::R3, ["—", "—", "—", "CI: deferred released", "—", "—", "—", "—", "—", "—", "—"]),
    (Row::R4, ["—", "—", "—", "R: \"Recording\", deferred released", "R: \"Recording\"", "R: \"Recording\"", "—", "—", "—", "—", "—"]),
    (Row::R5, ["—", "—", "—", "S: !, Tp, deferred released", "S: !, Tp, deferred released", "S: !, Tp, deferred released", "—", "—", "—", "—", "—"]),
    (Row::R6, ["—", "—", "—", "—", "—", "—", "—", "S: retained:pos, position:cur, \"Stopped\"", "STr: Resume check", "S", "S: msg"]),
    (Row::R7, ["—", "—", "—", "—", "—", "—", "—", "—", "STr: Resume check", "—", "—"]),
    (Row::R8, ["S: Tp", "SP: Tp", "P: Tp", "RP: Tp", "CI: Tp", "W: Tp", "R: Tp", "STp: Tp", "STr: Resume check, Tp", "STz: Tp", "RND: Tp"]),
    (Row::R9, ["S: Tp", "SP: Tp", "P: Tp", "RP: Tp", "CI: Tp", "W: Tp", "R: Tp", "STp: Tp", "STr: Tp", "STz: Tp", "RND: Tp"]),
    (Row::R10, ["S: \"All MIDI notes off\"", "SP: \"All MIDI notes off\"", "P: \"All MIDI notes off\"", "RP: \"All MIDI notes off\"", "CI: \"All MIDI notes off\"", "W: \"All MIDI notes off\"", "R: \"All MIDI notes off\"", "STp: \"All MIDI notes off\"", "STr: \"All MIDI notes off\"", "STz: \"All MIDI notes off\"", "RND: \"All MIDI notes off\""]),
    (Row::R11, ["S: msg", "SP: msg", "P: msg", "RP: msg", "CI: msg", "W: msg", "R: msg", "STp: msg", "STr: msg", "STz: msg", "RND: msg"]),
    (Row::R12, ["S: Tp", "SP: Tp", "P: Tp", "RP: Tp", "CI: Tp", "W: Tp", "R: Tp", "STp: Tp", "STr: Tp", "STz: Tp", "RND: Tp"]),
    (Row::R13, ["S: retire all, faulted, msg", "S: speed normal, retire all, faulted, msg", "S: speed normal, retire all, faulted, msg", "S: retire all, faulted, !, msg", "S: retire all, faulted, !, msg", "S: retire all, faulted, !, msg", "S: retire all, faulted, !, msg", "S: retire all, faulted, msg", "S: retire all, faulted, msg", "S: retire all, faulted, msg", "S: cancel, retire all, faulted, msg"]),
    (Row::O1, ["—", "—", "—", "CI: deferred released", "—", "—", "—", "—", "—", "—", "—"]),
    (Row::O2, ["—", "—", "—", "W: deferred released", "W", "—", "—", "—", "—", "—", "—"]),
    (Row::O3, ["—", "—", "—", "R: \"Recording\", deferred released", "R: \"Recording\"", "R: \"Recording\"", "—", "—", "—", "—", "—"]),
    (Row::O4, ["—", "—", "—", "S: !, Tf, deferred released", "S: !, Tf, deferred released", "S: !, Tf, deferred released", "—", "—", "—", "—", "—"]),
    (Row::O5, ["—", "—", "—", "S: !, deferred released", "S: !, deferred released", "S: !, deferred released", "—", "—", "—", "—", "—"]),
    (Row::O6, ["—", "—", "S: speed normal", "—", "—", "—", "S: !", "—", "—", "—", "—"]),
    (Row::O7, ["P: logged", "—", "—", "—", "—", "—", "—", "—", "—", "—", "—"]),
    (Row::E1, ["RND: render Play", "Tx", "Tx", "Tx", "Tx", "Tx", "Tx", "Tx", "Tx", "Tx", "RenderRunning"]),
    (Row::E2, ["—", "—", "—", "—", "—", "—", "—", "—", "—", "—", "RND retired: cancel"]),
    (Row::E3a, ["—", "—", "—", "—", "—", "—", "—", "—", "—", "—", "RND: obligations"]),
    (Row::E3, ["—", "—", "—", "—", "—", "—", "—", "—", "—", "—", "RND retired"]),
    (Row::E5, ["—", "—", "—", "—", "—", "—", "—", "—", "—", "—", "S: msg"]),
    (Row::W1, ["—", "—", "—", "—", "—", "—", "—", "STp: retry", "STr: retry", "STz: retry", "RND: retry"]),
    (Row::V1, ["S: msg", "SP: msg", "STp: speed normal, Stop, msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: msg", "STp: Stop, deferred discarded, msg", "STz: msg", "RND retired: cancel, msg"]),
    (Row::V2, ["S: msg", "SP: msg", "P: msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: !, Stop, deferred discarded, msg", "STp: msg", "STp: Stop, deferred discarded, msg", "STz: msg", "RND: msg"]),
    (Row::V3, ["S: retire all, msg", "S: speed normal, retire all, msg", "S: speed normal, retire all, msg", "S: retire all, !, msg", "S: retire all, !, msg", "S: retire all, !, msg", "S: retire all, !, msg", "S: retire all, msg", "S: retire all, msg", "S: retire all, msg", "S: cancel, retire all, msg"]),
    (Row::V4a, ["S: retire all, fault cleared", "STp: speed normal, Stop", "STp: speed normal, Stop", "Td", "Td", "Td", "Td", "—", "Td", "—", "RND retired: cancel"]),
    (Row::V4b, ["S: retire all, fault cleared", "S: speed normal, retire all, fault cleared", "S: speed normal, retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared", "S: retire all, fault cleared"]),
    (Row::V5, ["STp: offers cancelled, held dropped, deferred discarded, recovering, msg, Stop", "STp: speed normal, offers cancelled, held dropped, deferred discarded, recovering, msg, Stop, group closed", "STp: speed normal, offers cancelled, held dropped, deferred discarded, recovering, msg, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, !, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, !, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, !, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, !, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, Stop", "STp: offers cancelled, held dropped, deferred discarded, recovering, msg, Stop", "STz: offers cancelled, held dropped, deferred discarded, recovering, msg, Stop", "RND: offers cancelled, held dropped, deferred discarded, recovering, msg"]),
    (Row::V6, ["S: msg", "SP: msg", "P: msg", "RP: msg", "CI: msg", "W: msg", "R: msg", "STp: msg", "STr: msg", "STz: msg", "RND: msg"]),
    (Row::L1, ["S: loop_seen", "SP: set_loop", "P: set_loop", "RP: keep loop", "CI: keep loop", "W: keep loop", "R: keep loop", "STp: loop_seen", "STr: keep loop", "—", "—"]),
    (Row::T1, ["—", "—", "—", "—", "—", "—", "STp: !, Stop, deferred discarded, note", "—", "—", "—", "—"]),
    (Row::H2, ["—", "—", "—", "—", "—", "—", "—", "—", "RP: Record", "—", "—"]),
];

#[test]
fn transport_phase_matrix() {
    let mut wrong = Vec::new();
    for (row, cells) in GRID {
        for col in Col::ALL {
            let before = column_phase(col, row);
            let step = next(&before, &row_event(row, col), &row_facts(row));
            let got = render(&before, &step);
            if got != cells[col.index()] {
                wrong.push(format!("{row:?} {col:?}: want {}, got {got}", cells[col.index()]));
            }
        }
    }
    assert!(wrong.is_empty(), "{} cells differ:\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(GRID.map(|(row, _)| row), Row::ALL, "the grid has every row, in tables.md order");
}
