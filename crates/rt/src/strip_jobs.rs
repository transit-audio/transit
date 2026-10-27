//! Per-strip job slots for the multicore strip render (issue #124, ADR 0029): the ownership
//! protocol between the audio callback (the one publisher) and the render workers.
//!
//! Each strip has one [`JobSlot`] with one atomic state and the block generation it was
//! published for. The state persists across blocks.
//!
//! | From | Operation | To | Who | Ordering on success | Refused (counted fault) when |
//! |---|---|---|---|---|---|
//! | `Idle` | [`JobSlot::publish`] | `Published` | publisher | Release | not `Idle`, or a generation not newer than the last |
//! | `Published` | [`JobSlot::try_claim`] | `Claimed` | worker | CAS, Acquire | never (a lost race is `None`) |
//! | `Published` | [`JobSlot::reclaim`] | `Reclaimed` | publisher | CAS, Acquire | never (a lost race is `false`) |
//! | `Claimed` | [`JobSlot::complete`] (needs the [`ClaimToken`]) | `Done` | the claiming worker | CAS, Release | not `Claimed` (no token can exist then) |
//! | `Done` (this generation) | [`JobSlot::consume`] returns [`Consume::Ready`] | `Idle` | publisher | Acquire | a generation newer than the one asked for |
//! | `Done`, `Reclaimed` | [`JobSlot::retire`] | `Idle` | publisher | Acquire | `Idle`, `Published` or `Claimed` |
//!
//! **Gate 1 invariant (exclusive execution).** Per publish, at most one of [`JobSlot::try_claim`]
//! and [`JobSlot::reclaim`] succeeds: both are a compare-and-swap from `Published`, and the only
//! way back to `Published` is a new publish, which needs `Idle`, which only the publisher reaches,
//! and only from `Done` or `Reclaimed`. So a strip's buffers and plug-in instance are touched by
//! exactly one thread per publish, and a worker that lost the swap holds no token and moves on.
//!
//! **Gate 2 invariant (no incomplete or stale audio).** [`JobSlot::consume`] reports
//! [`Consume::Ready`] only for `Done` with the generation asked for, and moves the slot to `Idle`
//! in the same compare-and-swap, so one result is ready once. `Done` with an older generation is
//! [`Consume::Stale`]: a late worker's result, retired unread with [`JobSlot::retire`].
//!
//! Single-publisher precondition: publish, reclaim, consume and retire (and [`JobSet::begin`])
//! are called by one thread, the audio callback. Only those operations leave or enter `Idle`,
//! which is what makes the generation store in `publish` race-free. Workers only claim and
//! complete.
//!
//! Every operation is one atomic load plus at most one compare-and-swap (publish adds two relaxed
//! generation accesses), allocates nothing, takes no lock and never waits. No `unsafe`.
//!
//! An impossible transition is refused, leaves the slot as it was, and adds one to the slot's
//! fault counter (the stream fault). In this crate's tests it also fails an assertion, unless the
//! test declared that it expects faults.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

/// The state of a [`JobSlot`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// No job; the publisher owns the strip's buffers and instance.
    Idle = 0,
    /// Published for a block; no thread owns the strip yet.
    Published = 1,
    /// A worker won the claim and owns the strip until it completes.
    Claimed = 2,
    /// The publisher took the job back to render it inline; it owns the strip.
    Reclaimed = 3,
    /// A worker finished; the publisher owns the strip again (after its Acquire observation).
    Done = 4,
}

impl State {
    /// Decodes a stored state. Only this module stores, and only `State as u8`; an unknown value
    /// cannot occur, and maps to `Claimed`, the state no publisher operation touches.
    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::Published,
            3 => Self::Reclaimed,
            4 => Self::Done,
            _ => Self::Claimed,
        }
    }
}

/// Why [`JobSlot::publish`] (or [`JobSet::publish`]) refused. Every refusal is a counted fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    /// The slot was not `Idle` (a worker still holds it, or its result was not retired).
    NotIdle(State),
    /// The generation was not newer than the slot's last one (generations only grow; 0 is never
    /// valid).
    GenerationNotNewer {
        /// The slot's last generation.
        last: u64,
    },
    /// [`JobSet::publish`]: no slot at that index.
    OutOfRange,
}

/// What [`JobSlot::consume`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Consume {
    /// `Done` for the generation asked for: the worker's writes are visible and the slot is now
    /// `Idle` (consumed). Mix the strip's output.
    Ready,
    /// `Done` for an older generation: a late result. Never mix it; retire it unread.
    Stale,
    /// Not done: `Idle`, `Published`, `Claimed` or `Reclaimed`. Never read the strip's output.
    Pending,
}

