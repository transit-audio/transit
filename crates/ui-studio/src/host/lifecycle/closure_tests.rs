//! ADR 0062 slice S2a: a reachability closure over the matrix. Starting from Stopped with nothing
//! tracked (a new Player's state; the faulted variant after a fault is reached from it and checked
//! to be), every admissible event is applied with the free observations each row reads, the host's
//! bookkeeping and a capability model are carried across transitions, and the exploration runs to
//! a fixed point over a finite abstraction ([`Abs`]).
//!
//! What it proves on every reachable state: D-5 (Playing holds no outstanding current-group
//! start); Stopped's request side of section 3's invariant (unless faulted, nothing still capable
//! of moving the transport, no Stop tracked or owed, no render request tracked); and a restricted
//! playback assertion: a transition that enters Stopped from another phase, without a fault or a
//! full retirement, does so with playback not observed. Every refusal keeps the phase with no
//! effect.
//!
//! What it does not establish: observed playback is not carried, so a resident Stopped (staying
//! Stopped while playback is observed) and the readback after V3 or V4b are left to S2d's
//! integration gate over complete admissions and steps; section 3's speed invariant is S2g's.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::collections::{HashSet, VecDeque};
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

/// The record starts a host can prepare (record section 3's contract): a count-in with an
/// explicit start, an explicit start, a punch; and the two local refusals.
fn preps(t: Ticket) -> [RecordPrep; 5] {
    [
        RecordPrep::Ready(RecordStart { ticket: t, from: Some(0), count_in: Some(PLAN) }),
        RecordPrep::Ready(RecordStart { ticket: t, from: Some(0), count_in: None }),
        RecordPrep::Ready(RecordStart { ticket: t, from: None, count_in: None }),
        RecordPrep::NoArmedTrack,
        RecordPrep::Cannot,
    ]
}

const STARTS: [StartCommand; 6] = [
    StartCommand::Play,
    StartCommand::PlayExact(AT),
    StartCommand::PlaySelection,
    StartCommand::TogglePlay,
    StartCommand::Record,
    StartCommand::HalfSpeed,
];

/// The abstraction the closure runs over: the host's tracked, owed, group, held and deferred facts
/// and the capability model, beside the phase. Counts saturate at 2 ("two or more"): no cell
/// distinguishes more than none, one and several (the group's `members > 0`, `stops > 0`, and
/// closed starts as `starts > members`), and a decrement from 2 yields both 1 and 2. Deferred is
/// none, some or full (the only bound a cell reads is the 32nd). The saturated comparison
/// `starts > members` (closed-group starts exist) holds under its reachable-state qualification:
/// capable closed starts coexist with no current group until a Stop neutralizes them, so
/// saturation alone is not relied on to preserve the inequality. Every value a phase carries
/// (tickets, positions, count-ins, render payloads) comes from the finite event set below, so the
/// phases reached are finite too. Observed playback, a pending reopen and `finalizing` are not
/// carried: they are chosen freely on every transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
struct Abs {
    /// Every tracked start, in all groups.
    starts: u8,
    /// The current group's tracked members (outstanding until their receipts, record section 4).
    members: u8,
    applied: bool,
    record: bool,
    record_off: bool,
    stops: u8,
    owed: bool,
    locates: bool,
    release: bool,
    render_play: bool,
    render_stops: bool,
    /// Park's held start (an index into `STARTS`) and whether its reset generation is current.
    held: Option<(u8, bool)>,
    deferred: u8,
    half: bool,
    faulted: bool,
    /// Capability model: the current group's starts can still execute.
    current: bool,
    /// Starts of closed groups can still execute (until every Stop is acknowledged).
    closed: bool,
    /// The pass's record start can still start the transport (its count-in or clock wait).
    continuation: bool,
}

fn inc(n: u8) -> u8 {
    (n + 1).min(2)
}

/// The possible counts after one retirement.
fn dec(n: u8) -> &'static [u8] {
    match n {
        0 => &[],
        1 => &[0],
        _ => &[1, 2],
    }
}

/// The free observations of one transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Env {
    playing: bool,
    reopen: bool,
    finalizing: bool,
}

