//! ADR 0062 slice S2a: the literal transition matrix and its pure reducer. `transport_phase_matrix`
//! pins every cell of tables.md section A in a second, independent grid; the conditions the cells
//! name are pinned branch by branch; `the_matrix_has_no_empty_cell` enumerates every phase, event,
//! row and column; `stopped_implies_nothing_tracked` checks section 3's invariant over every
//! transition into Stopped.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::collections::HashSet;
use std::time::Duration;
use transit_playback::CountIn;
use transit_playback::record::start::{Point, Ticket};
use transit_playback::timeline_map::Position;

const PLAN: CountIn = CountIn { frames: 96_000, beat: 24_000, per_bar: 4 };
const AT: i64 = 1_000;

/// The current pass's ticket and an older one.
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

fn abbr(phase: &TransportPhase) -> &'static str {
    match phase {
        TransportPhase::Stopped => "S",
        TransportPhase::StartPending => "SP",
        TransportPhase::Playing => "P",
        TransportPhase::RecordPending { .. } => "RP",
        TransportPhase::CountIn { .. } => "CI",
        TransportPhase::WaitingForClock { .. } => "W",
        TransportPhase::Recording { .. } => "R",
        TransportPhase::StopPending { then: AfterStop::Park { .. } } => "STp",
        TransportPhase::StopPending { then: AfterStop::Resume(_) } => "STr",
        TransportPhase::StopPending { then: AfterStop::Reset } => "STz",
        TransportPhase::Rendering { .. } => "RND",
    }
}

/// A step written as tables.md writes a cell: a refusal's code; `hold`, `def`; `—` for no effect;
/// otherwise the phase entered (or kept, with an effect), `!` when the pass ends.
fn summary(before: &TransportPhase, step: &Step) -> String {
    match step.result {
        Err(refusal) => format!("{refusal:?}"),
        Ok(Disposition::Held) => "hold".into(),
        Ok(Disposition::Deferred) => "def".into(),
        Ok(_) if step.effects.is_empty() && step.phase == *before => "—".into(),
        Ok(_) => format!("{}{}", abbr(&step.phase), if step.effects.contains(&Effect::FinishPass) { "!" } else { "" }),
    }
}

/// tables.md section A, cell by cell, as [`summary`] writes each outcome. A merged "every column"
/// cell is spelled out; `—` in STz for C12 ("STz") is the phase kept with no effect. W is pinned as
/// an explicit start (`from` given), so O6 W is "otherwise —"; the punch is pinned separately.
#[rustfmt::skip]
const GRID: [(Row, [&str; 11]); 49] = [
    //          S       SP      P       RP      CI      W       R       STp     STr     STz     RND
    (Row::C1,  ["SP",  "—",    "—",    "—",    "STp!", "—",    "—",    "hold", "def",  "—",    "Tn"]),
    (Row::C2,  ["SP",  "SP",   "SP",   "—",    "STp!", "—",    "—",    "hold", "def",  "—",    "Tn"]),
    (Row::C3,  ["SP",  "SP",   "SP",   "—",    "STp!", "—",    "—",    "hold", "def",  "—",    "Tn"]),
    (Row::C4,  ["SP",  "STp",  "STp",  "STp!", "STp!", "STp!", "STp!", "hold", "def",  "—",    "Tn"]),
    (Row::C5,  ["STp", "STp",  "STp",  "STp!", "STp!", "STp!", "STp!", "STp",  "STp",  "STz",  "Tn"]),
    (Row::C6,  ["—",   "STp",  "STp",  "STp!", "STp!", "STp!", "STp!", "STp",  "STp",  "—",    "Tn"]),
    (Row::C7,  ["RP",  "RP",   "RP",   "STp!", "STp!", "STp!", "STp!", "hold", "STp",  "—",    "Tn"]),
    (Row::C8,  ["S",   "SP",   "P",    "STr!", "STr!", "STr!", "STr!", "STp",  "STr",  "—",    "Tn"]),
    (Row::C9,  ["SP",  "SP",   "SP",   "Th",   "STp!", "Th",   "Th",   "hold", "Th",   "—",    "Tn"]),
    (Row::C10, ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::C11, ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::C12, ["STz", "STz",  "STz",  "STz!", "STz!", "STz!", "STz!", "STz",  "STz",  "—",    "RND"]),
    (Row::C13, ["S",   "SP",   "P",    "STr!", "STr!", "STr!", "STr!", "STp",  "STr",  "—",    "Tn"]),
    (Row::R1,  ["—",   "P",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—"]),
    (Row::R2,  ["S",   "S",    "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R3,  ["—",   "—",    "—",    "CI",   "—",    "—",    "—",    "—",    "—",    "—",    "—"]),
    (Row::R4,  ["—",   "—",    "—",    "R",    "R",    "R",    "—",    "—",    "—",    "—",    "—"]),
    (Row::R5,  ["—",   "—",    "—",    "S!",   "S!",   "S!",   "—",    "—",    "—",    "—",    "—"]),
    (Row::R6,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "S",    "STr",  "S",    "S"]),
    (Row::R7,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "STr",  "—",    "—"]),
    (Row::R8,  ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R9,  ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R10, ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R11, ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R12, ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::R13, ["S",   "S",    "S",    "S!",   "S!",   "S!",   "S!",   "S",    "S",    "S",    "S"]),
    (Row::O1,  ["—",   "—",    "—",    "CI",   "—",    "—",    "—",    "—",    "—",    "—",    "—"]),
    (Row::O2,  ["—",   "—",    "—",    "W",    "W",    "—",    "—",    "—",    "—",    "—",    "—"]),
    (Row::O3,  ["—",   "—",    "—",    "R",    "R",    "R",    "—",    "—",    "—",    "—",    "—"]),
    (Row::O4,  ["—",   "—",    "—",    "S!",   "S!",   "S!",   "—",    "—",    "—",    "—",    "—"]),
    (Row::O5,  ["—",   "—",    "—",    "S!",   "S!",   "S!",   "—",    "—",    "—",    "—",    "—"]),
    (Row::O6,  ["—",   "—",    "S",    "—",    "—",    "—",    "S!",   "—",    "—",    "—",    "—"]),
    (Row::O7,  ["P",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—"]),
    (Row::E1,  ["RND", "Tx",   "Tx",   "Tx",   "Tx",   "Tx",   "Tx",   "Tx",   "Tx",   "Tx",   "RenderRunning"]),
    (Row::E2,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "RND"]),
    (Row::E3a, ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "RND"]),
    (Row::E3,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "RND"]),
    (Row::E5,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "—",    "S"]),
    (Row::W1,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "STp",  "STr",  "STz",  "RND"]),
    (Row::V1,  ["S",   "SP",   "STp",  "STp!", "STp!", "STp!", "STp!", "STp",  "STp",  "STz",  "RND"]),
    (Row::V2,  ["S",   "SP",   "P",    "STp!", "STp!", "STp!", "STp!", "STp",  "STp",  "STz",  "RND"]),
    (Row::V3,  ["S",   "S",    "S",    "S!",   "S!",   "S!",   "S!",   "S",    "S",    "S",    "S"]),
    (Row::V4a, ["S",   "STp",  "STp",  "Td",   "Td",   "Td",   "Td",   "—",    "Td",   "—",    "RND"]),
    (Row::V4b, ["S",   "S",    "S",    "S",    "S",    "S",    "S",    "S",    "S",    "S",    "S"]),
    (Row::V5,  ["STp", "STp",  "STp",  "STp!", "STp!", "STp!", "STp!", "STp",  "STp",  "STz",  "RND"]),
    (Row::V6,  ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "STz",  "RND"]),
    (Row::L1,  ["S",   "SP",   "P",    "RP",   "CI",   "W",    "R",    "STp",  "STr",  "—",    "—"]),
    (Row::T1,  ["—",   "—",    "—",    "—",    "—",    "—",    "STp!", "—",    "—",    "—",    "—"]),
    (Row::H2,  ["—",   "—",    "—",    "—",    "—",    "—",    "—",    "—",    "RP",   "—",    "—"]),
];

