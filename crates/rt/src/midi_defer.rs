//! Deferred MIDI for strips the render skipped (issue #124 gate 4, ADR 0029): a fixed-capacity
//! per-strip ring that carries the events of a skipped block forward to the strip's next
//! processed block, with the carry-forward ordering rule and the overflow rule.
//!
//! A strip whose [`Outcome`] is `Overrun(Busy)` or `Overrun(Unstarted)` never received its
//! block's events: [`should_defer`] says so, and the caller hands those events to
//! [`DeferSet::defer_block`] (or [`DeferSet::defer_for`], which checks the outcome itself). A
//! `Late` strip's worker consumed its events with the block, so they are never deferred; nor are
//! a `Faulted` strip's (its instance is suspect and gets replaced).
//!
//! **Ordering rule (replay).** At the strip's next processed block the caller calls
//! [`DeferSet::deliver`] (deferred events, then the block's own live events) or
//! [`DeferSet::drain_into`] (deferred events only). Every event lands at its original time
//! projected into the current block, `offset - (now - block) * block_len`, clamped to
//! `0..=block_len - 1`: an event from a skipped block is due in the past, so it lands at offset
//! 0 (shifted by whole blocks and clamped to the start of the next processed block). Delivery
//! keeps the original order, across several skipped blocks too, and offsets never decrease: an
//! event lands no earlier than the event delivered before it. When a note-off (or a note-on with
//! velocity 0) would land on the same sample as the note-on of the same channel and key delivered
//! in this call, it moves one sample later. If that would pass `block_len - 1` (the note-on is
//! already on the last sample), the note-off and everything after it stay in the ring for the
//! next processed block, where they land at offset 0: a note is never zero samples long and
//! order is never broken. Live events that follow carried events are deferred behind them.
//!
//! **Overflow rule**, per strip, when a ring cannot hold a block's events: the oldest events
//! outside the release class are dropped (note-ons, ordinary controllers, pitch bend, pressure,
//! program changes, anything that is not a channel release), the release class stays (note-off,
//! note-on with velocity 0, CC 64 below 64, CC 120 All Sound Off, CC 123 All Notes Off). If the
//! release class alone does not fit, the ring's whole content becomes exactly two events, CC 120
//! and CC 123 at offset 0 on the strip's channel ([`DeferSet::set_channel`], channel 1 by
//! default), and the strip is *silenced*: events deferred while it is silenced are dropped, and
//! the next drain emits only those two and clears the flag. Every dropped event and every
//! silence is a counted overflow fault ([`DeferStats`]).
//!
//! **One producer, one consumer, one thread.** The audio callback defers after a block and
//! drains at a later block; both take `&mut self`, so the ring needs no atomics and no lock. Only
//! the fault counters are atomics, so [`DeferStats`] (shared through an [`Arc`] taken off the
//! audio path) can be read from any thread.
//!
//! Everything is allocated in [`DeferSet::new`]; nothing after it allocates, locks or waits. Every
//! operation is bounded by the ring capacity plus the events passed in. No `unsafe`.

use crate::worker_set::{Outcome, OverrunCause};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The continuity test instrument (feature `testing`).
#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// MIDI status nibbles and controller numbers the rules look at.
const NOTE_OFF: u8 = 0x80;
const NOTE_ON: u8 = 0x90;
const CONTROL: u8 = 0xB0;
const CC_SUSTAIN: u8 = 64;
/// CC 120, All Sound Off.
pub const CC_ALL_SOUND_OFF: u8 = 120;
/// CC 123, All Notes Off.
pub const CC_ALL_NOTES_OFF: u8 = 123;
/// Channels times keys: the note table of one drain.
const NOTE_SLOTS: usize = 16 * 128;