fn facts(a: &Abs, env: Env) -> Facts {
    Facts {
        tracked: Tracked {
            starts: u32::from(a.starts),
            record: a.record,
            record_off: a.record_off,
            stops: u32::from(a.stops),
            locates: u32::from(a.locates),
            release: a.release,
            set_loop: false,
            render_play: a.render_play,
            render_stops: u32::from(a.render_stops),
        },
        group: StartGroup { members: u32::from(a.members), applied: a.applied },
        owed_stop_since: a.owed.then_some(Duration::ZERO),
        held: a.held.map(|(i, current)| Held { start: STARTS[usize::from(i)], generation: u64::from(current) }),
        deferred: [0, 1, DEFERRED_LIMIT][usize::from(a.deferred)],
        reset_generation: 1,
        speed: if a.half { Speed::Half } else { Speed::Normal },
        playing: env.playing,
        position: 70,
        cursor: 40,
        faulted: a.faulted,
        finalizing: env.finalizing,
        reopen_pending: env.reopen,
    }
}

fn command(start: StartCommand, prep: RecordPrep) -> Event {
    match start {
        StartCommand::Play => Event::Play,
        StartCommand::PlayExact(at) => Event::PlayExact(at),
        StartCommand::PlaySelection => Event::PlaySelection,
        StartCommand::TogglePlay => Event::TogglePlay,
        StartCommand::Record => Event::Record(prep),
        StartCommand::HalfSpeed => Event::HalfSpeed,
    }
}

/// Every admissible event in `a`, each with the facts after the host's own retirement for it (a
/// receipt is an event only while its fact is live, preamble of the receipts; R12 is the
/// untracked refusal).
fn events(a: &Abs, t: Ticket, older: Ticket) -> Vec<(Event, Abs)> {
    let mut out = Vec::new();
    let mut push = |event, next: Abs| out.push((event, next));
    for event in [
        Event::Play,
        Event::PlayExact(AT),
        Event::PlaySelection,
        Event::TogglePlay,
        Event::Stop,
        Event::Pause,
        Event::Locate(AT),
        Event::HalfSpeed,
        Event::Scrub(AT),
        Event::AllNotesOff,
        Event::RulerClick(AT),
        Event::SessionRejected,
        Event::UntrackedRefusal,
        Event::PlayerFaulted,
        Event::StoppedByItself,
        Event::PlaybackObserved,
        Event::RenderStart,
        Event::RenderCancel(CancelReason::Dialog),
        Event::RenderPoll,
        Event::RenderResult(RenderOutcome::Completed),
        Event::RenderSettled,
        Event::OutputTrouble,
        Event::InputTrouble,
        Event::DriverReset,
        Event::ReplaceRequested,
        Event::OldPlayerRetired,
        Event::MidiTransportLost,
        Event::InputLoss,
        Event::LoopChanged,
        Event::LapLimit,
    ] {
        push(event, *a);
    }
    for prep in preps(t) {
        push(Event::Record(prep), *a);
        push(Event::ResumeCheck(prep), *a);
    }
    // ResetSession increments the reset generation in admission step 1 (record section 7).
    push(Event::ResetSession, Abs { held: a.held.map(|(i, _)| (i, false)), ..*a });
    // Play receipts: the current group's, a closed group's (while `starts > members` can hold),
    // the render's.
    if a.members > 0 {
        for &members in dec(a.members) {
            for &starts in dec(a.starts) {
                if starts >= members {
                    let next = Abs { members, starts, ..*a };
                    push(Event::PlayApplied(PlayOf::Group { current: true }), next);
                    push(Event::PlayRefused(PlayOf::Group { current: true }), next);
                }
            }
        }
    }
    if a.starts > a.members || a.starts == 2 {
        for &starts in dec(a.starts) {
            if starts >= a.members {
                let next = Abs { starts, ..*a };
                push(Event::PlayApplied(PlayOf::Group { current: false }), next);
                push(Event::PlayRefused(PlayOf::Group { current: false }), next);
            }
        }
    }
    if a.render_play {
        let next = Abs { render_play: false, ..*a };
        push(Event::PlayApplied(PlayOf::Render), next);
        push(Event::PlayRefused(PlayOf::Render), next);
    }
    // Record receipts (of the current pass's ticket, or of an older Record still tracked).
    if a.record {
        let retired = Abs { record: false, ..*a };
        for ticket in [t, older] {
            push(Event::RecordAdmitted(ticket), retired);
            push(Event::RecordStarted { ticket, latch: point() }, retired);
            let refused = if ticket == t { Abs { continuation: false, ..retired } } else { retired };
            push(Event::RecordRefused { ticket, retired_by_stage: false }, refused);
        }
    }
    // Stages: the current ticket's, and an older ticket's (which change nothing but may retire
    // that Record's fact).
    for ticket in [t, older] {
        push(Event::StageCountingIn(ticket), *a);
        push(Event::StageWaiting(ticket), *a);
        push(Event::StageStarted { ticket, latch: point() }, *a);
        let terminal = if ticket == t { Abs { record: false, continuation: false, ..*a } } else { Abs { record: false, ..*a } };
        push(Event::StageFailed(ticket), terminal);
        push(Event::StageCancelled(ticket), terminal);
    }
    // Stop frontiers: reaching the newest Stop (every Stop and render Stop retired), or an older
    // one while a newer is tracked.
    if a.stops > 0 || a.render_stops {
        push(Event::StopFrontier, Abs { stops: 0, render_stops: false, ..*a });
        if a.stops == 2 {
            push(Event::StopFrontier, Abs { stops: 1, ..*a });
        }
    }
    if a.record_off {
        let next = Abs { record_off: false, ..*a };
        push(Event::RecordOffApplied, next);
        push(Event::RecordOffRefused, next);
    }
    if a.locates {
        let next = Abs { locates: false, ..*a };
        push(Event::LocateOrLoopReceipt { applied: true }, next);
        push(Event::LocateOrLoopReceipt { applied: false }, next);
    }
    if a.release {
        push(Event::ReleaseFrontier, Abs { release: false, ..*a });
    }
    if a.owed {
        push(Event::OwedStop { now: Duration::from_secs(1) }, *a);
        push(Event::OwedStop { now: Duration::from_secs(3) }, *a);
    }
    out
}

