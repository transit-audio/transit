//! F10 (ADR 0012 G5), CLAP side: the fixture's events plug-in (built with the `events` feature)
//! writes every event it receives into its output, so these tests see the channel, `note_id`,
//! time and order of each one as the plug-in does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};

mod common;
use common::events_fixture_dir;

const EVENTS: &str = "clap:org.soundcraft.test.events";

fn plugin(block: usize) -> Box<dyn Plugin> {
    events_fixture_dir();
    let mut p = transit_clap_host::create(EVENTS).unwrap();
    p.prepare(48_000.0, block, 2);
    p
}

/// Runs one block and decodes the plug-in's records: (code, time, a, b).
fn records(p: &mut Box<dyn Plugin>, frames: usize) -> Vec<(u32, u32, i64, i64)> {
    let mut io = vec![vec![0.0f32; frames], vec![0.0f32; frames]];
    p.process(&mut io, frames);
    io[0].chunks(4).take_while(|r| r[0] != 0.0).map(|r| (r[0] as u32, r[1] as u32, r[2] as i64, r[3] as i64)).collect()
}

fn ev(channel: u8, note_id: i32, message: Message) -> PluginEvent {
    PluginEvent { channel, note_id, message }
}

/// ADR 0059, Decision 1: each note goes out with its ledger entry's own native `note_id`, and its
/// release with the same one, so the plug-in cannot pair them differently.
#[test]
fn notes_carry_channel_and_their_entrys_note_id_and_pressure_is_an_expression() {
    let mut p = plugin(256);
    let n = NoteId::NONE;
    p.event(10, &ev(2, 7, Message::NoteOn { key: 60, velocity: 127, id: n }));
    p.event(10, &ev(9, 8, Message::NoteOn { key: 60, velocity: 127, id: n }));
    p.event(20, &ev(9, 8, Message::PolyPressure { key: 60, value: 127, id: n }));
    p.event(30, &ev(2, 7, Message::NoteOff { key: 60, velocity: 0, id: n }));
    p.event(40, &ev(3, -1, Message::Control { cc: 1, value: 5 }));
    let r = records(&mut p, 256);
    let (a, b) = (r[0].3, r[1].3);
    assert_ne!(a, b, "every entry has its own native id");
    assert_eq!(
        r,
        vec![(1, 10, 2060, a), (1, 10, 9060, b), (3, 20, 9060, 1_000_000), (2, 30, 2060, a), (5, 40, 0xB3_01_05, 0)],
        "controllers go to the MIDI dialect with the channel nibble"
    );
}

#[test]
fn set_param_at_sends_one_point_per_offset() {
    let mut p = plugin(256);
    assert!(p.set_param_at(40, "5", 0.8));
    assert!(p.set_param_at(5, "5", 0.25));
    assert!(p.set_param_at(5, "5", 0.2), "the same offset coalesces");
    let r = records(&mut p, 256);
    assert_eq!(r, vec![(4, 5, 5, 200_000), (4, 40, 5, 800_000)]);
}

/// #117: a timed point the full queue refuses still lands: the parameter's latest queued point
/// takes its value (the last point wins). Under ADR 0059 note and MIDI events no longer share the
/// timed points' budget, so the budget is filled with timed points (1,024 at distinct offsets).
#[test]
fn a_timed_point_past_a_full_queue_overwrites_the_latest() {
    let mut p = plugin(8192);
    for t in 1..=1024u32 {
        assert!(p.set_param_at(t, "5", 0.25));
    }
    let n = NoteId::NONE;
    assert!(p.event(2, &ev(0, -1, Message::PolyPressure { key: 60, value: 1, id: n })), "MIDI has its own allowance");
    assert!(p.set_param_at(2000, "5", 0.75), "absorbed by the queued point");
    let r = records(&mut p, 8192);
    let points: Vec<_> = r.iter().filter(|x| x.0 == 4).copied().collect();
    assert_eq!(points.len(), 1024, "no point at 2000");
    assert_eq!(points.last(), Some(&(4, 1024, 5, 750_000)));
}

/// #117 review: a block the plug-in is not called for (here: the early return of a block with no
/// frames) collapses its timed points to time 0, the latest per parameter, so the next processed
/// block does not replay them at the old offsets. Not an unprepared instance: `create` already
/// prepares it (48 kHz, 1024 frames), so a `process` before the test's `prepare` is a processed
/// block that delivers the points at their offsets and proves nothing.
#[test]
fn timed_points_of_a_skipped_block_collapse_to_the_next_block_start() {
    let mut p = plugin(256);
    assert!(p.set_param_at(5, "5", 0.25));
    assert!(p.set_param_at(40, "5", 0.75));
    let mut io = vec![vec![0.0f32; 256], vec![0.0f32; 256]];
    p.process(&mut io, 0);
    let r = records(&mut p, 256);
    assert_eq!(r.iter().filter(|x| x.0 == 4).copied().collect::<Vec<_>>(), vec![(4, 0, 5, 750_000)], "{r:?}");
}