#[test]
fn transport_phase_matrix() {
    let mut wrong = Vec::new();
    for (row, cells) in GRID {
        for col in Col::ALL {
            let before = column_phase(col, row);
            let step = next(&before, &row_event(row, col), &row_facts(row));
            let got = summary(&before, &step);
            if got != cells[col.index()] {
                wrong.push(format!("{row:?} {col:?}: want {}, got {got} ({step:?})", cells[col.index()]));
            }
        }
    }
    assert!(wrong.is_empty(), "{} cells differ:\n{}", wrong.len(), wrong.join("\n"));
    assert_eq!(GRID.map(|(row, _)| row), Row::ALL, "the grid has every row, in tables.md order");
}

fn run(phase: TransportPhase, event: Event, facts: Facts) -> Step {
    next(&phase, &event, &facts)
}

#[test]
fn matrix_conditions_follow_the_cell_text() {
    let (t, older) = tickets();
    let f = Facts { position: 70, cursor: 40, ..Facts::default() };
    let rp = TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: None };

    // Refusals: (Tr), (Ta), (Te), half: Ts; the fault refuses every start; the 33rd deferred.
    let refused = |phase, event, facts| run(phase, event, facts).result;
    assert_eq!(refused(TransportPhase::Stopped, Event::Play, Facts { reopen_pending: true, ..f }), Err(Refusal::Tr));
    assert_eq!(refused(TransportPhase::Stopped, Event::Play, Facts { faulted: true, ..f }), Err(Refusal::Faulted));
    assert_eq!(refused(TransportPhase::Stopped, Event::Record(RecordPrep::NoArmedTrack), f), Err(Refusal::Ta));
    assert_eq!(refused(TransportPhase::Stopped, Event::Record(RecordPrep::Cannot), f), Err(Refusal::Te));
    assert_eq!(refused(TransportPhase::StartPending, Event::Record(ready(t)), Facts { speed: Speed::Half, ..f }), Err(Refusal::Ts));
    let resume = TransportPhase::StopPending { then: AfterStop::Resume(500) };
    assert_eq!(refused(resume, Event::Play, Facts { deferred: DEFERRED_LIMIT, ..f }), Err(Refusal::DeferredFull));
    assert_eq!(refused(resume, Event::Play, Facts { deferred: DEFERRED_LIMIT - 1, ..f }), Ok(Disposition::Deferred));
    assert_eq!(refused(TransportPhase::Stopped, Event::RenderStart, Facts { finalizing: true, ..f }), Err(Refusal::Tx));
    let kept = run(rp, Event::HalfSpeed, f);
    assert_eq!((kept.phase, kept.effects.len(), kept.result), (rp, 0, Err(Refusal::Th)), "a refusal keeps the phase");

    // Park's end: the last command wins (U-2); the completion applies exactly one end.
    let park = |end| TransportPhase::StopPending { then: AfterStop::Park { end } };
    let stop = run(TransportPhase::Stopped, Event::Stop, f);
    assert_eq!((stop.phase, stop.effects), (park(ParkEnd::ReturnToZero), vec![Effect::SubmitStop]));
    let located = run(stop.phase, Event::Locate(AT), f);
    assert_eq!((located.phase, located.effects, located.result), (park(ParkEnd::Target(AT)), vec![Effect::SetCursor(AT)], Ok(Disposition::NoEffect)));
    assert_eq!(run(located.phase, Event::Stop, f).phase, park(ParkEnd::ReturnToZero), "D9: a second Stop after the Locate");
    let done = |end, facts| run(park(end), Event::StopFrontier, facts);
    let stopped = Effect::Outcome(Outcome::Stopped);
    assert_eq!(done(ParkEnd::Target(AT), f).effects, [Effect::SetRetained(AT), Effect::SetPosition(AT), Effect::SetCursor(AT), stopped]);
    assert_eq!(done(ParkEnd::ReturnToZero, f).effects, [Effect::ReturnToZero, Effect::SetRetained(70), stopped]);
    assert_eq!(done(ParkEnd::AtCursor, f).effects, [Effect::SetRetained(70), Effect::SetPosition(40), stopped]);
    let one_stop = Tracked { stops: 1, ..Tracked::default() };
    for waiting in [Facts { tracked: one_stop, ..f }, Facts { owed_stop_since: Some(Duration::ZERO), ..f }, Facts { playing: true, ..f }] {
        assert_eq!(done(ParkEnd::AtCursor, waiting).phase, PARK, "{waiting:?}");
        let reset = TransportPhase::StopPending { then: AfterStop::Reset };
        assert_eq!(run(reset, Event::StopFrontier, waiting).phase, reset, "{waiting:?}");
    }
    let release = Facts { tracked: Tracked { release: true, ..Tracked::default() }, ..f };
    assert!(!done(ParkEnd::AtCursor, release).effects.contains(&stopped), "a live state keeps Stopped from being written");
    let held = run(PARK, Event::TogglePlay, Facts { reset_generation: 3, ..f });
    assert_eq!(held.effects, [Effect::Hold(Held { start: StartCommand::TogglePlay, generation: 3 })]);
    let h = Some(Held { start: StartCommand::Play, generation: 3 });
    assert!(done(ParkEnd::AtCursor, Facts { held: h, reset_generation: 3, ..f }).effects.contains(&Effect::AdmitHeld));
    assert!(done(ParkEnd::AtCursor, Facts { held: h, reset_generation: 4, ..f }).effects.contains(&Effect::DropHeld));

    // The record start: only the current ticket moves the pass; by record section 4.
    let other = run(rp, Event::RecordAdmitted(older), f);
    assert_eq!((other.phase, other.effects.len()), (rp, 0));
    assert_eq!(run(rp, Event::RecordAdmitted(t), f).phase, TransportPhase::WaitingForClock { ticket: t, from: Some(0), count_in: None });
    let with_plan = TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: Some(PLAN) };
    assert_eq!(run(with_plan, Event::StageCountingIn(t), f).phase, TransportPhase::CountIn { ticket: t, from: 0, plan: PLAN });
    assert_eq!(run(rp, Event::StageCountingIn(t), f).effects, [], "O1 in RP: no count-in, no effect");
    let latched = run(with_plan, Event::StageStarted { ticket: t, latch: point() }, f);
    assert_eq!(latched.phase, TransportPhase::Recording { latch: point(), count_in: Some(PLAN) });
    let refused_record = |retired_by_stage, facts| run(rp, Event::RecordRefused { ticket: t, retired_by_stage }, facts);
    assert_eq!(refused_record(true, f).effects, []);
    assert_eq!(refused_record(false, f).effects, [Effect::FinishPass, Effect::Message(Message::PlayerRefusal), Effect::ReleaseDeferred]);
    assert_eq!(refused_record(false, Facts { playing: true, ..f }).phase, TransportPhase::Playing);
    assert_eq!(refused_record(false, Facts { group: StartGroup { members: 0, applied: true }, ..f }).phase, TransportPhase::Stopped, "D-4");
    assert_eq!(refused_record(false, Facts { group: StartGroup { members: 1, applied: false }, ..f }).phase, TransportPhase::StartPending);

    // Start groups.
    let sp = TransportPhase::StartPending;
    let member = Facts { group: StartGroup { members: 1, applied: false }, ..f };
    let applied = run(sp, Event::PlayApplied(PlayOf::Group { current: true }), member);
    assert_eq!((applied.phase, applied.effects), (sp, vec![Effect::GroupApplied]));
    assert_eq!(run(sp, Event::PlayApplied(PlayOf::Group { current: false }), f).effects, [], "a closed group lends nothing");
    let lent = Facts { group: StartGroup { members: 0, applied: true }, ..f };
    assert_eq!(run(sp, Event::PlayRefused(PlayOf::Group { current: true }), lent).phase, TransportPhase::Playing);
    let refused_play = run(sp, Event::PlayRefused(PlayOf::Group { current: true }), f);
    assert_eq!(
        (refused_play.phase, refused_play.effects),
        (TransportPhase::Stopped, vec![Effect::Message(Message::PlayerRefusal), Effect::SetSpeed(Speed::Normal)])
    );

    // W1: retried until the deadline, then fault_runtime() before R13's cleanup.
    let owed = Facts { owed_stop_since: Some(Duration::from_millis(500)), ..f };
    assert_eq!(run(PARK, Event::OwedStop { now: Duration::from_millis(2_499) }, owed).effects, [Effect::RetryOwedStop]);
    let fault = run(PARK, Event::OwedStop { now: Duration::from_millis(2_500) }, owed);
    assert_eq!(fault.phase, TransportPhase::Stopped);
    assert_eq!(fault.effects, [Effect::FaultRuntime, Effect::RetireAll, Effect::SetFaulted, Effect::Message(Message::Fault)]);
    let live = TransportPhase::Rendering { job: RenderJob::Live };
    assert_eq!(
        run(live, Event::OwedStop { now: Duration::from_secs(3) }, owed).effects,
        [Effect::FaultRuntime, Effect::RetireAll, Effect::SetFaulted, Effect::CancelRender(CancelReason::Fault), Effect::Message(Message::Fault)]
    );

    // The render: a cancel retires a live job only; settling waits; then the Reset continuation.
    let cancelled = run(live, Event::RenderCancel(CancelReason::Dialog), f);
    let retired = |reason, then_reset| TransportPhase::Rendering { job: RenderJob::Retired { result: None, reason: Some(reason), then_reset } };
    assert_eq!(cancelled.phase, retired(CancelReason::Dialog, false));
    assert_eq!(
        run(TransportPhase::Rendering { job: RETIRED }, Event::RenderCancel(CancelReason::Dialog), f).effects,
        [],
        "the collected result stands"
    );
    let reset = run(live, Event::ResetSession, f).phase;
    assert_eq!(reset, retired(CancelReason::SessionReplaced, true));
    let render_stop = Facts { tracked: Tracked { render_stops: 1, ..Tracked::default() }, ..f };
    assert_eq!(run(reset, Event::RenderSettled, render_stop).phase, reset);
    let settled = run(reset, Event::RenderSettled, f);
    let message = Effect::Message(Message::RenderSettled { result: None, reason: Some(CancelReason::SessionReplaced) });
    assert_eq!((settled.phase, settled.effects), (TransportPhase::Stopped, vec![message, Effect::ResetContinuation]));

    // O7 adopts only with nothing tracked and not faulted.
    let observed = Facts { playing: true, ..f };
    assert_eq!(run(TransportPhase::Stopped, Event::PlaybackObserved, observed).phase, TransportPhase::Playing);
    assert_eq!(run(TransportPhase::Stopped, Event::PlaybackObserved, Facts { faulted: true, ..observed }).phase, TransportPhase::Stopped);
    let locating = Facts { tracked: Tracked { locates: 1, ..Tracked::default() }, ..observed };
    assert_eq!(run(TransportPhase::Stopped, Event::PlaybackObserved, locating).phase, TransportPhase::Stopped);

    // H2: the Resume restarts only when its Stops and record-off retired and playback stopped.
    let record_off = Facts { tracked: Tracked { record_off: true, ..Tracked::default() }, ..f };
    assert_eq!(run(resume, Event::ResumeCheck(ready(t)), record_off).phase, resume);
    let local = run(resume, Event::ResumeCheck(RecordPrep::NoArmedTrack), f);
    assert_eq!(
        (local.phase, local.effects),
        (TransportPhase::Stopped, vec![Effect::Message(Message::ResumeRefused(Refusal::Ta)), Effect::ReleaseDeferred])
    );
}

