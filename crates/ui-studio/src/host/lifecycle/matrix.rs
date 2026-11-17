//! ADR 0062 appendix section A, the transition matrix, as a literal table (slice S2a).
//!
//! [`row`] has one line per event row of tables.md section A, in the same order, and eleven cells
//! per line in the same column order: S, SP, P, RP, CI, W, R, STp, STr, STz, RND. Each cell is the
//! list of [`Op`]s its text names, in the order the text names them; `—` is written [`Op::Dash`],
//! so no cell is ever empty. A merged "every column" cell is written in all eleven columns. The
//! text in an event name that applies to every column (R13, V3, V5) is [`header`]; the conditions
//! in an event name (O6, O7, E5, H2, W1) are checked by the reducer before the cell. The reducer
//! ([`super::next`]) interprets the ops; nothing here reads facts.
//!
//! The cells are the final reducer (the compatibility selectors are slice S2d's). A transition
//! out of StartPending or Playing sets speed Normal (the legend; applied by the reducer, not
//! written in each cell).

use super::phase::{AfterStop, TransportPhase};

/// A column of the matrix: the phase, with StopPending split by what follows the Stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Col {
    S,
    Sp,
    P,
    Rp,
    Ci,
    W,
    R,
    Stp,
    Str,
    Stz,
    Rnd,
}

impl Col {
    /// Every column, in tables.md order.
    pub const ALL: [Col; 11] = [Col::S, Col::Sp, Col::P, Col::Rp, Col::Ci, Col::W, Col::R, Col::Stp, Col::Str, Col::Stz, Col::Rnd];

    /// The column a phase reads.
    pub fn of(phase: &TransportPhase) -> Col {
        match phase {
            TransportPhase::Stopped => Col::S,
            TransportPhase::StartPending => Col::Sp,
            TransportPhase::Playing => Col::P,
            TransportPhase::RecordPending { .. } => Col::Rp,
            TransportPhase::CountIn { .. } => Col::Ci,
            TransportPhase::WaitingForClock { .. } => Col::W,
            TransportPhase::Recording { .. } => Col::R,
            TransportPhase::StopPending { then: AfterStop::Park { .. } } => Col::Stp,
            TransportPhase::StopPending { then: AfterStop::Resume(_) } => Col::Str,
            TransportPhase::StopPending { then: AfterStop::Reset } => Col::Stz,
            TransportPhase::Rendering { .. } => Col::Rnd,
        }
    }

    /// The position of the column in a row.
    pub fn index(self) -> usize {
        match self {
            Col::S => 0,
            Col::Sp => 1,
            Col::P => 2,
            Col::Rp => 3,
            Col::Ci => 4,
            Col::W => 5,
            Col::R => 6,
            Col::Stp => 7,
            Col::Str => 8,
            Col::Stz => 9,
            Col::Rnd => 10,
        }
    }
}

/// An event row of the matrix, named as tables.md names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    C1,
    C2,
    C3,
    C4,
    C5,
    C6,
    C7,
    C8,
    C9,
    C10,
    C11,
    C12,
    C13,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    O1,
    O2,
    O3,
    O4,
    O5,
    O6,
    O7,
    E1,
    E2,
    E3a,
    E3,
    E5,
    W1,
    V1,
    V2,
    V3,
    V4a,
    V4b,
    V5,
    V6,
    L1,
    T1,
    H2,
}

impl Row {
    /// Every row, in tables.md order.
    #[rustfmt::skip]
    pub const ALL: [Row; 49] = [
        Row::C1, Row::C2, Row::C3, Row::C4, Row::C5, Row::C6, Row::C7, Row::C8, Row::C9, Row::C10, Row::C11, Row::C12, Row::C13,
        Row::R1, Row::R2, Row::R3, Row::R4, Row::R5, Row::R6, Row::R7, Row::R8, Row::R9, Row::R10, Row::R11, Row::R12, Row::R13,
        Row::O1, Row::O2, Row::O3, Row::O4, Row::O5, Row::O6, Row::O7,
        Row::E1, Row::E2, Row::E3a, Row::E3, Row::E5, Row::W1,
        Row::V1, Row::V2, Row::V3, Row::V4a, Row::V4b, Row::V5, Row::V6,
        Row::L1, Row::T1, Row::H2,
    ];
}

