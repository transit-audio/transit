//! Transition rules, the allocation proof and the threaded races for the strip job slots.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::testing::measure;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

/// A stall bound, not a latency claim: far above any scheduling jitter, it only turns a hang
/// (a lost wake-up, a slot stuck claimed) into a failure instead of a stuck test run.
const STALL: Duration = Duration::from_secs(60);

/// A small xorshift generator: deterministic per seed, no dependency.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// The adversarial jitter before or after a compare-and-swap: nothing, a short spin, a
    /// yield, or both.
    pub(super) fn jitter(&mut self) {
        let r = self.next();
        if r & 1 == 1 {
            for _ in 0..(r >> 8) % 512 {
                std::hint::spin_loop();
            }
        }
        if r & 6 == 6 {
            std::thread::yield_now();
        }
    }
}

/// A strip's processor stand-in: panics if two threads are inside it at once.
#[derive(Default)]
pub(super) struct Reentry(AtomicBool);

impl Reentry {
    pub(super) fn enter(&self) {
        assert!(!self.0.swap(true, Ordering::AcqRel), "two threads inside one strip's processor");
    }

    pub(super) fn leave(&self) {
        self.0.store(false, Ordering::Release);
    }
}

fn published(generation: u64) -> JobSlot {
    let slot = JobSlot::new();
    assert_eq!(slot.publish(generation), Ok(()));
    slot
}

#[test]
fn a_job_runs_idle_published_claimed_done_and_is_ready_once() {
    let slot = published(1);
    assert_eq!(slot.state(), State::Published);
    let token = slot.try_claim().expect("published");
    assert_eq!(token.generation(), 1);
    assert_eq!(slot.state(), State::Claimed);
    assert_eq!(slot.consume(1), Consume::Pending, "never read while claimed");
    slot.complete(token);
    assert_eq!(slot.state(), State::Done);
    assert_eq!(slot.consume(1), Consume::Ready);
    assert_eq!(slot.state(), State::Idle, "consuming is the Done -> Idle transition");
    assert_eq!(slot.consume(1), Consume::Pending, "a result is ready once");
    assert_eq!(slot.faults(), 0);
}

#[test]
fn claim_and_reclaim_exclude_each_other_on_one_publish() {
    let slot = published(1);
    assert!(slot.try_claim().is_some_and(|t| {
        t.complete();
        true
    }));
    assert!(!slot.reclaim(), "a claimed job cannot be reclaimed");
    assert_eq!(slot.consume(1), Consume::Ready);

    let slot = published(1);
    assert!(slot.reclaim());
    assert!(slot.try_claim().is_none(), "a reclaimed job cannot be claimed");
    assert!(!slot.reclaim(), "nor reclaimed twice");
    assert_eq!(slot.consume(1), Consume::Pending, "an inline render is the publisher's, not a result");
    assert!(slot.retire());
    assert_eq!(slot.state(), State::Idle);
    assert_eq!(slot.faults(), 0);
}

#[test]
fn a_result_for_an_older_block_is_stale_never_ready() {
    let slot = published(7);
    slot.try_claim().expect("published").complete();
    assert_eq!(slot.consume(8), Consume::Stale, "done for 7, asked at 8");
    assert_eq!(slot.consume(9), Consume::Stale, "stays stale, stays done");
    assert_eq!(slot.state(), State::Done);
    assert!(slot.retire(), "retired unread");
    assert_eq!(slot.consume(7), Consume::Pending, "never ready after it was stale and retired");
    assert_eq!(slot.consume(8), Consume::Pending);
    assert_eq!(slot.faults(), 0, "a stale result is the overrun path, not a fault");

    // A result newer than the block asked for cannot exist under the protocol: counted.
    let slot = published(5);
    slot.try_claim().expect("published").complete();
    assert_eq!(expecting_faults(|| slot.consume(4)), Consume::Stale);
    assert_eq!(slot.faults(), 1);
    assert_eq!(slot.state(), State::Done, "the refusal changes nothing");
}

