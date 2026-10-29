//! A fixed set of strip render workers and the audio callback's bounded deadline policy (issue
//! #124 slice 2, ADR 0029), built on the job slots of [`crate::strip_jobs`].
//!
//! [`WorkerSet::new`] starts N threads once, off the audio path; each promotes itself once with
//! [`crate::promote_audio_thread`] (unless told not to), then loops: claim a published strip with
//! [`JobSet::try_claim_next`], run the caller's [`StripRunner`] on it, complete it, and park when
//! nothing is left. The [`WorkerSet`] value is the one publisher: its block methods take
//! `&mut self`, so only the thread that owns it (the audio callback) can call them.
//!
//! One block, on the audio callback:
//!
//! 1. [`WorkerSet::begin_block`]: the per-block rule. Every slot is read first: a `Done` left
//!    from an earlier block (a late worker's result) is retired unread; a slot still `Claimed`
//!    marks its strip busy for this whole block.
//! 2. [`WorkerSet::publish`] for each strip to render. Write a strip's inputs before its publish,
//!    and only when [`WorkerSet::is_busy`] is false: a busy strip belongs to a worker. Publishing
//!    a busy strip returns [`Published::Skipped`] and the strip is an overrun for the block.
//! 3. [`WorkerSet::wake`] to unpark idle workers (allocation-free, never blocks).
//! 4. [`WorkerSet::finish`] with the block deadline. The deadline policy:
//!    - a slot `Done` for this block is consumed: [`Outcome::Mixed`] (the caller mixes it);
//!    - a slot still `Published` is reclaimed and rendered inline through the caller's closure,
//!      only if the admission rule below lets it: [`Outcome::Inline`]; otherwise it stays
//!      available to the workers until the deadline;
//!    - a slot `Claimed` is waited for by sweeping again, spinning with
//!      [`std::hint::spin_loop`] between sweeps: never a yield, a lock or a sleep, and never past
//!      the deadline (a yield on Windows is `SwitchToThread`, which can hand the core to a
//!      normal-priority thread for a whole scheduler quantum: a late block under load);
//!    - at the deadline every strip not resolved is an [`Outcome::Overrun`]: a slot still
//!      `Published` is reclaimed and retired unrendered ([`OverrunCause::Unstarted`]), a slot
//!      still `Claimed` is left alone ([`OverrunCause::Late`]; the next block skips it).
//!
//! `finish` returns no later than the deadline plus one sweep, unless an admitted inline render
//! ran longer than the strip's worst observed cost (a render slower than any it has had): then by
//! that excess. It never blocks on a worker. Silence for an overrun strip is the caller's job;
//! the set reports one [`Outcome`] per strip in a preallocated table and counts overruns, inline
//! renders, refused inline renders and late results in [`Stats`].
//!
//! **Admission** (issue #168; ADR 0029, deviation 16). An inline render cannot be bounded once it
//! has started, so the callback starts one only when the strip's history says it fits. Each
//! render's duration (worker or inline) updates two per-strip figures, each an `AtomicU64` of
//! nanoseconds written only by the strip's one executor:
//!
//! - the cost estimate, an EWMA with weight 1/4;
//! - the worst case, a decaying maximum: a render longer than it replaces it; otherwise each
//!   render moves it 1/[`WORST_DECAY`] of the way down toward the EWMA (never below it). A spike
//!   is remembered for a few hundred renders (from 40 ms down to 1 ms over a 0.1 ms EWMA takes
//!   about 240 renders, 0.3 s at 64 frames and 48 kHz), then forgotten unless it comes back. A
//!   decaying maximum rather than the maximum over the last N renders: one word per strip instead
//!   of N, an O(1) update, and a spike fades instead of vanishing at once when it leaves a window.
//!
//! The history belongs to the instance a strip slot holds, not to the slot: both figures are 0
//! until the strip's first render ("no history"), survive every publish, and reset with the
//! set (a new [`WorkerSet`], which a device reopen builds), with [`WorkerSet::clear_suspect`]
//! (a replaced instance), and with [`WorkerSet::reset_history`], which the caller calls when the
//! slot comes to hold another processing chain (a plug-in loaded or replaced, a track moved onto
//! the index). [`WorkerSet::set_cost_estimate`] seeds both.
//!
//! A strip still `Published` is reclaimed for an inline render only when, checked in this order:
//!
//! 1. it has history: a chain never rendered is never rendered inline (its first render is where
//!    a plug-in does its one-time load); else refused as [`InlineRefusal::NoHistory`];
//! 2. it has not overrun in the last [`OVERRUN_BAR_BLOCKS`] blocks: an `Overrun(Late)`, an
//!    `Overrun(Busy)`, or an inline render that ended past its deadline bars the strip from
//!    inline renders for the next [`OVERRUN_BAR_BLOCKS`] blocks; else refused as
//!    [`InlineRefusal::RecentOverrun`]. An `Overrun(Unstarted)` does not bar it: no render took
//!    place, so it says nothing of the strip's cost, and counting it would keep a barred strip the
//!    workers do not reach barred for good (each refusal would make another `Unstarted`);
//! 3. time is left before the deadline (no time left is the deadline itself, not a refusal);
//! 4. its worst case fits in the time left; else refused as [`InlineRefusal::WorstCase`].
//!
//! A refused strip stays claimable by the workers until the deadline and is otherwise an
//! `Overrun(Unstarted)`, the honest failure. Each refused strip is counted once per block, by the
//! first reason that refused it ([`Stats::inline_refused_no_history`] and its two siblings).
//!
//! **Faults in the runner.** The worker loop catches a panic from [`StripRunner::run`]
//! (`catch_unwind`), completes the slot anyway (so it is never left `Claimed` by an unwind) and
//! flags the strip: [`Outcome::Faulted`], counted in [`Stats::runner_panics`]. Its output must not
//! be mixed and its instance may be in any state. The strip becomes suspect (also when the
//! panic was in a late result, retired at a later `begin_block`): [`WorkerSet::publish`] refuses
//! it with [`PublishRefused::Suspect`] until the caller replaces the instance and calls
//! [`WorkerSet::clear_suspect`]. The inline closure must not panic: an unwind through `finish`
//! leaves the block unresolved (the slot stays `Reclaimed` until the next `begin_block`).
//!
//! **Diagnostic figures** (issue #169; [`figures`]; ADR 0029, deviation 17). Pool runs only, for
//! the diagnostics report and the bench; no protocol decision reads them. Per strip, in one cache
//! line beside the cost figures: overruns by cause, inline renders, and two stamps. Per set: two
//! histograms with the buckets of [`figures::LIMITS_NS`] (up to 50 µs, 100 µs, 200 µs, 500 µs,
//! 1 ms, 2 ms, more):
//!
//! - **lateness** of `Late` results: the worker stamps its completion (the clock read that ends
//!   the render's timing, before its Release complete); when [`WorkerSet::begin_block`] retires the
//!   late result, the callback records how far that stamp is past the deadline the strip was
//!   late for (kept from `finish`, no clock read);
//! - **wake** latency, publish to claim: [`WorkerSet::publish`] stamps every strip it publishes
//!   with one clock read per block (taken at the block's first publish, before the slot's
//!   Release); the worker records its claim time (the clock read that starts the render's timing)
//!   less that stamp. Strips published later in the block, and every strip until
//!   [`WorkerSet::wake`], count the publisher's own time between the first publish and theirs.
//!
//! Stamps are nanoseconds since the set's construction in `AtomicU64`s; the job slot is not
//! touched. The worker adds no clock read; the callback adds one per block. Read them from any
//! thread through [`WorkerSet::figures`].
//!
//! Never drop a [`WorkerSet`] on the audio thread: `Drop` runs [`WorkerSet::shutdown`], which
//! sleeps while it waits (up to 1 s).
//!
//! **Retirement and quarantine** (gate 5). [`WorkerSet::shutdown`] stops and wakes every
//! worker and joins each within one bound. A worker still running at the bound is abandoned: its
//! `JoinHandle` is forgotten (never joined later), its slot stays `Claimed`, one clone of the
//! shared state (and with it the [`StripRunner`]) is leaked so nothing it may still execute in is
//! freed, and the [`Quarantine`] report names the worker and its strip. **The caller must never
//! reuse or free a quarantined strip's instance or buffers.**
//!
//! Nothing after [`WorkerSet::new`] allocates on the publisher's path or the worker loop.
//! No `unsafe`.

