//! The render pool's diagnostic figures (issue #169): per strip, overruns by cause, inline
//! renders and the cost history the admission rule reads; per set, two histograms, the lateness
//! of `Late` results and the workers' wake latency (publish to claim).
//!
//! **Per strip, one cache line.** Each strip's figures are one `StripCells` aligned to 64 bytes:
//! the cost EWMA and worst case (#168, read by the admission rule), the four counts and two
//! stamps, eight `AtomicU64`, so the executors of neighboring strips never write one line. The
//! histograms are per set: two of seven buckets each would add 112 bytes per strip (two more
//! lines), and the question they answer (how late, how slow to wake) is about the pool, not a
//! strip.
//!
//! **Stamps.** Nanoseconds since the set's construction (the origin), in `AtomicU64`s beside the
//! cost figures, not in the job slot: the slot's own fields are still written only by its
//! transitions (ADR 0029). The publisher writes a strip's publish stamp (one clock read per
//! block, at its first publish, shared by every strip published in it) before the slot's Release
//! publish; the claiming worker reads it after its Acquire claim. The worker writes its
//! completion stamp before its Release complete; the publisher reads it after its Acquire retire.
//! Each stamp is ordered by the slot's own transitions, and no protocol decision reads one: they
//! are informational (ADR 0029 deviation 17).
//!
//! **Clock reads.** The worker adds none: its claim stamp is the clock read it already takes to
//! time the render, its completion stamp the one that ends the timing. The callback adds one per
//! block (the publish stamp); the lateness of a late result is computed from the completion
//! stamp and the deadline `finish` was given, with no clock read.
//!
//! **Upstream misses** (issue #174). A strip skipped because a task it depends on missed counts an
//! upstream miss, with the origin (the first task of the branch that missed on its own), in a
//! second per-strip table beside the line: written by the publisher alone, so the executors' line
//! keeps its eight words. The origin's own overrun is counted once, at the origin.
//!
//! Writers use `Relaxed` (statistics; nothing else is published through them). Readers (the UI
//! thread, through [`Figures`]) get each figure exact on its own, not a consistent snapshot.
//! Allocation-free after construction. No `unsafe`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Upper limits of the histogram buckets, nanoseconds: up to 50 µs, 100 µs, 200 µs, 500 µs,
/// 1 ms, 2 ms; the last bucket is everything longer.
pub const LIMITS_NS: [u64; 6] = [50_000, 100_000, 200_000, 500_000, 1_000_000, 2_000_000];

/// Buckets per histogram: one per limit, plus the open last one.
pub const BUCKETS: usize = LIMITS_NS.len() + 1;

/// The bucket of a duration of `ns`: the first whose limit it does not exceed, else the last.
#[must_use]
pub fn bucket(ns: u64) -> usize {
    LIMITS_NS.iter().position(|&limit| ns <= limit).unwrap_or(LIMITS_NS.len())
}

/// How far `end` is past `start`, both nanoseconds since the origin (0 when it is not past).
#[must_use]
pub const fn past(end: u64, start: u64) -> u64 {
    end.saturating_sub(start)
}

/// A fixed histogram of durations, [`BUCKETS`] counters. Aligned to a cache line of its own, so
/// the workers' `fetch_add` on the wake histogram never shares a line with the fields the
/// callback reads (the origin, the strips' pointer), which would inflate the latency measured.
#[derive(Debug, Default)]
#[repr(align(64))]
pub struct Histogram([AtomicU64; BUCKETS]);

impl Histogram {
    /// Counts one duration of `ns`. Relaxed; allocation-free.
    pub fn record(&self, ns: u64) {
        self.0[bucket(ns)].fetch_add(1, Ordering::Relaxed);
    }

    /// The counts now, by bucket.
    #[must_use]
    pub fn counts(&self) -> [u64; BUCKETS] {
        std::array::from_fn(|i| self.0[i].load(Ordering::Relaxed))
    }
}