/// One MIDI channel message as the ring stores it: the three bytes, the sample offset within its
/// block, and the block generation it belonged to. `Copy`, 16 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// The status byte (message type in the high nibble, channel 0..=15 in the low one).
    pub status: u8,
    /// The first data byte (key, controller number).
    pub data1: u8,
    /// The second data byte (velocity, controller value).
    pub data2: u8,
    /// The sample offset within its block. On delivery, the offset within the current block.
    pub offset: u32,
    /// The block generation the event belonged to; the ring stamps it on defer. Delivery keeps
    /// it, so a delivered event still names the block it was deferred from.
    pub block: u64,
}

const _: () = assert!(size_of::<Event>() == 16);

impl Event {
    /// A message at `offset` (block 0 until deferred).
    #[must_use]
    pub const fn new(status: u8, data1: u8, data2: u8, offset: u32) -> Self {
        Self { status, data1, data2, offset, block: 0 }
    }

    /// A note-on (`channel` 0..=15).
    #[must_use]
    pub const fn note_on(channel: u8, key: u8, velocity: u8, offset: u32) -> Self {
        Self::new(NOTE_ON | (channel & 0x0F), key, velocity, offset)
    }

    /// A note-off (`channel` 0..=15).
    #[must_use]
    pub const fn note_off(channel: u8, key: u8, velocity: u8, offset: u32) -> Self {
        Self::new(NOTE_OFF | (channel & 0x0F), key, velocity, offset)
    }

    /// A controller change (`channel` 0..=15).
    #[must_use]
    pub const fn control(channel: u8, cc: u8, value: u8, offset: u32) -> Self {
        Self::new(CONTROL | (channel & 0x0F), cc, value, offset)
    }

    /// The channel, 0..=15.
    #[must_use]
    pub const fn channel(&self) -> u8 {
        self.status & 0x0F
    }

    /// A note-on with a non-zero velocity.
    #[must_use]
    pub const fn is_note_on(&self) -> bool {
        self.status & 0xF0 == NOTE_ON && self.data2 > 0
    }

    /// A note-off, or a note-on with velocity 0.
    #[must_use]
    pub const fn is_note_off(&self) -> bool {
        let kind = self.status & 0xF0;
        kind == NOTE_OFF || (kind == NOTE_ON && self.data2 == 0)
    }

    /// The release class the overflow rule keeps: a note-off, a note-on with velocity 0, CC 64
    /// below 64 (sustain off), CC 120 (All Sound Off), CC 123 (All Notes Off).
    #[must_use]
    pub const fn is_release(&self) -> bool {
        if self.is_note_off() {
            return true;
        }
        self.status & 0xF0 == CONTROL
            && ((self.data1 == CC_SUSTAIN && self.data2 < 64) || self.data1 == CC_ALL_SOUND_OFF || self.data1 == CC_ALL_NOTES_OFF)
    }

    /// The note table index of a note message (channel and key).
    const fn note_slot(&self) -> usize {
        (self.channel() as usize) * 128 + (self.data1 & 0x7F) as usize
    }

    const fn at(self, offset: u32) -> Self {
        Self { offset, ..self }
    }

    const fn in_block(self, block: u64) -> Self {
        Self { block, ..self }
    }
}

/// True only for `Overrun(Busy)` and `Overrun(Unstarted)`: the strips that never received their
/// block's events. A `Late` strip's worker consumed them; `Mixed` and `Inline` were rendered; a
/// `Faulted` instance is replaced; `NotPublished` and `Pending` had no block to miss.
#[must_use]
pub const fn should_defer(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Overrun(OverrunCause::Busy | OverrunCause::Unstarted))
}

/// Where an event of block `block` at `offset` lands in block `now` of `block_len` samples: its
/// original time projected into the current block, clamped to `0..=block_len - 1` (0 for
/// `block_len == 0`). For an event of an earlier block this is 0.
#[must_use]
pub fn landing(offset: u32, block: u64, now: u64, block_len: u32) -> u32 {
    let last = u64::from(block_len.saturating_sub(1));
    let len = u64::from(block_len);
    let o = u64::from(offset);
    let at = if block <= now { o.saturating_sub((now - block).saturating_mul(len)) } else { o.saturating_add((block - now).saturating_mul(len)) };
    u32::try_from(at.min(last)).unwrap_or(0)
}