pub mod figures;
mod stepwise;

pub use stepwise::Poll;

use crate::Promotion;
use crate::strip_jobs::{Consume, JobSet, PublishError, State};
use figures::{Count, Figures};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Renders one strip for one block. Called on a worker thread for a claimed strip; the
/// implementation owns the strip's instance and buffers for the call (ADR 0029, buffer
/// ownership follows the slot state).
pub trait StripRunner: Send + Sync + 'static {
    /// Renders `strip` for block `generation`. Must not allocate, lock or block on the audio
    /// path; a panic is caught by the worker and reported as [`Outcome::Faulted`].
    fn run(&self, strip: usize, generation: u64);
}

/// How a [`WorkerSet`] is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Worker threads (the caller decides, typically cores minus one; 0 renders everything
    /// inline).
    pub workers: usize,
    /// Strip slots (the most strips a plan can hold).
    pub strips: usize,
    /// Promote each worker with [`crate::promote_audio_thread`] at start.
    pub promote: bool,
}

/// What became of one strip in the current block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not published this block.
    NotPublished,
    /// Published; [`WorkerSet::finish`] has not resolved it yet.
    Pending,
    /// A worker rendered it for this block and the result was consumed: mix it.
    Mixed,
    /// The callback rendered it inline through the `finish` closure: mix it.
    Inline,
    /// No result for this block: the strip is silence (a counted stream fault).
    Overrun(OverrunCause),
    /// The runner panicked on a worker: no result, the instance is suspect.
    Faulted,
}

/// Why a strip is an [`Outcome::Overrun`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrunCause {
    /// A worker still held it from an earlier block, so it was not published (the per-block
    /// rule).
    Busy,
    /// A worker claimed it this block and had not finished at the deadline.
    Late,
    /// No thread had started it at the deadline and the admission rule refused an inline
    /// render; it was taken back unrendered.
    Unstarted,
    /// Not published: a task it depends on had no result for this block (issue #174, the
    /// dependency graph of [`crate::task_graph`]; see [`WorkerSet::skip_upstream`]). Counted apart
    /// from the other causes, which are counted once, at the origin.
    UpstreamMissed,
}

/// Why the admission rule refused to render an unstarted strip inline (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineRefusal {
    /// The strip has never been rendered: its cost is unknown.
    NoHistory,
    /// The strip overran (or an inline render of it ended past its deadline) within the last
    /// [`OVERRUN_BAR_BLOCKS`] blocks.
    RecentOverrun,
    /// The strip's worst observed cost is more than the time left.
    WorstCase,
}

/// Blocks a strip is barred from inline renders after an overrun (`Late`, `Busy`, or an inline
/// render that ended past its deadline). Small, so a strip that recovers is soon a candidate
/// again; never zero, so a strip that just overran is not run on the callback right after.
pub const OVERRUN_BAR_BLOCKS: u64 = 8;

/// The worst case's decay per render: it moves `1 / WORST_DECAY` of the way down toward the
/// EWMA. See the module docs.
pub const WORST_DECAY: u64 = 64;

/// What [`WorkerSet::publish`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Published {
    /// The strip is published for this block.
    Published,
    /// A worker still holds the strip from an earlier block: not published, an overrun.
    Skipped,
}