#[test]
fn retire_from_claimed_is_refused_and_counted_and_republish_waits_for_retire() {
    let slot = published(1);
    let token = slot.try_claim().expect("published");
    expecting_faults(|| {
        assert!(!slot.retire(), "a worker owns the strip");
        assert_eq!(slot.publish(2), Err(PublishError::NotIdle(State::Claimed)));
    });
    assert_eq!(slot.faults(), 2);
    assert_eq!(slot.state(), State::Claimed, "refusals change nothing");
    slot.complete(token);
    expecting_faults(|| assert_eq!(slot.publish(2), Err(PublishError::NotIdle(State::Done)), "not before retire"));
    assert_eq!(slot.faults(), 3);
    assert!(slot.retire());
    assert_eq!(slot.publish(2), Ok(()));
    assert_eq!(slot.try_claim().map(|t| t.generation()), Some(2), "{:?}", slot.state());
}

#[test]
fn retire_from_idle_or_published_is_refused_and_counted() {
    let slot = JobSlot::new();
    expecting_faults(|| assert!(!slot.retire()));
    slot.publish(1).expect("idle");
    expecting_faults(|| assert!(!slot.retire(), "an unclaimed job is reclaimed, not retired"));
    assert_eq!(slot.state(), State::Published);
    assert_eq!(slot.faults(), 2);
}

#[test]
fn generations_only_grow_and_zero_is_never_one() {
    let slot = JobSlot::new();
    expecting_faults(|| assert_eq!(slot.publish(0), Err(PublishError::GenerationNotNewer { last: 0 })));
    slot.publish(3).expect("idle");
    assert!(slot.reclaim() && slot.retire());
    expecting_faults(|| {
        assert_eq!(slot.publish(3), Err(PublishError::GenerationNotNewer { last: 3 }));
        assert_eq!(slot.publish(2), Err(PublishError::GenerationNotNewer { last: 3 }));
    });
    assert_eq!(slot.faults(), 3);
    assert_eq!(slot.state(), State::Idle);
    assert_eq!(slot.publish(4), Ok(()));
}

#[test]
fn a_token_completes_its_own_slot_even_when_passed_to_another() {
    let a = published(1);
    let b = published(1);
    let token = a.try_claim().expect("published");
    expecting_faults(|| b.complete(token));
    assert_eq!(b.faults(), 1, "the misuse is counted on the slot it was passed to");
    assert_eq!(a.state(), State::Done, "the claimed slot is completed, not left claimed");
    assert_eq!(b.state(), State::Published, "the other slot is untouched");
}

#[test]
fn a_set_begins_blocks_in_order_and_counts_its_slots() {
    let set = JobSet::new(4);
    assert_eq!((set.capacity(), set.generation()), (4, 0));
    assert!(set.begin(1));
    expecting_faults(|| assert!(!set.begin(1), "not newer"));
    assert_eq!(set.generation(), 1);
    for i in 0..3 {
        set.publish(i).expect("idle");
    }
    expecting_faults(|| {
        assert_eq!(set.publish(4), Err(PublishError::OutOfRange));
        assert_eq!(set.consume(9), Consume::Pending);
    });
    assert_eq!(set.faults(), 3);
    let (i, token) = set.try_claim_next(1).expect("published");
    assert_eq!(i, 1);
    assert!(set.slot(2).is_some_and(JobSlot::reclaim));
    assert_eq!(set.counts(), Counts { idle: 1, published: 1, claimed: 1, reclaimed: 1, done: 0 });
    let mut seen = [None; 4];
    set.for_each_pending(|i, s| seen[i] = Some(s));
    assert_eq!(seen, [Some(State::Published), Some(State::Claimed), Some(State::Reclaimed), None]);
    token.complete();
    assert_eq!(set.consume(1), Consume::Ready);
    assert_eq!(set.try_claim_next(3).map(|(i, t)| (i, t.generation())), Some((0, 1)), "wraps around");
    assert!(JobSet::new(0).try_claim_next(5).is_none(), "an empty set has nothing to claim");
}

