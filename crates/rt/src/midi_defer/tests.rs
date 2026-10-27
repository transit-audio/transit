//! Gate 4 (#124): the carry-forward ordering rule, the overflow rule, the outcome rule, the
//! allocation proof, and the continuity instrument fed by the ring.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::cast_possible_truncation)]

use super::testing::{ContinuityFaults, ContinuityInstrument, Hole};
use super::*;
use crate::testing::measure;

/// A small xorshift generator: deterministic per seed, no dependency.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn drain(set: &mut DeferSet, strip: usize, now: u64, block_len: u32) -> (Vec<Event>, Drained) {
    let mut out = Vec::new();
    let d = set.drain_into(strip, now, block_len, &mut |e| out.push(e));
    (out, d)
}

fn deliver(set: &mut DeferSet, strip: usize, now: u64, block_len: u32, live: &[Event]) -> (Vec<Event>, Drained) {
    let mut out = Vec::new();
    let d = set.deliver(strip, now, block_len, live, &mut |e| out.push(e));
    (out, d)
}

fn bytes(e: &Event) -> (u8, u8, u8) {
    (e.status, e.data1, e.data2)
}

fn offsets(es: &[Event]) -> Vec<u32> {
    es.iter().map(|e| e.offset).collect()
}

// ---- The outcome rule ----

#[test]
fn only_busy_and_unstarted_overruns_defer() {
    let all = [
        (Outcome::NotPublished, false),
        (Outcome::Pending, false),
        (Outcome::Mixed, false),
        (Outcome::Inline, false),
        (Outcome::Overrun(OverrunCause::Busy), true),
        (Outcome::Overrun(OverrunCause::Late), false),
        (Outcome::Overrun(OverrunCause::Unstarted), true),
        (Outcome::Faulted, false),
    ];
    for (outcome, defer) in all {
        assert_eq!(should_defer(outcome), defer, "{outcome:?}");
        let mut set = DeferSet::new(1, 16);
        let took = set.defer_for(0, 5, outcome, &[Event::note_on(0, 60, 100, 3)]);
        assert_eq!(took, defer, "{outcome:?}");
        assert_eq!(set.ring(0).unwrap().len(), usize::from(defer), "a Late strip consumed its events: {outcome:?}");
    }
}

// ---- The ordering rule ----

#[test]
fn landing_projects_into_the_current_block_and_clamps() {
    // Same block: the offset stands; past the end it clamps to the last sample.
    assert_eq!(landing(17, 4, 4, 64), 17);
    assert_eq!(landing(100, 4, 4, 64), 63);
    // One and two blocks back: the original time is in the past, so offset 0.
    assert_eq!(landing(63, 3, 4, 64), 0);
    assert_eq!(landing(0, 2, 4, 64), 0);
    assert_eq!(landing(u32::MAX, 0, u64::MAX, u32::MAX), 0, "no overflow");
    // A block from the future clamps to the last sample, never past it.
    assert_eq!(landing(5, 5, 4, 64), 63);
    assert_eq!(landing(5, u64::MAX, 0, u32::MAX), u32::MAX - 1);
    assert_eq!(landing(5, 1, 1, 0), 0, "an empty block");
}

#[test]
fn one_skipped_block_replays_in_order_shifted_by_one_block() {
    let mut set = DeferSet::new(2, 64);
    let skipped = [Event::control(0, 1, 40, 2), Event::note_on(0, 60, 90, 10), Event::control(0, 7, 100, 10), Event::note_on(0, 64, 80, 31)];
    set.defer_block(1, 7, &skipped);
    let ring = set.ring(1).unwrap();
    assert_eq!(offsets(ring.events()), [2, 10, 10, 31], "the ring keeps the original offsets");
    assert!(ring.events().iter().all(|e| e.block == 7), "stamped with the skipped generation");
    let (out, d) = drain(&mut set, 1, 8, 32);
    assert_eq!(out.iter().map(bytes).collect::<Vec<_>>(), skipped.iter().map(bytes).collect::<Vec<_>>(), "original order");
    // Shifted by one block of 32: 2 - 32, 10 - 32, 31 - 32 are all before the block, so 0.
    assert_eq!(offsets(&out), [0, 0, 0, 0]);
    assert!(out.iter().all(|e| e.block == 7), "a delivered event still names its block");
    assert_eq!(d, Drained { deferred: 4, live: 0, carried: 0, silenced: false, floor: 0 });
    assert!(set.ring(1).unwrap().is_empty());
    assert!(set.ring(0).unwrap().is_empty(), "other strips untouched");
    assert_eq!(drain(&mut set, 1, 9, 32).0, [], "drained once");
}