/// Why [`WorkerSet::publish`] refused. Each one is a caller error counted in [`Stats::misuse`],
/// except [`PublishRefused::Suspect`], which is the set protecting a suspect instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishRefused {
    /// No block is open ([`WorkerSet::begin_block`] not called or refused, or the set is shut
    /// down).
    NoBlock,
    /// No strip slot at that index.
    OutOfRange,
    /// The strip was already published (or skipped) in this block.
    Twice,
    /// The slot refused the publish (see [`PublishError`]).
    Slot(PublishError),
    /// The runner panicked on this strip (in this block's result or a late one): its instance
    /// is suspect and the strip is not published again until [`WorkerSet::clear_suspect`].
    Suspect,
}

/// Counts for one block, from [`WorkerSet::finish`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockReport {
    /// [`Outcome::Mixed`] strips.
    pub mixed: usize,
    /// [`Outcome::Inline`] strips.
    pub inline: usize,
    /// [`Outcome::Overrun`] strips, all causes.
    pub overrun: usize,
    /// [`Outcome::Faulted`] strips.
    pub faulted: usize,
    /// Strips the admission rule refused to render inline this block (any reason; each strip
    /// once).
    pub inline_refused: usize,
    /// Strips skipped because a task they depend on missed ([`OverrunCause::UpstreamMissed`];
    /// not in `overrun`).
    pub upstream_missed: usize,
}

/// Running totals since construction (stream-stats style; a snapshot, workers keep moving).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Blocks begun.
    pub blocks: u64,
    /// Worker results consumed ([`Outcome::Mixed`]).
    pub mixed: u64,
    /// Inline renders by the callback ([`Outcome::Inline`]).
    pub inline_renders: u64,
    /// Runner calls completed by workers (including late and faulted ones).
    pub worker_runs: u64,
    /// Overruns of the three causes of a strip's own (late, busy, unstarted); upstream-missed
    /// skips are apart ([`Stats::upstream_missed`]).
    pub overruns: u64,
    /// Overruns with [`OverrunCause::Late`]: claimed, not done at the deadline (late claims).
    pub overruns_late: u64,
    /// Overruns with [`OverrunCause::Busy`]: skipped, a worker still held the strip.
    pub overruns_busy: u64,
    /// Overruns with [`OverrunCause::Unstarted`]: no thread had started the strip at the
    /// deadline.
    pub overruns_unstarted: u64,
    /// Inline renders refused because the strip had no cost history
    /// ([`InlineRefusal::NoHistory`]; each strip once per block).
    pub inline_refused_no_history: u64,
    /// Inline renders refused because the strip's worst case did not fit in the time left
    /// ([`InlineRefusal::WorstCase`]; each strip once per block).
    pub inline_refused_worst_case: u64,
    /// Inline renders refused because the strip overran recently
    /// ([`InlineRefusal::RecentOverrun`]; each strip once per block).
    pub inline_refused_recent_overrun: u64,
    /// Late results retired unread at a later `begin_block`.
    pub late_results: u64,
    /// Runner panics caught by workers ([`Outcome::Faulted`]).
    pub runner_panics: u64,
    /// Refused publisher calls (caller errors).
    pub misuse: u64,
    /// Strips skipped because a task they depend on missed ([`OverrunCause::UpstreamMissed`]).
    pub upstream_missed: u64,
}

#[derive(Default)]
struct Counters {
    blocks: AtomicU64,
    mixed: AtomicU64,
    inline_renders: AtomicU64,
    worker_runs: AtomicU64,
    overruns_late: AtomicU64,
    overruns_busy: AtomicU64,
    overruns_unstarted: AtomicU64,
    refused_no_history: AtomicU64,
    refused_worst_case: AtomicU64,
    refused_recent_overrun: AtomicU64,
    late_results: AtomicU64,
    runner_panics: AtomicU64,
    misuse: AtomicU64,
    upstream_missed: AtomicU64,
}

/// Adds one to a statistic. Relaxed: no other memory is published through it.
fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// One worker's shared state.
struct WorkerSlot {
    /// Set by the publisher's wake (and by shutdown); taken by the worker before it sweeps.
    woken: AtomicBool,
    /// The strip the worker is inside, plus one (0: none). For wake selection and quarantine.
    current: AtomicUsize,
    /// What the worker's promotion achieved (unset when promotion is off or not done yet).
    promotion: OnceLock<Promotion>,
}

struct Shared {
    jobs: JobSet,
    runner: Arc<dyn StripRunner>,
    /// Per strip: the cost history (EWMA and worst case) and the diagnostic figures; per set,
    /// the histograms. Its own `Arc`, so a reader holds nothing else of the set.
    figures: Arc<Figures>,
    /// Per strip: the runner panicked on the last claimed render. Written by the worker before
    /// its completing Release, read by the publisher after its consuming Acquire.
    panicked: Box<[AtomicBool]>,
    /// Per strip: a panic was observed (in a consumed or a late result); refused by `publish`
    /// until `clear_suspect`. Publisher-only (Relaxed).
    suspect: Box<[AtomicBool]>,
    workers: Box<[WorkerSlot]>,
    stop: AtomicBool,
    stats: Counters,
}

impl Shared {
    fn record_cost(&self, strip: usize, took: Duration) {
        let Some(cells) = self.figures.cells(strip) else { return };
        let (c, w) = (&cells.cost_ns, &cells.worst_ns);
        let ns = u64::try_from(took.as_nanos()).unwrap_or(u64::MAX).max(1);
        // Relaxed: estimates; one executor per strip at a time writes them.
        let old = c.load(Ordering::Relaxed);
        let ewma = if old == 0 { ns } else { old - old / 4 + ns / 4 };
        c.store(ewma, Ordering::Relaxed);
        w.store(next_worst(w.load(Ordering::Relaxed), ewma, ns), Ordering::Relaxed);
    }
}

