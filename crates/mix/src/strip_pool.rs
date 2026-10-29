//! The audio thread's strip fan-out (issue #124 slice 4, ADR 0029): phase-1 strips rendered by a
//! fixed [`WorkerSet`] of `transit-rt`, with the callback's bounded deadline, the deferred-MIDI
//! ring and the stream-fault counters. The default for the live engine: an engine has a pool only
//! after [`MixEngine::start_render_workers`], and uses it only while [`MixEngine::serial_forced`]
//! is off; `TRANSIT_RENDER_POOL=0` renders serially ([`RENDER_POOL_ENV`]). Offline renders never
//! have one.
//!
//! **Cells.** Each strip index below the pool's capacity has a cell: a `Mutex<Job>` holding a
//! spare [`Strip`] and what one strip render reads (the Session, the block's position and
//! flags). To publish strip `i` the callback swaps the table's strip into the cell (the table
//! keeps the spare, with the strip's routing state swapped back in: delay lines, edges, sink
//! routes, meter, latency), then publishes the slot. Whoever runs the strip (a worker through
//! [`StripRunner::run`], or the callback in [`WorkerSet::finish`]'s inline closure) takes the
//! cell's lock with `try_lock`, which never waits: the slot protocol already gives the strip to
//! exactly one thread, so the lock is a checked borrow, and a refused one is a counted fault
//! ([`StripFaults::conflicts`]), never a wait. The mutex is also what orders a worker's writes
//! before the callback's reads, so no `unsafe` is needed. After `finish`, a strip that is
//! `Mixed` or `Inline` is swapped back (the strip is "home") and mixed; one that is `Late` stays
//! in its cell (it is "away") and the next blocks skip it (`Busy`) until its worker is done.
//!
//! **What a worker render reads.** Only the strip and the job: no live input (a strip that reads
//! the hardware input, an External Instrument, a hardware insert or a monitored audio track,
//! renders on the callback while the workers run), its sidechain keys only from its job (a plan
//! with keys renders through the task graph of [`graph`] by default, or serially with
//! `TRANSIT_RENDER_GRAPH=0`: on the graph path the callback fills a keyed strip's keys when its
//! sources are ready and lends them to its job) and no bus (phase 1 has no aux).
//!
//! **While a strip is away** the engine's other entry points cannot reach its instrument: events
//! for it go to its deferred-MIDI buffer ([`StripPool::park`]), and a seek, reset, All Notes
//! Off, transport change or source revocation is recorded as a pending flag and applied when it
//! is back ([`StripPool::bring_home`]). A plan is adopted only when every strip is home
//! ([`MixEngine::render_workers_settled`]).

use super::events::EngineEvents;
use super::render_pool::{release_instrument, reset_render_state};
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};
use transit_rt::midi_defer::{DeferSet, DeferStats, Event};
use transit_rt::worker_set::figures::Figures;
use transit_rt::worker_set::{Config, Outcome, OverrunCause, Published, StripRunner, WorkerSet};

/// The most strips a pool has cells for; strips past it render on the callback.
pub(crate) const POOL_STRIPS: usize = 256;
/// Deferred-MIDI ring capacity per strip (events).
const RING: usize = 64;
/// Live events one away strip takes per block before they are pushed into its ring.
const LIVE: usize = 32;
/// Identity side-table entries per strip: everything the ring and the live buffer can hold.
const SIDE: usize = RING + LIVE;
/// `finish` returning later than this past its deadline counts as an overshoot (the callback
/// started late, or one admitted inline render ran over).
const OVERSHOOT: Duration = Duration::from_micros(100);
/// How long retirement waits for a worker before it quarantines it.
const RETIRE_BOUND: Duration = Duration::from_millis(500);

/// Pending while away: a seek or loop wrap (release the clips' notes, chase controllers).
pub(crate) const PENDING_SEEK: u8 = 1;
/// Pending while away: Stop or panic ([`MixEngine::reset`]).
pub(crate) const PENDING_RESET: u8 = 2;
/// Pending while away: [`MixEngine::all_notes_off`].
pub(crate) const PENDING_NOTES_OFF: u8 = 4;
/// Pending while away: [`MixEngine::set_transport_epoch`].
pub(crate) const PENDING_TRANSPORT: u8 = 8;
/// Pending while away: [`MixEngine::revoke_source`].
pub(crate) const PENDING_REVOKE: u8 = 16;

/// What one strip render reads, and the strip itself while it is away.
struct Job {
    /// The spare while the strip is home; the strip while it is away.
    strip: Strip,
    /// The plan's Session while the strip is away (a clone of the plan's `Arc`, so never the last
    /// owner: a plan is not adopted while a strip is away).
    session: Option<Arc<Session>>,
    pos: Samples,
    frames: usize,
    any_solo: bool,
    monitor_only: bool,
    recording: bool,
    no_suspend: bool,
    frame: u64,
    /// The engine had live input this block (its emptiness is all a worker strip reads of it).
    duplex: bool,
    /// A one-channel stand-in for the live input (never read: such strips render on the callback).
    no_input: Vec<Vec<f32>>,
    busses: BusTable,
    /// The strip's keys while it is away on the graph path (lent by the plan's bank; empty
    /// otherwise), with room for every keyed insert of a strip.
    keys: KeyBank,
    /// The graph generation and task identity of the publish (graph path; 0 otherwise).
    graph: u64,
    task: u64,
    /// What the last render of this cell was for.
    rendered: Stamp,
}

/// What a job's render was for: the block generation, and on the graph path the graph
/// generation and the task identity it was published with (ADR 0029 amendment section 2). A
/// result unlocks its dependents only when all three match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) graph: u64,
    pub(crate) task: u64,
    pub(crate) block: u64,
}

impl Job {
    fn new() -> Self {
        Job {
            strip: Strip::new(),
            session: None,
            pos: 0,
            frames: 0,
            any_solo: false,
            monitor_only: false,
            recording: false,
            no_suspend: false,
            frame: 0,
            duplex: false,
            no_input: vec![Vec::new()],
            busses: BusTable::default(),
            keys: KeyBank::with_room(transit_model::INSERT_SLOTS),
            graph: 0,
            task: 0,
            rendered: Stamp::default(),
        }
    }