#[test]
fn two_skipped_blocks_replay_shifted_by_two_and_one_in_original_order() {
    let mut set = DeferSet::new(1, 64);
    let first = [Event::note_on(0, 60, 90, 40), Event::control(0, 74, 9, 50)];
    let second = [Event::note_on(0, 62, 90, 1), Event::note_off(0, 60, 0, 5)];
    set.defer_block(0, 10, &first);
    set.defer_block(0, 11, &second);
    let (out, d) = drain(&mut set, 0, 12, 64);
    let want: Vec<_> = first.iter().chain(&second).map(bytes).collect();
    assert_eq!(out.iter().map(bytes).collect::<Vec<_>>(), want, "the later block's early offsets do not jump ahead");
    assert_eq!(out.iter().map(|e| e.block).collect::<Vec<_>>(), [10, 10, 11, 11]);
    // 40 - 128, 50 - 128, 1 - 64, 5 - 64: all clamp to 0; the off of key 60 then collapses onto
    // its on from the earlier block and moves one sample later.
    assert_eq!(offsets(&out), [0, 0, 0, 1]);
    assert_eq!(d.deferred, 4);
}

#[test]
fn offsets_clamp_at_the_last_sample_and_never_decrease() {
    // A drain for the deferral's own generation (a block that shrank) keeps offsets and clamps.
    let mut set = DeferSet::new(1, 64);
    set.defer_block(0, 3, &[Event::control(0, 1, 1, 5), Event::control(0, 1, 2, 200), Event::control(0, 1, 3, 100)]);
    let (out, _) = drain(&mut set, 0, 3, 128);
    assert_eq!(offsets(&out), [5, 127, 127], "clamped at block_len - 1 and never earlier than the event before");
    // Offsets out of order in the input are delivered in order, never earlier than the one before.
    set.defer_block(0, 4, &[Event::control(0, 1, 1, 50), Event::control(0, 1, 2, 10)]);
    let (out, d) = drain(&mut set, 0, 4, 128);
    assert_eq!(offsets(&out), [50, 50]);
    assert_eq!(d.floor, 50);
}

#[test]
fn a_collapsed_on_off_pair_gets_the_off_one_sample_later() {
    let mut set = DeferSet::new(1, 64);
    set.defer_block(
        0,
        1,
        &[
            Event::note_on(0, 60, 100, 3),
            Event::note_on(1, 60, 100, 4), // another channel, same key
            Event::note_on(0, 61, 100, 5), // same channel, another key
            Event::note_off(0, 60, 0, 9),
            Event::note_on(0, 62, 100, 10),
            Event::note_on(0, 62, 0, 11), // velocity 0 is a note-off
        ],
    );
    let (out, _) = drain(&mut set, 0, 2, 64);
    assert_eq!(offsets(&out), [0, 0, 0, 1, 1, 2]);
    // The other channel's note-off and the other key are not moved by key 60's pair.
    set.defer_block(0, 3, &[Event::note_off(1, 60, 0, 0), Event::note_off(0, 61, 0, 0)]);
    let (out, _) = drain(&mut set, 0, 4, 64);
    assert_eq!(offsets(&out), [0, 0], "their note-ons were delivered in an earlier block");
}

#[test]
fn an_off_whose_on_is_on_the_last_sample_is_carried_to_the_next_block() {
    // Block of 4: three collapsed pairs of one key push the fourth off past the last sample.
    let mut set = DeferSet::new(1, 64);
    let pairs: Vec<Event> = (0..4).flat_map(|_| [Event::note_on(0, 60, 100, 0), Event::note_off(0, 60, 0, 0)]).collect();
    set.defer_block(0, 1, &pairs);
    let tail = [Event::control(0, 1, 9, 0)];
    set.defer_block(0, 1, &tail);
    let (out, d) = drain(&mut set, 0, 2, 4);
    assert_eq!(offsets(&out), [0, 1, 1, 2, 2, 3, 3], "the fourth on is on the last sample");
    assert_eq!((d.deferred, d.carried), (7, 2), "the off and the event behind it wait");
    assert_eq!(set.stats().strip(0).carried, 2);
    // A live event of this block goes behind the carried ones, never ahead of them.
    let (out2, d2) = deliver(&mut set, 0, 3, 4, &[Event::control(0, 2, 1, 1)]);
    assert_eq!(out2.iter().map(bytes).collect::<Vec<_>>(), [(0x80, 60, 0), (0xB0, 1, 9), (0xB0, 2, 1)]);
    assert_eq!(offsets(&out2), [0, 0, 1]);
    assert_eq!(d2, Drained { deferred: 2, live: 1, carried: 0, silenced: false, floor: 1 });
}