/// #78: a block the plug-in is not called for (here: no frames) keeps the host's value and says so
/// (`params_pending`), so the mix keeps a sleeping insert awake; the next processed block delivers
/// it, once, with nothing sent again.
#[test]
fn a_value_a_skipped_block_kept_is_pending_until_a_block_delivers_it() {
    let mut p = plugin(256);
    assert!(!p.params_pending());
    assert!(p.set_param("5", 0.8));
    let mut io = vec![vec![0.0f32; 256], vec![0.0f32; 256]];
    p.process(&mut io, 0);
    assert!(p.params_pending(), "the skipped block kept it");
    let r = records(&mut p, 256);
    assert_eq!(r.iter().filter(|x| x.0 == 4).copied().collect::<Vec<_>>(), vec![(4, 0, 5, 800_000)], "{r:?}");
    assert!(!p.params_pending(), "delivered");
    let r = records(&mut p, 256);
    assert!(!r.iter().any(|x| x.0 == 4), "sent once: {r:?}");
}

#[test]
fn a_note_off_survives_an_overfull_queue() {
    // ADR 0059: the ordinary allowance is full, a start still gets a ledger entry, and every owned
    // release is admitted.
    let mut p = plugin(8192);
    let n = NoteId::NONE;
    let mut ordinary = 0u32;
    while p.event(1, &ev((ordinary % 16) as u8, -1, Message::PolyPressure { key: (ordinary % 128) as u8, value: 1, id: n })) {
        ordinary += 1;
    }
    assert_eq!(ordinary, 1024, "ordinary events stop at their allowance");
    let admitted: Vec<i32> =
        (0..300).filter(|&i| p.event(2, &ev((i % 16) as u8, i, Message::NoteOn { key: (i % 128) as u8, velocity: 90, id: n }))).collect();
    assert_eq!(admitted.len(), 300, "starts take ledger entries, not the ordinary allowance");
    for &i in &admitted {
        assert!(p.event(3, &ev((i % 16) as u8, i, Message::NoteOff { key: (i % 128) as u8, velocity: 0, id: n })), "the off of admitted note {i}");
    }
    let r = records(&mut p, 8192);
    let ons: Vec<(i64, i64)> = r.iter().filter(|x| x.0 == 1).map(|x| (x.2, x.3)).collect();
    let offs: Vec<(i64, i64)> = r.iter().filter(|x| x.0 == 2).map(|x| (x.2, x.3)).collect();
    assert_eq!(ons.len(), 300);
    assert_eq!(offs, ons, "every admitted note-on got its off, with its own note id");
}

/// What `MixEngine::reset` does at Stop: the mix hands over the pedal-up and note-offs, then
/// `all_notes_off` and `reset`. The plug-in must still get every release on the next block
/// (review F10-1: a reset kept only 0x80, so CC 64 0 was dropped and sustain stuck).
#[test]
fn a_reset_keeps_the_pedal_up_and_the_note_offs() {
    let mut p = plugin(256);
    let n = NoteId::NONE;
    assert!(p.event(0, &ev(0, -1, Message::Control { cc: 64, value: 127 })));
    assert!(p.event(0, &ev(0, 5, Message::NoteOn { key: 60, velocity: 100, id: n })));
    let r = records(&mut p, 256);
    let id = r[1].3;
    assert_eq!(r, vec![(5, 0, 0xB0_40_7F, 0), (1, 0, 60, id)]);
    assert!(p.event(0, &ev(0, -1, Message::Control { cc: 64, value: 0 })));
    assert!(p.event(0, &ev(0, -1, Message::Control { cc: 123, value: 0 })));
    assert!(p.event(0, &ev(0, 5, Message::NoteOff { key: 60, velocity: 0, id: n })));
    assert!(p.event(0, &ev(0, -1, Message::Control { cc: 1, value: 9 })), "a non-release, dropped by the reset");
    p.all_notes_off();
    p.reset();
    let r = records(&mut p, 256);
    assert!(r.contains(&(5, 0, 0xB0_40_00, 0)), "the pedal-up survives the reset: {r:?}");
    assert!(r.contains(&(5, 0, 0xB0_7B_00, 0)), "CC 123 survives the reset: {r:?}");
    assert!(r.contains(&(2, 0, 60, id)), "the note-off survives the reset: {r:?}");
    assert!(!r.iter().any(|x| x.0 == 5 && x.2 == 0xB0_01_09), "non-releases do not: {r:?}");
}

