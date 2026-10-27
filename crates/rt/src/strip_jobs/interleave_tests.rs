//! Deterministic, exhaustive interleavings of the slot state machine, with a hand-written
//! scheduler (no loom: a new dev-dependency in this crate is the maintainer's decision).
//!
//! Every operation is one atomic read-modify-write on the state (publish adds generation
//! accesses that only the single publisher makes while the slot is `Idle`, where no other
//! operation can change it), so interleaving whole operations covers every order the slot can
//! see. Two explorations:
//!
//! 1. Every sequence of operations by two actors up to a fixed length, against an independently
//!    written transition table, with the gate 1 and gate 2 invariants on every prefix.
//! 2. Every interleaving of a publisher following the per-block rule and a worker, over three
//!    blocks: each publish ends exactly one of {mixed, inline, overrun}, none runs twice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unreachable)]

use super::*;

/// One step in exploration 1. `Claim` and `Complete` belong to an actor (who holds the token);
/// the others are actor-agnostic (whichever actor runs them, they are the publisher's).
#[derive(Clone, Copy, Debug)]
enum Op {
    Begin,
    Publish,
    Claim(usize),
    Reclaim,
    Complete(usize),
    Consume,
    Retire,
}

const OPS: [Op; 9] = [Op::Begin, Op::Publish, Op::Claim(0), Op::Claim(1), Op::Reclaim, Op::Complete(0), Op::Complete(1), Op::Consume, Op::Retire];

/// Sequences up to this many operations (prefixes that complete without a token are skipped;
/// the test prints how many were visited).
const DEPTH: usize = 8;

/// The expected slot, written from the transition table in ADR 0029 and not from the code,
/// plus the history the invariants need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Model {
    state: State,
    generation: u64,
    /// The publisher's current block.
    block: u64,
    faults: u32,
    /// The generation each actor's token was claimed for.
    tokens: [Option<u64>; 2],
    /// Claims plus reclaims since the last publish (gate 1: at most one).
    owners: u8,
    /// Whether the last publish was reclaimed (its render is the publisher's, never a result).
    reclaimed: bool,
    /// Ready results since the last publish (gate 2: at most one).
    readies: u8,
}

/// What an operation returned, for the comparison with the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    None,
    Published(bool),
    Claimed(Option<u64>),
    Bool(bool),
    Consumed(Consume),
}

impl Model {
    const fn new() -> Self {
        Self { state: State::Idle, generation: 0, block: 0, faults: 0, tokens: [None; 2], owners: 0, reclaimed: false, readies: 0 }
    }

    /// The expected outcome of `op`, or `None` when the operation cannot be expressed (completing
    /// without a token: the type system forbids it).
    fn step(&mut self, op: Op) -> Option<Outcome> {
        Some(match op {
            Op::Begin => {
                self.block += 1;
                Outcome::None
            }
            Op::Publish => {
                let ok = self.state == State::Idle && self.block > self.generation;
                if ok {
                    *self = Self { state: State::Published, generation: self.block, owners: 0, reclaimed: false, readies: 0, ..*self };
                } else {
                    self.faults += 1;
                }
                Outcome::Published(ok)
            }
            Op::Claim(a) => {
                if self.state != State::Published {
                    return Some(Outcome::Claimed(None));
                }
                self.state = State::Claimed;
                self.tokens[a] = Some(self.generation);
                self.owners += 1;
                Outcome::Claimed(Some(self.generation))
            }
            Op::Reclaim => {
                let ok = self.state == State::Published;
                if ok {
                    self.state = State::Reclaimed;
                    self.owners += 1;
                    self.reclaimed = true;
                }
                Outcome::Bool(ok)
            }
            Op::Complete(a) => {
                self.tokens[a].take()?;
                self.state = State::Done;
                Outcome::None
            }
            Op::Consume => Outcome::Consumed(match self.state {
                State::Done if self.generation == self.block => {
                    self.state = State::Idle;
                    self.readies += 1;
                    Consume::Ready
                }
                State::Done => Consume::Stale,
                _ => Consume::Pending,
            }),
            Op::Retire => {
                let ok = matches!(self.state, State::Done | State::Reclaimed);
                if ok {
                    self.state = State::Idle;
                } else {
                    self.faults += 1;
                }
                Outcome::Bool(ok)
            }
        })
    }

    /// The invariants every reachable state keeps, stated without the transition table.
    fn check(&self, path: &[Op]) {
        assert!(self.owners <= 1, "gate 1: two owners of one publish: {path:?}");
        assert!(self.readies <= 1, "gate 2: a result ready twice: {path:?}");
        assert!(!(self.reclaimed && self.readies > 0), "gate 2: a reclaimed job reported ready: {path:?}");
        let held = self.tokens.iter().flatten().count();
        assert!(held <= 1, "two tokens held at once: {path:?}");
        assert_eq!(held == 1, self.state == State::Claimed, "a token exists exactly while claimed: {path:?}");
        if let Some(g) = self.tokens.iter().flatten().next() {
            assert_eq!(*g, self.generation, "a token is for the generation published: {path:?}");
        }
    }
}

