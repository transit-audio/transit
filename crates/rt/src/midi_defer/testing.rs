//! The continuity test instrument (issue #124 gate 6, ADR 0029): a fake strip that counts the
//! samples it processed against the transport position it was given per block, records every
//! hole (transport time it never processed) and every event with the sample it landed on, and
//! tracks note state, so a test can assert that a skipped block costs only its audio, never a
//! note.
//!
//! It is shaped like a strip render: one [`ContinuityInstrument::process`] per processed block,
//! with the block's transport position, its length and the events delivered for it. Slice 3
//! drives it directly; slice 4 drives it through a real `WorkerSet` with injected stalls (behind
//! a lock or a per-strip cell, since [`crate::worker_set::StripRunner::run`] takes `&self`).
//!
//! Its trace and hole lists are preallocated with the capacities given to
//! [`ContinuityInstrument::new`]; past them entries are counted, not stored, so `process`
//! allocates nothing.

use super::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, Event, NOTE_SLOTS};

/// Transport time the instrument never processed: `samples` samples from sample `at`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hole {
    /// The first transport sample missed.
    pub at: u64,
    /// How many samples were missed.
    pub samples: u64,
}

/// One event as the instrument received it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Landed {
    /// The transport sample it landed on (block position plus offset).
    pub sample: u64,
    /// The event as delivered (its `offset` is the offset in the processed block).
    pub event: Event,
}

/// Faults the instrument saw. A correct carry-forward leaves all of them zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContinuityFaults {
    /// A block that started before the end of the previous one (time processed twice).
    pub overlaps: u64,
    /// An event offset at or past the block length.
    pub out_of_block: u64,
    /// An event delivered at an earlier offset than the one before it in the same block.
    pub unsorted: u64,
    /// A note-off for a note that was not on.
    pub orphan_offs: u64,
    /// A note-off on the same sample as its note-on (a zero-length note).
    pub zero_length: u64,
    /// A note-on for a note already on.
    pub restrikes: u64,
    /// Trace or hole entries counted but not stored (capacity reached).
    pub unrecorded: u64,
}

/// What a run looked like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContinuityReport {
    /// Blocks processed.
    pub blocks: u64,
    /// Samples processed.
    pub processed_samples: u64,
    /// Holes seen (each a run of transport time never processed).
    pub holes: u64,
    /// Samples in those holes.
    pub hole_samples: u64,
    /// Notes on with no note-off (at the time of the report).
    pub stuck_notes: u64,
    /// Channels with the sustain pedal down (at the time of the report).
    pub stuck_pedals: u64,
    /// Notes ended by All Sound Off or All Notes Off.
    pub silenced_notes: u64,
    /// Events received.
    pub events: u64,
    /// Faults.
    pub faults: ContinuityFaults,
}

/// The continuity test instrument. See the module docs.
#[derive(Debug)]
pub struct ContinuityInstrument {
    next: Option<u64>,
    report: ContinuityReport,
    holes: Vec<Hole>,
    trace: Vec<Landed>,
    /// Per channel and key: the transport sample of the note-on that holds it.
    open: Box<[Option<u64>]>,
    pedal: [bool; 16],
}

impl ContinuityInstrument {
    /// An instrument that stores up to `trace` events and `holes` holes (allocates; build it off
    /// the audio path).
    #[must_use]
    pub fn new(trace: usize, holes: usize) -> Self {
        Self {
            next: None,
            report: ContinuityReport::default(),
            holes: Vec::with_capacity(holes),
            trace: Vec::with_capacity(trace),
            open: vec![None; NOTE_SLOTS].into_boxed_slice(),
            pedal: [false; 16],
        }
    }

    /// Sets the transport sample the first block is expected at, so blocks missed before the
    /// first processed one count as a hole too. Without it the first block sets the start.
    pub fn expect_from(&mut self, transport: u64) {
        self.next = Some(transport);
    }

    /// Processes one block of `frames` samples at transport sample `transport`, receiving
    /// `events` in delivery order (offsets within this block).
    pub fn process(&mut self, transport: u64, frames: u32, events: &[Event]) {
        if let Some(next) = self.next {
            if transport > next {
                self.report.holes += 1;
                self.report.hole_samples += transport - next;
                self.store_hole(Hole { at: next, samples: transport - next });
            } else if transport < next {
                self.report.faults.overlaps += 1;
            }
        }
        let mut prev = 0;
        for e in events {
            if e.offset >= frames {
                self.report.faults.out_of_block += 1;
            }
            if e.offset < prev {
                self.report.faults.unsorted += 1;
            }
            prev = e.offset;
            self.receive(transport + u64::from(e.offset), *e);
        }
        self.report.blocks += 1;
        self.report.processed_samples += u64::from(frames);
        self.next = Some(transport + u64::from(frames));
    }

    fn store_hole(&mut self, hole: Hole) {
        if self.holes.len() < self.holes.capacity() {
            self.holes.push(hole);
        } else {
            self.report.faults.unrecorded += 1;
        }
    }

    fn receive(&mut self, sample: u64, event: Event) {
        self.report.events += 1;
        if self.trace.len() < self.trace.capacity() {
            self.trace.push(Landed { sample, event });
        } else {
            self.report.faults.unrecorded += 1;
        }
        let ch = usize::from(event.channel());
        let slot = event.note_slot();
        if event.is_note_on() {
            if let Some(n) = self.open.get_mut(slot) {
                if n.is_some() {
                    self.report.faults.restrikes += 1;
                }
                *n = Some(sample);
            }
        } else if event.is_note_off() {
            match self.open.get_mut(slot).and_then(Option::take) {
                Some(on) if on >= sample => self.report.faults.zero_length += 1,
                Some(_) => {}
                None => self.report.faults.orphan_offs += 1,
            }
        } else if event.status & 0xF0 == 0xB0 {
            match event.data1 {
                64 => {
                    if let Some(p) = self.pedal.get_mut(ch) {
                        *p = event.data2 >= 64;
                    }
                }
                CC_ALL_SOUND_OFF | CC_ALL_NOTES_OFF => {
                    let base = ch * 128;
                    for n in self.open.iter_mut().skip(base).take(128) {
                        if n.take().is_some() {
                            self.report.silenced_notes += 1;
                        }
                    }
                    if event.data1 == CC_ALL_SOUND_OFF
                        && let Some(p) = self.pedal.get_mut(ch)
                    {
                        *p = false;
                    }
                }
                _ => {}
            }
        }
    }

    /// The report so far, with the stuck notes and pedals as of now.
    #[must_use]
    pub fn report(&self) -> ContinuityReport {
        ContinuityReport {
            stuck_notes: self.open.iter().filter(|n| n.is_some()).count() as u64,
            stuck_pedals: self.pedal.iter().filter(|&&p| p).count() as u64,
            ..self.report
        }
    }

    /// Notes on with no note-off.
    #[must_use]
    pub fn stuck_notes(&self) -> u64 {
        self.report().stuck_notes
    }

    /// The holes, in transport order.
    #[must_use]
    pub fn holes(&self) -> &[Hole] {
        &self.holes
    }

    /// Every event received, with the transport sample it landed on.
    #[must_use]
    pub fn trace(&self) -> &[Landed] {
        &self.trace
    }
}