#[test]
fn live_events_that_follow_carried_events_are_deferred_behind_them() {
    let mut set = DeferSet::new(1, 64);
    set.defer_block(0, 1, &[Event::note_on(0, 60, 100, 0), Event::note_off(0, 60, 0, 0)]);
    // A block of one sample: the on takes sample 0, the off cannot be split from it.
    let live = [Event::note_on(0, 61, 100, 0), Event::note_off(0, 61, 0, 0)];
    let (out, d) = deliver(&mut set, 0, 2, 1, &live);
    assert_eq!(out.len(), 1);
    assert_eq!((d.deferred, d.live, d.carried), (1, 0, 3));
    assert_eq!(set.ring(0).unwrap().events().iter().map(|e| e.block).collect::<Vec<_>>(), [1, 2, 2]);
    assert_eq!(set.stats().strip(0).carried, 3);
}

#[test]
fn live_events_are_floored_behind_the_deferred_and_split_from_their_on() {
    let mut set = DeferSet::new(1, 64);
    set.defer_block(0, 4, &[Event::note_on(0, 60, 100, 20)]);
    let live = [Event::note_off(0, 60, 0, 0), Event::note_on(0, 61, 100, 0), Event::control(0, 1, 2, 30)];
    let (out, d) = deliver(&mut set, 0, 5, 64, &live);
    assert_eq!(offsets(&out), [0, 1, 1, 30], "the live off is split from the deferred on; the rest keep order");
    assert!(out.iter().skip(1).all(|e| e.block == 5), "live events carry the current generation");
    assert_eq!((d.deferred, d.live, d.carried, d.floor), (1, 3, 0, 30));
    // With nothing deferred, live events keep their offsets.
    let (out, _) = deliver(&mut set, 0, 6, 64, &[Event::note_on(0, 1, 1, 7), Event::note_off(0, 1, 0, 9)]);
    assert_eq!(offsets(&out), [7, 9]);
}

#[test]
fn an_empty_block_delivers_nothing_and_keeps_everything() {
    let mut set = DeferSet::new(1, 8);
    set.defer_block(0, 1, &[Event::note_on(0, 60, 1, 0)]);
    let (out, d) = deliver(&mut set, 0, 2, 0, &[Event::note_off(0, 60, 0, 0)]);
    assert_eq!(out, []);
    assert_eq!(d.carried, 2);
    assert_eq!(drain(&mut set, 0, 3, 8).0.len(), 2);
}

// ---- The overflow rule ----