/// One strip's cells: the cost history, the stamps and the counts. One cache line.
#[derive(Debug, Default)]
#[repr(align(64))]
pub(crate) struct StripCells {
    /// Estimated render cost, nanoseconds (EWMA; 0 = never measured). Written by the strip's
    /// one executor.
    pub(crate) cost_ns: AtomicU64,
    /// Worst-case render cost, nanoseconds (a decaying maximum, never below the EWMA; 0 = never
    /// measured: no history). Written by the strip's one executor.
    pub(crate) worst_ns: AtomicU64,
    /// The publish stamp of the strip's last publish (0: never published). Publisher, before the
    /// slot's Release publish.
    published_ns: AtomicU64,
    /// The completion stamp of the strip's last worker render. The worker, before its Release
    /// complete.
    done_ns: AtomicU64,
    overruns_busy: AtomicU64,
    overruns_late: AtomicU64,
    overruns_unstarted: AtomicU64,
    inline_renders: AtomicU64,
}

/// One strip's figures now (see [`Figures::strip`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StripFigures {
    /// Overruns because a worker still held the strip from an earlier block.
    pub overruns_busy: u64,
    /// Overruns because the strip's worker had not finished at the deadline.
    pub overruns_late: u64,
    /// Overruns because no thread had started the strip at the deadline.
    pub overruns_unstarted: u64,
    /// Inline renders by the callback.
    pub inline_renders: u64,
    /// The render cost EWMA, nanoseconds (0: no history).
    pub cost_ns: u64,
    /// The worst-case render cost, nanoseconds (0: no history).
    pub worst_ns: u64,
    /// Blocks the strip was skipped because a task it depends on missed (issue #174; not in
    /// [`StripFigures::overruns`], which counts the strip's own).
    pub upstream_missed: u64,
    /// The origin of the last upstream miss: the strip index of the task that missed on its own.
    pub upstream_origin: Option<usize>,
}

impl StripFigures {
    /// Overruns of the strip's own causes (busy, late, unstarted).
    #[must_use]
    pub const fn overruns(&self) -> u64 {
        self.overruns_busy + self.overruns_late + self.overruns_unstarted
    }
}

/// What a per-strip count counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Count {
    Busy,
    Late,
    Unstarted,
    Inline,
}

/// One strip's upstream misses (issue #174): written by the publisher alone.
#[derive(Debug, Default)]
struct Upstream {
    missed: AtomicU64,
    /// The last origin's strip index plus one (0: none).
    origin: AtomicU64,
}

/// The set's figures, shared with the workers and with readers on other threads. A
/// [`super::WorkerSet::figures`] handle holds nothing else of the set: no runner, no strip.
#[derive(Debug)]
pub struct Figures {
    origin: Instant,
    strips: Box<[StripCells]>,
    upstream: Box<[Upstream]>,
    lateness: Histogram,
    wake: Histogram,
}

impl Figures {
    /// Figures for `strips` strips, the clock origin now (allocates; at set construction).
    pub(crate) fn new(strips: usize) -> Self {
        Self {
            origin: Instant::now(),
            strips: (0..strips).map(|_| StripCells::default()).collect(),
            upstream: (0..strips).map(|_| Upstream::default()).collect(),
            lateness: Histogram::default(),
            wake: Histogram::default(),
        }
    }

    /// `t` as nanoseconds since the origin (0 at or before it; saturates). No clock read.
    #[must_use]
    pub fn nanos(&self, t: Instant) -> u64 {
        u64::try_from(t.saturating_duration_since(self.origin).as_nanos()).unwrap_or(u64::MAX)
    }

    pub(crate) fn cells(&self, strip: usize) -> Option<&StripCells> {
        self.strips.get(strip)
    }

    /// Adds one to a strip's count. Relaxed.
    pub(crate) fn count(&self, strip: usize, what: Count) {
        let Some(c) = self.strips.get(strip) else { return };
        let cell = match what {
            Count::Busy => &c.overruns_busy,
            Count::Late => &c.overruns_late,
            Count::Unstarted => &c.overruns_unstarted,
            Count::Inline => &c.inline_renders,
        };
        cell.fetch_add(1, Ordering::Relaxed);
    }

    /// Publisher: zeroes a strip's counts (busy, late, unstarted, inline), all written by the
    /// publisher alone. The cost history and the stamps are left alone.
    pub(crate) fn reset_counts(&self, strip: usize) {
        if let Some(c) = self.strips.get(strip) {
            for a in [&c.overruns_busy, &c.overruns_late, &c.overruns_unstarted, &c.inline_renders] {
                a.store(0, Ordering::Relaxed);
            }
        }
        if let Some(u) = self.upstream.get(strip) {
            u.missed.store(0, Ordering::Relaxed);
            u.origin.store(0, Ordering::Relaxed);
        }
    }