/// Gate 1 and gate 3 groundwork: no operation after construction allocates or frees, on any
/// outcome, counted by the crate's counting allocator.
#[test]
fn no_operation_after_construction_allocates() {
    let set = JobSet::new(8);
    let slot = set.slot(0).expect("capacity 8");
    let ((), stats) = measure(|| {
        for g in 1..=64 {
            assert!(set.begin(g));
            assert!(set.publish(0).is_ok());
            let token = slot.try_claim().expect("published");
            assert!(!slot.reclaim());
            assert_eq!(slot.consume(g), Consume::Pending);
            slot.complete(token);
            assert_eq!(set.consume(0), Consume::Ready);

            assert!(set.publish(1).is_ok());
            assert!(set.slot(1).is_some_and(JobSlot::reclaim));
            assert!(set.slot(1).is_some_and(JobSlot::retire));

            assert!(set.publish(2).is_ok());
            let (i, t) = set.try_claim_next(usize::try_from(g).unwrap_or(0)).expect("slot 2 published");
            assert_eq!(i, 2);
            t.complete();
            assert_eq!(set.slot(2).map(|s| s.consume(g + 1)), Some(Consume::Stale));
            assert!(set.slot(2).is_some_and(JobSlot::retire));

            let mut n = 0;
            set.for_each_pending(|_, _| n += 1);
            std::hint::black_box((n, set.counts(), set.faults()));
        }
        // The refused paths too.
        expecting_faults(|| {
            assert!(!slot.retire());
            assert!(set.publish(99).is_err());
            assert!(!set.begin(1));
        });
    });
    assert!(stats.is_zero(), "{stats:?}");
}

/// What one round of the exclusivity race forces, so both owners are covered whatever the
/// scheduler does; the other rounds are free races.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Force {
    /// The reclaimer waits until a claimer has tried (and therefore won).
    Claim,
    /// The claimers wait until the reclaimer has tried (and therefore won).
    Reclaim,
    Free,
}

fn force(round: u64) -> Force {
    match round % 8 {
        0 => Force::Claim,
        1 => Force::Reclaim,
        _ => Force::Free,
    }
}

struct Race {
    slot: JobSlot,
    /// The round now published; claimers wait for it.
    round: AtomicU64,
    /// Claim attempts made in the current round.
    tried: AtomicUsize,
    /// The reclaimer has tried in the current round.
    reclaim_tried: AtomicBool,
    /// Claimers done with a round (cumulative).
    arrived: AtomicUsize,
    claims: AtomicU64,
    completions: AtomicU64,
    inside: Reentry,
    /// A claimer panicked (its assertion is the failure); the publisher stops waiting for it.
    failed: AtomicBool,
}

/// Sets the flag when the thread holding it unwinds, so a waiting thread fails at once instead
/// of at its stall bound.
pub(super) struct FailFlag<'a>(pub(super) &'a AtomicBool);

impl Drop for FailFlag<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Release);
        }
    }
}

pub(super) fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < STALL, "stalled waiting for {what}");
        std::thread::yield_now();
    }
}

fn claimer(race: &Race, rounds: u64, seed: u64) {
    let _flag = FailFlag(&race.failed);
    let mut rng = Rng::new(seed);
    for r in 1..=rounds {
        wait_until("a publish", || race.round.load(Ordering::Acquire) >= r);
        if force(r) == Force::Reclaim {
            wait_until("the reclaim", || race.reclaim_tried.load(Ordering::Acquire));
        }
        rng.jitter();
        let token = race.slot.try_claim();
        race.tried.fetch_add(1, Ordering::AcqRel);
        rng.jitter();
        if let Some(token) = token {
            assert_eq!(token.generation(), r, "a claim belongs to the round published");
            race.claims.fetch_add(1, Ordering::AcqRel);
            race.inside.enter();
            rng.jitter();
            race.inside.leave();
            race.completions.fetch_add(1, Ordering::AcqRel);
            token.complete();
        }
        // Lets the publisher check the round only after every claimer is done with it.
        race.arrived.fetch_add(1, Ordering::AcqRel);
    }
}

