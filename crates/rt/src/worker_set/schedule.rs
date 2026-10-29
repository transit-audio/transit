//! Seeded stall schedules and their shrinker (feature `testing`, issue #210, ADR 0029 amendment
//! section 8): the input of the scheduling-condition gate.
//!
//! A [`Schedule`] says, for each block, which strips are held and for how many blocks: a
//! [`Hold`] of strip `s` from block `b` for `n` blocks. A test realizes it through deterministic
//! seams only (a strip held inside its render on a worker and released by the test, a
//! deadline reached when only the held strips are left, a cost history that refuses an inline
//! render); nothing in it is a clock or a thread-timing outcome (E49). [`Schedule::seeded`]
//! makes one from a seed, the same on every run and platform.
//!
//! **Shrinking.** When a schedule fails, [`Schedule::shrink`] minimizes it by delta debugging
//! against the test's own predicate: it drops held blocks (whole holds, in halving chunks, then
//! one at a time), then shortens each remaining hold, then removes whole strips, and repeats
//! until nothing more can go. Each step keeps only a change that still fails, so the result is
//! a local minimum: removing any one hold, shortening any one hold by a block, or removing any
//! one strip makes it pass. [`Schedule::to_rust`] prints it as a Rust literal for a named
//! regression test.

use std::fmt::Write as _;

/// Strip `strip` (a table index) held from block `block` for `blocks` blocks (at least one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hold {
    /// The first block held.
    pub block: usize,
    /// The strip.
    pub strip: usize,
    /// How many blocks, at least one.
    pub blocks: usize,
}

impl Hold {
    /// The blocks it covers.
    #[must_use]
    pub fn covers(&self, block: usize) -> bool {
        (self.block..self.block + self.blocks).contains(&block)
    }

    /// The block after its last.
    #[must_use]
    pub fn end(&self) -> usize {
        self.block + self.blocks
    }
}

/// What [`Schedule::seeded`] may generate.
#[derive(Clone, Debug)]
pub struct Shape {
    /// The strips a hold may fall on (repeat one to weight it).
    pub strips: Vec<usize>,
    /// Holds start in `first..last` and end by `last` (the blocks after `last` are the window in
    /// which every state the chain holds must show whether it diverged).
    pub first: usize,
    /// See `first`.
    pub last: usize,
    /// The longest hold, in blocks.
    pub longest: usize,
    /// The most strips held in any one block (a worker must stay free for the rest).
    pub concurrent: usize,
    /// One in `rarity` blocks starts a hold (when allowed).
    pub rarity: u64,
}

/// A stall schedule (see the module docs): holds sorted by block, then strip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    /// The holds.
    pub holds: Vec<Hold>,
}

/// The `SplitMix64` generator: deterministic, the same on every platform.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    /// A generator for `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    /// The next value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..n` (0 for `n` 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

impl Schedule {
    /// A schedule of `(strip, block, blocks)` holds.
    #[must_use]
    pub fn new(holds: &[(usize, usize, usize)]) -> Schedule {
        let mut s = Schedule { holds: holds.iter().map(|&(strip, block, blocks)| Hold { block, strip, blocks: blocks.max(1) }).collect() };
        s.holds.sort_unstable();
        s
    }

    /// The schedule of `seed` within `shape`: walking the blocks in order, one in
    /// `shape.rarity` starts a hold on a strip of `shape.strips` for 1 to `shape.longest` blocks,
    /// unless that strip is held then or the block already holds `shape.concurrent` strips.
    #[must_use]
    pub fn seeded(seed: u64, shape: &Shape) -> Schedule {
        let mut rng = Rng::new(seed);
        let mut s = Schedule::default();
        if shape.strips.is_empty() {
            return s;
        }
        for b in shape.first..shape.last {
            if rng.below(shape.rarity.max(1)) != 0 {
                continue;
            }
            let strip = shape.strips[usize::try_from(rng.below(shape.strips.len() as u64)).unwrap_or(0)];
            let longest = shape.longest.min(shape.last - b).max(1);
            let blocks = 1 + usize::try_from(rng.below(longest as u64)).unwrap_or(0);
            let h = Hold { block: b, strip, blocks };
            let clash = s.holds.iter().any(|o| o.strip == strip && o.block < h.end() + 1 && h.block < o.end() + 1);
            let busy = (h.block..h.end()).any(|x| s.active(x).count() >= shape.concurrent);
            if !clash && !busy {
                s.holds.push(h);
            }
        }
        s.holds.sort_unstable();
        s
    }

