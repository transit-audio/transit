//! Recording starts are published by the applied audio action, not a UI receipt.
//! A non-wrapping ticket prevents a delayed action from starting a later take.
//!
//! The state word is the ticket's sequence plus a three-bit stage (ADR 0062 section 6). The audio
//! thread advances it monotonically, Pending, CountingIn, Waiting, then a terminal stage (Started,
//! Failed or Cancelled), each by one strong compare-and-swap from the matching ticket's
//! non-terminal state, never retried. Main's cancel retries when it loses to an advance, at most
//! three times, and stops on a terminal stage.
use crate::timeline_map::Position;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use transit_mix::EngineEpoch;

pub(crate) mod mapping;
#[cfg(test)]
mod stage_tests;

const FLAGS: u64 = 7;
const CANCELLED: u64 = 0;
const PENDING: u64 = 1;
const STARTED: u64 = 2;
const COUNTING_IN: u64 = 3;
const WAITING: u64 = 4;
const FAILED: u64 = 5;
/// Main's cancel loses to at most two advances (Pending to CountingIn to Waiting); the third
/// attempt meets a non-terminal state it can cancel or a terminal one it leaves.
const CANCEL_ATTEMPTS: usize = 3;

/// Why a recording start did not happen. Travels to Main as a code in one atomic, so the audio
/// thread allocates nothing to report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFailure {
    /// The device clock map was lost or went stale during the count-in.
    Clock = 1,
    /// The transport timeline refused the position.
    Timeline = 2,
    /// The start position was out of range.
    Position = 3,
    /// The transport was not at normal speed (#522): recording runs at 1x only.
    Speed = 4,
}

impl StartFailure {
    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Clock),
            2 => Some(Self::Timeline),
            3 => Some(Self::Position),
            4 => Some(Self::Speed),
            _ => None,
        }
    }

    /// Plain words for the status line.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Clock => "the audio device clock was not ready in time",
            Self::Timeline => "the transport timeline refused the start position",
            Self::Position => "the start position is out of range",
            Self::Speed => "the transport was not at normal speed",
        }
    }
}

/// The order of the non-terminal stages; `None` for a terminal one.
fn rank(stage: u64) -> Option<u8> {
    match stage {
        PENDING => Some(0),
        COUNTING_IN => Some(1),
        WAITING => Some(2),
        _ => None,
    }
}

/// One prepared recording operation within this runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    engine: EngineEpoch,
    sequence: u64,
}

/// The output boundary actually applied, retaining sub-sample session phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub host_ns: u64,
    pub output_frame: u64,
    pub position: Position,
}

/// The stage of one ticket's record start (ADR 0062 section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Submitted, not yet applied by the audio thread.
    Pending,
    /// Applied with a count-in that is clicking.
    CountingIn,
    /// Applied and waiting for the device clock (or normal speed) before it latches.
    Waiting,
    /// Latched at this applied boundary.
    Started(Point),
    /// The audio thread refused the start for this reason.
    Failed(StartFailure),
    /// Cancelled (by Main, a later transport action, or a refusal without a reason), or superseded
    /// by a later ticket.
    Cancelled,
}

struct Shared {
    engine: EngineEpoch,
    state: AtomicU64,
    /// The [`StartFailure`] code of a Failed stage, stored before its compare-and-swap.
    failure: AtomicU8,
    host_ns: AtomicU64,
    output_frame: AtomicU64,
    sample: AtomicI64,
    fraction: AtomicU64,
    denominator: AtomicU64,
    /// Audio input takes are part of this pass (set by Main when it prepares them).
    audio: AtomicBool,
    /// The input stream's next device frame and last block length, published by its callback.
    /// `u64::MAX` until the stream has delivered a block, and always for a separate input device.
    input_next: AtomicU64,
    input_block: AtomicU64,
    /// Input frame the audio thread chose for the origin without a host-time map (`NO_FRAME`: none).
    fallback_frame: AtomicU64,
    /// The take is placed by the degraded policy rather than a validated clock map.
    approximate: AtomicBool,
}

const NO_FRAME: u64 = u64::MAX;

/// What a pass without a validated device clock falls back to, once started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fallback {
    pub input_frame: Option<u64>,
    pub approximate: bool,
}

