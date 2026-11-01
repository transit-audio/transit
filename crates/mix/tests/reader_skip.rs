//! #276: skipped readers release notes and pedals once, without allocating.
#![allow(clippy::unwrap_used)]

#[path = "support/reader_skip.rs"]
mod support;
use std::sync::atomic::Ordering::Relaxed;
use support::Rig;
use transit_mix::Message;
use transit_model::TrackKind;
use transit_rt::testing::CountingAlloc;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn a_skip_lifts_the_held_pedal_before_the_next_notes_release() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.send(Message::Control { cc: 64, value: 127 });
    rig.block();
    rig.off();
    rig.block();
    rig.sounding();
    rig.bypass(true);
    rig.block();
    rig.bypass(false);
    // The first resumed block has a new note: pedal-up must precede it.
    rig.on();
    rig.block();
    rig.sounding();
    rig.off();
    rig.released_tail();
    assert_eq!(rig.trace.releases.load(Relaxed), 1);
    assert_eq!(rig.trace.pedal_ups.load(Relaxed), 16);
    assert_eq!(rig.trace.channels.load(Relaxed), 0xffff);
}

#[test]
fn consecutive_skipped_blocks_release_once_and_a_later_skip_releases_again() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.block();
    rig.bypass(true);
    for _ in 0..5 {
        rig.block();
    }
    assert_eq!(rig.trace.releases.load(Relaxed), 1);
    assert_eq!(rig.trace.pedal_ups.load(Relaxed), 16);
    assert_eq!(rig.trace.channels.load(Relaxed), 0xffff);
    rig.bypass(false);
    rig.on();
    rig.block();
    rig.sounding();
    rig.bypass(true);
    rig.block();
    assert_eq!(rig.trace.releases.load(Relaxed), 2);
    assert_eq!(rig.trace.pedal_ups.load(Relaxed), 32);
    assert_eq!(rig.trace.resets.load(Relaxed), 0);
}

#[test]
fn bypass_discards_pending_notes_and_pedal_down_without_counting_an_overflow() {
    let mut rig = Rig::new(769, TrackKind::Instrument);
    rig.on();
    rig.send(Message::Control { cc: 64, value: 127 });
    rig.block();
    assert_eq!(rig.trace.ons.load(Relaxed), 0, "the start is still in the reader's carry");
    rig.bypass(true);
    // The reader misses this release. Replaying the carried start would leave a stuck note.
    rig.off();
    rig.block();
    rig.bypass(false);
    for _ in 0..16 {
        rig.block();
        rig.silent();
    }
    assert_eq!(rig.trace.ons.load(Relaxed), 0);
    rig.on();
    for _ in 0..16 {
        rig.block();
    }
    rig.sounding();
    rig.off();
    for _ in 0..20 {
        rig.block();
    }
    rig.silent();
    assert_eq!(rig.trace.releases.load(Relaxed), 1);
    assert_eq!(rig.mix.event_status().midi_fx.note_view_dropped, 0);
}
