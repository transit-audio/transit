//! MIDI recording capture: the audio thread copies every admitted live event, with the device
//! frame it is placed at, into a bounded lock-free queue; the control side drains it into a
//! take list. The queue is preallocated when the player is prepared, so the callback never
//! allocates, locks or waits; a full queue drops the event and counts it.
//!
//! A second, smaller queue carries render-overrun marks ([`OverrunMark`], ADR 0029): a block in
//! which a render worker's strip was silence while the pass recorded. They go into the take
//! journal; the take itself is unaffected (events are captured at admission, before any render).
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use transit_mix::Message;
use transit_model::TrackId;

/// The most events one recording pass holds, in the queue and in the control-side take list.
pub const CAPACITY: usize = 64 * 1024;
/// Slots kept free for releases (note-offs, pedal-ups): losing one leaves a note open.
const RELEASE_RESERVE: usize = 1024;

/// One admitted event as the instrument receives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedEvent {
    /// The output-device frame the event is placed at (fitted from the device clock, else the
    /// approximate callback-relative placement).
    pub frame: u64,
    pub track: TrackId,
    pub channel: u8,
    pub message: Message,
    pub approximate: bool,
}

/// Overrun marks one pass keeps (a mark per block at most).
pub const MARK_CAPACITY: usize = 1024;

/// A block in which strips were silence because their render worker overran (ADR 0029), while a
/// pass recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrunMark {
    /// The output-device frame of the block's first sample.
    pub frame: u64,
    /// Strips silent in that block.
    pub strips: u32,
}

impl From<OverrunMark> for (u64, u32) {
    /// `(frame, strips)`, as the take journal writes a mark.
    fn from(m: OverrunMark) -> Self {
        (m.frame, m.strips)
    }
}

#[derive(Default)]
struct Counters {
    /// Events the audio side dropped because the queue was full.
    dropped: AtomicU64,
    /// Events the audio side has committed, for the UI's provisional notes.
    committed: AtomicU64,
}

/// The audio thread's producer. All operations are bounded and allocation free.
pub struct Sink {
    queue: rtrb::Producer<CapturedEvent>,
    marks: rtrb::Producer<OverrunMark>,
    counters: Arc<Counters>,
}

/// The control side's consumer and take list.
pub struct Capture {
    queue: rtrb::Consumer<CapturedEvent>,
    marks_rx: rtrb::Consumer<OverrunMark>,
    counters: Arc<Counters>,
    events: Vec<CapturedEvent>,
    marks: Vec<OverrunMark>,
    dropped_seen: u64,
    /// Events the take list refused (it holds [`CAPACITY`]).
    overflow: u64,
    #[cfg(feature = "test-hooks")]
    injected: Vec<CapturedEvent>,
}

pub fn pair(capacity: usize) -> (Capture, Sink) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(1));
    let (marks, marks_rx) = rtrb::RingBuffer::new(MARK_CAPACITY);
    let counters = Arc::new(Counters::default());
    (
        Capture {
            queue: consumer,
            marks_rx,
            counters: Arc::clone(&counters),
            events: Vec::new(),
            marks: Vec::new(),
            dropped_seen: 0,
            overflow: 0,
            #[cfg(feature = "test-hooks")]
            injected: Vec::new(),
        },
        Sink { queue: producer, marks, counters },
    )
}