/// The input callback's handle for publishing its position to the output's audio thread.
#[derive(Clone)]
pub struct InputProgress {
    shared: Arc<Shared>,
}

impl InputProgress {
    /// Two relaxed-order stores; callable from the audio thread.
    pub fn publish(&self, next_frame: u64, block_frames: u64) {
        self.shared.input_block.store(block_frames, Ordering::Relaxed);
        self.shared.input_next.store(next_frame, Ordering::Release);
    }
}

/// Main-side ticket issuer. Keep one issuer for the lifetime of the audio runtime.
pub struct Control {
    shared: Arc<Shared>,
    next: u64,
}

/// Sole mutable publisher, kept in the Main-anchored Audio owner.
pub struct Publisher {
    shared: Arc<Shared>,
}

/// Writer/control observer for one take. Cloning is control work only.
#[derive(Clone)]
pub struct Reader {
    shared: Arc<Shared>,
    ticket: Ticket,
}

pub fn pair(engine: EngineEpoch) -> (Control, Publisher) {
    let shared = Arc::new(Shared {
        engine,
        state: AtomicU64::new(0),
        failure: AtomicU8::new(0),
        host_ns: AtomicU64::new(0),
        output_frame: AtomicU64::new(0),
        sample: AtomicI64::new(0),
        fraction: AtomicU64::new(0),
        denominator: AtomicU64::new(0),
        audio: AtomicBool::new(false),
        input_next: AtomicU64::new(NO_FRAME),
        input_block: AtomicU64::new(0),
        fallback_frame: AtomicU64::new(NO_FRAME),
        approximate: AtomicBool::new(false),
    });
    (Control { shared: Arc::clone(&shared), next: 0 }, Publisher { shared })
}

impl Control {
    /// Supersede the previous ticket. Returns None instead of reusing an identity.
    pub fn prepare(&mut self) -> Option<Reader> {
        let next = self.next.checked_add(FLAGS + 1)?;
        self.next = next;
        self.shared.audio.store(false, Ordering::Relaxed);
        self.shared.fallback_frame.store(NO_FRAME, Ordering::Relaxed);
        self.shared.approximate.store(false, Ordering::Relaxed);
        self.shared.state.store(next | PENDING, Ordering::Release);
        Some(Reader { shared: Arc::clone(&self.shared), ticket: Ticket { engine: self.shared.engine, sequence: next } })
    }

    /// The handle a duplex device's input callback publishes its position through.
    pub fn input_progress(&self) -> InputProgress {
        InputProgress { shared: Arc::clone(&self.shared) }
    }

    /// Cancel a still-pending start before submitting Stop. An already applied
    /// boundary remains readable while the recorder drains and finalizes its take.
    pub fn cancel(&self) {
        let ticket = Ticket { engine: self.shared.engine, sequence: self.shared.state.load(Ordering::Acquire) & !FLAGS };
        cancel(&self.shared, ticket);
    }
}

impl Publisher {
    /// The state word when it is `ticket`'s and not terminal.
    fn live(&self, ticket: Ticket) -> Option<u64> {
        if ticket.engine != self.shared.engine {
            return None;
        }
        let state = self.shared.state.load(Ordering::Acquire);
        (state & !FLAGS == ticket.sequence && rank(state & FLAGS).is_some()).then_some(state)
    }

    /// The one compare-and-swap of a publication, from the state `live` read; never retried.
    fn publish(&self, current: u64, ticket: Ticket, stage: u64) -> bool {
        self.shared.state.compare_exchange(current, ticket.sequence | stage, Ordering::Release, Ordering::Relaxed).is_ok()
    }

    /// Advance `ticket` to the non-terminal `stage`. True when the ticket is at `stage` after the
    /// call (already there: nothing is written); false when the ticket is superseded, terminal,
    /// past `stage`, or Main cancelled it between the read and the compare-and-swap.
    fn advance(&mut self, ticket: Ticket, stage: u64) -> bool {
        let Some(current) = self.live(ticket) else { return false };
        match (rank(current & FLAGS), rank(stage)) {
            (Some(from), Some(to)) if from == to => true,
            (Some(from), Some(to)) if from < to => self.publish(current, ticket, stage),
            _ => false,
        }
    }

    /// Publish that `ticket`'s count-in is clicking. Bounded: no allocation, lock, wait or retry.
    pub fn counting_in(&mut self, ticket: Ticket) -> bool {
        self.advance(ticket, COUNTING_IN)
    }

