//! Per-attempt accounting and an acknowledged, single-consumer measurement mailbox.
//! Counters never participate in scheduling. See render-graph-measurement.md.

use super::{Shared, WorkerSet, bump};
use crate::strip_jobs::ClaimToken;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

macro_rules! counters {
    ($($field:ident: $doc:literal),+ $(,)?) => {
        /// Cumulative attempt counters. Only settled mailbox captures are consistent snapshots.
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub struct QueueSnapshot { $(#[doc = $doc] pub $field: u64,)+ }

        #[derive(Default)]
        pub(super) struct QueueCounters { $(pub(super) $field: AtomicU64,)+ }

        impl QueueCounters {
            pub(super) fn snapshot(&self) -> QueueSnapshot {
                QueueSnapshot { $($field: self.$field.load(Ordering::Relaxed),)+ }
            }
            fn store(&self, snapshot: QueueSnapshot) {
                $(self.$field.store(snapshot.$field, Ordering::Relaxed);)+
            }
            fn copy_with(&self, mut copied: impl FnMut()) -> QueueSnapshot {
                QueueSnapshot { $($field: {
                    let v = self.$field.load(Ordering::Relaxed);
                    copied();
                    v
                },)+ }
            }
        }
    };
}

counters! {
    published: "Successful Idle-to-Published transitions.",
    claimed: "Successful worker claims.",
    inline_pre: "Pre-deadline reclaims, irrespective of render completion time.",
    unclaimed_at_deadline: "Deadline reclaims, rendered or retired unrendered.",
    deadline_reclaimed_tail: "Tail renders among deadline reclaims (a subset, never added).",
    classified: "Worker completions classified, including runner faults.",
    queue_over: "Claims queued for more than one quarter of the original host period.",
    late_w: "Worker completions strictly after their publication's deadline.",
    late_after_queue: "Worker completions both late and queued over a quarter period.",
    forgone: "Closed-window Forgo decisions (not published attempts).",
    tail_callback: "Direct callback aux renders (not published attempts).",
}

impl QueueSnapshot {
    /// Whether both conservation equations hold. Uses wide arithmetic to reject overflow.
    #[must_use]
    pub fn conserves(self) -> bool {
        u128::from(self.published) == u128::from(self.claimed) + u128::from(self.inline_pre) + u128::from(self.unclaimed_at_deadline)
            && self.claimed == self.classified
    }
}

#[derive(Default)]
#[allow(clippy::struct_field_names)] // Units and names match the normative measurement document.
struct Stamps {
    pub_ns: AtomicU64,
    deadline_ns: AtomicU64,
    period_ns: AtomicU64,
}

pub(super) struct Queue {
    stamps: Box<[Stamps]>,
    pub(super) counts: QueueCounters,
}

impl Queue {
    pub(super) fn new(slots: usize) -> Self {
        Self { stamps: (0..slots).map(|_| Stamps::default()).collect(), counts: QueueCounters::default() }
    }

    /// Publisher owns an Idle slot; the following Release publish orders all three stores.
    pub(super) fn stamp(&self, slot: usize, pub_ns: u64, deadline_ns: u64, period_ns: u64) {
        if let Some(s) = self.stamps.get(slot) {
            s.pub_ns.store(pub_ns, Ordering::Relaxed);
            s.deadline_ns.store(deadline_ns, Ordering::Relaxed);
            s.period_ns.store(period_ns, Ordering::Relaxed);
        }
    }

    pub(super) fn claimed(&self) {
        bump(&self.counts.claimed);
    }

    /// Worker-only, after Acquire claim and before Release complete. Slot ownership keeps
    /// these stamps stable even if this render spans many host blocks.
    pub(super) fn classify(&self, slot: usize, claim_ns: u64, end_ns: u64) {
        let Some(s) = self.stamps.get(slot) else { return };
        let queued = 4 * u128::from(claim_ns.saturating_sub(s.pub_ns.load(Ordering::Relaxed))) > u128::from(s.period_ns.load(Ordering::Relaxed));
        let late = end_ns > s.deadline_ns.load(Ordering::Relaxed);
        if queued {
            bump(&self.counts.queue_over);
        }
        if late {
            bump(&self.counts.late_w);
        }
        if queued && late {
            bump(&self.counts.late_after_queue);
        }
        bump(&self.counts.classified);
    }
}

impl Shared {
    /// Exactly one claimed executor. Production supplies the render's two existing clock reads;
    /// rt tests inject both stamps to exercise the same completion and fault path.
    pub(super) fn run_attempt(&self, strip: usize, token: ClaimToken<'_>, mut clock: impl FnMut() -> Instant) {
        self.queue.claimed();
        let start = clock();
        self.figures.claimed(strip, self.figures.nanos(start));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runner.run(strip, token.generation())));
        let end = clock();
        self.record_cost(strip, end.saturating_duration_since(start));
        self.figures.done(strip, self.figures.nanos(end));
        self.queue.classify(strip, self.figures.nanos(start), self.figures.nanos(end));
        if result.is_err() {
            self.panicked[strip].store(true, Ordering::Relaxed);
            bump(&self.stats.runner_panics);
        }
        bump(&self.stats.worker_runs);
        drop(result);
        token.complete();
    }
}