/// The index of a row in [`Row::ALL`]; exhaustive, so a new row does not compile until it is here.
fn row_index(row: Row) -> usize {
    match row {
        Row::C1 => 0,
        Row::C2 => 1,
        Row::C3 => 2,
        Row::C4 => 3,
        Row::C5 => 4,
        Row::C6 => 5,
        Row::C7 => 6,
        Row::C8 => 7,
        Row::C9 => 8,
        Row::C10 => 9,
        Row::C11 => 10,
        Row::C12 => 11,
        Row::C13 => 12,
        Row::R1 => 13,
        Row::R2 => 14,
        Row::R3 => 15,
        Row::R4 => 16,
        Row::R5 => 17,
        Row::R6 => 18,
        Row::R7 => 19,
        Row::R8 => 20,
        Row::R9 => 21,
        Row::R10 => 22,
        Row::R11 => 23,
        Row::R12 => 24,
        Row::R13 => 25,
        Row::O1 => 26,
        Row::O2 => 27,
        Row::O3 => 28,
        Row::O4 => 29,
        Row::O5 => 30,
        Row::O6 => 31,
        Row::O7 => 32,
        Row::E1 => 33,
        Row::E2 => 34,
        Row::E3a => 35,
        Row::E3 => 36,
        Row::E5 => 37,
        Row::W1 => 38,
        Row::V1 => 39,
        Row::V2 => 40,
        Row::V3 => 41,
        Row::V4a => 42,
        Row::V4b => 43,
        Row::V5 => 44,
        Row::V6 => 45,
        Row::L1 => 46,
        Row::T1 => 47,
        Row::H2 => 48,
    }
}