    /// Publish that `ticket` waits for the device clock (or normal speed) before it latches.
    pub fn waiting(&mut self, ticket: Ticket) -> bool {
        self.advance(ticket, WAITING)
    }

    /// End `ticket` as refused: Failed with `failure` (its code stored first), or Cancelled when
    /// the refusal has no reason. False when the ticket was already terminal or superseded.
    pub fn fail(&mut self, ticket: Ticket, failure: Option<StartFailure>) -> bool {
        let Some(current) = self.live(ticket) else { return false };
        match failure {
            Some(failure) => {
                self.shared.failure.store(failure as u8, Ordering::Release);
                self.publish(current, ticket, FAILED)
            }
            None => self.publish(current, ticket, CANCELLED),
        }
    }

    /// One bounded publication attempt; no allocation, lock, wait or retry.
    /// The mutable endpoint is the sole payload writer. Main may cancel/rearm
    /// between its check and CAS, but the older action then cannot publish.
    pub fn start(&mut self, ticket: Ticket, point: Point) -> bool {
        self.start_placed(ticket, point, None, false)
    }

    /// As [`Publisher::start`], also publishing the input frame chosen without a host-time map
    /// and whether the placement is approximate. Started is published from any non-terminal stage
    /// of the ticket (a count-in or a clock wait ends in it).
    pub fn start_placed(&mut self, ticket: Ticket, point: Point, input_frame: Option<u64>, approximate: bool) -> bool {
        if point.position.denominator == 0 || point.position.fraction >= point.position.denominator {
            return false;
        }
        let Some(current) = self.live(ticket) else { return false };
        self.shared.fallback_frame.store(input_frame.unwrap_or(NO_FRAME), Ordering::Release);
        self.shared.approximate.store(approximate, Ordering::Release);
        self.shared.host_ns.store(point.host_ns, Ordering::Release);
        self.shared.output_frame.store(point.output_frame, Ordering::Release);
        self.shared.sample.store(point.position.sample, Ordering::Release);
        self.shared.fraction.store(point.position.fraction, Ordering::Release);
        self.shared.denominator.store(point.position.denominator, Ordering::Release);
        self.publish(current, ticket, STARTED)
    }

    /// Does this pass record audio input (so the take needs the input aligned to the output)?
    pub fn audio_pass(&self) -> bool {
        self.shared.audio.load(Ordering::Relaxed)
    }

    /// The input frame that coincides with the output block about to render when the
    /// duplex device's two streams share one callback but count frames independently, or `None`
    /// without a duplex input. The input callback of the same driver block may or may not have run
    /// yet, so the estimate is the earlier alternative: at most one block of extra pre-roll, never
    /// a clipped start.
    pub fn duplex_input_frame(&self) -> Option<u64> {
        let next = self.shared.input_next.load(Ordering::Acquire);
        (next != NO_FRAME).then(|| next.saturating_sub(self.shared.input_block.load(Ordering::Relaxed)))
    }

    /// Cancel this exact operation while it is not terminal (a later transport action withdrew
    /// it): one compare-and-swap, never retried.
    pub fn cancel(&self, ticket: Ticket) {
        if let Some(current) = self.live(ticket) {
            self.publish(current, ticket, CANCELLED);
        }
    }
}

/// Main's cancel of `ticket` while it is not terminal.
fn cancel(shared: &Shared, ticket: Ticket) {
    cancel_with(shared, ticket, |_| {});
}

/// [`cancel`], with `between(attempt)` run between each attempt's read and its compare-and-swap,
/// where a test interleaves the audio thread's advances. If publication wins, its boundary is
/// retained for final take placement; a new ticket is a different identity and this stale
/// cancellation cannot touch it. True when this call cancelled the ticket.
fn cancel_with(shared: &Shared, ticket: Ticket, mut between: impl FnMut(usize)) -> bool {
    if ticket.engine != shared.engine {
        return false;
    }
    let mut current = shared.state.load(Ordering::Acquire);
    for attempt in 0..CANCEL_ATTEMPTS {
        if current & !FLAGS != ticket.sequence || rank(current & FLAGS).is_none() {
            return false;
        }
        between(attempt);
        match shared.state.compare_exchange(current, ticket.sequence | CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            // Lost to a stage advance (or a terminal stage, or a rearm): look again.
            Err(seen) => current = seen,
        }
    }
    false
}