/// The worst case after a render of `ns`, the EWMA already updated to `ewma`: the render if it is
/// longer, else the old worst case moved `1 / WORST_DECAY` of the way down to the EWMA.
fn next_worst(old: u64, ewma: u64, ns: u64) -> u64 {
    let decayed = if old > ewma { old - (old - ewma).div_ceil(WORST_DECAY) } else { ewma };
    decayed.max(ns)
}

/// The workers [`WorkerSet::shutdown`] abandoned. Empty when every worker returned in time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[must_use]
pub struct Quarantine {
    /// Abandoned worker indices.
    pub workers: Vec<usize>,
    /// The strips those workers were inside (left `Claimed`). Never reuse or free their
    /// instances or buffers.
    pub strips: Vec<usize>,
}

impl Quarantine {
    /// True when no worker was abandoned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }
}

/// How long `Drop` waits for the workers when [`WorkerSet::shutdown`] was not called.
const DROP_BOUND: Duration = Duration::from_secs(1);

/// `spin_loop` hints between two sweeps of `finish` that made no progress.
const SPINS_PER_SWEEP: u32 = 32;

/// The fixed worker set and the publisher of its job slots. See the module docs.
pub struct WorkerSet {
    shared: Arc<Shared>,
    handles: Vec<Option<JoinHandle<()>>>,
    threads: Vec<std::thread::Thread>,
    /// The open block (0: none).
    generation: u64,
    outcomes: Box<[Outcome]>,
    /// Per strip: a worker held it at `begin_block`.
    busy: Box<[bool]>,
    /// Strips published this block, in publish order (the first `published_len`).
    published: Box<[usize]>,
    published_len: usize,
    /// Strips skipped this block (busy).
    skipped: usize,
    /// Strips not published this block because they became ready only in the deadline phase
    /// ([`WorkerSet::forgo`]; `Unstarted`).
    forgone: usize,
    /// Strips skipped this block because a task they depend on missed.
    upstream: usize,
    /// Blocks begun by this set (the clock of the overrun bar).
    block_index: u64,
    /// Per strip: the first block index in which the strip may be rendered inline again (0:
    /// never barred).
    barred_until: Box<[u64]>,
    /// Per strip: a refused inline render was already counted this block.
    refused: Box<[bool]>,
    /// Where the next wake starts looking for an idle worker.
    wake_cursor: usize,
    /// The open block's publish stamp (nanoseconds since the figures' origin; 0: none yet).
    publish_ns: u64,
    /// Per strip: the deadline (nanoseconds since the figures' origin) of the block its
    /// outstanding `Late` result was late for (0: none).
    late_deadline_ns: Box<[u64]>,
    shut: bool,
}

impl std::fmt::Debug for WorkerSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerSet")
            .field("workers", &self.threads.len())
            .field("strips", &self.outcomes.len())
            .field("generation", &self.generation)
            .field("shut", &self.shut)
            .finish_non_exhaustive()
    }
}

impl WorkerSet {
    /// Starts `config.workers` threads for `config.strips` strips, each running `runner` on the
    /// strips it claims. Allocates (threads, slots, tables); call it off the audio thread.
    ///
    /// # Errors
    ///
    /// The OS refused a thread; the workers already started are shut down first.
    pub fn new(config: Config, runner: Arc<dyn StripRunner>) -> std::io::Result<Self> {
        let n = config.workers;
        let strips = config.strips;
        let shared = Arc::new(Shared {
            jobs: JobSet::new(strips),
            runner,
            figures: Arc::new(Figures::new(strips)),
            panicked: (0..strips).map(|_| AtomicBool::new(false)).collect(),
            suspect: (0..strips).map(|_| AtomicBool::new(false)).collect(),
            workers: (0..n).map(|_| WorkerSlot { woken: AtomicBool::new(false), current: AtomicUsize::new(0), promotion: OnceLock::new() }).collect(),
            stop: AtomicBool::new(false),
            stats: Counters::default(),
        });
        let mut set = WorkerSet {
            shared,
            handles: Vec::with_capacity(n),
            threads: Vec::with_capacity(n),
            generation: 0,
            outcomes: vec![Outcome::NotPublished; strips].into_boxed_slice(),
            busy: vec![false; strips].into_boxed_slice(),
            published: vec![0; strips].into_boxed_slice(),
            published_len: 0,
            skipped: 0,
            forgone: 0,
            upstream: 0,
            block_index: 0,
            barred_until: vec![0; strips].into_boxed_slice(),
            refused: vec![false; strips].into_boxed_slice(),
            wake_cursor: 0,
            publish_ns: 0,
            late_deadline_ns: vec![0; strips].into_boxed_slice(),
            shut: false,
        };
        for k in 0..n {
            let shared = Arc::clone(&set.shared);
            let start = k * strips / n.max(1);
            let promote = config.promote;
            let spawned = std::thread::Builder::new().name(format!("transit-strip-{k}")).spawn(move || worker_main(&shared, k, start, promote));
            match spawned {
                Ok(h) => {
                    set.threads.push(h.thread().clone());
                    set.handles.push(Some(h));
                }
                Err(e) => {
                    let _ = set.shutdown(DROP_BOUND);
                    return Err(e);
                }
            }
        }
        Ok(set)
    }

    /// Worker threads.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// Strip slots.
    #[must_use]
    pub fn strips(&self) -> usize {
        self.outcomes.len()
    }

    /// The job slots (for `counts`, `faults` and `state`; publisher operations go through the
    /// set).
    #[must_use]
    pub fn jobs(&self) -> &JobSet {
        &self.shared.jobs
    }

    /// What worker `index`'s promotion achieved; `None` before it ran, or when promotion is off.
    #[must_use]
    pub fn promotion(&self, index: usize) -> Option<Promotion> {
        self.shared.workers.get(index).and_then(|w| w.promotion.get().copied())
    }