/// A phase's variant down to what follows a Stop and the render job; exhaustive.
fn kind(phase: &TransportPhase) -> &'static str {
    match phase {
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::AtCursor } } => "Park AtCursor",
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::Target(_) } } => "Park Target",
        TransportPhase::StopPending { then: AfterStop::Park { end: ParkEnd::ReturnToZero } } => "Park ReturnToZero",
        TransportPhase::Rendering { job: RenderJob::Live } => "Rendering Live",
        TransportPhase::Rendering { job: RenderJob::Retired { .. } } => "Rendering Retired",
        TransportPhase::Stopped
        | TransportPhase::StartPending
        | TransportPhase::Playing
        | TransportPhase::RecordPending { .. }
        | TransportPhase::CountIn { .. }
        | TransportPhase::WaitingForClock { .. }
        | TransportPhase::Recording { .. }
        | TransportPhase::StopPending { then: AfterStop::Resume(_) | AfterStop::Reset } => abbr(phase),
    }
}

/// Every phase variant, with the payload shapes the matrix distinguishes.
fn phases() -> Vec<TransportPhase> {
    let (t, _) = tickets();
    let park = |end| TransportPhase::StopPending { then: AfterStop::Park { end } };
    vec![
        TransportPhase::Stopped,
        TransportPhase::StartPending,
        TransportPhase::Playing,
        TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: Some(PLAN) },
        TransportPhase::RecordPending { ticket: t, from: None, count_in: None },
        TransportPhase::CountIn { ticket: t, from: 0, plan: PLAN },
        TransportPhase::WaitingForClock { ticket: t, from: Some(0), count_in: None },
        TransportPhase::WaitingForClock { ticket: t, from: Some(0), count_in: Some(PLAN) },
        TransportPhase::Recording { latch: point(), count_in: None },
        park(ParkEnd::AtCursor),
        park(ParkEnd::Target(AT)),
        park(ParkEnd::ReturnToZero),
        TransportPhase::StopPending { then: AfterStop::Resume(500) },
        TransportPhase::StopPending { then: AfterStop::Reset },
        TransportPhase::Rendering { job: RenderJob::Live },
        TransportPhase::Rendering { job: RETIRED },
        TransportPhase::Rendering { job: RenderJob::Retired { result: None, reason: Some(CancelReason::Dialog), then_reset: true } },
    ]
}