    /// Renders the strip for `generation` exactly as the serial path does (a plug-in panic is
    /// kept in the strip by [`guarded`]), and stamps the job with it. `marker` is the callback's
    /// phase marker (#162) when the callback renders the strip itself (inline), `None` on a
    /// worker: only the audio thread writes it.
    fn render(&mut self, generation: u64, marker: Option<&transit_rt::phase_marker::PhaseMarker>) {
        let Job {
            strip,
            session,
            pos,
            frames,
            any_solo,
            monitor_only,
            recording,
            no_suspend,
            frame,
            duplex,
            no_input,
            busses,
            keys,
            graph,
            task,
            rendered,
        } = self;
        if let Some(s) = session.as_deref()
            && let Some(t) = s.tracks.get(usize::from(strip.track))
        {
            let input: &[Vec<f32>] = if *duplex { no_input } else { &[] };
            // The early-key delay of the keys lent with the strip runs now that it renders, once
            // (#174, found by #210); nothing on the plain path (no keys are lent there).
            keys.apply_pending(None);
            let live =
                LiveInput { input, monitor_only: *monitor_only, recording: *recording, no_suspend: *no_suspend, frame: *frame, io: None, marker };
            let block = StripRender { s, busses, live: &live, pos: *pos, frames: *frames, any_solo: *any_solo, keys };
            guarded(strip, |st| process_strip(t, st, &block));
            // Only a strip actually rendered carries this block's generation; the callback mixes
            // nothing else (a counted fault, never stale audio).
            *rendered = Stamp { graph: *graph, task: *task, block: generation };
        }
    }
}

/// The routing state that stays in the table while a strip is away: what `fold_latency`,
/// `route_into` and the meters read, none of which a strip render touches.
fn swap_routing(a: &mut Strip, b: &mut Strip) {
    std::mem::swap(&mut a.tap, &mut b.tap);
    std::mem::swap(&mut a.tap_pre, &mut b.tap_pre);
    std::mem::swap(&mut a.outbuf, &mut b.outbuf);
    std::mem::swap(&mut a.edges, &mut b.edges);
    std::mem::swap(&mut a.sink_routes, &mut b.sink_routes);
    std::mem::swap(&mut a.meter, &mut b.meter);
    std::mem::swap(&mut a.pdc_out, &mut b.pdc_out);
    std::mem::swap(&mut a.pdc_pos, &mut b.pdc_pos);
}

/// Sends `table`'s strip into the cell: the cell holds the strip, the table the spare with the
/// strip's routing state and identity (so the delay walk and the event lookups see it).
fn send_away(table: &mut Strip, cell: &mut Strip) {
    std::mem::swap(table, cell);
    swap_routing(table, cell);
    table.latency = cell.latency;
    table.generation = cell.generation;
    table.track = cell.track;
    table.id = cell.id;
    table.layout = cell.layout;
    table.key_pre = cell.key_pre;
}

/// Brings the strip back from the cell (the inverse of [`send_away`]); the spare goes back to the
/// cell, cleared of the identity it carried.
fn take_home(table: &mut Strip, cell: &mut Strip) {
    swap_routing(table, cell);
    std::mem::swap(table, cell);
    cell.generation = SlotGen::VACANT;
}

/// True when the strip reads the live input or the hardware: it renders on the callback.
fn reads_input(t: &Track, st: &Strip) -> bool {
    !st.hw.is_empty() || st.ext.is_some() || (t.kind == TrackKind::Audio && (t.mixer.input_monitor || t.mixer.record_arm))
}

struct Cells(Box<[Mutex<Job>]>);

impl Cells {
    /// Runs `f` on cell `i`'s job when its lock is free (never waits); false when it was not.
    fn with(&self, i: usize, f: impl FnOnce(&mut Job)) -> bool {
        let Some(cell) = self.0.get(i) else { return false };
        match cell.try_lock() {
            Ok(mut job) => f(&mut job),
            // A panic unwound through a lock holder; the job's data is still whole (every render
            // catches its own panics, see `guarded`).
            Err(TryLockError::Poisoned(p)) => f(&mut p.into_inner()),
            Err(TryLockError::WouldBlock) => return false,
        }
        true
    }
}

/// The counters the callback keeps (relaxed; read by [`StripFaultsReader`]).
#[derive(Default)]
struct Counters {
    workers: AtomicU64,
    blocks: AtomicU64,
    overruns_busy: AtomicU64,
    overruns_late: AtomicU64,
    overruns_unstarted: AtomicU64,
    refused_no_history: AtomicU64,
    refused_worst_case: AtomicU64,
    refused_recent_overrun: AtomicU64,
    late_results: AtomicU64,
    inline_renders: AtomicU64,
    callback_renders: AtomicU64,
    faulted: AtomicU64,
    suspect: AtomicU64,
    conflicts: AtomicU64,
    stale: AtomicU64,
    overrun_blocks: AtomicU64,
    misuse: AtomicU64,
    overshoot: AtomicU64,
    deadline_percent: AtomicU64,
    deadline_frames: AtomicU64,
    deadline_rate: AtomicU64,
    deadline_overridden: AtomicBool,
    /// Strips skipped because a key source missed (#174): the worker set's count, mirrored.
    upstream_missed: AtomicU64,
    /// Auxes skipped because a phase-1 key source missed (#174).
    upstream_tail: AtomicU64,
}

/// Reads an engine's [`StripFaults`] from any thread (relaxed loads; each figure exact on its own).
#[derive(Clone)]
pub struct StripFaultsReader {
    counters: Arc<Counters>,
    defer: Arc<DeferStats>,
    /// The worker set's per-strip figures and histograms (#169; only the figures, never a strip).
    figures: Arc<Figures>,
    /// Per strip index: the track it last published (the phase marker's strip index).
    tracks: Arc<[AtomicU32]>,
    /// Per strip index: blocks in a row its strip has been away (#213).
    held: Arc<[AtomicU32]>,
    /// Per strip index: the id of the track it runs, stored while it is away (#213).
    held_ids: Arc<[AtomicU64]>,
}

/// How long a strip stays away before it counts as held by a hung worker (#213, K89): a late
/// result is back within a block or two, so this is a plug-in that does not return.
pub const HELD_SECS: u32 = 2;

impl StripFaultsReader {
    /// The ids of the tracks whose strips a render worker has
    /// held for [`HELD_SECS`] or more: the plug-in on it does not return from `process`. Relaxed
    /// loads off the audio thread (#213).
    #[must_use]
    pub fn held_tracks(&self) -> Vec<u64> {
        let (frames, rate) = (self.counters.deadline_frames.load(Ordering::Relaxed), self.counters.deadline_rate.load(Ordering::Relaxed));
        if frames == 0 {
            return Vec::new();
        }
        let limit = (u64::from(HELD_SECS) * rate).div_ceil(frames);
        self.held
            .iter()
            .enumerate()
            .filter(|(_, h)| u64::from(h.load(Ordering::Relaxed)) >= limit.max(1))
            .filter_map(|(i, _)| self.held_ids.get(i).map(|t| t.load(Ordering::Relaxed)))
            .collect()
    }