    /// Publisher: `strip` was skipped because `origin` missed. Relaxed.
    pub(crate) fn upstream(&self, strip: usize, origin: usize) {
        if let Some(u) = self.upstream.get(strip) {
            u.missed.fetch_add(1, Ordering::Relaxed);
            u.origin.store(u64::try_from(origin).unwrap_or(u64::MAX - 1).saturating_add(1), Ordering::Relaxed);
        }
    }

    /// Publisher, before the slot's Release publish: the block's publish stamp for `strip`.
    pub(crate) fn published(&self, strip: usize, at_ns: u64) {
        if let Some(c) = self.strips.get(strip) {
            c.published_ns.store(at_ns, Ordering::Relaxed);
        }
    }

    /// The claiming worker, after its Acquire claim, at `at_ns`: records the wake latency since
    /// the strip's publish stamp (nothing before a first publish).
    pub(crate) fn claimed(&self, strip: usize, at_ns: u64) {
        let Some(c) = self.strips.get(strip) else { return };
        let published = c.published_ns.load(Ordering::Relaxed);
        if published != 0 {
            self.wake.record(past(at_ns, published));
        }
    }

    /// The worker, before its Release complete: the strip's completion stamp.
    pub(crate) fn done(&self, strip: usize, at_ns: u64) {
        if let Some(c) = self.strips.get(strip) {
            c.done_ns.store(at_ns, Ordering::Relaxed);
        }
    }

    /// Publisher, after the Acquire retire of a late result: records how far its completion
    /// stamp is past `deadline_ns`, the deadline of the block it was late for.
    pub(crate) fn retired_late(&self, strip: usize, deadline_ns: u64) {
        if let Some(c) = self.strips.get(strip) {
            self.lateness.record(past(c.done_ns.load(Ordering::Relaxed), deadline_ns));
        }
    }

    /// Strips.
    #[must_use]
    pub fn len(&self) -> usize {
        self.strips.len()
    }

    /// True for a set of no strips.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strips.is_empty()
    }

    /// One strip's figures now (`None`: no such strip).
    #[must_use]
    pub fn strip(&self, strip: usize) -> Option<StripFigures> {
        let c = self.strips.get(strip)?;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let (missed, origin) = self.upstream.get(strip).map_or((0, 0), |u| (r(&u.missed), r(&u.origin)));
        Some(StripFigures {
            upstream_missed: missed,
            upstream_origin: origin.checked_sub(1).and_then(|o| usize::try_from(o).ok()),
            overruns_busy: r(&c.overruns_busy),
            overruns_late: r(&c.overruns_late),
            overruns_unstarted: r(&c.overruns_unstarted),
            inline_renders: r(&c.inline_renders),
            cost_ns: r(&c.cost_ns),
            worst_ns: r(&c.worst_ns),
        })
    }

    /// The `N` strips with the most overruns, most first (a tie keeps the lower index first),
    /// each with its index; only strips with an overrun or an upstream miss, the rest `None`.
    /// Ranked by their own overruns, then by upstream misses (issue #174). Allocation-free.
    #[must_use]
    pub fn top_overruns<const N: usize>(&self) -> [Option<(usize, StripFigures)>; N] {
        let mut top: [Option<(usize, StripFigures)>; N] = [None; N];
        let rank = |f: &StripFigures| (f.overruns(), f.upstream_missed);
        for i in 0..self.strips.len() {
            let Some(f) = self.strip(i).filter(|f| rank(f) > (0, 0)) else { continue };
            let Some(at) = top.iter().position(|t| t.is_none_or(|(_, g)| rank(&f) > rank(&g))) else { continue };
            for k in (at + 1..N).rev() {
                top[k] = top[k - 1];
            }
            top[at] = Some((i, f));
        }
        top
    }

    /// The lateness of `Late` results (how far past its block's deadline the render finished),
    /// counts by bucket ([`LIMITS_NS`]).
    #[must_use]
    pub fn lateness(&self) -> [u64; BUCKETS] {
        self.lateness.counts()
    }

    /// The workers' wake latency (a block's publish to a worker's claim of the strip), counts by
    /// bucket ([`LIMITS_NS`]).
    #[must_use]
    pub fn wake(&self) -> [u64; BUCKETS] {
        self.wake.counts()
    }
}