/// The facts and capability model after a step's effects. A Stop, a render obligation or a
/// retried owed Stop may be sent (tracked) or refused without a fault (owed, W1), so one effect
/// can lead to two states.
fn carry(effects: &[Effect], start: Abs, rendering: bool) -> Vec<Abs> {
    let mut states = vec![start];
    for effect in effects {
        let mut next = Vec::new();
        for a in states {
            match *effect {
                Effect::RetireAll => next.push(Abs { faulted: a.faulted, ..Abs::default() }),
                Effect::SetFaulted => next.push(Abs { faulted: true, ..a }),
                Effect::ClearFaulted => next.push(Abs { faulted: false, ..a }),
                // The pass ends: its record start is cancelled and a record-off submitted.
                Effect::FinishPass => next.push(Abs { continuation: false, record_off: true, ..a }),
                Effect::CloseGroup => {
                    next.push(Abs { closed: a.closed || (a.current && a.members > 0), current: false, members: 0, applied: false, ..a })
                }
                Effect::NewGroup => next.push(Abs { members: 0, applied: false, ..a }),
                Effect::SubmitStart(_) => next.push(Abs { starts: inc(a.starts), members: inc(a.members), current: true, ..a }),
                Effect::GroupApplied => next.push(Abs { applied: true, ..a }),
                Effect::SubmitStop => {
                    next.push(Abs { stops: inc(a.stops), ..a });
                    next.push(Abs { owed: true, ..a });
                }
                // In Rendering the owed Stop is the render's (E3a); elsewhere an ordinary Stop.
                Effect::RetryOwedStop if rendering => {
                    next.push(Abs { render_stops: true, owed: false, ..a });
                    next.push(a);
                }
                Effect::RetryOwedStop => {
                    next.push(Abs { stops: inc(a.stops), owed: false, ..a });
                    next.push(a);
                }
                Effect::TrackRenderObligations | Effect::CancelRender(_) => {
                    next.push(a);
                    next.push(Abs { render_stops: true, ..a });
                    next.push(Abs { owed: true, ..a });
                }
                Effect::SubmitRecord(_) => next.push(Abs { record: true, continuation: true, ..a }),
                Effect::SubmitLocate(_) | Effect::SubmitSetLoop => next.push(Abs { locates: true, ..a }),
                Effect::SubmitRelease => next.push(Abs { release: true, ..a }),
                Effect::SubmitRenderPlay => next.push(Abs { render_play: true, ..a }),
                Effect::SetSpeed(speed) => next.push(Abs { half: speed == Speed::Half, ..a }),
                Effect::Hold(held) => {
                    let i = STARTS.iter().position(|s| *s == held.start).unwrap() as u8;
                    next.push(Abs { held: Some((i, true)), ..a });
                }
                Effect::DropHeld | Effect::AdmitHeld => next.push(Abs { held: None, ..a }),
                Effect::Defer => {
                    next.push(Abs { deferred: 1.max(a.deferred), ..a });
                    next.push(Abs { deferred: 2, ..a });
                }
                // Released deferred commands are admitted later as ordinary events.
                Effect::DiscardDeferred | Effect::ReleaseDeferred => next.push(Abs { deferred: 0, ..a }),
                _ => next.push(a),
            }
        }
        states = next;
    }
    states
}