/// Overflow and carry counts, for one strip or the whole set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeferCounts {
    /// Events taken into a ring (after the overflow rule).
    pub deferred: u64,
    /// Events dropped by the overflow rule (an overflow fault each), including those discarded
    /// by a silence and those deferred while silenced.
    pub dropped: u64,
    /// Rings replaced by All Sound Off + All Notes Off (an overflow fault each).
    pub silences: u64,
    /// Events a drain left in the ring for the next block (the end-of-block note-off rule) and
    /// live events deferred behind them.
    pub carried: u64,
}

#[derive(Debug, Default)]
struct Counters {
    deferred: AtomicU64,
    dropped: AtomicU64,
    silences: AtomicU64,
    carried: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> DeferCounts {
        DeferCounts {
            deferred: self.deferred.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            silences: self.silences.load(Ordering::Relaxed),
            carried: self.carried.load(Ordering::Relaxed),
        }
    }
}

/// The overflow fault counters of a [`DeferSet`], per strip and for the set; readable from any
/// thread (relaxed atomics, each count exact on its own).
#[derive(Debug)]
pub struct DeferStats {
    strips: Box<[Counters]>,
    set: Counters,
    refused: AtomicU64,
}

impl DeferStats {
    /// The counts of `strip` (zeros for a strip out of range).
    #[must_use]
    pub fn strip(&self, strip: usize) -> DeferCounts {
        self.strips.get(strip).map(Counters::snapshot).unwrap_or_default()
    }

    /// The counts of the whole set.
    #[must_use]
    pub fn total(&self) -> DeferCounts {
        self.set.snapshot()
    }

    /// Calls refused for a strip index out of range (a caller fault).
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Overflow faults: dropped events plus silences.
    #[must_use]
    pub fn overflow_faults(&self) -> u64 {
        let t = self.total();
        t.dropped.saturating_add(t.silences)
    }

    fn add(&self, strip: usize, f: impl Fn(&Counters) -> &AtomicU64, n: u64) {
        if n == 0 {
            return;
        }
        if let Some(c) = self.strips.get(strip) {
            f(c).fetch_add(n, Ordering::Relaxed);
        }
        f(&self.set).fetch_add(n, Ordering::Relaxed);
    }
}

/// One strip's deferred events: a fixed-capacity buffer in original order, oldest first.
#[derive(Debug)]
pub struct DeferRing {
    buf: Box<[Event]>,
    len: usize,
    silenced: bool,
    channel: u8,
}

impl DeferRing {
    fn new(capacity: usize) -> Self {
        Self { buf: vec![Event::default(); capacity].into_boxed_slice(), len: 0, silenced: false, channel: 0 }
    }

    /// Events held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing is deferred.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The most events the ring holds.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// True after an overflow replaced the content with All Sound Off + All Notes Off, until the
    /// next drain.
    #[must_use]
    pub fn is_silenced(&self) -> bool {
        self.silenced
    }

