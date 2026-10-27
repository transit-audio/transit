//! Free-running blocks through real worker threads: random spins, strips slower than a block and
//! stalls released a random number of blocks later, with the reentry guard and generation check
//! of the fake runner (gate 1), every published strip resolved exactly once per block (gates 2
//! and 3), and the payload hand-off checked in both directions through the workers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::tests::{Behavior, Fake, STALL, wait_until};
use super::*;

/// A small xorshift generator: deterministic per seed, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// What the publisher observed, to cross-check the set's counters.
#[derive(Default, Debug)]
struct Tally {
    published: u64,
    mixed: u64,
    inline: u64,
    late: u64,
    busy: u64,
    unstarted: u64,
}

/// The block budget: short against the slow strips (`SLOW`), so overruns happen.
const BLOCK: Duration = Duration::from_millis(1);
const SLOW: Duration = Duration::from_millis(3);

/// The outcome of every strip in block `g` against what the fake executed.
fn check_outcomes(set: &WorkerSet, fake: &Fake, g: u64, payload: bool, tally: &mut Tally, late_in: &mut [u64]) {
    for (s, late) in late_in.iter_mut().enumerate() {
        let last = fake.strips[s].last_gen.load(Ordering::Acquire);
        match set.outcome(s).unwrap() {
            Outcome::Mixed => {
                tally.mixed += 1;
                assert_eq!(last, g, "strip {s}: mixed a result a worker did not run for block {g}");
                if payload {
                    fake.assert_output(s, g);
                }
            }
            Outcome::Inline => {
                tally.inline += 1;
                assert_eq!(last, g, "strip {s}: inline render for block {g}");
                if payload {
                    fake.assert_output(s, g);
                }
            }
            Outcome::Overrun(OverrunCause::Unstarted) => {
                tally.unstarted += 1;
                assert!(last < g, "strip {s}: taken back unrendered, yet executed for block {g}");
            }
            Outcome::Overrun(OverrunCause::Late) => {
                tally.late += 1;
                *late = g;
            }
            Outcome::Overrun(OverrunCause::Busy) => assert!(last < g, "strip {s}: skipped yet executed"),
            o => panic!("block {g} strip {s}: unresolved outcome {o:?}"),
        }
    }
}

fn run_blocks(seed: u64, blocks: u64, workers: usize, payload: bool) {
    const STRIPS: usize = 8;
    let fake = Fake::new(STRIPS, SLOW);
    fake.check_payload(payload);
    let mut set = super::tests::set_of(&fake, workers);
    let mut rng = Rng::new(seed);
    let mut tally = Tally::default();
    // Per strip: the block at which a stall is released (0: not stalled).
    let mut release_at = [0u64; STRIPS];
    // Per strip: the last block it was late in (its worker must have run it once it is free).
    let mut late_in = [0u64; STRIPS];

    for g in 1..=blocks {
        for (s, at) in release_at.iter_mut().enumerate() {
            if *at != 0 && g >= *at {
                fake.release(s);
                *at = 0;
            }
        }
        assert!(set.begin_block(g));
        for s in 0..STRIPS {
            if set.is_busy(s) {
                assert_eq!(set.publish(s), Ok(Published::Skipped));
                tally.busy += 1;
                continue;
            }
            // Not busy: no worker holds the strip, so its late block (if any) has been run.
            if late_in[s] != 0 {
                assert!(fake.strips[s].last_gen.load(Ordering::Acquire) >= late_in[s], "strip {s}: late block {} never ran", late_in[s]);
                late_in[s] = 0;
            }
            if release_at[s] == 0 {
                let roll = rng.below(100);
                let behavior = match roll {
                    0 if !payload => Behavior::Stall,
                    1..=4 => Behavior::Slow,
                    _ => Behavior::Normal,
                };
                if behavior == Behavior::Stall {
                    release_at[s] = g + 1 + rng.below(20);
                }
                fake.set(s, behavior);
                fake.strips[s].spins.store(u32::try_from(rng.below(4000)).unwrap_or(0), Ordering::Relaxed);
            }
            if payload {
                fake.write_inputs(s, g);
            }
            assert_eq!(set.publish(s), Ok(Published::Published));
            tally.published += 1;
        }
        if g.is_multiple_of(16) {
            // Worker coverage that does not depend on the scheduler: wake every worker and let
            // them take every published strip (unless all are stalled) and finish all but the
            // stalled ones.
            set.wake(workers);
            wait_until("the workers", || {
                let c = set.jobs().counts();
                let stalled = fake.stalled.load(Ordering::Acquire);
                (c.published == 0 || stalled >= workers) && c.claimed == stalled
            });
        } else {
            set.wake(usize::try_from(rng.below(workers as u64 + 1)).unwrap_or(0));
        }
        let r = set.finish(Instant::now() + BLOCK, |s| fake.execute(s, g, false));
        assert_eq!(r.faulted, 0, "block {g}: a runner assertion failed on a worker");
        check_outcomes(&set, &fake, g, payload, &mut tally, &mut late_in);
        assert_eq!(r.mixed + r.inline + r.overrun, STRIPS, "block {g}: every strip resolved exactly once: {r:?}");
    }

    for s in 0..STRIPS {
        fake.release(s);
    }
    wait_until("the late workers", || set.jobs().counts().claimed == 0);
    let st = set.stats();
    assert_eq!(st.mixed, tally.mixed, "{st:?} {tally:?}");
    assert_eq!(st.inline_renders, tally.inline);
    assert_eq!((st.overruns_late, st.overruns_busy, st.overruns_unstarted), (tally.late, tally.busy, tally.unstarted));
    assert_eq!(tally.mixed + tally.inline + tally.late + tally.unstarted, tally.published);
    assert_eq!(st.runner_panics, 0);
    assert!(tally.mixed > 0 && tally.inline > 0, "both executors took part: {tally:?}");
    let q = set.shutdown(STALL);
    assert!(q.is_empty(), "{q:?}");
    assert_eq!(set.jobs().faults(), 0, "no refused transition");
}

/// Gates 1 to 3 through real threads: random work, slow strips and stalls across many blocks.
#[test]
fn free_running_blocks_resolve_every_strip_once_with_random_stalls() {
    for (seed, workers) in [(1, 3), (2, 1), (3, 5)] {
        run_blocks(seed, 1500, workers, false);
    }
}

/// The payload hand-off through the workers: the callback writes a strip's inputs before
/// publish, the executor checks them and writes its output before completing, the callback
/// checks the output after `Mixed` or `Inline`. The words are relaxed, so only the slot orders
/// them (on x86 a weakened ordering still passes; the test is there for an ARM runner).
#[test]
fn payload_hand_off_through_the_workers_in_both_directions() {
    for (seed, workers) in [(11, 3), (12, 2)] {
        run_blocks(seed, 1500, workers, true);
    }
}