    /// The strip's estimated render cost (`None` for no such strip; zero before any render).
    #[must_use]
    pub fn cost_estimate(&self, strip: usize) -> Option<Duration> {
        self.shared.figures.cells(strip).map(|c| Duration::from_nanos(c.cost_ns.load(Ordering::Relaxed)))
    }

    /// The strip's worst-case render cost, a decaying maximum (`None` for no such strip; zero
    /// before any render: no history). See the module docs.
    #[must_use]
    pub fn worst_case(&self, strip: usize) -> Option<Duration> {
        self.shared.figures.cells(strip).map(|c| Duration::from_nanos(c.worst_ns.load(Ordering::Relaxed)))
    }

    /// The set's diagnostic figures (per strip and the two histograms; see the module docs), for
    /// a reader on any thread. The handle holds only the figures, never the runner or a strip.
    #[must_use]
    pub fn figures(&self) -> Arc<Figures> {
        Arc::clone(&self.shared.figures)
    }

    /// Seeds or overrides the strip's cost history (for example from a serial measurement): its
    /// estimate and its worst case both become `cost` (zero clears the history). Later renders
    /// keep updating both. Not while the strip is published.
    pub fn set_cost_estimate(&self, strip: usize, cost: Duration) {
        let ns = u64::try_from(cost.as_nanos()).unwrap_or(u64::MAX);
        if let Some(c) = self.shared.figures.cells(strip) {
            c.cost_ns.store(ns, Ordering::Relaxed);
            c.worst_ns.store(ns, Ordering::Relaxed);
        }
    }

    /// True when the strip is barred from inline renders in the open block, because it overran
    /// within the last [`OVERRUN_BAR_BLOCKS`] blocks.
    #[must_use]
    pub fn inline_barred(&self, strip: usize) -> bool {
        self.barred_until.get(strip).is_some_and(|&until| self.block_index < until)
    }

    /// Bars `strip` from inline renders for the next [`OVERRUN_BAR_BLOCKS`] blocks.
    fn bar(&mut self, strip: usize) {
        if let Some(b) = self.barred_until.get_mut(strip) {
            *b = self.block_index + OVERRUN_BAR_BLOCKS + 1;
        }
    }

    /// Running totals.
    #[must_use]
    pub fn stats(&self) -> Stats {
        let c = &self.shared.stats;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let (late, busy, unstarted) = (r(&c.overruns_late), r(&c.overruns_busy), r(&c.overruns_unstarted));
        Stats {
            blocks: r(&c.blocks),
            mixed: r(&c.mixed),
            inline_renders: r(&c.inline_renders),
            worker_runs: r(&c.worker_runs),
            overruns: late + busy + unstarted,
            overruns_late: late,
            overruns_busy: busy,
            overruns_unstarted: unstarted,
            inline_refused_no_history: r(&c.refused_no_history),
            inline_refused_worst_case: r(&c.refused_worst_case),
            inline_refused_recent_overrun: r(&c.refused_recent_overrun),
            late_results: r(&c.late_results),
            runner_panics: r(&c.runner_panics),
            misuse: r(&c.misuse),
            upstream_missed: r(&c.upstream_missed),
        }
    }

    /// The current block's outcome per strip (indexed by strip).
    #[must_use]
    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    /// The current block's outcome of one strip.
    #[must_use]
    pub fn outcome(&self, strip: usize) -> Option<Outcome> {
        self.outcomes.get(strip).copied()
    }

    /// True when a worker held the strip at this block's `begin_block`: do not touch its inputs;
    /// a publish will skip it.
    #[must_use]
    pub fn is_busy(&self, strip: usize) -> bool {
        self.busy.get(strip).copied().unwrap_or(false)
    }

    /// Publisher: opens block `generation` (newer than the last) and applies the per-block rule
    /// to every slot: a late `Done` is retired unread, a `Claimed` slot marks its strip busy.
    /// False when refused (not newer, or shut down; counted in [`Stats::misuse`]).
    pub fn begin_block(&mut self, generation: u64) -> bool {
        // Refuse first, so a refused begin cannot count a jobs fault through `JobSet::begin`.
        if self.shut || generation <= self.shared.jobs.generation() {
            bump(&self.shared.stats.misuse);
            return false;
        }
        if !self.shared.jobs.begin(generation) {
            bump(&self.shared.stats.misuse);
            return false;
        }
        self.generation = generation;
        self.published_len = 0;
        self.skipped = 0;
        self.forgone = 0;
        self.upstream = 0;
        self.publish_ns = 0;
        self.block_index += 1;
        bump(&self.shared.stats.blocks);
        for strip in 0..self.outcomes.len() {
            self.outcomes[strip] = Outcome::NotPublished;
            self.refused[strip] = false;
            self.busy[strip] = !self.settle(strip);
        }
        true
    }

    /// The per-block rule for one slot: brings it to `Idle` when no worker holds it. False when
    /// a worker does (the slot is `Claimed`).
    fn settle(&mut self, strip: usize) -> bool {
        let shared = Arc::clone(&self.shared);
        let Some(slot) = shared.jobs.slot(strip) else { return true };
        match slot.state() {
            State::Idle => true,
            State::Done => {
                // A late worker's result for an older block: retired unread (Acquire inside
                // orders the worker's writes before the strip is reused).
                // A late result whose run panicked (counted by its worker): the instance is
                // suspect, so the strip is not published again until the caller replaces it.
                if self.shared.panicked[strip].swap(false, Ordering::Relaxed) {
                    self.shared.suspect[strip].store(true, Ordering::Relaxed);
                }
                bump(&self.shared.stats.late_results);
                let retired = slot.retire();
                // After the Acquire retire, so the worker's completion stamp is visible. Only a
                // result `finish` counted `Late` has a deadline (not one of a block `finish` was
                // never called for).
                let deadline = std::mem::take(&mut self.late_deadline_ns[strip]);
                if retired && deadline != 0 {
                    shared.figures.retired_late(strip, deadline);
                }
                retired
            }
            // An inline render that unwound out of `finish`.
            State::Reclaimed => slot.retire(),
            // `finish` was not called for the last block: take the job back.
            State::Published => {
                if slot.reclaim() {
                    slot.retire()
                } else {
                    // A worker won it; settle what it left.
                    self.settle(strip)
                }
            }
            State::Claimed => false,
        }
    }