    /// The deferred events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        self.buf.get(..self.len).unwrap_or(&[])
    }

    /// Appends `events` stamped with `block`, under the overflow rule.
    fn push(&mut self, strip: usize, block: u64, events: &[Event], stats: &DeferStats) {
        let n = events.len();
        if n == 0 {
            return;
        }
        if self.silenced {
            stats.add(strip, |c| &c.dropped, n as u64);
            return;
        }
        let free = self.buf.len() - self.len;
        if n <= free {
            for (slot, e) in self.buf[self.len..self.len + n].iter_mut().zip(events) {
                *slot = e.in_block(block);
            }
            self.len += n;
            stats.add(strip, |c| &c.deferred, n as u64);
            return;
        }
        let excess = n - free;
        let droppable = self.events().iter().chain(events).filter(|e| !e.is_release()).count();
        if droppable < excess {
            // The release class alone does not fit: silence the strip.
            stats.add(strip, |c| &c.dropped, (self.len + n) as u64);
            stats.add(strip, |c| &c.silences, 1);
            let ch = self.channel;
            self.buf[0] = Event::control(ch, CC_ALL_SOUND_OFF, 0, 0).in_block(block);
            self.buf[1] = Event::control(ch, CC_ALL_NOTES_OFF, 0, 0).in_block(block);
            self.len = 2;
            self.silenced = true;
            return;
        }
        // One pass: drop the oldest `excess` non-release events across the ring and the new ones.
        let mut to_drop = excess;
        let mut w = 0;
        for r in 0..self.len {
            let e = self.buf[r];
            if to_drop > 0 && !e.is_release() {
                to_drop -= 1;
                continue;
            }
            self.buf[w] = e;
            w += 1;
        }
        let kept_before = w;
        for e in events {
            if to_drop > 0 && !e.is_release() {
                to_drop -= 1;
                continue;
            }
            self.buf[w] = e.in_block(block);
            w += 1;
        }
        debug_assert_eq!(w, self.buf.len());
        self.len = w;
        stats.add(strip, |c| &c.deferred, (w - kept_before) as u64);
        stats.add(strip, |c| &c.dropped, excess as u64);
    }
}

/// What one [`DeferSet::deliver`] or [`DeferSet::drain_into`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// Deferred events delivered.
    pub deferred: usize,
    /// Live events delivered.
    pub live: usize,
    /// Events left in the ring for the next processed block (the end-of-block note-off rule,
    /// and the live events behind it).
    pub carried: usize,
    /// The ring was silenced: the deferred events delivered were All Sound Off + All Notes Off.
    pub silenced: bool,
    /// The offset of the last event delivered (0 when none): a caller merging its own events
    /// after a [`DeferSet::drain_into`] delivers them no earlier than this.
    pub floor: u32,
}

/// Places events in one block: the projection, the non-decreasing floor and the collapsed-pair
/// rule.
struct Placer<'a> {
    notes: &'a mut [(u64, u32)],
    stamp: u64,
    now: u64,
    block_len: u32,
    floor: u32,
}

impl Placer<'_> {
    /// The offset `e` lands on, or `None` when it must wait for the next block.
    fn place(&mut self, e: &Event) -> Option<u32> {
        let last = self.block_len.checked_sub(1)?;
        let mut at = landing(e.offset, e.block, self.now, self.block_len).max(self.floor);
        let slot = e.note_slot();
        if e.is_note_off() && self.notes.get(slot).is_some_and(|&(s, on)| s == self.stamp && on == at) {
            at += 1;
            if at > last {
                return None;
            }
        }
        if e.is_note_on()
            && let Some(n) = self.notes.get_mut(slot)
        {
            *n = (self.stamp, at);
        }
        self.floor = at;
        Some(at)
    }
}

/// A [`DeferRing`] per strip, all allocated at construction, plus the overflow counters.
#[derive(Debug)]
pub struct DeferSet {
    rings: Box<[DeferRing]>,
    /// Per channel and key: (drain stamp, offset) of the last note-on delivered in that drain.
    notes: Box<[(u64, u32)]>,
    stamp: u64,
    stats: Arc<DeferStats>,
}

impl DeferSet {
    /// `strips` rings of `capacity` events each (at least 2, the silence). Allocates; call it off
    /// the audio path.
    #[must_use]
    pub fn new(strips: usize, capacity: usize) -> Self {
        let capacity = capacity.max(2);
        Self {
            rings: (0..strips).map(|_| DeferRing::new(capacity)).collect(),
            notes: vec![(0, 0); NOTE_SLOTS].into_boxed_slice(),
            stamp: 0,
            stats: Arc::new(DeferStats {
                strips: (0..strips).map(|_| Counters::default()).collect(),
                set: Counters::default(),
                refused: AtomicU64::new(0),
            }),
        }
    }