    /// The figures now.
    #[must_use]
    pub fn snapshot(&self) -> StripFaults {
        let c = &self.counters;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let d = self.defer.total();
        StripFaults {
            workers: r(&c.workers),
            deadline_percent: r(&c.deadline_percent),
            deadline_frames: r(&c.deadline_frames),
            deadline_rate: r(&c.deadline_rate),
            deadline_overridden: c.deadline_overridden.load(Ordering::Relaxed),
            blocks: r(&c.blocks),
            overruns_busy: r(&c.overruns_busy),
            overruns_late: r(&c.overruns_late),
            overruns_unstarted: r(&c.overruns_unstarted),
            late_results: r(&c.late_results),
            inline_renders: r(&c.inline_renders),
            inline_refused_no_history: r(&c.refused_no_history),
            inline_refused_worst_case: r(&c.refused_worst_case),
            inline_refused_recent_overrun: r(&c.refused_recent_overrun),
            callback_renders: r(&c.callback_renders),
            faulted: r(&c.faulted),
            suspect: r(&c.suspect),
            conflicts: r(&c.conflicts).saturating_add(r(&c.stale)).saturating_add(r(&c.misuse)),
            overrun_blocks: r(&c.overrun_blocks),
            deadline_overshoots: r(&c.overshoot),
            midi_deferred: d.deferred,
            midi_dropped: d.dropped,
            midi_silences: d.silences,
            upstream_missed: r(&c.upstream_missed).saturating_add(r(&c.upstream_tail)),
            top_strips: self.figures.top_overruns::<TOP_STRIPS>().map(|t| {
                t.map(|(i, f)| StripFigure {
                    track: self.tracks.get(i).map_or(u32::MAX, |t| t.load(Ordering::Relaxed)),
                    upstream_missed: f.upstream_missed,
                    upstream_origin: f.upstream_origin.and_then(|o| self.tracks.get(o)).map_or(u32::MAX, |t| t.load(Ordering::Relaxed)),
                    overruns_busy: f.overruns_busy,
                    overruns_late: f.overruns_late,
                    overruns_unstarted: f.overruns_unstarted,
                    inline_renders: f.inline_renders,
                    cost_ns: f.cost_ns,
                    worst_ns: f.worst_ns,
                })
            }),
            lateness: self.figures.lateness(),
            wake: self.figures.wake(),
        }
    }
}

/// The worker side: renders a strip from its cell.
struct Runner {
    cells: Arc<Cells>,
    counters: Arc<Counters>,
}