const IDLE: u8 = 0;
const REQUESTED: u8 = 1;
const RESPONSE: u8 = 2;
/// Maximum host-block capture attempts before an incomplete response is published.
pub const CAPTURE_BOUND: u32 = 1_000;

#[derive(Default)]
pub(super) struct Mailbox {
    state: AtomicU8,
    request: AtomicU64,
    complete: AtomicU8,
    payload: QueueCounters,
}

impl Mailbox {
    // One consumer owns this operation. Kept separate for the paused-reader interleaving test.
    pub(super) fn request(&self, identity: u64) -> bool {
        if self.state.load(Ordering::Acquire) != IDLE {
            return false;
        }
        self.request.store(identity, Ordering::Relaxed);
        self.state.store(REQUESTED, Ordering::Release);
        true
    }
}

/// One response, copied in full before acknowledgment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// Consumer-supplied request identity.
    pub request: u64,
    /// True only when ownership settlement succeeded at the capture boundary.
    pub complete: bool,
    /// Every cumulative counter at the boundary.
    pub counters: QueueSnapshot,
}

/// The unique control-thread consumer. Not cloneable; other readers receive copied responses.
pub struct MeasurementReader {
    mailbox: Arc<Mailbox>,
}

impl MeasurementReader {
    /// Posts a request only after the previous response has been copied and acknowledged.
    /// Refuses immediately while a request or response is outstanding.
    pub fn request(&mut self, identity: u64) -> bool {
        self.mailbox.request(identity)
    }

    /// Copies a published response, then acknowledges it. Never waits for the callback.
    pub fn read(&mut self) -> Option<Measurement> {
        self.copy_with(|| {})
    }

    pub(super) fn copy_with(&mut self, copied: impl FnMut()) -> Option<Measurement> {
        if self.mailbox.state.load(Ordering::Acquire) != RESPONSE {
            return None;
        }
        let response = Measurement {
            request: self.mailbox.request.load(Ordering::Relaxed),
            complete: self.mailbox.complete.load(Ordering::Relaxed) != 0,
            counters: self.mailbox.payload.copy_with(copied),
        };
        // The payload may not be written again until the callback observes this acknowledgment.
        self.mailbox.state.store(IDLE, Ordering::Release);
        Some(response)
    }
}

impl WorkerSet {
    /// Sets the publication stamps for this sub-block. `period` is always the original host
    /// block's period, never a tempo sub-block's. Call after `begin_block`, before any publish.
    pub fn set_block_timing(&mut self, deadline: Instant, period: Duration) {
        self.queue_deadline_ns = self.shared.figures.nanos(deadline);
        self.queue_period_ns = u64::try_from(period.as_nanos()).unwrap_or(u64::MAX);
    }

    /// Takes the sole consumer handle, once, on the control thread.
    pub fn measurement_reader(&mut self) -> Option<MeasurementReader> {
        if std::mem::replace(&mut self.reader_taken, true) {
            return None;
        }
        Some(MeasurementReader { mailbox: Arc::clone(&self.mailbox) })
    }

    /// Whether host-block entry should attempt ownership settlement and its panic checkpoint.
    #[must_use]
    pub fn measurement_requested(&self) -> bool {
        self.mailbox.state.load(Ordering::Acquire) == REQUESTED
    }

    /// Called once per host-block entry, after seam installation and ownership settlement,
    /// before publication. `settled` must include the host's strip cleanup and panic checkpoint.
    /// Never waits; publishes incomplete after 1,000 unsuccessful boundaries. An unacknowledged
    /// response is immutable across all subsequent boundaries.
    pub fn capture_measurement(&mut self, settled: bool) {
        if !self.measurement_requested() {
            return;
        }
        self.capture_attempts += 1;
        let complete = settled && self.shared.jobs.counts().idle == self.strips();
        if !complete && self.capture_attempts < CAPTURE_BOUND {
            return;
        }
        self.mailbox.payload.store(self.shared.queue.counts.snapshot());
        self.mailbox.complete.store(u8::from(complete), Ordering::Relaxed);
        self.capture_attempts = 0;
        self.mailbox.state.store(RESPONSE, Ordering::Release);
    }
}