/// Every event variant, with the payloads the cells distinguish (the current and an older ticket).
fn events(t: Ticket, older: Ticket) -> Vec<Event> {
    let group = |current| PlayOf::Group { current };
    vec![
        Event::Play,
        Event::PlayExact(AT),
        Event::PlaySelection,
        Event::TogglePlay,
        Event::Stop,
        Event::Pause,
        Event::Record(ready(t)),
        Event::Record(RecordPrep::NoArmedTrack),
        Event::Record(RecordPrep::Cannot),
        Event::Locate(AT),
        Event::HalfSpeed,
        Event::Scrub(AT),
        Event::AllNotesOff,
        Event::ResetSession,
        Event::RulerClick(AT),
        Event::PlayApplied(group(true)),
        Event::PlayApplied(group(false)),
        Event::PlayApplied(PlayOf::Render),
        Event::PlayRefused(group(true)),
        Event::PlayRefused(group(false)),
        Event::PlayRefused(PlayOf::Render),
        Event::RecordAdmitted(t),
        Event::RecordAdmitted(older),
        Event::RecordStarted { ticket: t, latch: point() },
        Event::RecordStarted { ticket: older, latch: point() },
        Event::RecordRefused { ticket: t, retired_by_stage: false },
        Event::RecordRefused { ticket: t, retired_by_stage: true },
        Event::RecordRefused { ticket: older, retired_by_stage: false },
        Event::StopFrontier,
        Event::RecordOffApplied,
        Event::RecordOffRefused,
        Event::LocateOrLoopReceipt { applied: true },
        Event::LocateOrLoopReceipt { applied: false },
        Event::ReleaseFrontier,
        Event::SessionRejected,
        Event::UntrackedRefusal,
        Event::PlayerFaulted,
        Event::StageCountingIn(t),
        Event::StageCountingIn(older),
        Event::StageWaiting(t),
        Event::StageStarted { ticket: t, latch: point() },
        Event::StageFailed(t),
        Event::StageFailed(older),
        Event::StageCancelled(t),
        Event::StoppedByItself,
        Event::PlaybackObserved,
        Event::RenderStart,
        Event::RenderCancel(CancelReason::Dialog),
        Event::RenderCancel(CancelReason::DeviceLost),
        Event::RenderPoll,
        Event::RenderResult(RenderOutcome::Completed),
        Event::RenderResult(RenderOutcome::Failed),
        Event::RenderSettled,
        Event::OwedStop { now: Duration::from_secs(1) },
        Event::OwedStop { now: Duration::from_secs(3) },
        Event::OutputTrouble,
        Event::InputTrouble,
        Event::DriverReset,
        Event::ReplaceRequested,
        Event::OldPlayerRetired,
        Event::MidiTransportLost,
        Event::InputLoss,
        Event::LoopChanged,
        Event::LapLimit,
        Event::ResumeCheck(ready(t)),
        Event::ResumeCheck(RecordPrep::NoArmedTrack),
        Event::ResumeCheck(RecordPrep::Cannot),
    ]
}

/// Which outstanding requests can still move the transport: a test-side model beside the facts,
/// which count outstanding identities only (the reducer cannot see it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Capable {
    /// The current group's starts.
    current: bool,
    /// Starts of groups a Stop closed.
    closed: bool,
    /// The pass's record-start continuation (its count-in or clock wait).
    continuation: bool,
}