    /// The strips held in `block`.
    #[must_use = "an iterator"]
    pub fn active(&self, block: usize) -> impl Iterator<Item = usize> + '_ {
        self.holds.iter().filter(move |h| h.covers(block)).map(|h| h.strip)
    }

    /// The strips whose hold starts in `block`.
    #[must_use = "an iterator"]
    pub fn starting(&self, block: usize) -> impl Iterator<Item = usize> + '_ {
        self.holds.iter().filter(move |h| h.block == block).map(|h| h.strip)
    }

    /// The strips whose hold ended with the block before `block` (released before it).
    #[must_use = "an iterator"]
    pub fn ending(&self, block: usize) -> impl Iterator<Item = usize> + '_ {
        self.holds.iter().filter(move |h| h.end() == block).map(|h| h.strip)
    }

    /// The block after the last hold (0 for none).
    #[must_use]
    pub fn end(&self) -> usize {
        self.holds.iter().map(Hold::end).max().unwrap_or(0)
    }

    /// The schedule as a Rust literal, for a regression test; `name` gives each strip's name for
    /// a comment.
    #[must_use]
    pub fn to_rust(&self, name: &dyn Fn(usize) -> String) -> String {
        let mut out = String::from("Schedule::new(&[\n");
        for h in &self.holds {
            let _ = writeln!(out, "    ({}, {}, {}), // {} held from block {} for {}", h.strip, h.block, h.blocks, name(h.strip), h.block, h.blocks);
        }
        out.push_str("])");
        out
    }

    /// The smallest schedule found that still `fails` (see the module docs). `self` must fail.
    /// `fails` is called once per candidate.
    #[must_use]
    pub fn shrink(&self, fails: &mut dyn FnMut(&Schedule) -> bool) -> Schedule {
        let mut best = self.clone();
        loop {
            let before = best.clone();
            best = drop_holds(best, fails);
            best = shorten_holds(best, fails);
            best = remove_strips(best, fails);
            if best == before {
                return best;
            }
        }
    }
}

/// Delta debugging over the holds: try removing chunks of `n` holds (n halving down to one).
fn drop_holds(mut s: Schedule, fails: &mut dyn FnMut(&Schedule) -> bool) -> Schedule {
    let mut n = s.holds.len().div_ceil(2).max(1);
    loop {
        let mut i = 0;
        while i < s.holds.len() {
            let mut c = s.clone();
            c.holds.drain(i..(i + n).min(c.holds.len()));
            if fails(&c) {
                s = c;
            } else {
                i += n;
            }
        }
        if n == 1 {
            return s;
        }
        n = n.div_ceil(2);
    }
}

/// Each hold as short as it still fails: to one block, else one block shorter at a time.
fn shorten_holds(mut s: Schedule, fails: &mut dyn FnMut(&Schedule) -> bool) -> Schedule {
    for i in 0..s.holds.len() {
        while s.holds[i].blocks > 1 {
            let mut c = s.clone();
            c.holds[i].blocks = 1;
            if fails(&c) {
                s = c;
                break;
            }
            c.holds[i].blocks = s.holds[i].blocks - 1;
            if !fails(&c) {
                break;
            }
            s = c;
        }
    }
    s
}

/// Fewer strips: each strip's holds removed together while it still fails.
fn remove_strips(mut s: Schedule, fails: &mut dyn FnMut(&Schedule) -> bool) -> Schedule {
    let mut strips: Vec<usize> = s.holds.iter().map(|h| h.strip).collect();
    strips.sort_unstable();
    strips.dedup();
    for strip in strips {
        let c = Schedule { holds: s.holds.iter().copied().filter(|h| h.strip != strip).collect() };
        if c.holds.len() < s.holds.len() && fails(&c) {
            s = c;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn shape() -> Shape {
        Shape { strips: vec![0, 1, 2, 3, 0, 3], first: 2, last: 60, longest: 3, concurrent: 2, rarity: 4 }
    }

    #[test]
    fn a_seed_gives_the_same_schedule_within_its_shape() {
        let (a, b) = (Schedule::seeded(7, &shape()), Schedule::seeded(7, &shape()));
        assert_eq!(a, b);
        assert!(a.holds.len() > 1, "{a:?}");
        assert_ne!(a, Schedule::seeded(8, &shape()));
        for s in (0..64).map(|seed| Schedule::seeded(seed, &shape())) {
            assert!(s.holds.iter().all(|h| h.block >= 2 && h.end() <= 60 && (1..=3).contains(&h.blocks)), "{s:?}");
            assert!((0..70).all(|b| s.active(b).count() <= 2), "{s:?}");
            for (i, h) in s.holds.iter().enumerate() {
                assert!(s.holds[i + 1..].iter().all(|o| o.strip != h.strip || o.block > h.end()), "{s:?}");
            }
        }
    }

    #[test]
    fn the_shrinker_finds_the_one_hold_that_fails() {
        let s = Schedule::new(&[(0, 3, 2), (1, 5, 3), (2, 9, 1), (1, 20, 3), (3, 30, 2)]);
        // Fails when strip 1 is held in block 22.
        let mut calls = 0;
        let min = s.shrink(&mut |c: &Schedule| {
            calls += 1;
            c.active(22).any(|x| x == 1)
        });
        assert_eq!(min, Schedule::new(&[(1, 20, 3)]), "the hold that covers block 22, not shortened past it");
        assert!(calls < 64, "{calls} predicate calls");
        let text = min.to_rust(&|s| format!("strip {s}"));
        assert!(text.contains("(1, 20, 3), // strip 1 held from block 20 for 3"), "{text}");
    }

    #[test]
    fn the_shrinker_shortens_and_keeps_both_halves_of_a_pair() {
        let s = Schedule::new(&[(0, 3, 3), (2, 4, 3), (1, 10, 2)]);
        // Fails when strips 0 and 2 are held together.
        let min = s.shrink(&mut |c: &Schedule| (0..20).any(|b| c.active(b).any(|x| x == 0) && c.active(b).any(|x| x == 2)));
        assert_eq!(min.holds.len(), 2, "{min:?}");
        assert!(min.holds.iter().all(|h| h.strip != 1), "{min:?}");
        assert_eq!(min.holds.iter().map(|h| h.blocks).sum::<usize>(), 3, "shortened as far as it still overlaps: {min:?}");
    }
}