impl Reader {
    pub fn ticket(&self) -> Ticket {
        self.ticket
    }
    pub fn cancel(&self) {
        cancel(&self.shared, self.ticket);
    }
    /// Mark the pass as carrying audio input takes. Main only, before the start is applied.
    pub(crate) fn mark_audio(&self) {
        self.shared.audio.store(true, Ordering::Relaxed);
    }

    /// Record that this started pass is placed by the degraded policy. A no-op once rearmed.
    pub(crate) fn mark_approximate(&self) {
        if self.shared.state.load(Ordering::Acquire) == self.ticket.sequence | STARTED {
            self.shared.approximate.store(true, Ordering::Release);
        }
    }

    /// The degraded-placement data of a started pass, validated against a concurrent rearm.
    pub fn fallback(&self) -> Option<Fallback> {
        let before = self.shared.state.load(Ordering::Acquire);
        if before != self.ticket.sequence | STARTED {
            return None;
        }
        let frame = self.shared.fallback_frame.load(Ordering::Acquire);
        let approximate = self.shared.approximate.load(Ordering::Acquire);
        (self.shared.state.load(Ordering::Acquire) == before).then_some(Fallback { input_frame: (frame != NO_FRAME).then_some(frame), approximate })
    }

    /// At most two state reads. An intervening rearm discards the payload.
    /// Payload words use Release/Acquire: seeing a word from the next publisher
    /// establishes happens-before from its pending-state check to our validation.
    /// The validation therefore cannot accept the old state with newer words.
    pub fn status(&self) -> Status {
        let before = self.shared.state.load(Ordering::Acquire);
        if before & !FLAGS != self.ticket.sequence {
            // A later ticket superseded this one.
            return Status::Cancelled;
        }
        match before & FLAGS {
            PENDING => return Status::Pending,
            COUNTING_IN => return Status::CountingIn,
            WAITING => return Status::Waiting,
            STARTED => {}
            FAILED => {
                let failure = StartFailure::from_code(self.shared.failure.load(Ordering::Acquire));
                return match failure {
                    Some(failure) if self.shared.state.load(Ordering::Acquire) == before => Status::Failed(failure),
                    _ => Status::Cancelled,
                };
            }
            _ => return Status::Cancelled,
        }
        let point = Point {
            host_ns: self.shared.host_ns.load(Ordering::Acquire),
            output_frame: self.shared.output_frame.load(Ordering::Acquire),
            position: Position {
                sample: self.shared.sample.load(Ordering::Acquire),
                fraction: self.shared.fraction.load(Ordering::Acquire),
                denominator: self.shared.denominator.load(Ordering::Acquire),
            },
        };
        if self.shared.state.load(Ordering::Acquire) == before { Status::Started(point) } else { Status::Cancelled }
    }
}

/// The take writer's refusal of a start before the first retained input frame.
pub const OUTSIDE_HISTORY: &str = "recording start fell outside retained input history";

/// Writer-only bounded pre-start history. Device ordinals, never callback counts,
/// decide which samples precede the applied output boundary.
pub(super) struct History {
    data: std::collections::VecDeque<f32>,
    channels: usize,
    capacity: usize,
    first: Option<u64>,
    next: Option<u64>,
    start: Option<u64>,
    /// The first frame captured after arm; unlike `first`, eviction never advances it.
    arm_first: Option<u64>,
    /// The input's longest block, in frames (#587): a start that maps before the first frame
    /// captured after arm by less than this, and less than `max_early`, starts the take there.
    early: u64,
    /// The cap on that tolerance (100 ms in a recorder; 0, no move, until set).
    max_early: u64,
    /// The start was moved to the first retained frame: the take is approximate.
    moved: bool,
}

impl History {
    pub(super) fn new(channels: usize, frames: usize) -> Self {
        let channels = channels.max(1);
        let capacity = frames.saturating_mul(channels).min(1 << 25) / channels * channels;
        Self {
            data: std::collections::VecDeque::with_capacity(capacity),
            channels,
            capacity,
            first: None,
            next: None,
            start: None,
            arm_first: None,
            early: 0,
            max_early: 0,
            moved: false,
        }
    }