impl Sink {
    /// Push one event; false when it was dropped (and counted). A non-release event leaves
    /// [`RELEASE_RESERVE`] slots to releases.
    pub fn push(&mut self, event: CapturedEvent) -> bool {
        let free = self.queue.slots();
        let room = if event.message.is_release() { free > 0 } else { free > RELEASE_RESERVE.min(self.queue.buffer().capacity() / 2) };
        if room && self.queue.push(event).is_ok() {
            self.counters.committed.fetch_add(1, Ordering::Release);
            true
        } else {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Push one overrun mark; false when the mark queue was full (the mark is lost, the take is
    /// not). Bounded and allocation free.
    pub fn mark_overrun(&mut self, mark: OverrunMark) -> bool {
        self.marks.push(mark).is_ok()
    }
}

impl Capture {
    /// Move everything the audio side committed into the take list. Control thread, any rate.
    pub fn poll(&mut self) {
        while let Ok(event) = self.queue.pop() {
            if self.events.len() < CAPACITY {
                self.events.push(event);
            } else {
                self.overflow += 1;
            }
        }
        while let Ok(mark) = self.marks_rx.pop() {
            if self.marks.len() < MARK_CAPACITY {
                self.marks.push(mark);
            }
        }
        #[cfg(feature = "test-hooks")]
        self.events.append(&mut self.injected);
    }

    /// The events captured so far, oldest first (after [`Capture::poll`]).
    pub fn events(&self) -> &[CapturedEvent] {
        &self.events
    }

    /// The overrun marks of the pass so far, oldest first (after [`Capture::poll`]).
    pub fn marks(&self) -> &[OverrunMark] {
        &self.marks
    }

    /// Events lost to a full queue or take list since the last [`Capture::clear`].
    pub fn dropped(&self) -> u64 {
        self.counters.dropped.load(Ordering::Relaxed).saturating_sub(self.dropped_seen) + self.overflow
    }

    /// Events the audio side has committed since the player was prepared.
    pub fn committed(&self) -> u64 {
        self.counters.committed.load(Ordering::Acquire)
    }

    /// Start a new pass: forget the take list and the drop count. Leftovers still in the queue
    /// from before the pass are discarded.
    pub fn clear(&mut self) {
        while self.queue.pop().is_ok() {}
        while self.marks_rx.pop().is_ok() {}
        self.events.clear();
        self.marks.clear();
        self.overflow = 0;
        self.dropped_seen = self.counters.dropped.load(Ordering::Relaxed);
        #[cfg(feature = "test-hooks")]
        self.injected.clear();
    }

    /// Take the list and the drop count, ending the pass (the overrun marks stay readable through
    /// [`Capture::take_marks`] until the next [`Capture::clear`]).
    pub fn finish(&mut self) -> (Vec<CapturedEvent>, u64) {
        self.poll();
        let dropped = self.dropped();
        let events = std::mem::take(&mut self.events);
        self.overflow = 0;
        self.dropped_seen = self.counters.dropped.load(Ordering::Relaxed);
        (events, dropped)
    }

    /// The pass's overrun marks, taken.
    pub fn take_marks(&mut self) -> Vec<OverrunMark> {
        std::mem::take(&mut self.marks)
    }

    /// Stand in for the audio thread in device-free tests.
    #[cfg(feature = "test-hooks")]
    pub fn inject(&mut self, event: CapturedEvent) {
        self.injected.push(event);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use transit_mix::NoteId;
    use transit_rt::testing::measure;

    fn on(frame: u64) -> CapturedEvent {
        CapturedEvent {
            frame,
            track: TrackId(1),
            channel: 0,
            message: Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE },
            approximate: false,
        }
    }
    fn off(frame: u64) -> CapturedEvent {
        CapturedEvent { message: Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE }, ..on(frame) }
    }

    #[test]
    fn pushing_allocates_nothing_and_keeps_frames_in_order() {
        let (mut capture, mut sink) = pair(64);
        let (pushed, heap) = measure(|| (0..10).filter(|i| sink.push(on(*i * 100))).count());
        assert!(heap.is_zero(), "{heap:?}");
        assert_eq!(pushed, 10);
        assert_eq!(capture.committed(), 10);
        capture.poll();
        assert_eq!(capture.events().iter().map(|e| e.frame).collect::<Vec<_>>(), (0..10).map(|i| i * 100).collect::<Vec<_>>());
        assert_eq!(capture.dropped(), 0);
    }

    #[test]
    fn a_full_queue_drops_and_counts_but_keeps_room_for_releases() {
        let (mut capture, mut sink) = pair(8);
        let ((ons, offs), heap) = measure(|| {
            let ons = (0..8).filter(|i| sink.push(on(*i))).count();
            let offs = (0..8).filter(|i| sink.push(off(100 + *i))).count();
            (ons, offs)
        });
        assert!(heap.is_zero(), "{heap:?}");
        // Half the queue is reserved for releases in a queue this small.
        assert_eq!((ons, offs), (4, 4));
        assert_eq!(capture.dropped(), 8);
        capture.poll();
        assert_eq!(capture.events().len(), 8);
        let (events, dropped) = capture.finish();
        assert_eq!((events.len(), dropped), (8, 8));
        assert_eq!(capture.dropped(), 0, "finish ends the pass");
    }

    #[test]
    fn overrun_marks_travel_beside_the_events_allocation_free() {
        let (mut capture, mut sink) = pair(16);
        let (ok, heap) = measure(|| sink.push(on(5)) && sink.mark_overrun(OverrunMark { frame: 128, strips: 2 }));
        assert!(ok && heap.is_zero(), "{heap:?}");
        capture.poll();
        assert_eq!(capture.events().len(), 1);
        assert_eq!(capture.marks(), [OverrunMark { frame: 128, strips: 2 }]);
        assert_eq!(capture.take_marks().len(), 1);
        sink.mark_overrun(OverrunMark { frame: 256, strips: 1 });
        capture.clear();
        capture.poll();
        assert!(capture.marks().is_empty(), "a new pass forgets the old marks");
    }

    #[test]
    fn clear_discards_what_came_before_the_pass() {
        let (mut capture, mut sink) = pair(16);
        sink.push(on(1));
        capture.poll();
        sink.push(on(2));
        capture.clear();
        assert!(capture.events().is_empty());
        capture.poll();
        assert!(capture.events().is_empty());
    }

    #[test]
    fn draining_for_the_journal_leaves_the_audio_side_push_allocation_free() {
        // The control side polls and reads `events()` every frame to append them to the MIDI
        // journal; the audio side's `push` stays bounded and allocation free around it.
        let (mut capture, mut sink) = pair(64);
        for round in 0..4 {
            let (pushed, heap) = measure(|| (0..8).filter(|i| sink.push(on(round * 100 + *i))).count());
            assert!(heap.is_zero(), "{heap:?}");
            assert_eq!(pushed, 8);
            capture.poll();
            assert_eq!(capture.events().len(), usize::try_from((round + 1) * 8).unwrap());
        }
    }
}