/// The impossible transitions, for the fault message in tests.
#[derive(Clone, Copy, Debug)]
enum Fault {
    PublishNotIdle,
    PublishOldGeneration,
    CompleteNotClaimed,
    CompleteForeignToken,
    ConsumeFutureGeneration,
    ConsumeLostDone,
    RetireNotRetirable,
    RetireLostRace,
    BeginNotNewer,
    OutOfRange,
}

#[cfg(test)]
std::thread_local! {
    static FAULTS_EXPECTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` on this thread with impossible transitions counted but not asserted (tests that
/// provoke them on purpose).
#[cfg(test)]
pub(crate) fn expecting_faults<R>(f: impl FnOnce() -> R) -> R {
    let was = FAULTS_EXPECTED.replace(true);
    let r = f();
    FAULTS_EXPECTED.set(was);
    r
}

fn fault(counter: &AtomicU32, what: Fault) {
    // Relaxed: a statistic; no other memory is published through it.
    counter.fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    assert!(FAULTS_EXPECTED.get(), "strip job slot: impossible transition {what:?}");
    #[cfg(not(test))]
    let _ = what;
}

/// One strip's job slot: an atomic state and the generation of its last publish.
///
/// Padded to a cache line so workers claiming neighboring strips do not share one.
#[derive(Debug)]
#[repr(align(64))]
pub struct JobSlot {
    state: AtomicU8,
    /// Written only by [`JobSlot::publish`], only while the slot is `Idle`.
    generation: AtomicU64,
    /// Refused (impossible) transitions on this slot.
    faults: AtomicU32,
}

impl Default for JobSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl JobSlot {
    /// An `Idle` slot that was never published (generation 0).
    #[must_use]
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(State::Idle as u8), generation: AtomicU64::new(0), faults: AtomicU32::new(0) }
    }

    /// The current state (Acquire: a `Done` seen here also shows the worker's writes).
    #[must_use]
    pub fn state(&self) -> State {
        State::from_u8(self.state.load(Ordering::Acquire))
    }

    /// The generation of the last publish (0 before the first). Meaningful to the publisher,
    /// which wrote it; a worker reads the claimed generation from its [`ClaimToken`].
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Refused transitions on this slot so far.
    #[must_use]
    pub fn faults(&self) -> u32 {
        self.faults.load(Ordering::Relaxed)
    }

    fn cas(&self, from: State, to: State, success: Ordering) -> Result<(), State> {
        self.state.compare_exchange(from as u8, to as u8, success, Ordering::Relaxed).map(drop).map_err(State::from_u8)
    }

    /// Publisher: `Idle -> Published` for block `generation`, which must be newer than the last.
    ///
    /// # Errors
    ///
    /// [`PublishError::NotIdle`] when the slot is not `Idle` (the per-block rule reads the slot
    /// first: a `Claimed` slot is skipped as an overrun, a `Done` one retired before this call),
    /// and [`PublishError::GenerationNotNewer`]. Both are counted faults and change nothing.
    pub fn publish(&self, generation: u64) -> Result<(), PublishError> {
        // Relaxed: only the publisher enters `Idle` (retire, consume), and it did so with an
        // Acquire that already ordered the last worker's writes before this call. While the slot
        // is `Idle` no other thread changes it, so the generation store below cannot race.
        let now = State::from_u8(self.state.load(Ordering::Relaxed));
        if now != State::Idle {
            fault(&self.faults, Fault::PublishNotIdle);
            return Err(PublishError::NotIdle(now));
        }
        let last = self.generation.load(Ordering::Relaxed);
        if generation <= last {
            fault(&self.faults, Fault::PublishOldGeneration);
            return Err(PublishError::GenerationNotNewer { last });
        }
        // Relaxed: published by the Release below.
        self.generation.store(generation, Ordering::Relaxed);
        // Release: the generation and every write the publisher made to the strip's inputs
        // happen before a worker's Acquire claim (or the publisher's own reclaim) of this job.
        // The compare-and-swap (not a store) keeps the check above honest: it fails only if a
        // second publisher broke the single-publisher precondition.
        self.cas(State::Idle, State::Published, Ordering::Release).map_err(|s| {
            fault(&self.faults, Fault::PublishNotIdle);
            PublishError::NotIdle(s)
        })
    }

    /// Worker: `Published -> Claimed`. `None` when there is nothing to claim or another thread
    /// won; that is not a fault. The token is the only way to complete the job.
    ///
    /// A worker that read `Published` long ago and claims after the slot was reclaimed, retired
    /// and published again claims the new job, with the new generation in its token: still one
    /// executor per publish.
    #[must_use]
    pub fn try_claim(&self) -> Option<ClaimToken<'_>> {
        // Acquire on success pairs with publish's Release: the worker sees the strip's inputs and
        // the generation. Failure reads nothing, so Relaxed.
        self.cas(State::Published, State::Claimed, Ordering::Acquire).ok()?;
        // Relaxed: ordered after the publish by the Acquire above, and not written again until
        // the slot is `Idle`, which needs this claim's completion first.
        let generation = self.generation.load(Ordering::Relaxed);
        Some(ClaimToken { slot: self, generation })
    }

    /// Publisher: `Published -> Reclaimed`, to render the strip inline. False when a worker
    /// claimed it first (or nothing was published); that is not a fault. On true the publisher
    /// owns the strip until it calls [`JobSlot::retire`].
    #[must_use]
    pub fn reclaim(&self) -> bool {
        // Acquire on success, as for a claim (the publisher wrote the inputs itself, so this only
        // matters for symmetry). Failure reads nothing, so Relaxed.
        self.cas(State::Published, State::Reclaimed, Ordering::Acquire).is_ok()
    }

    /// Worker: `Claimed -> Done`, ending the claim the token stands for. A token always completes
    /// the slot it was claimed from; passing it to another slot is a counted fault (and the
    /// token's own slot is completed anyway, so no slot is left claimed).
    pub fn complete(&self, token: ClaimToken<'_>) {
        if !std::ptr::eq(self, token.slot) {
            fault(&self.faults, Fault::CompleteForeignToken);
        }
        token.complete();
    }

    /// Publisher: is the strip's output for `generation` ready to mix? See [`Consume`].
    ///
    /// [`Consume::Ready`] also moves the slot `Done -> Idle`: the result is consumed and cannot
    /// be ready twice, and the publisher owns the strip again. [`Consume::Stale`] leaves the slot
    /// `Done` for [`JobSlot::retire`].
    pub fn consume(&self, generation: u64) -> Consume {
        // Acquire pairs with complete's Release: a `Done` seen here shows all the worker's writes.
        if self.state() != State::Done {
            return Consume::Pending;
        }
        // Relaxed: the publisher wrote it; it does not change while the slot is `Done`.
        let done = self.generation.load(Ordering::Relaxed);
        if done < generation {
            return Consume::Stale;
        }
        if done > generation {
            fault(&self.faults, Fault::ConsumeFutureGeneration);
            return Consume::Stale;
        }
        // Acquire (the Done was already acquired above; kept for the transition's own record).
        // Only the publisher leaves `Done`, so this cannot fail under the precondition.
        if self.cas(State::Done, State::Idle, Ordering::Acquire).is_ok() {
            Consume::Ready
        } else {
            fault(&self.faults, Fault::ConsumeLostDone);
            Consume::Pending
        }
    }

    /// Publisher: `Done -> Idle` (a result retired unread: stale, or overrun) or
    /// `Reclaimed -> Idle` (after the inline render). True when retired. From `Claimed` it is
    /// refused (a worker owns the strip), as from `Idle` or `Published`: a counted fault that
    /// changes nothing.
    pub fn retire(&self) -> bool {
        // Acquire: from `Done` it pairs with complete's Release, so the late worker's writes
        // happen before the publisher reuses the strip's buffers.
        let now = self.state();
        if !matches!(now, State::Done | State::Reclaimed) {
            fault(&self.faults, Fault::RetireNotRetirable);
            return false;
        }
        // Only the publisher leaves `Done` and `Reclaimed`, so this cannot fail under the
        // precondition.
        if self.cas(now, State::Idle, Ordering::Acquire).is_err() {
            fault(&self.faults, Fault::RetireLostRace);
            return false;
        }
        true
    }
}

/// Proof that the holder won [`JobSlot::try_claim`]: it owns the strip until it completes.
/// Not `Clone`, so a job completes at most once, and only by the thread that claimed it.
///
/// A token dropped without [`ClaimToken::complete`] leaves the slot `Claimed` for good: the
/// strip is overrun every block from then on and the engine's retirement quarantines it (gate 5,
/// a later slice).
#[derive(Debug)]
#[must_use = "a claimed job stays Claimed until the token completes it"]
pub struct ClaimToken<'a> {
    slot: &'a JobSlot,
    generation: u64,
}

impl ClaimToken<'_> {
    /// The block generation the claimed job was published for.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// `Claimed -> Done` on the slot this token was claimed from.
    pub fn complete(self) {
        // Release: every write the worker made to the strip's buffers and instance happens before
        // the publisher's Acquire observation of `Done` (consume, retire, state). A failure is
        // impossible (only the token's holder leaves `Claimed`), so it reads nothing.
        if self.slot.cas(State::Claimed, State::Done, Ordering::Release).is_err() {
            fault(&self.slot.faults, Fault::CompleteNotClaimed);
        }
    }
}

/// How many slots of a [`JobSet`] are in each state (a snapshot; workers keep moving).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// `Idle` slots.
    pub idle: usize,
    /// `Published` slots (no thread owns them yet).
    pub published: usize,
    /// `Claimed` slots (a worker is rendering).
    pub claimed: usize,
    /// `Reclaimed` slots (the publisher is rendering inline).
    pub reclaimed: usize,
    /// `Done` slots.
    pub done: usize,
}

/// A fixed set of job slots, one per strip, allocated once (outside the audio path) and never
/// resized, with the current block generation. Nothing after [`JobSet::new`] allocates.
#[derive(Debug)]
pub struct JobSet {
    slots: Box<[JobSlot]>,
    /// The current block, set by [`JobSet::begin`]; read and written by the publisher only.
    generation: AtomicU64,
    /// Refused set-level operations (`begin` not newer, an index out of range).
    faults: AtomicU32,
}

impl JobSet {
    /// `capacity` idle slots (the most strips the plan can hold). Allocates; call it off the
    /// audio thread.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { slots: (0..capacity).map(|_| JobSlot::new()).collect(), generation: AtomicU64::new(0), faults: AtomicU32::new(0) }
    }

    /// The number of slots.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// The slot at `index`.
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<&JobSlot> {
        self.slots.get(index)
    }

    /// Publisher: starts block `generation`, which must be newer than the current one (a counted
    /// fault otherwise, and the current block stays). True when started.
    pub fn begin(&self, generation: u64) -> bool {
        // Relaxed: publisher-only state; workers read generations from their tokens.
        if generation <= self.generation.load(Ordering::Relaxed) {
            fault(&self.faults, Fault::BeginNotNewer);
            return false;
        }
        self.generation.store(generation, Ordering::Relaxed);
        true
    }

    /// The current block generation (0 before the first [`JobSet::begin`]).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Publisher: publishes slot `index` for the current block. See [`JobSlot::publish`].
    ///
    /// # Errors
    ///
    /// As [`JobSlot::publish`], plus [`PublishError::OutOfRange`] (a counted fault).
    pub fn publish(&self, index: usize) -> Result<(), PublishError> {
        let Some(slot) = self.slots.get(index) else {
            fault(&self.faults, Fault::OutOfRange);
            return Err(PublishError::OutOfRange);
        };
        slot.publish(self.generation())
    }

    /// Publisher: [`JobSlot::consume`] of slot `index` for the current block (`Pending` and a
    /// counted fault when out of range).
    pub fn consume(&self, index: usize) -> Consume {
        let Some(slot) = self.slots.get(index) else {
            fault(&self.faults, Fault::OutOfRange);
            return Consume::Pending;
        };
        slot.consume(self.generation())
    }

    /// Worker: claims the first published slot at or after `start` (wrapping), visiting each
    /// slot once. Bounded by the capacity.
    #[must_use]
    pub fn try_claim_next(&self, start: usize) -> Option<(usize, ClaimToken<'_>)> {
        let n = self.slots.len();
        (0..n).map(|k| (start % n.max(1) + k) % n).find_map(|i| self.slots[i].try_claim().map(|t| (i, t)))
    }

    /// Calls `f(index, state)` for each slot that is published, claimed or reclaimed (neither
    /// idle nor done), in index order. Bounded by the capacity; allocates nothing.
    pub fn for_each_pending(&self, mut f: impl FnMut(usize, State)) {
        for (i, slot) in self.slots.iter().enumerate() {
            let s = slot.state();
            if matches!(s, State::Published | State::Claimed | State::Reclaimed) {
                f(i, s);
            }
        }
    }

    /// How many slots are in each state now.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for slot in &*self.slots {
            match slot.state() {
                State::Idle => c.idle += 1,
                State::Published => c.published += 1,
                State::Claimed => c.claimed += 1,
                State::Reclaimed => c.reclaimed += 1,
                State::Done => c.done += 1,
            }
        }
        c
    }

    /// Refused transitions on the set and all its slots.
    #[must_use]
    pub fn faults(&self) -> u64 {
        self.slots.iter().map(|s| u64::from(s.faults())).sum::<u64>() + u64::from(self.faults.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod interleave_tests;