impl StripRunner for Runner {
    fn run(&self, strip: usize, generation: u64) {
        // A worker only ever renders for the audio thread (the logger never blocks there).
        mark_audio_thread();
        #[cfg(test)]
        tests::late_panic(strip);
        // A worker never writes the callback's phase marker (#162).
        if !self.cells.with(strip, |job| {
            job.render(generation, None);
            #[cfg(test)]
            tests::corrupt_stamp(strip, &mut job.rendered);
        }) {
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One block's inputs to [`StripPool::render`].
pub(crate) struct PoolBlock<'a> {
    pub(crate) session: &'a Arc<Session>,
    pub(crate) busses: &'a BusTable,
    pub(crate) keys: &'a KeyBank,
    pub(crate) live: &'a LiveInput<'a>,
    pub(crate) pos: Samples,
    pub(crate) frames: usize,
    pub(crate) any_solo: bool,
    pub(crate) sample_rate: u32,
    /// When the block's render began (the deadline is measured from it).
    pub(crate) start: Instant,
    /// Overrides the strips' share of the period ([`MixEngine::set_strip_deadline`]).
    pub(crate) budget: Option<Duration>,
    /// The deadline from the callback's entry ([`MixEngine::set_block_deadline`]), else
    /// `start` plus the budget.
    pub(crate) deadline: Option<Instant>,
}

/// An engine's render workers and everything the fan-out keeps per strip. See the module docs.
pub(crate) struct StripPool {
    set: WorkerSet,
    cells: Arc<Cells>,
    defer: DeferSet,
    counters: Arc<Counters>,
    /// Per strip: the track index it last published, for [`StripFaultsReader`] (#169).
    tracks: Arc<[AtomicU32]>,
    /// Per strip: blocks in a row it has been away (the audio thread stores; #213).
    held: Arc<[AtomicU32]>,
    /// Per strip index: the id of the track it runs, stored while it is away (#213).
    held_ids: Arc<[AtomicU64]>,
    /// Per strip: in its cell (a worker may hold it).
    away: Box<[bool]>,
    /// Per strip: what to apply when it is back (`PENDING_*`).
    pending: Box<[u8]>,
    /// Per strip, while away: its instrument's generation (`None`: no instrument).
    binding: Box<[Option<SlotGen>]>,
    /// Per strip: this block's live events while away ([`LIVE`] each), tagged into `side`.
    live: Box<[Event]>,
    live_len: Box<[usize]>,
    /// Per strip: the identity of each tagged event ([`SIDE`] each).
    side: Box<[Option<InstrumentEvent>]>,
    /// Per strip: no output this block (an overrun, a fault, a suspect instance).
    silent: Box<[bool]>,
    /// Per strip: the instance generation that faulted (cleared when the strip holds another).
    suspect_gen: Box<[SlotGen]>,
    /// Per strip: the callback renders it itself this block (it reads the live input, or its
    /// cell refused).
    on_callback: Box<[bool]>,
    generation: u64,
    /// Strips silent by an overrun in the last block.
    last_overruns: u32,
    /// Per strip: the identity of the processing chain the cell last published ([`chain_identity`];
    /// `None` before its first publish). When it changes, the strip's cost history is reset.
    chain: Box<[Option<u64>]>,
    /// The strips' deadline in effect, percent of the block period (#172):
    /// [`resolve_strip_deadline_percent`] of `deadline_override` and `device_block`.
    deadline_percent: u32,
    /// [`STRIP_DEADLINE_PERCENT_ENV`]'s value, read once before construction (never here).
    deadline_override: Option<u32>,
    /// The device callback the share was last resolved for: frames, rate (Hz).
    device_block: (u32, u32),
    /// Test seam (issue #210): the strips the test holds this block (`seam`).
    #[cfg(feature = "testing")]
    pub(crate) gate: Vec<bool>,
}

impl StripPool {
    /// `workers` threads for `strips` cells, the strips' deadline resolved from
    /// `deadline_override` and a callback of `frames` at `rate` until
    /// [`StripPool::set_device_block`] says otherwise (control thread; allocates).
    pub(crate) fn new(workers: usize, strips: usize, promote: bool, deadline_override: Option<u32>, frames: u32, rate: u32) -> std::io::Result<Self> {
        let cells = Arc::new(Cells((0..strips).map(|_| Mutex::new(Job::new())).collect()));
        let counters = Arc::new(Counters::default());
        let runner = Arc::new(Runner { cells: Arc::clone(&cells), counters: Arc::clone(&counters) });
        let set = WorkerSet::new(Config { workers, strips, promote }, runner)?;
        counters.workers.store(set.workers() as u64, Ordering::Relaxed);
        counters.deadline_overridden.store(deadline_override.is_some(), Ordering::Relaxed);
        let mut pool = StripPool {
            set,
            cells,
            defer: DeferSet::new(strips, RING),
            counters,
            tracks: (0..strips).map(|i| AtomicU32::new(u32::try_from(i).unwrap_or(u32::MAX))).collect(),
            held: (0..strips).map(|_| AtomicU32::new(0)).collect(),
            held_ids: (0..strips).map(|_| AtomicU64::new(u64::MAX)).collect(),
            away: vec![false; strips].into_boxed_slice(),
            pending: vec![0; strips].into_boxed_slice(),
            binding: vec![None; strips].into_boxed_slice(),
            live: vec![Event::default(); strips * LIVE].into_boxed_slice(),
            live_len: vec![0; strips].into_boxed_slice(),
            side: vec![None; strips * SIDE].into_boxed_slice(),
            silent: vec![false; strips].into_boxed_slice(),
            suspect_gen: vec![SlotGen::VACANT; strips].into_boxed_slice(),
            on_callback: vec![false; strips].into_boxed_slice(),
            generation: 0,
            last_overruns: 0,
            chain: vec![None; strips].into_boxed_slice(),
            deadline_percent: 0,
            deadline_override,
            device_block: (0, 0),
            #[cfg(feature = "testing")]
            gate: Vec::new(),
        };
        pool.set_device_block(frames, rate);
        Ok(pool)
    }

    pub(crate) fn workers(&self) -> usize {
        self.set.workers()
    }

    /// The strips' deadline in effect, percent of the block period.
    pub(crate) fn deadline_percent(&self) -> u32 {
        self.deadline_percent
    }

    /// The device's callback is `frames` long at `rate` Hz (#172): resolves the share again when
    /// either differs from the callback it was last resolved for (a device opened at another
    /// buffer size or rate than the engine's stand-in, a driver that changed its buffer). The
    /// override, read before construction, still wins. A compare of both and, on a change, three
    /// relaxed stores: allocation-free, no environment or clock read; any thread that owns the
    /// engine, the audio thread included. Zero frames or a zero rate is ignored.
    pub(crate) fn set_device_block(&mut self, frames: u32, rate: u32) {
        self.resolve_share(frames, rate, false);
    }

    /// Replaces the override the pool was built with (tests: `None` makes the rule decide whatever
    /// the process environment says) and resolves the share again for the device's callback.
    pub(crate) fn set_deadline_override(&mut self, overridden: Option<u32>) {
        self.deadline_override = overridden;
        self.counters.deadline_overridden.store(overridden.is_some(), Ordering::Relaxed);
        let (frames, rate) = self.device_block;
        self.resolve_share(frames, rate, true);
    }

    fn resolve_share(&mut self, frames: u32, rate: u32, again: bool) {
        if frames == 0 || rate == 0 || (!again && (frames, rate) == self.device_block) {
            return;
        }
        self.device_block = (frames, rate);
        self.deadline_percent = resolve_strip_deadline_percent(self.deadline_override, frames, rate);
        self.counters.deadline_percent.store(u64::from(self.deadline_percent), Ordering::Relaxed);
        self.counters.deadline_frames.store(u64::from(frames), Ordering::Relaxed);
        self.counters.deadline_rate.store(u64::from(rate), Ordering::Relaxed);
    }

    /// The strips' budget of one block of `frames` at `rate`, measured from the block's start:
    /// the pool's share of the period (#172). Allocation-free.
    pub(crate) fn strip_budget(&self, frames: usize, rate: u32) -> Duration {
        strip_deadline_share(block_period(frames, rate), self.deadline_percent)
    }

    /// Seeds every cell's cost history with `cost` (zero clears it); see
    /// [`MixEngine::seed_strip_costs`]. Between blocks only. A cell whose chain changes later is
    /// reset as usual.
    pub(crate) fn seed_costs(&mut self, cost: Duration) {
        for i in 0..self.set.strips() {
            self.set.set_cost_estimate(i, cost);
        }
    }

    pub(crate) fn promotions(&self) -> Vec<Option<transit_rt::Promotion>> {
        (0..self.set.workers()).map(|k| self.set.promotion(k)).collect()
    }

    pub(crate) fn reader(&self) -> StripFaultsReader {
        StripFaultsReader {
            counters: Arc::clone(&self.counters),
            defer: Arc::clone(self.defer.stats()),
            figures: self.set.figures(),
            tracks: Arc::clone(&self.tracks),
            held: Arc::clone(&self.held),
            held_ids: Arc::clone(&self.held_ids),
        }
    }

    /// The strips the pool has cells for (strips past them render on the callback).
    pub(crate) fn capacity(&self) -> usize {
        self.away.len()
    }

    /// Strip `i` is in its cell.
    pub(crate) fn is_away(&self, i: usize) -> bool {
        self.away.get(i).copied().unwrap_or(false)
    }

    /// Some strip is in its cell.
    pub(crate) fn any_away(&self) -> bool {
        self.away.iter().any(|&a| a)
    }

    /// Records `flag` for every strip that is away, to apply when it is back.
    pub(crate) fn mark_away(&mut self, flag: u8) {
        for (p, &a) in self.pending.iter_mut().zip(self.away.iter()) {
            if a {
                *p |= flag;
            }
        }
    }

    /// Strips silent by an overrun in the last block rendered.
    pub(crate) fn last_overruns(&self) -> u32 {
        self.last_overruns
    }

    /// Strip `i` has no output this block.
    pub(crate) fn is_silent(&self, i: usize) -> bool {
        self.silent.get(i).copied().unwrap_or(false)
    }

    /// [`MixEngine::queue_event`] for strip `i` while it is away: the checks that need no
    /// instrument, then the event goes into the strip's live buffer for the next block, tagged
    /// with its identity. `Queued` once taken (playback must not see a release refused).
    ///
    /// `home` is the strip's live instrument generation when the strip is home (it parks only
    /// behind events it still holds); the binding is checked against it, not against the one
    /// stored when the strip was sent away (an adoption may have put another instance there).
    pub(crate) fn park(
        &mut self,
        i: usize,
        ev: &InstrumentEvent,
        events: &EngineEvents,
        block_len: usize,
        home: Option<Option<SlotGen>>,
    ) -> (EventOutcome, Option<u64>) {
        let (_, slot, generation) = ev.target;
        let current = if self.is_away(i) { self.binding.get(i).copied().flatten() } else { home.flatten() };
        match current {
            Some(g) if slot == INSTRUMENT_SLOT && generation != SlotGen::VACANT && g == generation => {}
            Some(_) if slot == INSTRUMENT_SLOT => return (EventOutcome::StaleBinding, None),
            _ => return (EventOutcome::NoInstrument, None),
        }
        if let Err(o) = events.transport_ok(ev) {
            return (o, None);
        }
        let next = events.next_frame;
        let due = match ev.due {
            Due::Next => next,
            Due::Frame(RenderFrame(f)) => f,
        };
        let last = u32::try_from(block_len.saturating_sub(1)).unwrap_or(u32::MAX);
        let offset = u32::try_from(due.saturating_sub(next)).unwrap_or(u32::MAX).min(last);
        if self.live_len.get(i).is_some_and(|&n| n >= LIVE) {
            // The buffer is full: what it holds goes into the ring now, for the coming block.
            self.flush_live(i, self.generation + 1);
        }
        let Some(tag) = self.tag(i, ev) else {
            // Unreachable: the side table holds what the ring and the live buffer can.
            return (EventOutcome::Refused(Shed::QueueFull), None);
        };
        let e = to_event(ev, offset).with_tag(tag);
        if let (Some(n), Some(buf)) = (self.live_len.get_mut(i), self.live.get_mut(i * LIVE..(i + 1) * LIVE))
            && let Some(slot) = buf.get_mut(*n)
        {
            *slot = e;
            *n += 1;
        }
        (EventOutcome::Queued, Some(due.max(next)))
    }

    /// A free side-table entry of strip `i` holding `ev`, its index the tag. Entries whose event
    /// the ring dropped are collected first when none is free.
    fn tag(&mut self, i: usize, ev: &InstrumentEvent) -> Option<u32> {
        for pass in 0..2 {
            if pass == 1 {
                self.collect(i);
            }
            let side = self.side.get_mut(i * SIDE..(i + 1) * SIDE)?;
            if let Some((k, e)) = side.iter_mut().enumerate().find(|(_, e)| e.is_none()) {
                *e = Some(*ev);
                return u32::try_from(k).ok();
            }
        }
        None
    }

    /// Frees every side-table entry of strip `i` that neither the ring nor the live buffer holds.
    fn collect(&mut self, i: usize) {
        let Self { side, defer, live, live_len, .. } = self;
        let (Some(side), Some(ring)) = (side.get_mut(i * SIDE..(i + 1) * SIDE), defer.ring(i)) else { return };
        let held = live.get(i * LIVE..i * LIVE + live_len.get(i).copied().unwrap_or(0)).unwrap_or(&[]);
        for (k, e) in side.iter_mut().enumerate() {
            let k = u32::try_from(k).unwrap_or(u32::MAX);
            if !ring.events().iter().chain(held).any(|x| x.tag == k) {
                *e = None;
            }
        }
    }

    /// Pushes strip `i`'s live buffer into its ring as block `generation`'s events.
    fn flush_live(&mut self, i: usize, generation: u64) {
        let n = self.live_len.get(i).copied().unwrap_or(0);
        if n == 0 {
            return;
        }
        if let Some(events) = self.live.get(i * LIVE..i * LIVE + n) {
            self.defer.defer_block(i, generation, events);
        }
        if let Some(l) = self.live_len.get_mut(i) {
            *l = 0;
        }
    }

    /// True when strip `i` has deferred MIDI to deliver.
    fn has_parked(&self, i: usize) -> bool {
        self.live_len.get(i).is_some_and(|&n| n > 0) || self.defer.ring(i).is_some_and(|r| !r.is_empty() || r.is_silenced())
    }

    /// Delivers strip `i`'s deferred MIDI, then the live events taken while it was away, into
    /// its instrument's queue (ADR 0029: after `begin_block`, before the strip's `publish`): each
    /// event is admitted again with its identity, due no earlier than where the ring lands it;
    /// a silence releases everything the instrument holds.
    fn deliver(&mut self, i: usize, st: &mut Strip, now: u64, b: &BlockClock, events: &mut EngineEvents, s: &Session) {
        if !self.has_parked(i) {
            return;
        }
        let n = self.live_len.get(i).copied().unwrap_or(0);
        let Self { defer, live, side, .. } = self;
        let live = live.get(i * LIVE..i * LIVE + n).unwrap_or(&[]);
        let side = side.get_mut(i * SIDE..(i + 1) * SIDE).unwrap_or(&mut []);
        let mut silenced = false;
        let block_len = u32::try_from(b.frames).unwrap_or(u32::MAX);
        // `render_active` has already moved the render clock past this block; an event admitted
        // now is due in this block, so it is admitted against this block's first frame.
        let next = std::mem::replace(&mut events.next_frame, b.frame);
        defer.deliver(i, now, block_len, live, &mut |e: Event| {
            if e.tag == Event::NO_TAG {
                if !silenced {
                    silenced = true;
                    release_instrument(st);
                }
                return;
            }
            let Some(ev) = side.get_mut(e.tag as usize).and_then(Option::take) else { return };
            let landed = b.frame.saturating_add(u64::from(e.offset));
            let due = match ev.due {
                Due::Frame(RenderFrame(f)) => f.max(landed),
                Due::Next => landed,
            };
            events.readmit(s, st, &InstrumentEvent { due: Due::Frame(RenderFrame(due)), ..ev });
        });
        events.next_frame = next;
        if let Some(l) = self.live_len.get_mut(i) {
            *l = 0;
        }
    }

    /// Takes strip `i` back from its cell into `table` and applies what happened while it was
    /// away. False when the cell's lock was not free (a counted fault; the strip stays away).
    fn bring_home(&mut self, i: usize, table: &mut Strip, events: &EngineEvents) -> bool {
        let mut session = None;
        let ok = self.cells.with(i, |job| {
            take_home(table, &mut job.strip);
            session = job.session.take();
        });
        if !ok {
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // The plan still holds its own `Arc`: this drop is never the last owner.
        drop(session);
        if let Some(a) = self.away.get_mut(i) {
            *a = false;
        }
        // Back from its worker: a strip that is only late every block is not held (#213).
        if let Some(h) = self.held.get(i) {
            h.store(0, Ordering::Relaxed);
        }
        // Kept current while the strip is home: events still parked for it (the ring's
        // end-of-block carry) keep new ones parking behind them, checked against it.
        if let Some(b) = self.binding.get_mut(i) {
            *b = table.instrument.as_ref().map(|inst| inst.generation);
        }
        let flags = self.pending.get_mut(i).map_or(0, std::mem::take);
        if flags != 0 {
            apply_pending(table, flags, events);
        }
        true
    }

    /// Before a plan is adopted (every strip home): delivers every strip's deferred MIDI into its
    /// instrument now, so the events travel with the instance whatever the new plan moves (a
    /// strip's index may change). Events the end-of-block rule still carries stay (a new strip
    /// at that index refuses them as a stale binding).
    pub(crate) fn flush_parked(&mut self, strips: &mut [Strip], events: &mut EngineEvents, s: &Session, block_len: usize) {
        let clock = BlockClock { frame: events.next_frame, frames: block_len };
        let g = self.generation + 1;
        for (i, st) in strips.iter_mut().enumerate().take(self.away.len()) {
            if !self.is_away(i) {
                self.deliver(i, st, g, &clock, events, s);
            }
        }
    }

    /// Stop, panic or All Notes Off ([`MixEngine::reset`], [`MixEngine::all_notes_off`]): the
    /// deferred MIDI of every strip is dropped, not delivered after the release (a parked
    /// note-on would start a note after Stop). Events taken later are kept.
    pub(crate) fn discard_parked(&mut self) {
        for i in 0..self.away.len() {
            if self.has_parked(i) {
                // Drained into nothing (a silence's mask is cleared with it).
                self.defer.drain_into(i, self.generation + 1, u32::MAX, &mut |_| {});
                if let Some(n) = self.live_len.get_mut(i) {
                    *n = 0;
                }
                if let Some(side) = self.side.get_mut(i * SIDE..(i + 1) * SIDE) {
                    side.fill(None);
                }
            }
        }
    }

    /// Between blocks: brings home every away strip whose worker is done (its slot no longer
    /// `Claimed`; the cell's lock orders the worker's writes) and delivers what was parked for it
    /// into its instrument at once, so an event that arrives next (admitted directly, the strip
    /// being home) lands behind the older ones: a note-off is never admitted ahead of its parked
    /// note-on. True when every strip is home.
    pub(crate) fn settle(&mut self, strips: &mut [Strip], keys: &mut KeyBank, events: &mut EngineEvents, s: &Session, block_len: usize) -> bool {
        let clock = BlockClock { frame: events.next_frame, frames: block_len };
        let g = self.generation + 1;
        let mut home = true;
        for (i, st) in strips.iter_mut().enumerate().take(self.away.len()) {
            if !self.is_away(i) {
                continue;
            }
            let claimed = self.set.jobs().slot(i).is_some_and(|s| s.state() == transit_rt::strip_jobs::State::Claimed);
            if claimed || !self.bring_home(i, st, events) {
                home = false;
                continue;
            }
            // Its keys too, when it went away on the graph path.
            self.take_keys_back(i, keys);
            self.deliver(i, st, g, &clock, events, s);
        }
        home
    }

    /// The serial path (render workers on, but this block not through them): delivers what is
    /// still parked for every home strip (the ring's end-of-block carry, events parked behind it)
    /// against this block's clock before the strips render, so an instrument never stops
    /// receiving because the next blocks took the serial path.
    pub(crate) fn deliver_home(&mut self, strips: &mut [Strip], events: &mut EngineEvents, s: &Session, frame: u64, frames: usize) {
        if !(0..self.away.len()).any(|i| self.has_parked(i)) {
            return;
        }
        // A block of its own for the rings' clock (`begin_block` only needs newer generations).
        self.generation += 1;
        let g = self.generation;
        let clock = BlockClock { frame, frames };
        for (i, st) in strips.iter_mut().enumerate().take(self.away.len()) {
            if !self.is_away(i) {
                self.deliver(i, st, g, &clock, events, s);
            }
        }
    }

    /// Strip `i` is away, or still has deferred MIDI (the ring's end-of-block carry): an event
    /// for it is parked behind what it holds, never admitted ahead of it.
    pub(crate) fn parks(&self, i: usize) -> bool {
        self.is_away(i) || self.has_parked(i)
    }

    /// Phase 1 of one block through the worker set (see the module docs and ADR 0029):
    /// `begin_block`, bring home what is done, per strip deliver its MIDI, send it away and
    /// `publish`, `wake`, render the callback's own strips, `finish` by the deadline, bring home
    /// and mix only `Mixed` and `Inline`, silence and count the rest, defer the MIDI of `Busy`
    /// strips. Allocation-free; never waits on a worker past the deadline.
    pub(crate) fn render(&mut self, b: &PoolBlock<'_>, p1: &mut [Strip], events: &mut EngineEvents) {
        let s: &Session = b.session;
        let g = self.generation + 1;
        let clock = BlockClock { frame: b.live.frame, frames: b.frames };
        let cells = self.away.len();
        if !self.set.begin_block(g) {
            // Refused (a protocol fault): render what is home serially, leave the rest silent.
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            for (i, st) in p1.iter_mut().enumerate() {
                if let Some(t) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t))
                    && !self.is_away(i)
                {
                    guarded(st, |st| process_strip(t, st, &b.render(s)));
                }
            }
            self.note_held(s);
            return;
        }
        self.generation = g;
        let mut published = 0usize;
        for (i, st) in p1.iter_mut().enumerate() {
            if i < cells {
                self.silent[i] = false;
                self.on_callback[i] = false;
                // A worker finished since the last block: its (late) result was retired unread.
                if self.away[i] && !self.set.is_busy(i) {
                    self.bring_home(i, st, events);
                }
                if !self.away[i] {
                    self.deliver(i, st, g, &clock, events, s);
                }
            }
            let Some(t) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t)) else { continue };
            if i >= cells {
                continue;
            }
            if !self.away[i] && reads_input(t, st) {
                self.on_callback[i] = true;
                continue;
            }
            if self.away[i] {
                // A worker still holds it: the per-block rule skips it (`Overrun(Busy)`). (Away
                // but not busy only after a refused borrow: never published with a stale job.)
                if self.set.is_busy(i) {
                    let _ = self.set.publish(i);
                }
                self.silent[i] = true;
                continue;
            }
            if !self.admit(i, st) {
                continue;
            }
            if self.send(i, st, b) {
                if let Ok(Published::Published) = self.set.publish(i) {
                    published += 1;
                } else {
                    // Refused (a protocol fault): take it back and render it here.
                    self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
                    self.bring_home(i, st, events);
                    self.on_callback[i] = true;
                }
            } else {
                self.on_callback[i] = true;
            }
        }
        self.set.wake(published);
        // The callback's own strips, while the workers render: those that read the live input,
        // those a cell refused, and those past the cells.
        let mut callback = 0u64;
        for (i, st) in p1.iter_mut().enumerate() {
            if i < cells && !self.on_callback[i] {
                continue;
            }
            if let Some(t) = s.tracks.get(usize::from(st.track)).filter(|t| live(t) && !is_aux(t)) {
                guarded(st, |st| process_strip(t, st, &b.render(s)));
                callback += 1;
            }
        }
        self.counters.callback_renders.fetch_add(callback, Ordering::Relaxed);
        let deadline = self.deadline(b);
        let cells_ref = &self.cells;
        let report = self.set.finish(deadline, |i| {
            // Reclaimed by the callback: no worker can hold the cell.
            // On the callback: the strip marks the phase marker as a serial render does.
            cells_ref.with(i, |job| job.render(g, b.live.marker));
        });
        if Instant::now().saturating_duration_since(deadline) > OVERSHOOT {
            self.counters.overshoot.fetch_add(1, Ordering::Relaxed);
        }
        let mut overruns = 0u32;
        for (i, st) in p1.iter_mut().enumerate().take(cells) {
            let Some(outcome) = self.set.outcome(i) else { continue };
            match outcome {
                Outcome::Mixed | Outcome::Inline => {
                    let mut rendered = 0;
                    self.cells.with(i, |job| rendered = job.rendered.block);
                    if self.bring_home(i, st, events) && rendered != g {
                        // A result of another block (a protocol fault): never mixed.
                        self.counters.stale.fetch_add(1, Ordering::Relaxed);
                        self.silent[i] = true;
                    }
                }
                Outcome::Overrun(cause) => {
                    overruns += 1;
                    self.silent[i] = true;
                    match cause {
                        // Taken back unrendered: home again, its queue keeps this block's events.
                        OverrunCause::Unstarted => {
                            self.bring_home(i, st, events);
                        }
                        // The worker keeps it; the next blocks skip it until it is done.
                        OverrunCause::Late => {}
                        OverrunCause::Busy => {}
                        // Only the graph path skips a strip for its source (`strip_pool::graph`).
                        OverrunCause::UpstreamMissed => {}
                    }
                }
                Outcome::Faulted => {
                    self.bring_home(i, st, events);
                    self.silent[i] = true;
                    self.suspect_gen[i] = st.generation;
                    self.counters.faulted.fetch_add(1, Ordering::Relaxed);
                }
                Outcome::NotPublished | Outcome::Pending => {}
            }
            // Only `Busy` and `Unstarted` defer; an `Unstarted` strip's events are already in its
            // queue (delivered before its publish), so its live buffer is empty.
            let n = self.live_len[i];
            if n > 0 && self.defer.defer_for(i, g, outcome, &self.live[i * LIVE..i * LIVE + n]) {
                self.live_len[i] = 0;
            }
        }
        self.last_overruns = overruns;
        self.note_held(s);
        self.mirror(report.overrun > 0);
    }

    /// Before strip `i` (home, its track live) is sent away: a suspect instance stays silent for
    /// the block (false, counted); a replaced one is cleared; a new chain on the index starts
    /// with no cost history; the report's name follows the track on the index.
    fn admit(&mut self, i: usize, st: &Strip) -> bool {
        if self.set.is_suspect(i) {
            // A late result that panicked made it suspect at `begin_block`: this instance.
            if self.suspect_gen[i] == SlotGen::VACANT {
                self.suspect_gen[i] = st.generation;
            }
            if st.generation == self.suspect_gen[i] {
                self.silent[i] = true;
                self.counters.suspect.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            // The instance was replaced (ADR 0007: per plug-in, by the next plan).
            self.set.clear_suspect(i);
            self.suspect_gen[i] = SlotGen::VACANT;
        }
        // Another chain on this index (a plug-in loaded or replaced, another track's strip
        // moved here): its cost history is not this one's (#168, ADR 0029 deviation 16).
        let id = chain_identity(st);
        if self.chain[i].is_some_and(|prev| prev != id) {
            self.set.reset_history(i);
        }
        self.chain[i] = Some(id);
        // The name the report gives this index's figures (#169): a store only on a change,
        // and another track's counts start from zero, so a name's counts are its track's.
        let track = u32::from(st.track);
        if self.tracks[i].load(Ordering::Relaxed) != track {
            self.tracks[i].store(track, Ordering::Relaxed);
            self.set.reset_counts(i);
        }
        true
    }

    /// The strips' deadline of block `b`. A test's budget override wins; then the callback's
    /// deadline (playback computes it with this pool's share, `MixEngine::strip_deadline_percent`);
    /// then the block's entry. One deadline: `finish` waits to it and its inline admission
    /// measures the time left to it.
    fn deadline(&self, b: &PoolBlock<'_>) -> Instant {
        let budget = b.budget.unwrap_or_else(|| self.strip_budget(b.frames, b.sample_rate));
        match (b.budget, b.deadline) {
            (None, Some(d)) => d,
            _ => b.start + budget.min(Duration::from_secs(3600)),
        }
    }

    /// Sends strip `i` away into its cell with the block's inputs. False when the cell's lock was
    /// not free (a counted fault; the strip stays home and renders on the callback).
    fn send(&mut self, i: usize, st: &mut Strip, b: &PoolBlock<'_>) -> bool {
        let binding = st.instrument.as_ref().map(|inst| inst.generation);
        let ok = self.cells.with(i, |job| {
            send_away(st, &mut job.strip);
            job.session = Some(Arc::clone(b.session));
            job.pos = b.pos;
            job.frames = b.frames;
            job.any_solo = b.any_solo;
            job.monitor_only = b.live.monitor_only;
            job.recording = b.live.recording;
            job.no_suspend = b.live.no_suspend;
            job.frame = b.live.frame;
            job.duplex = !b.live.input.is_empty();
            job.graph = 0;
            job.task = 0;
        });
        if !ok {
            self.counters.conflicts.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.away[i] = true;
        self.binding[i] = binding;
        true
    }

    /// Once per block, both render paths: counts the blocks each strip has now been away in a row
    /// and names the track a held strip runs (#213). Audio thread: relaxed loads, a store only on
    /// a change, no allocation. The track is the id in the plan the strip was sent away under,
    /// which is the session of every block while it is away (a held strip refuses plan changes).
    fn note_held(&self, s: &Session) {
        for (i, (h, &away)) in self.held.iter().zip(self.away.iter()).enumerate() {
            let now = h.load(Ordering::Relaxed);
            let n = if away { now.saturating_add(1) } else { 0 };
            if n != now {
                h.store(n, Ordering::Relaxed);
            }
            if away && let (Some(id), Some(t)) = (self.held_ids.get(i), s.tracks.get(self.tracks[i].load(Ordering::Relaxed) as usize)) {
                id.store(t.id.0, Ordering::Relaxed);
            }
        }
    }

    /// Copies the worker set's running totals into the counters the reader sees.
    fn mirror(&self, overrun: bool) {
        let st = self.set.stats();
        let c = &self.counters;
        c.blocks.store(st.blocks, Ordering::Relaxed);
        c.overruns_busy.store(st.overruns_busy, Ordering::Relaxed);
        c.overruns_late.store(st.overruns_late, Ordering::Relaxed);
        c.overruns_unstarted.store(st.overruns_unstarted, Ordering::Relaxed);
        c.late_results.store(st.late_results, Ordering::Relaxed);
        c.inline_renders.store(st.inline_renders, Ordering::Relaxed);
        c.refused_no_history.store(st.inline_refused_no_history, Ordering::Relaxed);
        c.refused_worst_case.store(st.inline_refused_worst_case, Ordering::Relaxed);
        c.refused_recent_overrun.store(st.inline_refused_recent_overrun, Ordering::Relaxed);
        c.misuse.store(st.misuse, Ordering::Relaxed);
        c.upstream_missed.store(st.upstream_missed, Ordering::Relaxed);
        if overrun {
            c.overrun_blocks.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Control thread, at engine retirement: shuts the workers down within a bound and brings
    /// every strip home. A worker that does not return is quarantined: its strip stays in its
    /// cell, which is leaked with the worker's state and never freed or reused, and the fault is
    /// logged with the track and its plug-ins.
    pub(crate) fn retire(mut self, strips: &mut [Strip], session: &Session) {
        let q = self.set.shutdown(RETIRE_BOUND);
        for (i, st) in strips.iter_mut().enumerate().take(self.away.len()) {
            if !self.away[i] {
                continue;
            }
            if q.strips.contains(&i) || !self.cells.with(i, |job| take_home(st, &mut job.strip)) {
                let track = session.tracks.get(usize::from(st.track));
                let name = track.map_or("?", |t| t.name.as_str());
                let plugins: Vec<&str> = track
                    .map(|t| t.instrument.iter().chain(t.mixer.inserts.iter().flatten()).map(|x| x.plugin.as_str()).collect())
                    .unwrap_or_default();
                log::error!("render worker quarantined with track {name:?} (plug-ins {plugins:?}); its instances are never freed");
                continue;
            }
            self.away[i] = false;
        }
    }
}

/// The identity of `st`'s processing chain: the strip state's generation and the generation of
/// its instrument and of each insert, folded into one word (FNV-1a over the words). Every
/// generation comes from the compiler's one counter, so a new instance, a moved strip or a slot
/// emptied changes it. Allocation-free.
fn chain_identity(st: &Strip) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
    mix(st.generation.get());
    mix(st.instrument.as_ref().map_or(0, |p| p.generation.get()));
    for p in &st.plugins {
        mix(p.as_ref().map_or(0, |p| p.generation.get()));
    }
    h
}

/// Where a block starts on the render clock, and its length.
struct BlockClock {
    frame: u64,
    frames: usize,
}

impl PoolBlock<'_> {
    /// The [`StripRender`] the callback renders a strip with.
    fn render<'a>(&'a self, s: &'a Session) -> StripRender<'a> {
        StripRender { s, busses: self.busses, live: self.live, pos: self.pos, frames: self.frames, any_solo: self.any_solo, keys: self.keys }
    }
}

/// One block's period at `rate` (a 1 ms floor for an unknown rate).
fn block_period(frames: usize, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::from_millis(1);
    }
    Duration::from_nanos(u64::try_from(frames).unwrap_or(u64::MAX).saturating_mul(1_000_000_000) / u64::from(rate))
}

