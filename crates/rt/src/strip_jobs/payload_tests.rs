//! The memory orderings of the slot, tested through a payload hand-off: each strip has 64
//! relaxed words beside its slot, standing in for its buffers. The publisher writes them before
//! `publish` and the claimer must see those writes after its claim (publish Release, claim
//! Acquire); the worker writes them before `complete` and the publisher must see those writes
//! once `consume` is `Ready` (complete Release, consume Acquire).
//!
//! The words are `Relaxed`, so nothing but the slot orders them, and the threads share no other
//! synchronization on the hand-off path (no round barrier between a write and its check). On x86
//! a weakened ordering still passes, because the hardware orders stores; the test is there so
//! an ARM runner or loom catches one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{FailFlag, Reentry, Rng, wait_until};
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

const WORDS: usize = 64;
/// What a worker adds to the generation when it writes the payload back.
const WORKER_MARK: u64 = 1_000_000;

/// A strip's buffers, stand-in: written and read only with `Relaxed`.
struct Payload([AtomicU64; WORDS]);

impl Payload {
    fn new() -> Self {
        Self(std::array::from_fn(|_| AtomicU64::new(0)))
    }

    fn write(&self, value: u64) {
        for w in &self.0 {
            w.store(value, Ordering::Relaxed);
        }
    }

    fn assert_all(&self, value: u64, what: &str) {
        for (i, w) in self.0.iter().enumerate() {
            let got = w.load(Ordering::Relaxed);
            assert_eq!(got, value, "{what}: word {i}");
        }
    }
}

/// The worker's half of a hand-off: check the publisher's payload, render, write the result,
/// complete.
fn render_claimed(token: ClaimToken<'_>, payload: &Payload, inside: &Reentry, rng: &mut Rng) {
    let g = token.generation();
    inside.enter();
    payload.assert_all(g, "a claimer sees the publisher's payload");
    rng.jitter();
    payload.write(g + WORKER_MARK);
    inside.leave();
    token.complete();
}

struct HandOff {
    slot: JobSlot,
    payload: Payload,
    inside: Reentry,
    /// Claimers hold off while set (rounds that aim for a reclaim); a hint, not a guarantee.
    pause: AtomicBool,
    claims: AtomicUsize,
    stop: AtomicBool,
    failed: AtomicBool,
}

fn hand_off_claimer(h: &HandOff, seed: u64) {
    let _flag = FailFlag(&h.failed);
    let mut rng = Rng::new(seed);
    while !h.stop.load(Ordering::Acquire) {
        if h.pause.load(Ordering::Acquire) {
            std::thread::yield_now();
            continue;
        }
        rng.jitter();
        if let Some(token) = h.slot.try_claim() {
            rng.jitter();
            h.claims.fetch_add(1, Ordering::AcqRel);
            render_claimed(token, &h.payload, &h.inside, &mut rng);
        }
    }
}

/// Gate 1 evidence for the orderings: three free-spinning claimers and the publisher's reclaim
/// race on every publish, with spins and yields around each compare-and-swap; the payload crosses
/// threads only through the slot.
#[test]
fn the_payload_crosses_threads_only_through_the_slot_orderings() {
    const ROUNDS: u64 = 5_000;
    const CLAIMERS: u64 = 3;
    let h = Arc::new(HandOff {
        slot: JobSlot::new(),
        payload: Payload::new(),
        inside: Reentry::default(),
        pause: AtomicBool::new(false),
        claims: AtomicUsize::new(0),
        stop: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    });
    let threads: Vec<_> = (0..CLAIMERS)
        .map(|k| {
            let h = Arc::clone(&h);
            std::thread::spawn(move || hand_off_claimer(&h, 20 + k))
        })
        .collect();
    let mut rng = Rng::new(3);
    let (mut claimed, mut reclaimed) = (0_u64, 0_u64);
    for r in 1..=ROUNDS {
        let aim_reclaim = r % 4 == 1;
        h.pause.store(aim_reclaim, Ordering::Release);
        h.payload.write(r);
        h.slot.publish(r).expect("idle at every round");
        if r % 4 == 0 {
            // Aim for a claim: let a claimer take it first.
            wait_until("a claim", || h.slot.state() != State::Published || h.failed.load(Ordering::Acquire));
        }
        rng.jitter();
        if h.slot.reclaim() {
            h.inside.enter();
            h.payload.assert_all(r, "the publisher's own payload");
            h.inside.leave();
            assert!(h.slot.retire());
            reclaimed += 1;
        } else {
            // The only path to the worker's result: consume, until Ready.
            wait_until("the claimer's result", || h.slot.consume(r) == Consume::Ready || h.failed.load(Ordering::Acquire));
            assert!(!h.failed.load(Ordering::Acquire), "round {r}: a claimer failed (its panic is above)");
            h.payload.assert_all(r + WORKER_MARK, "the publisher sees the worker's payload after Ready");
            claimed += 1;
        }
    }
    h.stop.store(true, Ordering::Release);
    for t in threads {
        t.join().expect("claimer");
    }
    assert_eq!(h.claims.load(Ordering::Acquire) as u64, claimed, "one claim per claimed publish");
    assert_eq!(claimed + reclaimed, ROUNDS);
    assert!(claimed > 0 && reclaimed > 0, "both owners covered: {claimed} claimed, {reclaimed} reclaimed");
    assert_eq!(h.slot.faults(), 0);
}