#[test]
fn overflow_drops_the_oldest_non_release_events_and_keeps_the_release_class() {
    let mut set = DeferSet::new(3, 8);
    let fill = [
        Event::note_on(0, 60, 100, 0), // 0 droppable
        Event::note_off(0, 59, 0, 1),  // release
        Event::control(0, 1, 10, 2),   // droppable
        Event::control(0, 64, 0, 3),   // release (sustain off)
        Event::new(0xE0, 0, 64, 4),    // droppable (pitch bend)
        Event::control(0, 64, 127, 5), // droppable (sustain on)
        Event::new(0xD0, 30, 0, 6),    // droppable (channel pressure)
        Event::note_on(0, 61, 0, 7),   // release (velocity 0)
    ];
    set.defer_block(1, 1, &fill);
    assert_eq!(set.ring(1).unwrap().len(), 8);
    assert_eq!(set.stats().strip(1), DeferCounts { deferred: 8, ..DeferCounts::default() });
    // Three more, one of them a release: three droppable go, oldest first, the new ones included
    // only after every older one.
    set.defer_block(1, 2, &[Event::control(0, CC_ALL_SOUND_OFF, 0, 0), Event::note_on(0, 62, 90, 1), Event::control(0, CC_ALL_NOTES_OFF, 0, 2)]);
    let kept: Vec<_> = set.ring(1).unwrap().events().iter().map(bytes).collect();
    assert_eq!(
        kept,
        [(0x80, 59, 0), (0xB0, 64, 0), (0xB0, 64, 127), (0xD0, 30, 0), (0x90, 61, 0), (0xB0, 120, 0), (0x90, 62, 90), (0xB0, 123, 0)],
        "note-on 60, CC 1 and the bend dropped"
    );
    assert_eq!(set.stats().strip(1), DeferCounts { deferred: 11, dropped: 3, silences: 0, carried: 0 });
    // A droppable event into a ring with only one droppable left: the newer one is the one kept.
    set.defer_block(1, 3, &[Event::new(0xE0, 1, 1, 0)]);
    let ring = set.ring(1).unwrap();
    assert!(!ring.events().iter().any(|e| bytes(e) == (0xB0, 64, 127)), "the oldest droppable went");
    assert_eq!(bytes(ring.events().last().unwrap()), (0xE0, 1, 1));
    assert!(!ring.is_silenced());
    assert_eq!(set.stats().total(), DeferCounts { deferred: 12, dropped: 4, silences: 0, carried: 0 });
    assert_eq!(set.stats().strip(0), DeferCounts::default(), "per strip");
}

#[test]
fn a_ring_full_of_releases_drops_a_new_droppable_event() {
    let mut set = DeferSet::new(1, 4);
    let offs: Vec<_> = (0..4u8).map(|k| Event::note_off(0, k, 0, 0)).collect();
    set.defer_block(0, 1, &offs);
    set.defer_block(0, 2, &[Event::note_on(0, 9, 100, 0)]);
    let ring = set.ring(0).unwrap();
    assert_eq!(ring.events().iter().map(bytes).collect::<Vec<_>>(), offs.iter().map(bytes).collect::<Vec<_>>());
    assert!(!ring.is_silenced(), "the release class still fits");
    assert_eq!(set.stats().strip(0), DeferCounts { deferred: 4, dropped: 1, silences: 0, carried: 0 });
}

#[test]
fn past_the_release_class_the_ring_becomes_all_sound_off_and_all_notes_off() {
    let mut set = DeferSet::new(2, 6);
    set.set_channel(1, 9);
    let mixed = [Event::note_on(9, 36, 100, 0), Event::note_off(9, 35, 0, 1), Event::control(9, 64, 0, 2), Event::note_off(9, 38, 0, 3)];
    set.defer_block(1, 4, &mixed);
    let releases: Vec<_> = (40..44u8).map(|k| Event::note_off(9, k, 0, 5)).collect();
    set.defer_block(1, 5, &releases);
    // 8 events, 7 releases, capacity 6: the release class alone does not fit.
    let ring = set.ring(1).unwrap();
    assert!(ring.is_silenced());
    assert_eq!(ring.events(), [Event::control(9, 120, 0, 0).in_block(5), Event::control(9, 123, 0, 0).in_block(5)]);
    assert_eq!(set.stats().strip(1), DeferCounts { deferred: 4, dropped: 8, silences: 1, carried: 0 });
    // While silenced, more deferred events are dropped and counted.
    set.defer_block(1, 6, &[Event::note_on(9, 50, 100, 0), Event::note_off(9, 50, 0, 1)]);
    assert_eq!(set.ring(1).unwrap().len(), 2);
    assert_eq!(set.stats().strip(1).dropped, 10);
    // The drain emits exactly the two at offset 0, then the block's live events, and clears the flag.
    let (out, d) = deliver(&mut set, 1, 7, 64, &[Event::note_on(9, 60, 100, 8)]);
    assert_eq!(out.iter().map(|e| (bytes(e), e.offset)).collect::<Vec<_>>(), [((0xB9, 120, 0), 0), ((0xB9, 123, 0), 0), ((0x99, 60, 100), 8)]);
    assert!(d.silenced);
    assert_eq!((d.deferred, d.live), (2, 1));
    assert!(!set.ring(1).unwrap().is_silenced());
    assert!(set.ring(1).unwrap().is_empty());
    // After the drain, the ring defers normally again.
    set.defer_block(1, 8, &[Event::note_on(9, 61, 1, 0)]);
    assert_eq!(set.ring(1).unwrap().len(), 1);
    let s = set.stats();
    assert_eq!(s.total(), DeferCounts { deferred: 5, dropped: 10, silences: 1, carried: 0 });
    assert_eq!(s.overflow_faults(), 11);
    assert_eq!(s.strip(0), DeferCounts::default());
}