    /// Strips in the set.
    #[must_use]
    pub fn strips(&self) -> usize {
        self.rings.len()
    }

    /// The ring of `strip`.
    #[must_use]
    pub fn ring(&self, strip: usize) -> Option<&DeferRing> {
        self.rings.get(strip)
    }

    /// The counters. Clone the `Arc` off the audio path to read them from another thread.
    #[must_use]
    pub fn stats(&self) -> &Arc<DeferStats> {
        &self.stats
    }

    /// The channel (0..=15) a silence of `strip` sends All Sound Off and All Notes Off on.
    pub fn set_channel(&mut self, strip: usize, channel: u8) {
        if let Some(r) = self.rings.get_mut(strip) {
            r.channel = channel & 0x0F;
        } else {
            self.refuse();
        }
    }

    fn refuse(&self) {
        self.stats.refused.fetch_add(1, Ordering::Relaxed);
    }

    /// Defers the events block `generation` would have delivered to `strip`, in order, with
    /// their offsets, under the overflow rule. Call it only for a strip [`should_defer`] names.
    pub fn defer_block(&mut self, strip: usize, generation: u64, events: &[Event]) {
        let Some(ring) = self.rings.get_mut(strip) else {
            self.refuse();
            return;
        };
        ring.push(strip, generation, events, &self.stats);
    }

    /// [`DeferSet::defer_block`] when [`should_defer`] holds for `outcome`; otherwise nothing.
    /// Returns whether the events were deferred.
    pub fn defer_for(&mut self, strip: usize, generation: u64, outcome: Outcome, events: &[Event]) -> bool {
        if !should_defer(outcome) {
            return false;
        }
        self.defer_block(strip, generation, events);
        true
    }

    /// Delivers `strip`'s deferred events into block `now` of `block_len` samples under the
    /// ordering rule (see the module docs). The delivered events carry their landing offset.
    pub fn drain_into(&mut self, strip: usize, now: u64, block_len: u32, out: &mut impl FnMut(Event)) -> Drained {
        self.deliver(strip, now, block_len, &[], out)
    }

    /// Delivers `strip`'s deferred events and then `live`, the processed block's own events (in
    /// order, offsets within the block), under one ordering rule: offsets never decrease and a
    /// collapsed on/off pair is split, across the two groups too. Live events that cannot be
    /// delivered in order (behind carried events) are deferred with generation `now`.
    pub fn deliver(&mut self, strip: usize, now: u64, block_len: u32, live: &[Event], out: &mut impl FnMut(Event)) -> Drained {
        let Self { rings, notes, stamp, stats } = self;
        let Some(ring) = rings.get_mut(strip) else {
            stats.refused.fetch_add(1, Ordering::Relaxed);
            return Drained::default();
        };
        *stamp = stamp.wrapping_add(1);
        let mut placer = Placer { notes, stamp: *stamp, now, block_len, floor: 0 };
        let mut done = Drained::default();
        let mut taken = 0;
        if ring.silenced && block_len > 0 {
            for e in ring.events() {
                out(e.at(0));
            }
            taken = ring.len;
            done.silenced = true;
            ring.silenced = false;
        } else {
            for e in ring.events() {
                let Some(at) = placer.place(e) else { break };
                out(e.at(at));
                taken += 1;
            }
        }
        ring.buf.copy_within(taken..ring.len, 0);
        ring.len -= taken;
        done.deferred = taken;
        let carried_deferred = ring.len;
        if ring.len == 0 {
            for e in live {
                let e = e.in_block(now);
                let Some(at) = placer.place(&e) else { break };
                out(e.at(at));
                done.live += 1;
            }
        }
        let rest = live.get(done.live..).unwrap_or(&[]);
        ring.push(strip, now, rest, stats);
        done.carried = ring.len;
        if block_len > 0 {
            stats.add(strip, |c| &c.carried, (carried_deferred + rest.len()) as u64);
        }
        done.floor = placer.floor;
        done
    }
}

#[cfg(test)]
mod tests;