    /// Publisher: publishes `strip` for the open block, or skips it as an overrun when a worker
    /// still holds it ([`WorkerSet::is_busy`]). Write the strip's inputs before this call, and
    /// only when it is not busy.
    ///
    /// # Errors
    ///
    /// [`PublishRefused`]: a caller error, counted in [`Stats::misuse`]; nothing changes.
    pub fn publish(&mut self, strip: usize) -> Result<Published, PublishRefused> {
        let refused = |set: &Self, why| {
            bump(&set.shared.stats.misuse);
            Err(why)
        };
        if self.generation == 0 || self.shut {
            return refused(self, PublishRefused::NoBlock);
        }
        let Some(&outcome) = self.outcomes.get(strip) else {
            return refused(self, PublishRefused::OutOfRange);
        };
        if outcome != Outcome::NotPublished {
            return refused(self, PublishRefused::Twice);
        }
        if self.shared.suspect[strip].load(Ordering::Relaxed) {
            return Err(PublishRefused::Suspect);
        }
        if self.busy[strip] {
            self.outcomes[strip] = Outcome::Overrun(OverrunCause::Busy);
            self.skipped += 1;
            bump(&self.shared.stats.overruns_busy);
            self.shared.figures.count(strip, Count::Busy);
            self.bar(strip);
            return Ok(Published::Skipped);
        }
        // The block's publish stamp: one clock read per block, at its first publish; written
        // before the slot's Release publish, so the claiming worker sees it.
        if self.publish_ns == 0 {
            self.publish_ns = self.shared.figures.nanos(Instant::now()).max(1);
        }
        self.shared.figures.published(strip, self.publish_ns);
        if let Err(e) = self.shared.jobs.publish(strip) {
            return refused(self, PublishRefused::Slot(e));
        }
        self.outcomes[strip] = Outcome::Pending;
        self.published[self.published_len] = strip;
        self.published_len += 1;
        Ok(Published::Published)
    }