/// A refusal of tables.md's legend: the phase is kept, the command returns `Err`, the text goes
/// to the status line. `Faulted` is the global fault refusal; `DeferredFull` the 33rd deferred
/// command (record section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Tr,
    Ta,
    Te,
    Ts,
    Th,
    Td,
    Tn,
    Tx,
    /// E1 in RND: "A real-time render is already running."
    RenderRunning,
    Faulted,
    DeferredFull,
}

/// One phrase of a cell's text. The reducer applies a cell's ops in order; a refusal ends the
/// cell with `Err`, a guard that does not hold ends it with what was applied so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// `—`: no effect, no message.
    Dash,
    /// The phase the cell names is the current one; nothing else (C12 in STz, "Preserve").
    Same,
    /// The receipt's retirement (done before the cell, preamble) is the whole effect.
    Retired,
    /// `(Tr)`: refused while a reopen is pending.
    Tr,
    /// `(Ta)`: refused when no track is armed (with the Record hint).
    Ta,
    /// `(Te)`: refused with "Cannot record: ...".
    Te,
    /// `half: Ts`: refused at half speed, speed kept.
    Ts,
    /// An unconditional refusal.
    Refuse(Refusal),
    /// `SP`: the event's start is submitted into a new group.
    Sp,
    /// `joins the group`.
    Join,
    /// `SP, new group`: the current group is closed and the start opens a new one.
    NewGroup,
    /// `½`: half speed.
    Half,
    /// `retained := at`.
    Retained,
    /// `cursor := at`.
    Cursor,
    /// `position := at`.
    Position,
    /// `hold`: held until the stop completes, replacing any held start.
    Hold,
    /// `def`: deferred (the 33rd is refused).
    Defer,
    /// `!`: the pass ends here.
    Finish,
    /// O6 in W: `from absent (punch)`; otherwise the cell has no effect.
    Punch,
    /// `STp`: StopPending Park with `end` AtCursor; Stop submitted (owed by W1 if refused).
    Park,
    /// `STp with end = ReturnToZero; submit Stop`.
    ParkRtz,
    /// `group closed`.
    CloseGroup,
    /// `end := ReturnToZero`.
    EndRtz,
    /// `end := Target(at)`.
    EndTarget,
    /// `held start dropped`, `held discarded`.
    DropHeld,
    /// `deferred (commands) discarded`.
    DiscardDeferred,
    /// `Stop re-sent`.
    ResendStop,
    /// `RP`: the prepared record start is submitted from stopped (or from fresh playback).
    Rp,
    /// `RP punch`.
    RpPunch,
    /// `Player Locate (tracked)`.
    LocatePlayer,
    /// `STr(at)`: the take-safe locate; Stop submitted.
    TakeSafe,
    /// `target := at` (StopPending Resume).
    Retarget,
    /// `release sent (tracked)`.
    Release,
    /// `render cancelled` (E2 with the event's reason).
    CancelRender,
    /// `then_reset`.
    ThenReset,
    /// `STz`: StopPending Reset; Stop submitted.
    Reset,
    /// `as C8`: the C8 cell of the same column.
    AsC8,
    /// R1 in SP: `current group: applied := true; group empty: P, "Playing"`.
    GroupApplied,
    /// R2 in SP: `current group empty: P if applied or playback observed, else S, speed Normal`.
    GroupRefused,
    /// R2 in RND: `render Play: if job Live, dispose without Stop and retain Rendering with job
    /// Retired { result: None, reason: None, then_reset: false }; if already Retired, keep its
    /// entire payload`.
    DisposeRender,
    /// `Tp`: the Player's refusal text.
    Tp,
    /// `Tf once per ticket`: the stage's failure text.
    Tf,
    /// R3 in RP: `with both from and count_in, CI; with count_in absent, W; with count_in present
    /// but from absent, no effect`.
    Admitted,
    /// O1 in RP: `with both from and count_in: CI`; otherwise no effect.
    CountIn,
    /// `W` (stage Waiting).
    Wait,
    /// `R, "Recording"`.
    Latch,
    /// `deferred released once`.
    ReleaseDeferred,
    /// R5: `unless a terminal stage retired Record`.
    UnlessStageRetired,
    /// `by 4`: Playing, StartPending or Stopped by record section 4.
    ByFour,
    /// R6: `when all Stops required by this continuation have retired, none is owed, and fresh
    /// playback is false` (otherwise the phase is retained).
    StopsSettled,
    /// R6 in STp: the `end` branch (Target, ReturnToZero, AtCursor).
    ApplyEnd,
    /// `S`.
    ToStopped,
    /// `"Stopped" unless a state is live`.
    Stopped,
    /// `the held start (current reset generation) admitted`.
    AdmitHeld,
    /// `schedule Resume check (H2) at H4`.
    ResumeCheck,
    /// E5: `S; the result's or the reason's message; then the Reset continuation if set`.
    Settle,
    /// R10: `"All MIDI notes off"`.
    NotesOff,
    /// `msg`: the event's message.
    Msg,
    /// R13: every fact and owed Stop retired, groups closed, held and deferred discarded,
    /// resources finalized.
    RetireAll,
    /// R13: `faulted`, adoption off.
    Faulted,
    /// V4b (and V4a in S): every fact retired, `faulted` cleared; S with the new Player.
    NewPlayer,
    /// `speed Normal`.
    Normal,
    /// O7: `P (adopted, logged)`.
    Adopt,
    /// E1 in S: `with exclusive_ok: RND, job Live, render Play tracked; without: Tx`.
    RenderStart,
    /// E3a: `newly submitted Stops tracked, newly unsent ones owed by W1`.
    RenderPoll,
    /// E3: `store the collected result: Live becomes Retired with reason None and then_reset
    /// false; Retired with result None gains the result, preserving reason and then_reset; an
    /// already collected result stands`.
    RenderResult,
    /// W1: `retried each tick, an expired deadline checked first; sent: tracked; 2 s unsent:
    /// fault_runtime(), then R13`.
    RetryOwed,
    /// V5: `offers cancelled`.
    CancelOffers,
    /// V5: `recovering`.
    Recovering,
    /// L1: `loop_seen := wanted`.
    LoopSeen,
    /// L1: `set_loop sent (tracked)`.
    SetLoop,
    /// L1: `keep only the latest wanted-loop fact; submit nothing`.
    KeepLoop,
    /// T1: `Note (section E)`.
    LapNote,
    /// H2 in STr: `the full Record start at the target: RP; a local refusal: S, take preserved,
    /// reason shown, deferred released`.
    ResumeStart,
    /// R9: `refused: Tp`.
    LocateReceipt,
}