fn midi_plugin(block: usize) -> Box<dyn Plugin> {
    events_fixture_dir();
    let mut p = transit_clap_host::create("clap:org.soundcraft.test.events-midi").unwrap();
    p.prepare(48_000.0, block, 2);
    p
}

fn note_on(channel: u8, id: i32) -> PluginEvent {
    ev(channel, id, Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE })
}

fn note_off(channel: u8, id: i32) -> PluginEvent {
    ev(channel, id, Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE })
}

/// What a MIDI-only plug-in's voices do with the bytes it received, for one channel and key:
/// `stacking` releases one voice per note-off (the oldest); otherwise a note-off ends every voice
/// of its key. Returns the voices left sounding per (channel, key).
fn voices_left(bytes: &[u32], stacking: bool) -> std::collections::BTreeMap<(u32, u32), u32> {
    let mut held = std::collections::BTreeMap::new();
    for &b in bytes {
        let (status, key, vel) = (b >> 16, (b >> 8) & 0x7F, b & 0x7F);
        let (kind, ch) = (status & 0xF0, status & 0x0F);
        let n: &mut u32 = held.entry((ch, key)).or_default();
        match kind {
            0x90 if vel > 0 => *n += 1,
            0x80 | 0x90 => *n = if stacking { n.saturating_sub(1) } else { 0 },
            0xB0 if key == 123 || key == 120 => {
                for ((c, _), v) in held.iter_mut() {
                    if *c == ch {
                        *v = 0;
                    }
                }
            }
            _ => {}
        }
    }
    held.retain(|_, v| *v > 0);
    held
}

fn bytes(r: &[(u32, u32, i64, i64)]) -> Vec<u32> {
    r.iter().filter(|x| x.0 == 5).map(|x| x.2 as u32).collect()
}

/// ADR 0059, Decision 1 (ruled 2026-11-27): a MIDI-dialect port gets one note-off for each note-on
/// of a key, whatever identities overlap, so a plug-in that stacks voices ends every one and a
/// plug-in that ends every voice of a key on one off ends them too (the overlap may be cut short).
#[test]
fn midi_dialect_sends_one_off_for_each_on() {
    let mut p = midi_plugin(256);
    for first in [101, 102] {
        let mut all = Vec::new();
        assert!(p.event(0, &note_on(3, 101)));
        assert!(p.event(1, &note_on(3, 102)));
        let r = records(&mut p, 256);
        assert_eq!(r, vec![(5, 0, 0x933C64, 0), (5, 1, 0x933C64, 0)]);
        all.extend(bytes(&r));
        assert!(p.event(10, &note_off(3, first)));
        let r = records(&mut p, 256);
        assert_eq!(r, vec![(5, 10, 0x83_3C_00, 0)], "the first identity's off is delivered");
        all.extend(bytes(&r));
        assert!(p.event(20, &note_off(3, 203 - first)));
        let r = records(&mut p, 256);
        assert_eq!(r, vec![(5, 20, 0x83_3C_00, 0)]);
        all.extend(bytes(&r));
        assert!(voices_left(&all, true).is_empty(), "a stacking plug-in ends both voices");
        assert!(voices_left(&all, false).is_empty(), "a key-level plug-in ends them too");
    }
}

#[test]
fn midi_dialect_uses_delivery_order_and_keeps_channels_separate() {
    let mut p = midi_plugin(256);
    // Reverse arrival order: the on at 50 arrives first, the off at 30 still goes out at 30.
    assert!(p.event(50, &note_on(3, 102)));
    assert!(p.event(10, &note_on(3, 101)));
    assert!(p.event(30, &note_off(3, 101)));
    assert!(p.event(0, &note_on(4, 101)));
    assert_eq!(records(&mut p, 256), vec![(5, 0, 0x943C64, 0), (5, 10, 0x933C64, 0), (5, 30, 0x83_3C_00, 0), (5, 50, 0x933C64, 0)]);
    assert!(p.event(15, &note_on(3, 103)));
    assert!(p.event(15, &note_off(3, 102)));
    assert!(p.event(25, &note_off(3, 103)));
    assert!(p.event(30, &note_off(4, 101)));
    assert_eq!(
        records(&mut p, 256),
        vec![(5, 15, 0x933C64, 0), (5, 15, 0x83_3C_00, 0), (5, 25, 0x83_3C_00, 0), (5, 30, 0x84_3C_00, 0)],
        "each identity's off, in arrival order at one offset"
    );
}