    /// True when the runner panicked on this strip (in a consumed or a late result): its
    /// instance is suspect and [`WorkerSet::publish`] refuses it until
    /// [`WorkerSet::clear_suspect`].
    #[must_use]
    pub fn is_suspect(&self, strip: usize) -> bool {
        self.shared.suspect.get(strip).is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// Publisher: the strip's instance has been replaced; it may be published again. The new
    /// instance starts with no history ([`WorkerSet::reset_history`]).
    pub fn clear_suspect(&mut self, strip: usize) {
        if let Some(f) = self.shared.suspect.get(strip) {
            f.store(false, Ordering::Relaxed);
        }
        self.reset_history(strip);
    }

    /// Publisher: zeroes the strip's diagnostic counts (overruns by cause, inline renders; see
    /// [`figures::StripFigures`]), for a caller whose strip index now holds another track, so the
    /// counts it reports under a name are that track's. Never touches the cost history, the
    /// stamps or the histograms; no protocol decision reads the counts. Allocation-free.
    pub fn reset_counts(&mut self, strip: usize) {
        self.shared.figures.reset_counts(strip);
    }

    /// Publisher: the strip now holds another processing chain (a plug-in loaded or replaced,
    /// another track's strip moved onto this index). Its cost history and its overrun bar are
    /// cleared, so it is not rendered inline until it has been rendered again (its first render
    /// is where a plug-in does its one-time load). Not while the strip is published or held by a
    /// worker. Allocation-free.
    pub fn reset_history(&mut self, strip: usize) {
        self.set_cost_estimate(strip, Duration::ZERO);
        if let Some(b) = self.barred_until.get_mut(strip) {
            *b = 0;
        }
    }

    /// Publisher: unparks up to `count` workers that are neither working nor already woken, and
    /// returns how many. Pass the number of strips published. Allocation-free and non-blocking
    /// (`Thread::unpark`).
    pub fn wake(&mut self, count: usize) -> usize {
        // Store-buffering pattern (Dekker): the publisher stores slot states (publish) then loads
        // a worker's `current`; the worker stores `current = 0` then loads slot states (its next
        // sweep). Without a SeqCst fence on both sides, a weakly ordered CPU (AArch64) can let
        // both loads see the old values: the publisher skips the worker as busy and the worker's
        // sweep misses the new job, so it parks with a strip published. With the two fences, at
        // least one side sees the other's store: the publisher wakes the worker, or the worker
        // claims the job. Paired with the fence after `current.store(0)` in `worker_main`.
        // Kept (rather than dropping the busy skip) so `wake(count)` reaches idle workers
        // instead of spending its count on busy ones.
        std::sync::atomic::fence(Ordering::SeqCst);
        let n = self.threads.len();
        let mut woken = 0;
        for k in 0..n {
            if woken >= count {
                break;
            }
            let i = (self.wake_cursor + k) % n;
            let w = &self.shared.workers[i];
            if w.current.load(Ordering::Relaxed) != 0 {
                continue;
            }
            // AcqRel: pairs with the worker's taking swap; the jobs themselves are ordered by
            // the slots' own Release/Acquire.
            if !w.woken.swap(true, Ordering::AcqRel) {
                self.threads[i].unpark();
                woken += 1;
            }
        }
        if n > 0 {
            self.wake_cursor = (self.wake_cursor + woken.max(1)) % n;
        }
        woken
    }

    /// The admission rule (see the module docs): `Ok` when the callback may start an inline
    /// render of `strip` with `left` before the deadline; `Err(None)` when no time is left (not a
    /// refusal), `Err(Some(reason))` when the rule refuses it.
    fn admission(&self, strip: usize, left: Duration) -> Result<(), Option<InlineRefusal>> {
        let worst = self.shared.figures.cells(strip).map_or(0, |c| c.worst_ns.load(Ordering::Relaxed));
        if worst == 0 {
            return Err(Some(InlineRefusal::NoHistory));
        }
        if self.inline_barred(strip) {
            return Err(Some(InlineRefusal::RecentOverrun));
        }
        if left.is_zero() {
            return Err(None);
        }
        if u128::from(worst) > left.as_nanos() {
            return Err(Some(InlineRefusal::WorstCase));
        }
        Ok(())
    }

    /// Admission at `now`; a refusal is counted once per strip per block.
    fn admits(&mut self, strip: usize, now: Instant, deadline: Instant) -> bool {
        match self.admission(strip, deadline.saturating_duration_since(now)) {
            Ok(()) => true,
            Err(None) => false,
            Err(Some(why)) => {
                if let Some(seen) = self.refused.get_mut(strip)
                    && !std::mem::replace(seen, true)
                {
                    let c = &self.shared.stats;
                    bump(match why {
                        InlineRefusal::NoHistory => &c.refused_no_history,
                        InlineRefusal::WorstCase => &c.refused_worst_case,
                        InlineRefusal::RecentOverrun => &c.refused_recent_overrun,
                    });
                }
                false
            }
        }
    }

    /// Publisher: resolves every strip published this block by `deadline` (see the module docs),
    /// rendering admitted unstarted strips inline through `inline(strip)`, and returns the
    /// block's counts. Each strip's [`Outcome`] is in [`WorkerSet::outcomes`]. Never blocks on a
    /// worker; allocation-free.
    pub fn finish(&mut self, deadline: Instant, mut inline: impl FnMut(usize)) -> BlockReport {
        let g = self.generation;
        // A second handle on the shared state (no allocation), so a slot can be held across the
        // publisher-only bookkeeping (admission counts, the overrun bar) that takes `&mut self`.
        let shared = Arc::clone(&self.shared);
        let mut open = (0..self.published_len).filter(|&k| self.outcomes[self.published[k]] == Outcome::Pending).count();
        while open > 0 {
            let mut progressed = false;
            // One clock read per sweep, again only after an inline render (its cost).
            let mut now = Instant::now();
            for k in 0..self.published_len {
                let strip = self.published[k];
                if self.outcomes[strip] != Outcome::Pending {
                    continue;
                }
                let Some(slot) = shared.jobs.slot(strip) else { continue };
                match slot.state() {
                    State::Done => {
                        if let Some(o) = self.take_done(strip, g) {
                            self.outcomes[strip] = o;
                            open -= 1;
                            progressed = true;
                        }
                    }
                    // Admitted, and the reclaim won against the workers (a lost reclaim means a
                    // worker claimed it; it is waited for like any claimed slot).
                    State::Published if self.admits(strip, now, deadline) && slot.reclaim() => {
                        let t0 = Instant::now();
                        inline(strip);
                        now = Instant::now();
                        shared.record_cost(strip, now.saturating_duration_since(t0));
                        if now > deadline {
                            // Ran past the deadline: longer than its worst case said.
                            self.bar(strip);
                        }
                        slot.retire();
                        bump(&self.shared.stats.inline_renders);
                        shared.figures.count(strip, Count::Inline);
                        self.outcomes[strip] = Outcome::Inline;
                        open -= 1;
                        progressed = true;
                    }
                    _ => {}
                }
            }
            if open == 0 {
                break;
            }
            if now >= deadline {
                self.overrun_the_rest(g, deadline);
                break;
            }
            // Spin, bounded by the deadline (checked once per sweep above): a worker sharing this
            // core waits at most until the deadline, and then its strip is a counted overrun.
            if !progressed {
                for _ in 0..SPINS_PER_SWEEP {
                    std::hint::spin_loop();
                }
            }
        }
        self.report()
    }

    /// Consumes a `Done` slot for block `g`: the outcome, or `None` when it is not ready.
    fn take_done(&self, strip: usize, g: u64) -> Option<Outcome> {
        match self.shared.jobs.slot(strip)?.consume(g) {
            Consume::Ready => {
                // Relaxed: ordered after the worker's write by consume's Acquire.
                if self.shared.panicked[strip].swap(false, Ordering::Relaxed) {
                    self.shared.suspect[strip].store(true, Ordering::Relaxed);
                    Some(Outcome::Faulted)
                } else {
                    bump(&self.shared.stats.mixed);
                    Some(Outcome::Mixed)
                }
            }
            // Stale cannot occur for a strip published this block (its Done is this block's).
            Consume::Stale | Consume::Pending => None,
        }
    }

    /// At the deadline: a last read of `Done`, then every unresolved strip is an overrun.
    fn overrun_the_rest(&mut self, g: u64, deadline: Instant) {
        // For the lateness of the `Late` results, when they are retired (no clock read).
        let deadline_ns = self.shared.figures.nanos(deadline).max(1);
        for k in 0..self.published_len {
            let strip = self.published[k];
            if self.outcomes[strip] == Outcome::Pending {
                self.overrun_strip(strip, g, deadline_ns);
            }
        }
    }

    /// At the deadline, one strip still pending: a last read of `Done`, else an overrun
    /// (`Unstarted` when it is taken back unrendered, `Late` when a worker holds it). Sets and
    /// returns its outcome (`Pending` only for a strip with no slot).
    fn overrun_strip(&mut self, strip: usize, g: u64, deadline_ns: u64) -> Outcome {
        if let Some(o) = self.take_done(strip, g) {
            self.outcomes[strip] = o;
            return o;
        }
        let shared = Arc::clone(&self.shared);
        let Some(slot) = shared.jobs.slot(strip) else { return Outcome::Pending };
        let cause = if slot.reclaim() {
            // Taken back unrendered, so no worker starts it after the deadline.
            slot.retire();
            bump(&self.shared.stats.overruns_unstarted);
            shared.figures.count(strip, Count::Unstarted);
            OverrunCause::Unstarted
        } else if let Some(o) = self.take_done(strip, g) {
            // Completed between the two reads.
            self.outcomes[strip] = o;
            return o;
        } else {
            bump(&self.shared.stats.overruns_late);
            shared.figures.count(strip, Count::Late);
            self.late_deadline_ns[strip] = deadline_ns;
            self.bar(strip);
            OverrunCause::Late
        };
        self.outcomes[strip] = Outcome::Overrun(cause);
        self.outcomes[strip]
    }

    fn report(&self) -> BlockReport {
        let refused = self.refused.iter().filter(|&&r| r).count();
        let mut r = BlockReport {
            overrun: self.skipped + self.forgone,
            inline_refused: refused,
            upstream_missed: self.upstream,
            ..BlockReport::default()
        };
        for &strip in &self.published[..self.published_len] {
            match self.outcomes[strip] {
                Outcome::Mixed => r.mixed += 1,
                Outcome::Inline => r.inline += 1,
                Outcome::Overrun(_) => r.overrun += 1,
                Outcome::Faulted => r.faulted += 1,
                Outcome::NotPublished | Outcome::Pending => {}
            }
        }
        r
    }

    /// Retires the workers: sets the stop flag, wakes every worker and joins each one within
    /// `bound` (one bound for all). A worker still running at the bound is abandoned and
    /// reported; see the module docs ("Retirement and quarantine"). Off the audio thread. After
    /// it, `begin_block` and `publish` are refused. A second call returns an empty report.
    pub fn shutdown(&mut self, bound: Duration) -> Quarantine {
        let mut q = Quarantine::default();
        if self.shut {
            return q;
        }
        self.shut = true;
        self.generation = 0;
        self.shared.stop.store(true, Ordering::Release);
        for (w, t) in self.shared.workers.iter().zip(&self.threads) {
            w.woken.store(true, Ordering::Release);
            t.unpark();
        }
        let deadline = Instant::now().checked_add(bound);
        for (k, handle) in self.handles.iter_mut().enumerate() {
            let Some(h) = handle.take() else { continue };
            // No representable deadline (a bound such as `Duration::MAX`) is unbounded.
            while !h.is_finished() && deadline.is_none_or(|d| Instant::now() < d) {
                std::thread::sleep(Duration::from_millis(1));
            }
            if h.is_finished() {
                // The worker loop catches runner panics, so a join error cannot carry one.
                let _ = h.join();
                continue;
            }
            // Abandoned: never joined, and the shared state (the runner, the slots) is never
            // freed, because the thread may still be executing in it.
            std::mem::forget(h);
            std::mem::forget(Arc::clone(&self.shared));
            q.workers.push(k);
            let current = self.shared.workers[k].current.load(Ordering::Acquire);
            if let Some(strip) = current.checked_sub(1) {
                q.strips.push(strip);
            }
            log::error!("strip worker {k} did not return within {bound:?}; quarantined (strip {:?})", current.checked_sub(1));
        }
        q
    }
}

impl Drop for WorkerSet {
    fn drop(&mut self) {
        if !self.shut {
            let q = self.shutdown(DROP_BOUND);
            if !q.is_empty() {
                log::error!("strip workers quarantined at drop: {q:?}");
            }
        }
    }
}

/// A worker: promote once, then claim, run and complete until stopped, parking when idle.
fn worker_main(shared: &Shared, index: usize, start: usize, promote: bool) {
    let me = &shared.workers[index];
    let promotion = promote.then(crate::promote_audio_thread);
    if let Some(p) = promotion {
        let _ = me.promotion.set(p);
    }
    let mut next = start;
    'run: loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        while let Some((strip, token)) = shared.jobs.try_claim_next(next) {
            me.current.store(strip + 1, Ordering::Relaxed);
            let generation = token.generation();
            // The two clock reads that time the render are also its claim and completion stamps
            // (#169): the wake latency now, the completion stamp before the Release complete.
            let t0 = Instant::now();
            shared.figures.claimed(strip, shared.figures.nanos(t0));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.runner.run(strip, generation)));
            let end = Instant::now();
            shared.record_cost(strip, end.saturating_duration_since(t0));
            shared.figures.done(strip, shared.figures.nanos(end));
            if r.is_err() {
                // Relaxed: published by the completing Release below.
                shared.panicked[strip].store(true, Ordering::Relaxed);
                bump(&shared.stats.runner_panics);
            }
            bump(&shared.stats.worker_runs);
            // Drop the panic payload (if any) before completing, outside the strip's ownership.
            drop(r);
            token.complete();
            me.current.store(0, Ordering::Relaxed);
            // The worker's half of the store-buffering pair (see `WorkerSet::wake`): `current = 0`
            // is ordered before the next sweep's loads of the slot states.
            std::sync::atomic::fence(Ordering::SeqCst);
            next = strip + 1;
            if shared.stop.load(Ordering::Acquire) {
                break 'run;
            }
        }
        // Park until woken. A spurious return of `park` (or an unpark that is not a wake) only
        // loops; only the flag ends the wait, and a wake that lands before the park leaves the
        // flag set (and the unpark token), so none is lost.
        loop {
            if me.woken.swap(false, Ordering::AcqRel) || shared.stop.load(Ordering::Acquire) {
                break;
            }
            std::thread::park();
        }
    }
    if let Some(p) = promotion {
        crate::demote(p);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod stress_tests;

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod figures_tests;

#[cfg(test)]
mod graph_tests;