const STRIPS: usize = 8;
const BLOCKS: u64 = 2_000;
const WORKERS: usize = 3;

struct Free {
    set: JobSet,
    payload: [Payload; STRIPS],
    inside: [Reentry; STRIPS],
    stop: AtomicBool,
    failed: AtomicBool,
}

fn free_worker(f: &Free, k: usize) {
    let _flag = FailFlag(&f.failed);
    let mut rng = Rng::new(200 + k as u64);
    let mut start = k;
    while !f.stop.load(Ordering::Acquire) {
        rng.jitter();
        if let Some((i, token)) = f.set.try_claim_next(start) {
            render_claimed(token, &f.payload[i], &f.inside[i], &mut rng);
            start = i + 1;
        } else {
            std::thread::yield_now();
        }
    }
}

/// The per-block rule with payloads: a published strip's payload reaches the worker, a mixed
/// strip's result reaches the publisher, and a late worker's result is retired before the
/// publisher writes the strip's next payload.
#[test]
fn the_per_block_rule_hands_payloads_over_through_the_slots() {
    let f = Arc::new(Free {
        set: JobSet::new(STRIPS),
        payload: std::array::from_fn(|_| Payload::new()),
        inside: Default::default(),
        stop: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    });
    let workers: Vec<_> = (0..WORKERS)
        .map(|k| {
            let f = Arc::clone(&f);
            std::thread::spawn(move || free_worker(&f, k))
        })
        .collect();
    let mut rng = Rng::new(11);
    let (mut mixed, mut inline) = (0_u64, 0_u64);
    let mut was_published = [false; STRIPS];
    for b in 1..=BLOCKS {
        assert!(f.set.begin(b));
        for (i, flag) in was_published.iter_mut().enumerate() {
            let slot = f.set.slot(i).expect("in range");
            *flag = false;
            match slot.state() {
                State::Claimed => continue,
                State::Done => {
                    let late = slot.generation();
                    assert_eq!(slot.consume(b), Consume::Stale);
                    assert!(slot.retire());
                    f.payload[i].assert_all(late + WORKER_MARK, "a retired late result is complete");
                }
                State::Idle => {}
                s => panic!("strip {i} at block {b}: {s:?} at block start"),
            }
            f.payload[i].write(b);
            f.set.publish(i).expect("idle");
            *flag = true;
        }
        rng.jitter();
        rng.jitter();
        for (i, &flag) in was_published.iter().enumerate() {
            if !flag {
                continue;
            }
            let slot = f.set.slot(i).expect("in range");
            if slot.reclaim() {
                f.inside[i].enter();
                f.payload[i].assert_all(b, "the publisher's own payload");
                f.inside[i].leave();
                assert!(slot.retire());
                inline += 1;
            } else if f.set.consume(i) == Consume::Ready {
                f.payload[i].assert_all(b + WORKER_MARK, "a mixed strip shows the worker's payload");
                mixed += 1;
            }
        }
    }
    wait_until("the last claims", || f.set.counts().claimed == 0 || f.failed.load(Ordering::Acquire));
    assert!(!f.failed.load(Ordering::Acquire), "a worker failed (its panic is above)");
    f.stop.store(true, Ordering::Release);
    for w in workers {
        w.join().expect("worker");
    }
    assert!(mixed > 0 && inline > 0, "both paths covered: {mixed} mixed, {inline} inline");
    assert_eq!(f.set.faults(), 0);
}