#[test]
fn midi_dialect_keeps_every_release_at_saturation_and_reclaims_identities() {
    // More lifetimes than the ledger can hold at once: entries must be reclaimed. (8,192 frames: the
    // fixture records one event per four frames, and the allowance alone is 1,024 events.)
    let mut p = midi_plugin(8192);
    for _ in 0..8 {
        let mut all = Vec::new();
        for id in 0..224 {
            assert!(p.event(0, &note_on(3, id)));
        }
        all.extend(bytes(&records(&mut p, 8192)));
        for i in 0..1024 {
            assert!(p.event(i.min(799), &ev(3, -1, Message::Control { cc: 1, value: 1 })));
        }
        assert!(!p.event(0, &ev(3, -1, Message::Control { cc: 1, value: 1 })), "the ordinary allowance is saturated");
        for id in 0..224 {
            assert!(p.event(800, &note_off(3, id)), "an owned release beside a full allowance");
        }
        let r = records(&mut p, 8192);
        assert_eq!(r.iter().filter(|r| r.2 == 0x83_3C_00).count(), 224, "one off for each on");
        assert_eq!(r.last(), Some(&(5, 800, 0x83_3C_00, 0)));
        all.extend(bytes(&r));
        assert!(voices_left(&all, true).is_empty() && voices_left(&all, false).is_empty());
    }
}

#[test]
fn midi_dialect_panic_and_reset_release_every_owned_voice() {
    for reset in [false, true] {
        let mut p = midi_plugin(256);
        let mut all = Vec::new();
        p.event(0, &note_on(3, 101));
        p.event(0, &note_on(3, 102));
        all.extend(bytes(&records(&mut p, 256)));
        p.event(0, &note_off(3, 101));
        let r = records(&mut p, 256);
        assert_eq!(r, vec![(5, 0, 0x83_3C_00, 0)], "101's off is delivered while 102 sounds");
        all.extend(bytes(&r));
        p.event(100, &note_on(3, 103));
        p.event(0, &ev(3, -1, Message::Control { cc: 64, value: 63 }));
        if reset {
            p.all_notes_off();
            p.reset();
        } else {
            p.all_notes_off();
        }
        let r = records(&mut p, 256);
        assert_eq!(r.iter().filter(|r| r.2 == 0x83_3C_00).count(), 1, "102's owed off: {r:?}");
        assert!(r.contains(&(5, 0, 0xB3_40_3F, 0)));
        assert!(!r.iter().any(|r| r.2 == 0x933C64), "the queued start 103 was cancelled");
        all.extend(bytes(&r));
        assert!(voices_left(&all, true).is_empty() && voices_left(&all, false).is_empty());
        p.event(0, &note_on(3, 104));
        p.event(10, &note_off(3, 104));
        assert_eq!(records(&mut p, 256), vec![(5, 0, 0x933C64, 0), (5, 10, 0x83_3C_00, 0)]);
    }
}

#[test]
fn midi_dialect_velocity_zero_releases_its_identity() {
    let mut p = midi_plugin(256);
    p.event(0, &note_on(3, 101));
    p.event(0, &note_on(3, 102));
    records(&mut p, 256);
    for id in [101, 102] {
        assert!(p.event(10, &ev(3, id, Message::NoteOn { key: 60, velocity: 0, id: NoteId::NONE })));
        assert_eq!(records(&mut p, 256), vec![(5, 10, 0x83_3C_00, 0)], "identity {id}");
    }
}