    pub(super) fn set_start(&mut self, frame: u64) {
        self.start = Some(frame);
    }

    /// Cap the tolerance of [`History::set_block`] at `frames`.
    pub(super) fn with_early_cap(mut self, frames: u64) -> Self {
        self.max_early = frames;
        self
    }

    /// The input's block length in frames, for a start just before the first frame after arm.
    pub(super) fn set_block(&mut self, frames: u64) {
        self.early = frames;
    }

    /// Was the start moved to the first retained frame (see [`History::set_block`])?
    pub(super) fn moved(&self) -> bool {
        self.moved
    }

    pub(super) fn push(&mut self, first: u64, data: &[f32], mut write: impl FnMut(&[f32])) -> Result<(), &'static str> {
        if !data.len().is_multiple_of(self.channels) || self.next.is_some_and(|next| next != first) {
            return Err("recording input frames are discontinuous");
        }
        let frames = (data.len() / self.channels) as u64;
        self.next = Some(first.checked_add(frames).ok_or("recording input position exhausted")?);
        if self.first.is_none() {
            self.first = Some(first);
        }
        if self.arm_first.is_none() {
            self.arm_first = Some(first);
        }
        if let Some(mut start) = self.start {
            let oldest = self.first.unwrap_or(first);
            // Only before the first frame captured after arm, never before frames the bounded
            // history evicted (a late writer): moving the start there would drop captured audio.
            if start < oldest && self.arm_first == Some(oldest) && oldest - start < self.early.min(self.max_early) {
                // Writer thread, once per take (#587): the input captured those frames before arm,
                // so the history never received them; the take starts at the first frame after arm,
                // approximately.
                log::warn!(
                    "recording start at input frame {start} is {} frames before the first retained frame {oldest}; the take starts there and is approximate",
                    oldest - start
                );
                self.moved = true;
                start = oldest;
            }
            if start < oldest {
                // Writer thread, once per failed take. The figures tell a clock disagreement (the
                // start mapped before the first frame captured after arm) from a late writer (#429).
                log::warn!("recording start at input frame {start} is {} frames before the first retained frame {oldest}", oldest - start);
                return Err(OUTSIDE_HISTORY);
            }
            let skip = usize::try_from(start.saturating_sub(oldest)).unwrap_or(usize::MAX).saturating_mul(self.channels).min(self.data.len());
            self.data.drain(..skip);
            if !self.data.is_empty() {
                write(self.data.make_contiguous());
                self.data.clear();
            }
            let skip = usize::try_from(start.saturating_sub(first)).unwrap_or(usize::MAX).saturating_mul(self.channels).min(data.len());
            write(data.get(skip..).unwrap_or_default());
            // Once the boundary is crossed, all later contiguous frames belong.
            self.start = Some(start.max(self.next.unwrap_or(first)));
            self.first = self.next;
            return Ok(());
        }
        let keep = data.len().min(self.capacity);
        let evict = self.data.len().saturating_add(keep).saturating_sub(self.capacity);
        self.data.drain(..evict);
        self.data.extend(data.get(data.len().saturating_sub(keep)..).unwrap_or_default());
        self.first = self.next.and_then(|next| next.checked_sub((self.data.len() / self.channels) as u64));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use transit_rt::testing::measure;

    fn point(n: u64) -> Point {
        Point { host_ns: n * 1000, output_frame: n, position: Position { sample: -(n as i64), fraction: n, denominator: n + 1 } }
    }

    #[test]
    fn input_before_or_after_the_output_boundary_produces_the_same_take_prefix() {
        for partition in [vec![12], vec![4, 4, 4], vec![1, 2, 5, 1, 3]] {
            for published_after in 0..=partition.len() {
                let mut history = History::new(2, 24);
                let mut got = Vec::new();
                let input: Vec<f32> = (0..12).flat_map(|frame| [frame as f32, -(frame as f32)]).collect();
                let mut offset = 0;
                for (index, &frames) in partition.iter().enumerate() {
                    if index == published_after {
                        history.set_start(104);
                    }
                    history.push(100 + offset as u64, &input[offset * 2..(offset + frames) * 2], |data| got.extend_from_slice(data)).unwrap();
                    offset += frames;
                }
                if published_after == partition.len() {
                    history.set_start(104);
                }
                history.push(112, &[], |data| got.extend_from_slice(data)).unwrap();
                assert_eq!(got, input[8..], "partition {partition:?}, publication {published_after}");
            }
        }
    }