/// The ring's view of an event: the status byte and data bytes its rules classify by (note on
/// and off, the release class, the channel), at `offset`. The identity travels in the tag.
fn to_event(ev: &InstrumentEvent, offset: u32) -> Event {
    let ch = ev.source.channel & 0x0F;
    match ev.message {
        Message::NoteOn { key, velocity, .. } => Event::note_on(ch, key, velocity, offset),
        Message::NoteOff { key, velocity, .. } => Event::note_off(ch, key, velocity, offset),
        Message::Control { cc, value } => Event::control(ch, cc, value, offset),
        m => match m.midi1(ch) {
            [Some([s, d1, d2]), _] => Event::new(s, d1, d2, offset),
            _ => Event::new(0xF0, 0, 0, offset),
        },
    }
}

/// What the engine did to its instruments while strip `st` was away, in the order the engine
/// would have done it.
fn apply_pending(st: &mut Strip, flags: u8, events: &EngineEvents) {
    if flags & PENDING_TRANSPORT != 0
        && let Some(epoch) = events.transport
        && let Some(e) = st.instrument.as_mut().and_then(|i| i.events.as_deref_mut())
    {
        e.transport_changed(epoch);
    }
    if flags & PENDING_REVOKE != 0
        && let Some(i) = st.instrument.as_mut()
        && let Some(e) = i.events.as_deref_mut()
    {
        for (port, generation) in events.revocations() {
            e.release_sender(&mut *i.plugin, port, generation);
        }
    }
    if flags & PENDING_SEEK != 0 {
        if let Some(i) = &mut st.instrument {
            match i.events.as_deref_mut() {
                Some(e) => e.release_clips(&mut *i.plugin),
                None => i.plugin.all_notes_off(),
            }
        }
        st.chase = true;
    }
    if flags & PENDING_RESET != 0 {
        reset_render_state(st);
    } else if flags & PENDING_NOTES_OFF != 0 {
        release_instrument(st);
    }
}

/// The graph path (issue #174): a keyed plan's phase-1 strips as a task graph.
mod graph;
#[cfg(feature = "testing")]
mod seam;
#[cfg(feature = "testing")]
pub(crate) use seam::pinned;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod graph_tests;
#[cfg(test)]
pub(crate) use tests::SWITCH;