/// Gate 1, the central test: claimers and the reclaimer race on every publish, with spins and
/// yields injected around each compare-and-swap. Exactly one of them wins each publish, the
/// strip is entered by one thread at a time, a job completes at most once, and no transition is
/// refused.
fn exclusivity_race(rounds: u64, claimers: usize) {
    let race = Arc::new(Race {
        slot: JobSlot::new(),
        round: AtomicU64::new(0),
        tried: AtomicUsize::new(0),
        reclaim_tried: AtomicBool::new(false),
        arrived: AtomicUsize::new(0),
        claims: AtomicU64::new(0),
        completions: AtomicU64::new(0),
        inside: Reentry::default(),
        failed: AtomicBool::new(false),
    });
    let go = Arc::new(Barrier::new(claimers + 1));
    let threads: Vec<_> = (0..claimers)
        .map(|k| {
            let (race, go) = (Arc::clone(&race), Arc::clone(&go));
            std::thread::spawn(move || {
                go.wait();
                claimer(&race, rounds, k as u64 + 2);
            })
        })
        .collect();
    go.wait();
    let mut rng = Rng::new(1);
    let (mut claimed, mut reclaimed) = (0_u64, 0_u64);
    for r in 1..=rounds {
        let claims_before = race.claims.load(Ordering::Acquire);
        race.tried.store(0, Ordering::Release);
        race.reclaim_tried.store(false, Ordering::Release);
        race.slot.publish(r).expect("the slot is idle at every round");
        race.round.store(r, Ordering::Release);
        if force(r) == Force::Claim {
            wait_until("a claim attempt", || race.tried.load(Ordering::Acquire) > 0 || race.failed.load(Ordering::Acquire));
        }
        rng.jitter();
        let won = race.slot.reclaim();
        race.reclaim_tried.store(true, Ordering::Release);
        rng.jitter();
        if won {
            race.inside.enter();
            rng.jitter();
            race.inside.leave();
        }
        let all = claimers * usize::try_from(r).expect("rounds fit usize");
        wait_until("the claimers", || race.arrived.load(Ordering::Acquire) >= all || race.failed.load(Ordering::Acquire));
        assert!(!race.failed.load(Ordering::Acquire), "round {r}: a claimer failed (its panic is above)");
        let claims = race.claims.load(Ordering::Acquire) - claims_before;
        assert_eq!(claims + u64::from(won), 1, "round {r}: claims {claims}, reclaimed {won}");
        match force(r) {
            Force::Claim => assert!(!won, "round {r}: forced claim"),
            Force::Reclaim => assert!(won, "round {r}: forced reclaim"),
            Force::Free => {}
        }
        if won {
            assert_eq!(race.slot.consume(r), Consume::Pending, "round {r}");
            assert!(race.slot.retire(), "round {r}");
            reclaimed += 1;
        } else {
            assert_eq!(race.slot.consume(r), Consume::Ready, "round {r}: the winner completed before arriving");
            claimed += 1;
        }
    }
    for t in threads {
        t.join().expect("claimer");
    }
    assert_eq!(race.claims.load(Ordering::Acquire), claimed);
    assert_eq!(race.completions.load(Ordering::Acquire), claimed, "one completion per claim, never two");
    assert_eq!(claimed + reclaimed, rounds);
    assert!(claimed > 0 && reclaimed > 0, "both owners covered: {claimed} claimed, {reclaimed} reclaimed");
    assert_eq!(race.slot.faults(), 0);
    assert_eq!(race.slot.state(), State::Idle);
}

#[test]
fn exactly_one_of_claim_and_reclaim_wins_every_publish_under_injected_delays() {
    exclusivity_race(5_000, 3);
}

const FREE_STRIPS: usize = 8;
const FREE_BLOCKS: u64 = 2_000;
const FREE_WORKERS: usize = 3;
/// The strip a forced-overrun block holds inside its worker.
const HELD: usize = 3;

/// How one publish of the free-running test ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Mixed,
    Inline,
    Overrun,
}

/// What a block of the free-running test forces, so each way a publish can end is taken by
/// construction whatever the scheduler does (#381, E49); the other blocks are free races.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Path {
    /// Every worker is parked before the publish: no claim can happen, every reclaim wins.
    Inline,
    /// The publisher waits until every published strip is done before its pass: every consume
    /// is ready.
    Mixed,
    /// A worker holds strip [`HELD`] inside its render until the publisher has consumed it.
    Overrun,
    Free,
}

fn path(block: u64) -> Path {
    match block % 8 {
        1 => Path::Inline,
        2 => Path::Mixed,
        3 => Path::Overrun,
        _ => Path::Free,
    }
}