    #[test]
    fn long_preparation_is_bounded_and_missing_start_history_is_explicit() {
        let mut history = History::new(1, 4);
        for frame in 0..10_000 {
            history.push(frame, &[frame as f32], |_| panic!("pending take wrote audio")).unwrap();
            assert!(history.data.len() <= 4);
        }
        history.set_start(9_997);
        let mut got = Vec::new();
        history.push(10_000, &[10_000.0], |data| got.extend_from_slice(data)).unwrap();
        assert_eq!(got, [9_997.0, 9_998.0, 9_999.0, 10_000.0]);

        let mut missing = History::new(1, 4);
        missing.push(100, &[0.0; 8], |_| panic!("pending take wrote audio")).unwrap();
        missing.set_start(102);
        assert!(missing.push(108, &[], |_| panic!("shifted take was written")).is_err());
    }

    #[test]
    fn only_the_applied_ticket_starts_and_callback_access_has_no_heap_operations() {
        let (mut control, mut audio) = pair(EngineEpoch(1));
        let first = control.prepare().unwrap();
        assert_eq!(first.status(), Status::Pending);
        control.cancel();
        let second = control.prepare().unwrap();
        let ((old, new, status), heap) = measure(|| {
            let old = audio.start(first.ticket(), point(1));
            let new = audio.start(second.ticket(), point(2));
            (old, new, second.status())
        });
        assert!(heap.is_zero(), "{heap:?}");
        assert!(!old);
        assert!(new);
        assert_eq!(status, Status::Started(point(2)));
        assert_eq!(first.status(), Status::Cancelled);
        first.cancel();
        assert_eq!(second.status(), Status::Started(point(2)));
        audio.cancel(second.ticket());
        assert_eq!(second.status(), Status::Started(point(2)), "Stop must retain the applied boundary for final placement");
        assert!(!audio.start(second.ticket(), point(3)));
    }

    #[test]
    fn concurrent_rearm_never_exposes_a_torn_or_different_takes_boundary() {
        let (mut control, mut audio) = pair(EngineEpoch(1));
        let (send, receive) = std::sync::mpsc::sync_channel::<(Reader, u64)>(1);
        let worker = std::thread::spawn(move || {
            while let Ok((reader, n)) = receive.recv() {
                audio.start(reader.ticket(), point(n));
                if let Status::Started(p) = reader.status() {
                    assert_eq!(p, point(n));
                }
            }
        });
        for n in 0..10_000 {
            let reader = control.prepare().unwrap();
            send.send((reader.clone(), n)).unwrap();
            if let Status::Started(p) = reader.status() {
                assert_eq!(p, point(n));
            }
            control.cancel();
            match reader.status() {
                Status::Started(p) => assert_eq!(p, point(n)),
                Status::Cancelled => {}
                Status::Pending | Status::CountingIn | Status::Waiting => panic!("cancelled start is still pending"),
                Status::Failed(failure) => panic!("nothing refused this start: {failure:?}"),
            }
        }
        drop(send);
        worker.join().unwrap();
    }

    #[test]
    fn a_foreign_runtime_cannot_start_or_cancel_the_same_numbered_take() {
        let (mut first, _) = pair(EngineEpoch(1));
        let (mut second, mut audio) = pair(EngineEpoch(2));
        let a = first.prepare().unwrap();
        let b = second.prepare().unwrap();
        assert!(!audio.start(a.ticket(), point(1)));
        audio.cancel(a.ticket());
        assert_eq!(b.status(), Status::Pending);
        assert!(audio.start(b.ticket(), point(2)));
    }

    #[test]
    fn identities_exhaust_without_aliasing_a_previous_recording() {
        let (mut control, mut audio) = pair(EngineEpoch(1));
        // Sequences step by 8 (three flag bits): the last one issued is `u64::MAX - 7`.
        control.next = u64::MAX - 15;
        let last = control.prepare().unwrap();
        assert!(control.prepare().is_none());
        assert!(audio.start(last.ticket(), point(1)));
        assert_eq!(last.status(), Status::Started(point(1)));
    }
}