/// Puts the real slot in the model's slot state (this module sees the private fields).
fn load(slot: &JobSlot, m: &Model) {
    slot.state.store(m.state as u8, Ordering::Relaxed);
    slot.generation.store(m.generation, Ordering::Relaxed);
    slot.faults.store(m.faults, Ordering::Relaxed);
}

/// Applies `op` to the real slot. A token is represented between steps by its generation and
/// rebuilt for `complete` exactly as `try_claim` builds it.
fn apply(slot: &JobSlot, m: &Model, op: Op) -> Outcome {
    match op {
        Op::Begin => Outcome::None,
        Op::Publish => Outcome::Published(slot.publish(m.block).is_ok()),
        Op::Claim(_) => Outcome::Claimed(slot.try_claim().map(|t| t.generation())),
        Op::Reclaim => Outcome::Bool(slot.reclaim()),
        Op::Complete(a) => {
            let generation = m.tokens[a].expect("the model skips a completion without a token");
            slot.complete(ClaimToken { slot, generation });
            Outcome::None
        }
        Op::Consume => Outcome::Consumed(slot.consume(m.block)),
        Op::Retire => Outcome::Bool(slot.retire()),
    }
}

fn explore(slot: &JobSlot, m: Model, path: &mut Vec<Op>, visited: &mut u64) {
    *visited += 1;
    if path.len() == DEPTH {
        return;
    }
    for op in OPS {
        let mut next = m;
        let Some(expected) = next.step(op) else { continue };
        load(slot, &m);
        let got = apply(slot, &m, op);
        path.push(op);
        assert_eq!(got, expected, "outcome after {path:?}");
        assert_eq!((slot.state(), slot.generation(), slot.faults()), (next.state, next.generation, next.faults), "slot after {path:?}");
        next.check(path);
        explore(slot, next, path, visited);
        path.pop();
    }
}

#[test]
fn every_operation_sequence_of_two_actors_keeps_the_transition_table_and_the_gates() {
    let slot = JobSlot::new();
    let mut visited = 0;
    expecting_faults(|| explore(&slot, Model::new(), &mut Vec::with_capacity(DEPTH), &mut visited));
    // The exploration is deterministic: every prefix of every length up to DEPTH was visited
    // except those that complete without a token.
    println!("operation sequences: {visited} prefixes");
    assert!(visited > 1, "{visited}");
}

// ---- Exploration 2: the per-block rule, publisher against worker. ----

const BLOCKS: u64 = 3;
/// Claim attempts the worker may make (successful or not).
const WORKER_TRIES: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Begin,
    Read,
    Deadline,
    InlineEnter,
    InlineExit,
    Drain,
    Finished,
}

#[derive(Default, Debug)]
struct Tally {
    published: u64,
    mixed: u64,
    inline: u64,
    overrun: u64,
    skipped: u64,
    retired_unread: u64,
    completions: u64,
    /// Generations executed (worker or inline), to prove none runs twice.
    executed: Vec<u64>,
    inside: bool,
}

impl Tally {
    fn enter(&mut self, generation: u64, schedule: &[u8]) {
        assert!(!self.inside, "two executors inside the strip: {schedule:?}");
        assert!(!self.executed.contains(&generation), "generation {generation} executed twice: {schedule:?}");
        self.inside = true;
        self.executed.push(generation);
    }
}

struct Run<'a> {
    set: &'a JobSet,
    block: u64,
    phase: Phase,
    published_now: bool,
    tries: u32,
    token: Option<ClaimToken<'a>>,
    t: Tally,
}

impl<'a> Run<'a> {
    fn new(set: &'a JobSet) -> Self {
        Self { set, block: 1, phase: Phase::Begin, published_now: false, tries: WORKER_TRIES, token: None, t: Tally::default() }
    }