#[test]
fn a_silenced_ring_stays_silenced_through_an_empty_block() {
    let mut set = DeferSet::new(1, 2);
    set.defer_block(0, 1, &[Event::note_off(0, 1, 0, 0), Event::note_off(0, 2, 0, 0), Event::note_off(0, 3, 0, 0)]);
    assert!(set.ring(0).unwrap().is_silenced());
    let (out, d) = drain(&mut set, 0, 2, 0);
    assert!(out.is_empty() && !d.silenced);
    assert!(set.ring(0).unwrap().is_silenced());
    let (out, d) = drain(&mut set, 0, 3, 16);
    assert_eq!(out.len(), 2);
    assert!(d.silenced);
}

#[test]
fn a_strip_out_of_range_is_refused_and_counted() {
    let mut set = DeferSet::new(2, 4);
    set.defer_block(2, 1, &[Event::note_on(0, 1, 1, 0)]);
    set.set_channel(9, 0);
    let (out, d) = deliver(&mut set, 5, 1, 64, &[Event::note_on(0, 1, 1, 0)]);
    assert_eq!(out, []);
    assert_eq!(d, Drained::default());
    assert_eq!(set.stats().refused(), 3);
    assert_eq!(set.stats().total(), DeferCounts::default());
    assert_eq!(set.stats().strip(7), DeferCounts::default());
}

#[test]
fn the_counters_are_readable_from_another_thread() {
    let mut set = DeferSet::new(1, 2);
    let stats = Arc::clone(set.stats());
    set.defer_block(0, 1, &[Event::note_on(0, 1, 1, 0), Event::note_on(0, 2, 1, 0), Event::note_on(0, 3, 1, 0)]);
    let seen = std::thread::spawn(move || (stats.total(), stats.strip(0))).join().unwrap();
    assert_eq!(seen.0, DeferCounts { deferred: 2, dropped: 1, silences: 0, carried: 0 }, "the oldest note-on went");
    assert_eq!(seen.0, seen.1);
}

// ---- The allocation proof ----

#[test]
fn defer_and_drain_allocate_nothing_after_construction() {
    let mut set = DeferSet::new(4, 1024);
    let block: Vec<Event> = (0..300u32).map(|i| Event::note_on(0, (i % 128) as u8, 100, i)).collect();
    let releases: Vec<Event> = (0..1100u32).map(|i| Event::note_off(0, (i % 128) as u8, 0, i % 64)).collect();
    let live = [Event::note_on(0, 1, 1, 0), Event::note_off(0, 1, 0, 0)];
    let mut delivered = 0usize;
    let ((), made) = measure(|| {
        let mut out = |_: Event| delivered += 1;
        // Empty ring: a drain and a deliver with nothing deferred.
        set.drain_into(0, 1, 64, &mut out);
        set.deliver(0, 1, 64, &live, &mut out);
        // Fill strip 1 to capacity and past it (the drop pass), then drain it full.
        for g in 2..6 {
            set.defer_block(1, g, &block);
        }
        set.drain_into(1, 6, 64, &mut out);
        // Silence strip 2, defer while silenced, drain the silence.
        set.defer_block(2, 7, &releases);
        set.defer_block(2, 8, &block);
        set.deliver(2, 9, 64, &live, &mut out);
        // The end-of-block carry with live events deferred behind it.
        set.defer_block(3, 10, &live);
        set.deliver(3, 11, 1, &live, &mut out);
        set.drain_into(3, 12, 64, &mut out);
        // Empty drains again.
        set.drain_into(1, 13, 64, &mut out);
        set.drain_into(2, 13, 64, &mut out);
    });
    assert!(made.is_zero(), "{made:?}");
    let t = set.stats().total();
    assert_eq!(t.silences, 1);
    assert!(t.dropped > 0 && t.carried > 0, "{t:?}");
    assert_eq!(delivered, 1024 + 2 + 2 + 2 + 4, "every path ran");
}

// ---- The continuity instrument ----