/// ADR 0059 Decisions 2 and 3a (review finding 3): translation is not delivery. When the plug-in
/// refuses `start_processing`, the call's events stay queued with their ownership as it was: All
/// MIDI Notes Off (or Stop) then cancels a start that never reached the plug-in, an already-queued
/// release is delivered once, and a sounding note gets its owed off.
#[test]
fn a_refused_start_keeps_ownership_as_it_was() {
    for stop in [false, true] {
        let mut p = plugin(256);
        let n = NoteId::NONE;
        assert!(p.event(0, &ev(0, -1, Message::NoteOn { key: 60, velocity: 100, id: n })));
        assert!(p.event(0, &ev(0, -1, Message::NoteOn { key: 62, velocity: 100, id: n })));
        let r = records(&mut p, 256);
        assert_eq!(r.iter().filter(|x| x.0 == 1).count(), 2, "60 and 62 sound");
        assert!(p.load_state(b"CMD:refuse-start-once"));
        p.prepare(48_000.0, 256, 2); // reactivates: the next call starts processing, refused once
        // The block after a refused one fades in over 2 ms (96 frames, 24 records): 32 ordinary
        // events that survive Stop (a sostenuto value with no press open) come first, so the
        // records under test lie past the fade.
        for _ in 0..32 {
            assert!(p.event(0, &ev(1, -1, Message::Control { cc: 66, value: 0 })));
        }
        assert!(p.event(0, &ev(0, -1, Message::NoteOn { key: 64, velocity: 100, id: n })), "a note start");
        assert!(p.event(0, &ev(0, -1, Message::Control { cc: 64, value: 127 })), "a pedal start");
        assert!(p.event(3, &ev(0, -1, Message::NoteOff { key: 62, velocity: 0, id: n })), "an already-queued release");
        let refused = records(&mut p, 256);
        assert!(refused.is_empty(), "nothing reached the plug-in: {refused:?}");
        p.all_notes_off();
        if stop {
            p.reset();
        }
        let mut io = vec![vec![0.0f32; 256], vec![0.0f32; 256]];
        p.process(&mut io, 256);
        let r: Vec<(u32, u32, i64, i64)> =
            io[0][128..].chunks(4).take_while(|r| r[0] != 0.0).map(|r| (r[0] as u32, r[1] as u32, r[2] as i64, r[3] as i64)).collect();
        assert!(!r.iter().any(|x| x.0 == 1), "the cancelled note start never plays: {r:?}");
        assert!(!r.iter().any(|x| x.0 == 5 && x.2 == 0xB0_40_7F), "the cancelled pedal press never plays: {r:?}");
        let offs: Vec<i64> = r.iter().filter(|x| x.0 == 2).map(|x| x.2).collect();
        let mut keys = offs.clone();
        keys.sort_unstable();
        assert_eq!(keys, [60, 62], "60's owed off and 62's queued off, once each (stop: {stop}): {r:?}");
        assert!(records(&mut p, 256).is_empty(), "nothing more is owed");
    }
}

/// Review finding 2: at a full parameter reserve and 1,024 timed points, 704 note-ons, a full
/// allowance of 14-bit controllers (two native events each) and the 704 note-offs all reach the
/// plug-in in one call; nothing accepted is dropped in translation.
#[test]
fn a_saturated_call_delivers_every_admitted_event() {
    let mut p = plugin(16_384);
    let n = NoteId::NONE;
    assert!(p.set_param("5", 0.3), "the parameter's block-start value");
    for t in 1..=1024u32 {
        assert!(p.set_param_at(t, "5", 0.25));
    }
    for k in 0..704u16 {
        assert!(p.event(2000, &ev((k % 16) as u8, -1, Message::NoteOn { key: (k / 16) as u8, velocity: 90, id: n })), "start {k}");
    }
    assert!(!p.event(2000, &ev(0, -1, Message::NoteOn { key: 100, velocity: 90, id: n })), "the 705th start is refused");
    let mut c14 = 0;
    while p.event(3000, &ev(1, -1, Message::Control14 { msb: 1, value: 9000 })) {
        c14 += 1;
    }
    assert_eq!(c14, 512, "each 14-bit controller reserves its two native events");
    for k in 0..704u16 {
        assert!(p.event(4000, &ev((k % 16) as u8, -1, Message::NoteOff { key: (k / 16) as u8, velocity: 0, id: n })), "release {k}");
    }
    let r = records(&mut p, 16_384);
    let ons: Vec<(i64, i64)> = r.iter().filter(|x| x.0 == 1).map(|x| (x.2, x.3)).collect();
    let mut offs: Vec<(i64, i64)> = r.iter().filter(|x| x.0 == 2).map(|x| (x.2, x.3)).collect();
    assert_eq!(ons.len(), 704);
    offs.sort_unstable_by_key(|x| x.1);
    let mut sorted_ons = ons.clone();
    sorted_ons.sort_unstable_by_key(|x| x.1);
    assert_eq!(offs, sorted_ons, "every off delivered with its on's note id");
    assert_eq!(r.iter().filter(|x| x.0 == 5).count(), 1024, "both halves of every 14-bit controller");
    assert_eq!(r.iter().filter(|x| x.0 == 4).count(), 1025, "the block-start value and every timed point");
    // Every reservation was settled by a delivery: the next 704 starts are all admitted again.
    for k in 0..704u16 {
        assert!(p.event(0, &ev((k % 16) as u8, -1, Message::NoteOn { key: (k / 16) as u8, velocity: 90, id: n })));
    }
}