/// What the free-running test's publisher and workers share.
struct Shared {
    set: JobSet,
    inside: [Reentry; FREE_STRIPS],
    /// The last generation each strip executed; strictly increasing, so no publish runs twice.
    last_run: [AtomicU64; FREE_STRIPS],
    completions: AtomicU64,
    stop: AtomicBool,
    failed: AtomicBool,
    /// Nonzero: the workers stay out of the set until it changes (an epoch: the forced-inline
    /// block's generation).
    pause: AtomicU64,
    /// Per worker, the last pause epoch it parked under: the publisher publishes a forced-inline
    /// block only once every worker parked under its epoch.
    parked: [AtomicU64; FREE_WORKERS],
    /// Nonzero: the worker that renders strip [`HELD`] for this generation waits inside the render
    /// until it changes.
    hold: AtomicU64,
    /// The generation a worker is holding strip [`HELD`] for, set inside the render.
    holding: AtomicU64,
}

/// A strip's render, by a worker or inline: entered by one thread at a time, each generation once.
fn execute(shared: &Shared, strip: usize, generation: u64, rng: &mut Rng, on_worker: bool) {
    shared.inside[strip].enter();
    let before = shared.last_run[strip].swap(generation, Ordering::AcqRel);
    assert!(before < generation, "strip {strip}: generation {generation} after {before}");
    rng.jitter();
    rng.jitter();
    if on_worker && strip == HELD && shared.hold.load(Ordering::Acquire) == generation {
        shared.holding.store(generation, Ordering::Release);
        wait_until("the held strip's release", || shared.hold.load(Ordering::Acquire) != generation);
    }
    shared.inside[strip].leave();
}

/// A worker of the free-running test: claims whatever is published until told to stop, and stays
/// out of the set while the publisher pauses it.
fn free_worker(shared: &Shared, k: usize) {
    let _flag = FailFlag(&shared.failed);
    let mut rng = Rng::new(100 + k as u64);
    let mut start = k;
    while !shared.stop.load(Ordering::Acquire) {
        let epoch = shared.pause.load(Ordering::Acquire);
        if epoch != 0 {
            shared.parked[k].store(epoch, Ordering::Release);
            while shared.pause.load(Ordering::Acquire) == epoch && !shared.stop.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            continue;
        }
        rng.jitter();
        if let Some((i, token)) = shared.set.try_claim_next(start) {
            execute(shared, i, token.generation(), &mut rng, true);
            shared.completions.fetch_add(1, Ordering::AcqRel);
            token.complete();
            start = i + 1;
        } else {
            std::thread::yield_now();
        }
    }
}