#[test]
fn the_instrument_reports_holes_stuck_notes_and_bad_deliveries() {
    let mut inst = ContinuityInstrument::new(16, 4);
    inst.process(0, 64, &[Event::note_on(0, 60, 100, 10)]);
    inst.process(128, 64, &[Event::note_on(0, 60, 100, 5), Event::note_off(0, 61, 0, 6)]);
    inst.process(192, 64, &[Event::note_on(0, 62, 100, 7), Event::note_off(0, 62, 0, 7), Event::control(0, 1, 1, 3), Event::control(0, 1, 1, 64)]);
    inst.process(200, 64, &[Event::control(0, 64, 127, 0)]);
    let r = inst.report();
    assert_eq!(inst.holes(), [Hole { at: 64, samples: 64 }]);
    assert_eq!((r.holes, r.hole_samples, r.blocks, r.processed_samples, r.events), (1, 64, 4, 256, 8));
    assert_eq!(r.faults.restrikes, 1);
    assert_eq!(r.faults.orphan_offs, 1);
    assert_eq!(r.faults.zero_length, 1);
    assert_eq!(r.faults.unsorted, 1);
    assert_eq!(r.faults.out_of_block, 1);
    assert_eq!(r.faults.overlaps, 1);
    assert_eq!((r.stuck_notes, r.stuck_pedals), (1, 1), "key 60 and the sustain pedal");
    assert_eq!(inst.trace()[1].sample, 133);
    inst.process(264, 64, &[Event::control(0, CC_ALL_SOUND_OFF, 0, 0)]);
    let r = inst.report();
    assert_eq!((r.stuck_notes, r.stuck_pedals, r.silenced_notes), (0, 0, 1));
}

/// The generator's state: which keys are on, so it only plays valid pairs.
struct Player {
    on: Vec<bool>,
    channels: u8,
    keys: u8,
}

impl Player {
    /// One block's events at sorted random offsets: note-ons and note-offs for a few keys (so
    /// pairs collapse often), controllers and bends between them.
    fn block(&mut self, rng: &mut Rng, block_len: u32, closing: bool) -> Vec<Event> {
        let n = if closing { 0 } else { rng.below(7) };
        let mut offs: Vec<u32> = (0..n).map(|_| rng.below(u64::from(block_len)) as u32).collect();
        offs.sort_unstable();
        let mut out = Vec::new();
        for o in offs {
            let ch = rng.below(u64::from(self.channels)) as u8;
            let key = 60 + rng.below(u64::from(self.keys)) as u8;
            let i = usize::from(ch) * 128 + usize::from(key);
            match rng.below(4) {
                0 => out.push(Event::control(ch, 1 + rng.below(30) as u8, rng.below(128) as u8, o)),
                _ if self.on[i] => {
                    self.on[i] = false;
                    let vel0 = rng.below(3) == 0;
                    out.push(if vel0 { Event::note_on(ch, key, 0, o) } else { Event::note_off(ch, key, 64, o) });
                }
                _ => {
                    self.on[i] = true;
                    out.push(Event::note_on(ch, key, 1 + rng.below(127) as u8, o));
                }
            }
        }
        if closing {
            for (i, on) in self.on.iter_mut().enumerate() {
                if std::mem::take(on) {
                    out.push(Event::note_off((i / 128) as u8, (i % 128) as u8, 0, block_len - 1));
                }
            }
        }
        out
    }
}