    fn slot(&self) -> &'a JobSlot {
        self.set.slot(0).expect("one strip")
    }

    fn publisher_runnable(&self) -> bool {
        match self.phase {
            Phase::Finished => false,
            // The bounded wait at the end of the run (engine retirement, gate 5's normal case).
            Phase::Drain => self.slot().state() != State::Claimed,
            _ => true,
        }
    }

    fn worker_runnable(&self) -> bool {
        self.token.is_some() || self.tries > 0
    }

    fn next_block(&mut self) {
        self.block += 1;
        self.phase = if self.block > BLOCKS { Phase::Drain } else { Phase::Begin };
    }

    fn step_publisher(&mut self, s: &[u8]) {
        let slot = self.slot();
        match self.phase {
            Phase::Begin => {
                assert!(self.set.begin(self.block));
                self.phase = Phase::Read;
            }
            Phase::Read => match slot.state() {
                State::Idle => {
                    assert_eq!(self.set.publish(0), Ok(()), "{s:?}");
                    self.t.published += 1;
                    self.published_now = true;
                    self.phase = Phase::Deadline;
                }
                State::Done => {
                    assert_eq!(self.set.consume(0), Consume::Stale, "a done slot at block start is late: {s:?}");
                    assert!(slot.retire());
                    self.t.retired_unread += 1;
                }
                State::Claimed => {
                    self.t.skipped += 1;
                    self.published_now = false;
                    self.phase = Phase::Deadline;
                }
                other => panic!("{other:?} at block start: {s:?}"),
            },
            Phase::Deadline => {
                if !self.published_now {
                    self.next_block();
                } else if slot.reclaim() {
                    self.phase = Phase::InlineEnter;
                } else {
                    match self.set.consume(0) {
                        Consume::Ready => self.t.mixed += 1,
                        Consume::Pending => self.t.overrun += 1,
                        Consume::Stale => panic!("published this block, stale this block: {s:?}"),
                    }
                    self.next_block();
                }
            }
            Phase::InlineEnter => {
                self.t.enter(self.block, s);
                self.phase = Phase::InlineExit;
            }
            Phase::InlineExit => {
                self.t.inside = false;
                assert!(slot.retire());
                self.t.inline += 1;
                self.next_block();
            }
            Phase::Drain => {
                if slot.state() == State::Done {
                    assert!(slot.retire());
                    self.t.retired_unread += 1;
                }
                self.phase = Phase::Finished;
            }
            Phase::Finished => unreachable!("not runnable"),
        }
    }

    fn step_worker(&mut self, s: &[u8]) {
        if let Some(token) = self.token.take() {
            self.t.inside = false;
            self.t.completions += 1;
            token.complete();
            return;
        }
        self.tries -= 1;
        if let Some(token) = self.slot().try_claim() {
            self.t.enter(token.generation(), s);
            self.token = Some(token);
        }
    }

    fn finish(&self, s: &[u8]) {
        let t = &self.t;
        assert_eq!(t.published + t.skipped, BLOCKS, "{s:?}");
        assert_eq!(t.published, t.mixed + t.inline + t.overrun, "every publish ends exactly one way: {s:?} {t:?}");
        assert_eq!(t.completions, t.mixed + t.overrun, "a worker ran exactly the claimed publishes: {s:?} {t:?}");
        assert_eq!(t.retired_unread, t.overrun, "every overrun retired unread: {s:?} {t:?}");
        assert_eq!(t.executed.len() as u64, t.inline + t.completions, "{s:?}");
        assert_eq!(self.set.faults(), 0, "{s:?}");
        assert_eq!(self.slot().state(), State::Idle, "{s:?}");
    }
}

/// Totals over every complete run, to show each ending was reached.
#[derive(Default, Debug)]
struct Explored {
    runs: u64,
    mixed: u64,
    inline: u64,
    overrun: u64,
}

/// Replays `schedule` (0 = publisher, 1 = worker) from a fresh set and returns which actors can
/// run next (finishing the run's checks, and adding it to `explored`, when neither can).
fn replay(schedule: &[u8], explored: &mut Explored) -> [bool; 2] {
    let set = JobSet::new(1);
    let mut run = Run::new(&set);
    for (i, &who) in schedule.iter().enumerate() {
        if who == 0 {
            run.step_publisher(&schedule[..=i]);
        } else {
            run.step_worker(&schedule[..=i]);
        }
    }
    let can = [run.publisher_runnable(), run.worker_runnable()];
    if can == [false, false] {
        run.finish(schedule);
        explored.runs += 1;
        explored.mixed += run.t.mixed;
        explored.inline += run.t.inline;
        explored.overrun += run.t.overrun;
    }
    can
}

fn schedules(schedule: &mut Vec<u8>, explored: &mut Explored) {
    let can = replay(schedule, explored);
    for (who, runnable) in (0_u8..).zip(can) {
        if runnable {
            schedule.push(who);
            schedules(schedule, explored);
            schedule.pop();
        }
    }
}

#[test]
fn every_interleaving_of_the_per_block_rule_ends_each_publish_exactly_one_way() {
    let mut explored = Explored::default();
    schedules(&mut Vec::new(), &mut explored);
    // Every ending is reached somewhere in the exploration (a deterministic enumeration).
    assert!(explored.mixed > 0 && explored.inline > 0 && explored.overrun > 0, "{explored:?}");
    println!("per-block rule: {explored:?}");
}
