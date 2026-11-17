//! ADR 0062 slice S2a: the literal transition matrix and its pure reducer. `transport_phase_matrix`
//! (in `grid_tests.rs`) pins every cell of tables.md section A in full; the conditions the cells
//! name are pinned branch by branch; `the_matrix_has_no_empty_cell` enumerates every phase, event,
//! row and column. Section 3's invariants are proven by the reachability closure in
//! `closure_tests.rs`.
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
        [Effect::FaultRuntime, Effect::CancelRender(CancelReason::Fault), Effect::RetireAll, Effect::SetFaulted, Effect::Message(Message::Fault)]
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

/// Facts for the totality sweep: every phase with the tracked, owed, group, held and render
/// facts it can carry, playback on and off, faulted or not. (Section 3's invariants are proven
/// over what the table reaches, in `closure_tests.rs`.)
fn worlds(phase: &TransportPhase) -> Vec<Facts> {
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
                                                    out.push(facts);
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
            for facts in worlds(phase) {
                let step = next(phase, event, &facts);
                if step.result.is_err() {
                    assert_eq!((step.phase, step.effects.len()), (*phase, 0), "{phase:?} {event:?} {facts:?}");
                }
                // Each effect appears at most once in a step (the effect interface).
                for (i, e) in step.effects.iter().enumerate() {
                    assert!(!step.effects[i + 1..].contains(e), "{e:?} twice: {phase:?} {event:?} {:?}", step.effects);
                }
                steps += 1;
            }
        }
    }
    assert!(steps > 10_000, "{steps}");
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

/// The legend's global rule: while `faulted`, every start refuses with the fault's text, before
/// the reopen, preparation, half-speed and recording-speed refusals its cell names.
#[test]
fn the_fault_refusal_comes_before_every_start_refusal() {
    let (t, _) = tickets();
    let faulted = Facts { faulted: true, ..Facts::default() };
    let rp = TransportPhase::RecordPending { ticket: t, from: Some(0), count_in: None };
    let cases = [
        (TransportPhase::Stopped, Event::Play, Facts { reopen_pending: true, ..faulted }),
        (TransportPhase::Stopped, Event::PlayExact(AT), Facts { reopen_pending: true, ..faulted }),
        (TransportPhase::Stopped, Event::Record(RecordPrep::NoArmedTrack), faulted),
        (TransportPhase::Stopped, Event::Record(RecordPrep::Cannot), faulted),
        (TransportPhase::StartPending, Event::Record(ready(t)), Facts { speed: Speed::Half, ..faulted }),
        (TransportPhase::Stopped, Event::HalfSpeed, Facts { reopen_pending: true, ..faulted }),
        (rp, Event::HalfSpeed, faulted),
    ];
    for (phase, event, facts) in cases {
        let step = next(&phase, &event, &facts);
        assert_eq!((step.phase, step.effects.len(), step.result), (phase, 0, Err(Refusal::Faulted)), "{phase:?} {event:?}");
    }
    // Not faulted, the cell's own refusals stand.
    assert_eq!(next(&rp, &Event::HalfSpeed, &Facts::default()).result, Err(Refusal::Th));
}

/// The effect interface: `SubmitRecord` carries the prepared start and its punch comes from the
/// start's `from`, never from observed playback; C9 sets the half speed before its start.
#[test]
fn the_effect_interface_carries_the_start_and_orders_half_speed_first() {
    let (t, _) = tickets();
    let explicit = RecordStart { ticket: t, from: Some(0), count_in: None };
    let punch = RecordStart { ticket: t, from: None, count_in: None };
    let playing = Facts { playing: true, ..Facts::default() };
    let s = run(TransportPhase::Stopped, Event::Record(RecordPrep::Ready(explicit)), playing);
    assert_eq!(s.effects, [Effect::SubmitRecord(explicit)]);
    assert!(!explicit.punch(), "fresh playback does not make an explicit start a punch");
    let p = run(TransportPhase::Playing, Event::Record(RecordPrep::Ready(punch)), playing);
    assert!(p.effects.contains(&Effect::SubmitRecord(punch)) && punch.punch());
    let resumed = run(TransportPhase::StopPending { then: AfterStop::Resume(AT) }, Event::ResumeCheck(RecordPrep::Ready(explicit)), Facts::default());
    assert_eq!(resumed.effects, [Effect::SubmitRecord(explicit)]);

    let half = Effect::SetSpeed(Speed::Half);
    let start = Effect::SubmitStart(StartCommand::HalfSpeed);
    let f = Facts::default();
    assert_eq!(run(TransportPhase::Stopped, Event::HalfSpeed, f).effects, [half, Effect::NewGroup, start]);
    assert_eq!(run(TransportPhase::StartPending, Event::HalfSpeed, f).effects, [half, start]);
    assert_eq!(run(TransportPhase::Playing, Event::HalfSpeed, f).effects, [half, Effect::CloseGroup, Effect::NewGroup, start]);
}