use Op::*;

const D: &[Op] = &[Dash];
const TN: &[Op] = &[Refuse(Refusal::Tn)];
const TH: &[Op] = &[Refuse(Refusal::Th)];
const TD: &[Op] = &[Refuse(Refusal::Td)];
const TX: &[Op] = &[Refuse(Refusal::Tx)];

/// What an event name says for every column (R13, V3, V5), applied before the cell.
pub fn header(row: Row) -> &'static [Op] {
    match row {
        // R13: "every column: every fact and owed Stop retired, groups closed, held and deferred
        // discarded, resources finalized, `faulted`, adoption off".
        Row::R13 => &[RetireAll, Faulted],
        // V3: "every fact retired as in R13, without setting faulted".
        Row::V3 => &[RetireAll],
        // V5: "offers cancelled, held and deferred cleared, `recovering`, msg".
        Row::V5 => &[CancelOffers, DropHeld, DiscardDeferred, Recovering, Msg],
        _ => &[],
    }
}

/// The cells of one event row, in column order S, SP, P, RP, CI, W, R, STp, STr, STz, RND.
#[rustfmt::skip]
pub fn row(row: Row) -> [&'static [Op]; 11] {
    match row {
        //                  S                              SP                     P                       RP                    CI                    W                     R                     STp                         STr                        STz                    RND
        // Commands
        Row::C1  => [&[Tr, Sp],                     D,                     D,                      D,                    &[Finish, Park],      D /* D1 */,           D,                    &[Hold] /* D4 */,           &[Defer],                  D,                     TN],
        Row::C2  => [&[Tr, Sp, Retained],           &[Join, Retained],     &[NewGroup, Retained],        D /* D2 */,           &[Finish, Park] /* D2 */, D /* D2 */,       D /* D2 */,           &[Hold] /* D4 */,           &[Defer],                  D,                     TN],
        Row::C3  => [&[Tr, Sp],                     &[Join],               &[NewGroup],            D /* D2 */,           &[Finish, Park],      D /* D2 */,           D /* D2 */,           &[Hold] /* D4 */,           &[Defer],                  D,                     TN],
        Row::C4  => [&[Tr, Sp],                     &[Park, CloseGroup],   &[Park],                &[Finish, Park],      &[Finish, Park],      &[Finish, Park] /* D1 */, &[Finish, Park],  &[Hold] /* a start, D4 */,  &[Defer],                  D,                     TN],
        // C5, C6, C7 in STr; C12 in STp and STr; V1, V2, V5 in STr: "submit Stop or owe it by W1"
        // (a new Stop even when one is tracked or owed, record section 4): Park and Reset submit it.
        Row::C5  => [&[ParkRtz],                    &[Park, CloseGroup],   &[Park],                &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[EndRtz, DropHeld] /* D9 */, &[Park],                 &[ResendStop],         TN],
        Row::C6  => [D /* D6 */,                    &[Park, CloseGroup],   &[Park],                &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[DropHeld],                &[Park],                   D,                     TN],
        // C7 in STr: the restart is disabled before the drain; the Record is consumed here.
        Row::C7  => [&[Tr, Ta, Te, Rp],             &[Ts, Ta, Te, Rp],     &[Ts, Ta, Te, RpPunch],     &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[Hold] /* D4 */,           &[Park, DiscardDeferred],  D,                     TN],
        Row::C8  => [&[Cursor, Position, Retained], &[Cursor, Retained],   &[LocatePlayer, Cursor, Retained], &[Finish, TakeSafe, Retained], &[Finish, TakeSafe, Retained], &[Finish, TakeSafe, Retained], &[Finish, TakeSafe, Retained], &[EndTarget, Cursor] /* D5 */, &[Retarget, Retained], D,                     TN],
        Row::C9  => [&[Tr, Sp, Half] /* B7 */,      &[Join, Half],         &[NewGroup, Half],      TH,                   &[Finish, Park],      TH,                   TH,                   &[Hold] /* D4 */,           TH,                        D,                     TN],
        Row::C10 => [&[Position],                   &[Position],           &[Position],            &[Position],          &[Position],          &[Position],          &[Position],          &[Position],                &[Position],               &[Position],           &[Position]],
        Row::C11 => [&[Release],                    &[Release],            &[Release],             &[Release],           &[Release],           &[Release],           &[Release],           &[Release],                 &[Release],                &[Release],            &[CancelRender, Release]],
        Row::C12 => [&[Reset],                      &[Reset, CloseGroup],  &[Reset],               &[Finish, Reset],     &[Finish, Reset],     &[Finish, Reset],     &[Finish, Reset],     &[Reset, DropHeld],         &[Reset, DiscardDeferred] /* D3 */, &[Same],      &[CancelRender, ThenReset] /* D8 */],
        Row::C13 => [&[AsC8] /* B14 */,             &[AsC8] /* B14 */,     &[AsC8] /* B14 */,      &[AsC8] /* B14 */,    &[AsC8],              &[AsC8],              &[AsC8],              &[AsC8] /* B14 */,          &[AsC8] /* B14 */,         D,                     TN /* B14 */],
        // Receipts
        Row::R1  => [D,                             &[GroupApplied],       D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     &[Retired]],
        Row::R2  => [&[Tp],                         &[Tp, GroupRefused],   &[Tp],                  &[Tp],                &[Tp],                &[Tp],                &[Tp],                &[Tp],                      &[Tp],                     &[Tp],                 &[Tp, DisposeRender]],
        Row::R3  => [D,                             D,                     D,                      &[Admitted, ReleaseDeferred], D,            D,                    D,                    D,                          D,                         D,                     D],
        Row::R4  => [D,                             D,                     D,                      &[Latch, ReleaseDeferred], &[Latch],        &[Latch],             D /* never regresses */, D,                       D,                         D,                     D],
        Row::R5  => [D,                             D,                     D,                      &[UnlessStageRetired, Finish, ByFour, Tp, ReleaseDeferred], &[UnlessStageRetired, Finish, ByFour, Tp, ReleaseDeferred], &[UnlessStageRetired, Finish, ByFour, Tp, ReleaseDeferred], D, D, D, D, D],
        Row::R6  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    &[StopsSettled, ApplyEnd, ToStopped, Stopped, AdmitHeld], &[ResumeCheck], &[StopsSettled, ToStopped], &[Retired, Settle]],
        Row::R7  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          &[ResumeCheck],            D,                     D],
        Row::R8  => [&[Tp],                         &[Tp],                 &[Tp],                  &[Tp],                &[Tp],                &[Tp],                &[Tp],                &[Tp],                      &[ResumeCheck, Tp],        &[Tp],                 &[Tp]],
        Row::R9  => [&[LocateReceipt],              &[LocateReceipt],      &[LocateReceipt],       &[LocateReceipt],     &[LocateReceipt],     &[LocateReceipt],     &[LocateReceipt],     &[LocateReceipt],           &[LocateReceipt],          &[LocateReceipt],      &[LocateReceipt]],
        Row::R10 => [&[NotesOff],                   &[NotesOff],           &[NotesOff],            &[NotesOff],          &[NotesOff],          &[NotesOff],          &[NotesOff],          &[NotesOff],                &[NotesOff],               &[NotesOff],           &[NotesOff]],
        Row::R11 => [&[Msg],                        &[Msg],                &[Msg],                 &[Msg],               &[Msg],               &[Msg],               &[Msg],               &[Msg],                     &[Msg],                    &[Msg],                &[Msg]],
        Row::R12 => [&[Tp],                         &[Tp],                 &[Tp],                  &[Tp],                &[Tp],                &[Tp],                &[Tp],                &[Tp],                      &[Tp],                     &[Tp],                 &[Tp]],
        Row::R13 => [&[ToStopped, Msg],             &[ToStopped, Msg],     &[ToStopped, Msg],      &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[ToStopped, Msg], &[ToStopped, Msg], &[ToStopped, Msg], &[CancelRender, ToStopped, Msg]],
        // Observations
        // R3 and O1 in RP: CountIn only with both `from` and the count-in (the preparation contract,
        // record section 3); otherwise no effect.
        Row::O1  => [D,                             D,                     D,                      &[CountIn, ReleaseDeferred], D,             D,                    D,                    D,                          D,                         D,                     D],
        Row::O2  => [D,                             D,                     D,                      &[Wait, ReleaseDeferred], &[Wait] /* B3 */, D,                    D,                    D,                          D,                         D,                     D],
        Row::O3  => [D,                             D,                     D,                      &[Latch, ReleaseDeferred], &[Latch],        &[Latch] /* B1 */,    D,                    D,                          D,                         D,                     D],
        Row::O4  => [D,                             D,                     D,                      &[Finish, ByFour, Tf, ReleaseDeferred], &[Finish, ByFour, Tf, ReleaseDeferred], &[Finish, ByFour, Tf, ReleaseDeferred], D, D, D, D, D],
        Row::O5  => [D,                             D,                     D,                      &[Finish, ByFour, ReleaseDeferred], &[Finish, ByFour, ReleaseDeferred], &[Finish, ByFour, ReleaseDeferred], D, D, D, D, D],
        Row::O6  => [D,                             D /* the group decides */, &[ToStopped, Normal], D,                  D,                    &[Punch, Finish, ToStopped], &[Finish, ToStopped], D,                          D,                         D,                     D],
        Row::O7  => [&[Adopt],                      D,                     D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     D],
        // Render, device, loop, timers, owed Stops, Resume
        Row::E1  => [&[RenderStart],                TX,                    TX,                     TX,                   TX,                   TX,                   TX,                   TX,                         TX,                        TX,                    &[Refuse(Refusal::RenderRunning)]],
        Row::E2  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     &[CancelRender]],
        Row::E3a => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     &[RenderPoll]],
        Row::E3  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     &[RenderResult]],
        Row::E5  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          D,                         D,                     &[Settle]],
        Row::W1  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    &[RetryOwed],               &[RetryOwed],              &[RetryOwed],          &[RetryOwed]],
        Row::V1  => [&[Msg],                        &[Msg] /* today */,    &[Park, Msg],           &[Finish, Park, Msg], &[Finish, Park, Msg], &[Finish, Park, Msg], &[Finish, Park, Msg], &[Msg],                     &[Park, Msg] /* resume dropped */, &[Msg],        &[CancelRender, Msg]],
        Row::V2  => [&[Msg],                        &[Msg],                &[Msg],                 &[Finish, Park, Msg], &[Finish, Park, Msg], &[Finish, Park, Msg], &[Finish, Park, Msg], &[Msg],                     &[Park, Msg],              &[Msg],                &[Msg]],
        Row::V3  => [&[Msg],                        &[ToStopped, Msg] /* B5 */, &[ToStopped, Msg], &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[Finish, ToStopped, Msg], &[ToStopped, Msg], &[ToStopped, Msg], &[ToStopped, Msg], &[CancelRender, ToStopped, Msg]],
        // V4a: "Preserve the phase and remaining obligations while replacement proceeds" is Same.
        Row::V4a => [&[NewPlayer] /* V4b */,        &[Park],               &[Park],                TD,                   TD,                   TD,                   TD,                   &[Same] /* B2, B5 */,       TD,                        &[Same],               &[CancelRender]],
        Row::V4b => [&[NewPlayer],                  &[NewPlayer],          &[NewPlayer],           &[NewPlayer],         &[NewPlayer],         &[NewPlayer],         &[NewPlayer],         &[NewPlayer],               &[NewPlayer],              &[NewPlayer],          &[NewPlayer]],
        Row::V5  => [&[Park] /* today sends Stop */, &[Park, CloseGroup],  &[Park],                &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[Finish, Park],      &[ResendStop],              &[Park, Msg],              &[ResendStop],         &[Msg]],
        Row::V6  => [&[Msg],                        &[Msg],                &[Msg],                 &[Msg],               &[Msg],               &[Msg],               &[Msg],               &[Msg],                     &[Msg],                    &[Msg],                &[Msg]],
        Row::L1  => [&[LoopSeen],                   &[SetLoop],            &[SetLoop],             &[KeepLoop],          &[KeepLoop],          &[KeepLoop],          &[KeepLoop],          &[LoopSeen] /* D5 */,       &[KeepLoop],               D,                     D],
        Row::T1  => [D,                             D,                     D,                      D,                    D,                    D,                    &[Finish, Park, LapNote], D,                      D,                         D,                     D],
        Row::H2  => [D,                             D,                     D,                      D,                    D,                    D,                    D,                    D,                          &[ResumeStart],            D,                     D],
    }
}