/// A reachable abstract state: the phase (interned) and the abstraction.
type Key = (usize, Abs);

struct Closure {
    phases: Vec<TransportPhase>,
    seen: HashSet<Key>,
    queue: VecDeque<Key>,
    /// How each state was first reached: (from, event, facts) for the shortest path report.
    parent: std::collections::HashMap<Key, (Key, String)>,
    transitions: usize,
    violations: Vec<String>,
}

impl Closure {
    fn intern(&mut self, phase: TransportPhase) -> usize {
        if let Some(i) = self.phases.iter().position(|p| *p == phase) {
            return i;
        }
        self.phases.push(phase);
        self.phases.len() - 1
    }

    /// Visit `key`; how it was reached is formatted only on its first visit.
    fn reach(&mut self, key: Key, from: Option<(Key, &dyn Fn() -> String)>) {
        if self.seen.insert(key) {
            if let Some((parent, how)) = from {
                self.parent.insert(key, (parent, how()));
            }
            self.queue.push_back(key);
        }
    }

    /// The shortest path from an entry state to `key`, as (phase, event, facts) steps.
    fn path(&self, mut key: Key) -> String {
        let mut steps = Vec::new();
        while let Some((from, how)) = self.parent.get(&key) {
            steps.push(format!("  {:?} --{how}", self.phases[from.0]));
            key = *from;
        }
        steps.reverse();
        steps.join("\n")
    }
}

/// One transition from (`phase`, `a`) by `event` under `env`, with the host's nested admissions
/// (the held start at Park completion, the Reset after a settled render); the states it leads to.
fn step(phase: &TransportPhase, event: &Event, a: &Abs, env: Env, t: Ticket, nested: bool) -> Result<Vec<(TransportPhase, Abs)>, String> {
    let f = facts(a, env);
    let s = next(phase, event, &f);
    if let Err(refusal) = s.result {
        if s.phase != *phase || !s.effects.is_empty() {
            return Err(format!("a refusal changed the phase or had effects: {phase:?} {event:?} {refusal:?}"));
        }
        return Ok(vec![]);
    }
    let mut start = *a;
    // An acknowledged Stop (every Stop retired, none owed) neutralizes earlier ordinary actions
    // and record-start continuations (record section 4).
    if *event == Event::StopFrontier && a.stops == 0 && !a.owed {
        start = Abs { current: false, closed: false, continuation: false, ..start };
    }
    // A receipt or stage of the current Record: every earlier start of the current group has
    // already been processed (applied or terminally refused) and cannot execute later.
    if let Event::RecordAdmitted(x)
    | Event::RecordStarted { ticket: x, .. }
    | Event::StageCountingIn(x)
    | Event::StageWaiting(x)
    | Event::StageStarted { ticket: x, .. } = *event
        && x == t
    {
        start.current = false;
    }
    let entered_stopped = s.phase == TransportPhase::Stopped && *phase != TransportPhase::Stopped;
    let retired_all = s.effects.contains(&Effect::RetireAll);
    if entered_stopped && !retired_all && !a.faulted && !s.effects.contains(&Effect::SetFaulted) && env.playing {
        return Err(format!("entered Stopped with playback observed: {phase:?} {event:?} {a:?}"));
    }
    let mut out = Vec::new();
    for b in carry(&s.effects, start, matches!(phase, TransportPhase::Rendering { .. })) {
        let mut results = vec![(s.phase, b)];
        let admits = s.effects.contains(&Effect::AdmitHeld) || s.effects.contains(&Effect::ResetContinuation);
        if !nested && admits {
            // The intermediate completion state is checked before the nested admission runs.
            if let Some(what) = check(&s.phase, &b) {
                return Err(format!("{what} at the completion before a nested admission: {phase:?} {event:?} {b:?}"));
            }
        }
        if !nested {
            if s.effects.contains(&Effect::AdmitHeld)
                && let Some(Held { start: held, .. }) = f.held
            {
                // A refused nested admission leaves the state the completion reached.
                let mut admitted = Vec::new();
                for prep in preps(t) {
                    let r = step(&s.phase, &command(held, prep), &b, env, t, true)?;
                    admitted.extend(if r.is_empty() { vec![(s.phase, b)] } else { r });
                }
                results = admitted;
            }
            if s.effects.contains(&Effect::ResetContinuation) {
                let r = step(&s.phase, &Event::ResetSession, &b, env, t, true)?;
                results = if r.is_empty() { vec![(s.phase, b)] } else { r };
            }
        }
        out.extend(results);
    }
    Ok(out)
}