/// Plays `blocks` blocks of random events, skipping random runs of blocks (the busy and
/// unstarted overruns), then closes every note and plays on until nothing is deferred. Returns
/// the instrument, every event the player made in order, the skipped block count and the set.
fn run(seed: u64, capacity: usize, channels: u8, blocks: u64) -> (ContinuityInstrument, Vec<Event>, u64, DeferSet) {
    let mut rng = Rng::new(seed);
    // Blocks of 1 and 2 samples are covered by the carry tests above; here a block holds a
    // few collapsed pairs.
    let block_len = [4u32, 16, 64, 128][rng.below(4) as usize];
    let mut set = DeferSet::new(1, capacity);
    let mut inst = ContinuityInstrument::new(1 << 16, 1 << 12);
    inst.expect_from(0);
    let mut musician = Player { on: vec![false; 16 * 128], channels, keys: 3 };
    let mut played = Vec::new();
    let mut skipped = 0;
    let mut out = Vec::with_capacity(4096);
    let mut g = 0u64;
    let mut skip_run = 0;
    let mut pending_close = true;
    let mut last_processed = true;
    loop {
        let closing = g >= blocks && pending_close;
        // Stop on a processed block with nothing left deferred, so every skip is a hole.
        if g >= blocks && !closing && skip_run == 0 && last_processed && set.ring(0).unwrap().is_empty() {
            break;
        }
        // After the closing block the player is silent; the loop only plays out the carry.
        let live = if g < blocks || closing { musician.block(&mut rng, block_len, closing) } else { Vec::new() };
        if closing {
            pending_close = false;
        }
        played.extend_from_slice(&live);
        if skip_run == 0 && g < blocks && rng.below(4) == 0 {
            skip_run = 1 + rng.below(4);
        }
        if skip_run > 0 {
            skip_run -= 1;
            skipped += 1;
            let cause = if rng.below(2) == 0 { OverrunCause::Busy } else { OverrunCause::Unstarted };
            assert!(set.defer_for(0, g, Outcome::Overrun(cause), &live));
            last_processed = false;
        } else {
            last_processed = true;
            out.clear();
            set.deliver(0, g, block_len, &live, &mut |e| out.push(e));
            inst.process(g * u64::from(block_len), block_len, &out);
        }
        g += 1;
        assert!(g < blocks + 1000, "the carry never drains (seed {seed})");
    }
    (inst, played, skipped, set)
}

#[test]
fn no_note_is_lost_across_random_skipped_blocks() {
    for seed in 1..=300 {
        let (inst, played, skipped, set) = run(seed, 1024, 3, 200);
        let r = inst.report();
        assert_eq!(set.stats().overflow_faults(), 0, "seed {seed}: no overflow at this capacity");
        assert_eq!(r.stuck_notes, 0, "seed {seed}: {r:?}");
        assert_eq!(r.faults, ContinuityFaults::default(), "seed {seed}: every off after its on, sorted, in the block: {r:?}");
        let got: Vec<_> = inst.trace().iter().map(|l| bytes(&l.event)).collect();
        let want: Vec<_> = played.iter().map(bytes).collect();
        assert_eq!(got, want, "seed {seed}: every event exactly once, in the order played");
        // Every skipped block is a hole and nothing else is.
        let hole_blocks: u64 = r.hole_samples / (r.processed_samples / r.blocks);
        assert_eq!(hole_blocks, skipped, "seed {seed}");
        // Every note-off lands strictly after its note-on.
        let mut on_at = vec![None; 16 * 128];
        for l in inst.trace() {
            let i = l.event.note_slot();
            if l.event.is_note_on() {
                on_at[i] = Some(l.sample);
            } else if l.event.is_note_off() {
                let on = on_at[i].take().unwrap();
                assert!(l.sample > on, "seed {seed}");
            }
        }
    }
}

#[test]
fn no_note_is_stuck_when_the_ring_overflows() {
    let mut silences = 0;
    let mut dropped = 0;
    for seed in 1..=300 {
        let (inst, played, _, set) = run(seed, 2 + (seed as usize % 7), 1, 120);
        let r = inst.report();
        let c = set.stats().total();
        silences += c.silences;
        dropped += c.dropped;
        assert_eq!(r.stuck_notes, 0, "seed {seed}: the release class or the silence ends every note: {r:?}");
        assert_eq!(r.stuck_pedals, 0, "seed {seed}");
        let f = r.faults;
        assert_eq!((f.overlaps, f.out_of_block, f.unsorted, f.zero_length, f.restrikes, f.unrecorded), (0, 0, 0, 0, 0, 0), "seed {seed}: {f:?}");
        // Conservation: every event played was delivered, dropped, or is gone in a silence; the
        // silences add two events each.
        assert_eq!(r.events + c.dropped, played.len() as u64 + 2 * c.silences, "seed {seed}: {c:?}");
        // What was delivered is a subsequence of what was played (plus the silence pairs).
        let mut it = played.iter().map(bytes);
        for l in inst.trace() {
            let b = bytes(&l.event);
            if l.event.status & 0xF0 == 0xB0 && (b.1 == CC_ALL_SOUND_OFF || b.1 == CC_ALL_NOTES_OFF) && b.2 == 0 {
                continue;
            }
            assert!(it.any(|p| p == b), "seed {seed}: delivered out of order or invented");
        }
    }
    assert!(silences > 0 && dropped > silences, "the overflow paths ran: {silences} silences, {dropped} dropped");
}