/// The per-block rule end to end on a set, free-running: workers claim whatever is published
/// with random delays inside the strip; the publisher reads each slot first (idle: publish; done:
/// retire unread, then publish; claimed: skip as an overrun), lets a random amount of work pass
/// as its deadline, reclaims the unstarted and consumes the rest. Every publish ends exactly one
/// of {mixed from a worker, rendered inline, overrun}; no publish executes twice. Which of the
/// three a free block's publish takes is the scheduler's; one block in eight forces each of them
/// (`path`), so all three are covered by construction, never by a race's outcome (#381).
#[test]
fn every_publish_ends_mixed_inline_or_overrun_under_free_running_workers() {
    let shared = Arc::new(Shared {
        set: JobSet::new(FREE_STRIPS),
        inside: Default::default(),
        last_run: Default::default(),
        completions: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        pause: AtomicU64::new(0),
        parked: Default::default(),
        hold: AtomicU64::new(0),
        holding: AtomicU64::new(0),
    });
    let workers: Vec<_> = (0..FREE_WORKERS)
        .map(|k| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || free_worker(&shared, k))
        })
        .collect();
    let failed = || shared.failed.load(Ordering::Acquire);

    let mut rng = Rng::new(7);
    let (mut published, mut mixed, mut inline, mut overrun, mut skipped, mut retired_unread) = (0_u64, 0, 0, 0, 0, 0);
    let mut was_published = [false; FREE_STRIPS];
    for b in 1..=FREE_BLOCKS {
        let forced = path(b);
        assert!(shared.set.begin(b));
        match forced {
            Path::Inline => {
                // Every worker parks under this block's epoch before anything is published, so
                // none is inside a claim and none can claim until the pass is over.
                shared.pause.store(b, Ordering::Release);
                wait_until("the workers park", || shared.parked.iter().all(|p| p.load(Ordering::Acquire) == b) || failed());
            }
            Path::Overrun => {
                // The strip held by this block must be published: wait out a claim of an earlier
                // block's overrun (a free block may leave one).
                let slot = shared.set.slot(HELD).expect("in range");
                wait_until("the held strip's earlier claim", || slot.state() != State::Claimed || failed());
                shared.hold.store(b, Ordering::Release);
            }
            Path::Mixed | Path::Free => {}
        }
        for (i, flag) in was_published.iter_mut().enumerate() {
            let slot = shared.set.slot(i).expect("in range");
            *flag = false;
            match slot.state() {
                State::Claimed => {
                    skipped += 1;
                    continue;
                }
                State::Done => {
                    assert_eq!(slot.consume(b), Consume::Stale, "a done slot at block start is a late result");
                    assert!(slot.retire());
                    retired_unread += 1;
                }
                State::Idle => {}
                s => panic!("strip {i} at block {b}: {s:?} at block start"),
            }
            shared.set.publish(i).expect("idle");
            *flag = true;
            published += 1;
        }
        // The callback's own work until its deadline: a random amount, never a clock.
        rng.jitter();
        rng.jitter();
        match forced {
            Path::Mixed => {
                let done = |i: usize| shared.set.slot(i).is_some_and(|s| s.state() == State::Done);
                let all_done = || was_published.iter().enumerate().all(|(i, &p)| !p || done(i));
                wait_until("the workers finish every published strip", || all_done() || failed());
            }
            Path::Overrun => wait_until("a worker holds the held strip", || shared.holding.load(Ordering::Acquire) == b || failed()),
            Path::Inline | Path::Free => {}
        }
        assert!(!failed(), "block {b}: a worker failed (its panic is above)");
        let mut ends = [None; FREE_STRIPS];
        for (i, &flag) in was_published.iter().enumerate() {
            if !flag {
                continue;
            }
            let slot = shared.set.slot(i).expect("in range");
            if slot.reclaim() {
                execute(&shared, i, b, &mut rng, false);
                assert!(slot.retire());
                inline += 1;
                ends[i] = Some(End::Inline);
                continue;
            }
            ends[i] = Some(match shared.set.consume(i) {
                Consume::Ready => {
                    mixed += 1;
                    End::Mixed
                }
                Consume::Pending => {
                    overrun += 1;
                    End::Overrun
                }
                Consume::Stale => panic!("strip {i} published at {b} cannot be stale at {b}"),
            });
        }
        match forced {
            Path::Inline => {
                assert_eq!(ends, [Some(End::Inline); FREE_STRIPS], "block {b}: forced inline, every strip published and reclaimed");
                shared.pause.store(0, Ordering::Release);
            }
            Path::Mixed => assert_eq!(ends, [Some(End::Mixed); FREE_STRIPS], "block {b}: forced mixed, every strip published and done"),
            Path::Overrun => {
                assert_eq!(ends[HELD], Some(End::Overrun), "block {b}: forced overrun of the held strip: {ends:?}");
                shared.hold.store(0, Ordering::Release);
            }
            Path::Free => {}
        }
    }
    // Drain: wait for the overrun workers, then retire their late results unread.
    wait_until("the last claims", || shared.set.counts().claimed == 0 || failed());
    assert!(!failed(), "a worker failed (its panic is above)");
    shared.stop.store(true, Ordering::Release);
    for w in workers {
        w.join().expect("worker");
    }
    for i in 0..FREE_STRIPS {
        let slot = shared.set.slot(i).expect("in range");
        if slot.state() == State::Done {
            assert_eq!(slot.consume(FREE_BLOCKS + 1), Consume::Stale);
            assert!(slot.retire());
            retired_unread += 1;
        }
    }
    assert_eq!(published + skipped, FREE_BLOCKS * FREE_STRIPS as u64);
    assert_eq!(published, mixed + inline + overrun, "every publish ends exactly one way");
    assert_eq!(shared.completions.load(Ordering::Acquire), mixed + overrun, "workers ran exactly the claimed publishes");
    assert_eq!(retired_unread, overrun, "every overrun's late result was retired unread");
    assert_eq!(shared.set.faults(), 0);
    assert_eq!(shared.set.counts().idle, FREE_STRIPS);
}