/// Section 3's invariants on one reachable state.
fn check(phase: &TransportPhase, a: &Abs) -> Option<&'static str> {
    if *phase == TransportPhase::Playing && a.members > 0 {
        return Some("D-5: Playing holds an outstanding current-group start");
    }
    if *phase == TransportPhase::Stopped && !a.faulted {
        if a.current && a.members > 0 {
            return Some("Stopped with a current start that can still execute");
        }
        if a.closed && a.starts > a.members {
            return Some("Stopped with a closed group's start that can still execute");
        }
        if a.continuation {
            return Some("Stopped with a record start that can still start the transport");
        }
        if a.stops > 0 || a.owed {
            return Some("Stopped with a Stop tracked or owed");
        }
        if a.render_play || a.render_stops {
            return Some("Stopped with the render's requests tracked");
        }
    }
    None
}

fn explore(t: Ticket, older: Ticket) -> Closure {
    let mut c = Closure {
        phases: Vec::new(),
        seen: HashSet::new(),
        queue: VecDeque::new(),
        parent: std::collections::HashMap::new(),
        transitions: 0,
        violations: Vec::new(),
    };
    let stopped = c.intern(TransportPhase::Stopped);
    // The faulted entry state (Stopped, nothing tracked, faulted) is reached by R13 from the
    // first; the test asserts it is, rather than seeding it twice.
    c.reach((stopped, Abs::default()), None);
    while let Some(key) = c.queue.pop_front() {
        let phase = c.phases[key.0];
        if let Some(what) = check(&phase, &key.1) {
            c.violations.push(format!("{what}: {phase:?} {:?}\n{}", key.1, c.path(key)));
            continue;
        }
        for (event, a) in events(&key.1, t, older) {
            for playing in [false, true] {
                for reopen in [false, true] {
                    for finalizing in [false, true] {
                        // A pending reopen and `finalizing` are read only by starts and E1, and by
                        // the rows that can admit a start or a Reset nested in their transition
                        // (R6 with a held start, E5 with a Reset continuation).
                        if (reopen || finalizing)
                            && !matches!(event.row(), Row::C1 | Row::C2 | Row::C3 | Row::C4 | Row::C7 | Row::C9 | Row::E1 | Row::R6 | Row::E5)
                        {
                            continue;
                        }
                        let env = Env { playing, reopen, finalizing };
                        c.transitions += 1;
                        match step(&phase, &event, &a, env, t, false) {
                            Err(what) => c.violations.push(format!("{what}\n{}", c.path(key))),
                            Ok(states) => {
                                for (next_phase, b) in states {
                                    let i = c.intern(next_phase);
                                    let how = || format!("{event:?}--> with {a:?} playing={playing}");
                                    c.reach((i, b), Some((key, &how)));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    c
}

#[test]
fn stopped_implies_nothing_tracked() {
    let (t, older) = tickets();
    let c = explore(t, older);
    eprintln!("closure: {} states, {} transitions, {} phases", c.seen.len(), c.transitions, c.phases.len());
    assert!(c.violations.is_empty(), "{} reachable violations:\n{}", c.violations.len(), c.violations.join("\n\n"));
    assert!(c.seen.contains(&(0, Abs { faulted: true, ..Abs::default() })), "the faulted entry state is reached");
    // The closure reached its fixed point (the queue drained); an empty exploration cannot pass.
    assert!(
        c.seen.len() > 80_000 && c.transitions > 15_000_000 && c.phases.len() > 30,
        "states {}, transitions {}, phases {}",
        c.seen.len(),
        c.transitions,
        c.phases.len()
    );
}