/// Facts a host can hold in `phase` after an event's own retirement, with what can still move
/// the transport. Outstanding identities are generated wherever they can be tracked (record
/// section 4: closed-group starts stay tracked until their receipts; `Started` does not retire
/// Record). What can still move it:
/// - Closed-group starts and the record continuation, in StopPending, until every Stop is
///   acknowledged (an acknowledged Stop neutralizes earlier ordinary actions and record-start
///   continuations, record section 4). Outside StopPending, the Stop that closed a group was
///   acknowledged.
/// - The current group's starts, in SP and RP. In CI, W and R the Record's action was processed
///   (its stage or receipt was seen), and ordinary actions are processed in publication order
///   (`P/player/control.rs:107-139`), so a start published before it has already been processed,
///   either applied or terminally refused, and cannot execute later; only its receipt may be
///   outstanding.
/// - The record continuation in RP, CI and W (an admitted Record can still start from its
///   clock wait); not in R (it has started).
fn worlds(phase: &TransportPhase) -> Vec<(Facts, Capable)> {
    let col = Col::of(phase);
    let stopping = matches!(col, Col::Stp | Col::Str | Col::Stz);
    let members: &[u32] = if matches!(col, Col::Sp | Col::Rp | Col::Ci | Col::W | Col::R) { &[0, 1] } else { &[0] };
    let stops: &[u32] = if stopping { &[0, 1] } else { &[0] };
    let owed: &[Option<Duration>] = if stopping || col == Col::Rnd { &[None, Some(Duration::ZERO)] } else { &[None] };
    let record: &[bool] = match col {
        Col::Rp => &[true],
        Col::Ci | Col::W | Col::R | Col::Stp | Col::Str | Col::Stz => &[false, true],
        _ => &[false],
    };
    let closed: &[u32] = if col == Col::Rnd { &[0] } else { &[0, 1] };
    let render: &[(bool, u32)] = if col == Col::Rnd { &[(false, 0), (true, 0), (false, 1)] } else { &[(false, 0)] };
    let held: &[Option<Held>] = if col == Col::Stp { &[None, Some(Held { start: StartCommand::Record, generation: 1 })] } else { &[None] };
    let speed: &[Speed] = if matches!(col, Col::Sp | Col::P) { &[Speed::Normal, Speed::Half] } else { &[Speed::Normal] };
    let mut out = Vec::new();
    for &playing in &[false, true] {
        for &members in members {
            for applied in [false, true] {
                for &stops in stops {
                    for &owed in owed {
                        for &record in record {
                            for &closed in closed {
                                for &(render_play, render_stops) in render {
                                    for &held in held {
                                        for &speed in speed {
                                            for release in [false, true] {
                                                for faulted in [false, true] {
                                                    let unacknowledged = stopping && (stops > 0 || owed.is_some());
                                                    let capable = Capable {
                                                        current: matches!(col, Col::Sp | Col::Rp),
                                                        closed: unacknowledged,
                                                        continuation: matches!(col, Col::Rp | Col::Ci | Col::W) || unacknowledged,
                                                    };
                                                    let tracked = Tracked {
                                                        starts: members + closed,
                                                        record,
                                                        stops,
                                                        release,
                                                        render_play,
                                                        render_stops,
                                                        ..Tracked::default()
                                                    };
                                                    let facts = Facts {
                                                        tracked,
                                                        group: StartGroup { members, applied },
                                                        owed_stop_since: owed,
                                                        held,
                                                        deferred: if col == Col::Str { 32 } else { 0 },
                                                        reset_generation: 1,
                                                        speed,
                                                        playing,
                                                        position: 70,
                                                        cursor: 40,
                                                        faulted,
                                                        finalizing: false,
                                                        reopen_pending: false,
                                                    };
                                                    out.push((facts, capable));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// The host's retirement before the reducer runs: a Record receipt or terminal stage of the
/// current ticket retires the Record, and a refusal or terminal stage ends its continuation.
fn retire(event: &Event, mut facts: Facts, mut capable: Capable, current: Ticket) -> (Facts, Capable) {
    if let Event::RecordAdmitted(t)
    | Event::RecordStarted { ticket: t, .. }
    | Event::RecordRefused { ticket: t, .. }
    | Event::StageFailed(t)
    | Event::StageCancelled(t) = *event
        && t == current
    {
        facts.tracked.record = false;
        if matches!(event, Event::RecordRefused { .. } | Event::StageFailed(_) | Event::StageCancelled(_)) {
            capable.continuation = false;
        }
    }
    (facts, capable)
}

#[test]
fn the_matrix_has_no_empty_cell() {
    for (i, row) in Row::ALL.into_iter().enumerate() {
        assert_eq!(row_index(row), i, "{row:?} is in Row::ALL once, in order");
    }
    for (i, col) in Col::ALL.into_iter().enumerate() {
        assert_eq!(col.index(), i, "{col:?}");
    }
    let (t, older) = tickets();
    let events = events(t, older);
    let rows: HashSet<Row> = events.iter().map(Event::row).collect();
    assert_eq!(rows, Row::ALL.into_iter().collect::<HashSet<_>>(), "every row has an event and every event a row");
    let phases = phases();
    let kinds: HashSet<&str> = phases.iter().map(kind).collect();
    let want = [
        "S",
        "SP",
        "P",
        "RP",
        "CI",
        "W",
        "R",
        "Park AtCursor",
        "Park Target",
        "Park ReturnToZero",
        "STr",
        "STz",
        "Rendering Live",
        "Rendering Retired",
    ];
    assert_eq!(kinds, want.into_iter().collect::<HashSet<_>>(), "every phase the record defines");
    assert_eq!(phases.iter().map(Col::of).collect::<HashSet<_>>(), Col::ALL.into_iter().collect::<HashSet<_>>());
    for row in Row::ALL {
        for col in Col::ALL {
            assert!(!cell(row, col).is_empty(), "{row:?} {col:?} is empty");
        }
    }
    // Total: every phase, event and facts give a step; a refusal keeps the phase, with no effect.
    let mut steps = 0usize;
    for phase in &phases {
        for event in &events {
            for (facts, _) in worlds(phase) {
                let step = next(phase, event, &facts);
                if step.result.is_err() {
                    assert_eq!((step.phase, step.effects.len()), (*phase, 0), "{phase:?} {event:?} {facts:?}");
                }
                steps += 1;
            }
        }
    }
    assert!(steps > 10_000, "{steps}");
}

#[test]
fn stopped_implies_nothing_tracked() {
    let (t, older) = tickets();
    let events = events(t, older);
    let mut entered = 0usize;
    for phase in phases() {
        for event in &events {
            for (facts, capable) in worlds(&phase) {
                let (facts, mut capable) = retire(event, facts, capable, t);
                let step = next(&phase, event, &facts);
                if step.phase != TransportPhase::Stopped || step.result.is_err() {
                    continue;
                }
                let why = format!("{phase:?} {event:?} {facts:?} {capable:?} -> {:?}", step.effects);
                let moving = step
                    .effects
                    .iter()
                    .any(|e| matches!(e, Effect::SubmitStart(_) | Effect::SubmitStop | Effect::SubmitRecord { .. } | Effect::SubmitRenderPlay));
                assert!(!moving, "Stopped with a request that moves the transport: {why}");
                let exempt = facts.faulted || step.effects.contains(&Effect::SetFaulted) || step.effects.contains(&Effect::RetireAll);
                if phase == TransportPhase::Stopped || exempt {
                    continue;
                }
                // What the transition neutralized: every Stop acknowledged, or the pass finished.
                if *event == Event::StopFrontier && facts.tracked.stops == 0 && facts.owed_stop_since.is_none() {
                    capable = Capable { current: false, closed: false, continuation: false };
                }
                if step.effects.contains(&Effect::FinishPass) {
                    capable.continuation = false;
                }
                let f = &facts;
                assert!(!(capable.current && f.group.members > 0), "a current start can still move the transport: {why}");
                assert!(!(capable.closed && f.tracked.starts > f.group.members), "a closed group's start can still move it: {why}");
                assert!(!capable.continuation, "the record start can still move it: {why}");
                assert_eq!(f.tracked.stops, 0, "a Stop is tracked: {why}");
                assert!(f.owed_stop_since.is_none(), "a Stop is owed: {why}");
                assert!(!f.playing, "playback is observed: {why}");
                assert!(!f.tracked.render_play && f.tracked.render_stops == 0, "the render's requests are tracked: {why}");
                entered += 1;
            }
        }
    }
    assert!(entered > 100, "transitions into Stopped checked: {entered}");
}

#[test]
fn judged_cells_follow_the_amended_text() {
    let (t, _) = tickets();
    let f = Facts { position: 70, cursor: 40, ..Facts::default() };
    let live = TransportPhase::Rendering { job: RenderJob::Live };
    let retired = |result, reason, then_reset| TransportPhase::Rendering { job: RenderJob::Retired { result, reason, then_reset } };

    // A cancel that returned a finished result: E3 fills it, keeping the reason and the Reset.
    let cancelled = run(live, Event::ResetSession, f).phase;
    assert_eq!(cancelled, retired(None, Some(CancelReason::SessionReplaced), true));
    let filled = run(cancelled, Event::RenderResult(RenderOutcome::Completed), f).phase;
    assert_eq!(filled, retired(Some(RenderOutcome::Completed), Some(CancelReason::SessionReplaced), true));
    assert_eq!(run(filled, Event::RenderResult(RenderOutcome::Failed), f).phase, filled, "a collected result stands");
    let settled = run(filled, Event::RenderSettled, f);
    let message = Message::RenderSettled { result: Some(RenderOutcome::Completed), reason: Some(CancelReason::SessionReplaced) };
    assert_eq!(settled.effects, [Effect::Message(message), Effect::ResetContinuation]);
    assert_eq!(run(live, Event::RenderResult(RenderOutcome::Failed), f).phase, retired(Some(RenderOutcome::Failed), None, false));

    // A render-Play refusal: a live job is disposed and retired empty; a retired one keeps all.
    let disposed = run(live, Event::PlayRefused(PlayOf::Render), f);
    assert_eq!(disposed.phase, retired(None, None, false));
    assert_eq!(disposed.effects, [Effect::Message(Message::PlayerRefusal), Effect::DisposeRenderWithoutStop]);
    let late = run(cancelled, Event::PlayRefused(PlayOf::Render), f);
    assert_eq!((late.phase, late.effects), (cancelled, vec![Effect::Message(Message::PlayerRefusal)]));
    assert_eq!(run(live, Event::RenderStart, f).result, Err(Refusal::RenderRunning));

    // O6 in W: only a punch ends the pass.
    let punch = TransportPhase::WaitingForClock { ticket: t, from: None, count_in: None };
    let stopped_start = TransportPhase::WaitingForClock { ticket: t, from: Some(0), count_in: None };
    let ended = run(punch, Event::StoppedByItself, f);
    assert_eq!((ended.phase, ended.effects), (TransportPhase::Stopped, vec![Effect::FinishPass]));
    let kept = run(stopped_start, Event::StoppedByItself, f);
    assert_eq!((kept.phase, kept.effects.len()), (stopped_start, 0));

    // (Tr) on C2 and C3 in S; (Ta, Te) on C7 in SP and P; retained := at where section 9 says.
    let reopen = Facts { reopen_pending: true, ..f };
    assert_eq!(run(TransportPhase::Stopped, Event::PlayExact(AT), reopen).result, Err(Refusal::Tr));
    assert_eq!(run(TransportPhase::Stopped, Event::PlaySelection, reopen).result, Err(Refusal::Tr));
    assert_eq!(run(TransportPhase::StartPending, Event::Record(RecordPrep::NoArmedTrack), f).result, Err(Refusal::Ta));
    assert_eq!(run(TransportPhase::Playing, Event::Record(RecordPrep::Cannot), f).result, Err(Refusal::Te));
    let retained = Effect::SetRetained(AT);
    for phase in [TransportPhase::StartPending, TransportPhase::Playing] {
        assert!(run(phase, Event::PlayExact(AT), f).effects.contains(&retained), "{phase:?}");
    }
    assert!(!run(PARK, Event::PlayExact(AT), f).effects.contains(&retained), "a held PlayExact writes nothing yet");
    let rp = TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: None };
    let take_safe = run(rp, Event::Locate(AT), f);
    assert_eq!(take_safe.phase, TransportPhase::StopPending { then: AfterStop::Resume(AT) });
    assert!(take_safe.effects.contains(&retained));
    let retarget = run(TransportPhase::StopPending { then: AfterStop::Resume(500) }, Event::Locate(AT), f);
    assert_eq!((retarget.phase, retarget.effects), (TransportPhase::StopPending { then: AfterStop::Resume(AT) }, vec![retained]));

    // Leaving StartPending or Playing sets speed Normal, first; staying does not.
    let half = Facts { speed: Speed::Half, ..f };
    let stop = run(TransportPhase::Playing, Event::Stop, half);
    assert_eq!(stop.effects, [Effect::SetSpeed(Speed::Normal), Effect::SubmitStop]);
    assert_eq!(run(TransportPhase::StartPending, Event::PlayerFaulted, half).effects[0], Effect::SetSpeed(Speed::Normal));
    assert!(!run(TransportPhase::Playing, Event::Locate(AT), half).effects.contains(&Effect::SetSpeed(Speed::Normal)));
    assert_eq!(run(TransportPhase::StartPending, Event::Record(ready(t)), f).effects[0], Effect::SetSpeed(Speed::Normal));

    // The count-in contract: a count-in without `from` changes nothing, not even the deferred.
    let broken = TransportPhase::RecordPending { ticket: t, from: None, count_in: Some(PLAN) };
    let admitted = run(broken, Event::RecordAdmitted(t), f);
    assert_eq!((admitted.phase, admitted.effects.len()), (broken, 0));

    // A Stop frontier preceding an older Play receipt: Stopped, and the late receipt changes
    // nothing but its refusal text.
    let older_play = Facts { tracked: Tracked { starts: 1, ..Tracked::default() }, ..f };
    assert_eq!(run(PARK, Event::StopFrontier, older_play).phase, TransportPhase::Stopped);
    let receipt = |event| run(TransportPhase::Stopped, event, f);
    let applied = receipt(Event::PlayApplied(PlayOf::Group { current: false }));
    assert_eq!((applied.phase, applied.effects.len()), (TransportPhase::Stopped, 0));
    let refused = receipt(Event::PlayRefused(PlayOf::Group { current: false }));
    assert_eq!((refused.phase, refused.effects), (TransportPhase::Stopped, vec![Effect::Message(Message::PlayerRefusal)]));
}

/// Judging round 2's counterexample (D-4): in RP the current group has one Play applied and ended
/// and another still outstanding; the Record fails before its audio action. The destination must
/// be StartPending, so that the Player then stopping by itself cannot reach Stopped while that
/// Play can still execute.
#[test]
fn a_failed_record_with_an_outstanding_start_waits_for_the_group() {
    let (t, _) = tickets();
    let rp = TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: None };
    let outstanding =
        Facts { tracked: Tracked { starts: 1, ..Tracked::default() }, group: StartGroup { members: 1, applied: true }, ..Facts::default() };
    for failure in [Event::StageCancelled(t), Event::StageFailed(t), Event::RecordRefused { ticket: t, retired_by_stage: false }] {
        let failed = run(rp, failure, outstanding);
        assert_eq!(failed.phase, TransportPhase::StartPending, "{failure:?}: the outstanding start decides");
        assert!(failed.effects.contains(&Effect::FinishPass), "{failure:?}");
        let then = run(failed.phase, Event::StoppedByItself, outstanding);
        assert_ne!(then.phase, TransportPhase::Stopped, "{failure:?}: Stopped with a start that can still execute");
    }
    // With no outstanding member: Playing only when playback is observed, else Stopped.
    let processed = Facts { group: StartGroup { members: 0, applied: true }, ..Facts::default() };
    assert_eq!(run(rp, Event::StageCancelled(t), processed).phase, TransportPhase::Stopped);
    assert_eq!(run(rp, Event::StageCancelled(t), Facts { playing: true, ..processed }).phase, TransportPhase::Playing);
}
