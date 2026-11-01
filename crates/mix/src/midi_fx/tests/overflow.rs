//! #278: exercise the actual slot at host sizes that fill its output buffer.
use super::*;

// 8 kHz makes even the 4096-frame row exceed 512 events with the 1/64 divider's
// eight-times gate. At 48 kHz the same tempo and block size need not overflow.

fn sequencer_overflow(frames: usize, tempo: f64) {
    let t = track(&[("random_sequencer", &[("rate", 20.0), ("play", 4.0), ("model", 4.0), ("gate_bias", 100.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    let mut sounding = [false; 128];
    let mut repeated = 0;
    let mut starts = 0;
    let mut pos = 0;
    for block in 0..=32 {
        let n = if block == 0 { 64 } else { frames };
        let transport = Transport { playing: true, samples: pos, quarters: pos as f64 * tempo / (60.0 * 8_000.0), tempo, ..Transport::default() };
        host.begin(&mut rec, &t, None, pos, &transport, 8_000.0, pos as u64, n);
        host.notes.clear();
        host.flush(&mut rec);
        for (_, _, e) in rec.heard.drain(..) {
            match e.message {
                Message::NoteOn { key, .. } => {
                    repeated += usize::from(sounding[usize::from(key)]);
                    sounding[usize::from(key)] = true;
                    starts += 1;
                }
                Message::NoteOff { key, .. } => sounding[usize::from(key)] = false,
                _ => {}
            }
        }
        pos += n as i64;
    }
    // Stop the generator itself, without host.panic/reset: that cleanup would mask lost offs.
    let transport = Transport { playing: false, samples: pos, tempo, ..Transport::default() };
    host.begin(&mut rec, &t, None, pos, &transport, 8_000.0, pos as u64, frames);
    host.flush(&mut rec);
    for (_, _, e) in &rec.heard {
        if let Message::NoteOff { key, .. } = e.message {
            sounding[usize::from(key)] = false;
        }
    }
    assert!(starts > 0);
    assert!(host.counts.overflow > 0, "the row must exercise output overflow");
    assert_eq!(host.counts.overflow, host.counts.refused_ons + host.counts.evicted_ons, "only ons are refused");
    assert_eq!((repeated, sounding.iter().filter(|on| **on).count()), (0, 0), "frames={frames}, tempo={tempo}, counts={:?}", host.counts);
}

#[test]
fn sequencer_300_bpm_4096() {
    sequencer_overflow(4096, 300.0);
}
#[test]
fn sequencer_300_bpm_8192() {
    sequencer_overflow(8192, 300.0);
}
#[test]
fn sequencer_300_bpm_16384() {
    sequencer_overflow(16384, 300.0);
}
#[test]
fn sequencer_1000_bpm_16384() {
    sequencer_overflow(16384, 1000.0);
}

#[test]
fn releases_evict_the_newest_on_and_keep_write_order() {
    for release in [
        off(900, 60),
        ev(-1, Message::Control { cc: 120, value: 0 }),
        ev(-1, Message::Control { cc: 123, value: 0 }),
        ev(-1, Message::Control { cc: 64, value: 63 }),
    ] {
        let mut out = Output::new();
        for i in 0..MIDI_FX_BLOCK {
            assert!(out.push(TimedEvent { offset: 10, event: on(i as i32, 60, 100) }));
        }
        assert!(!out.push(TimedEvent { offset: 10, event: on(999, 61, 100) }));
        assert!(out.push(TimedEvent { offset: 10, event: release }));
        let emitted: Vec<_> = out.events().collect();
        assert_eq!(emitted.len(), MIDI_FX_BLOCK);
        assert_eq!(emitted.last().unwrap().event, release);
        assert!(emitted.iter().all(|e| e.event.note_id != 511 && e.event.note_id != 999));
        assert_eq!((out.counts.overflow, out.counts.refused_ons, out.counts.evicted_ons), (2, 1, 1));
    }
}

#[test]
fn eviction_keeps_same_offset_releases_after_the_surviving_events() {
    let mut out = Output::new();
    for i in 0..MIDI_FX_BLOCK {
        let e = if i % 2 == 0 { on(i as i32, 60, 100) } else { off(i as i32, 60) };
        out.push(TimedEvent { offset: 0, event: e });
    }
    out.push(TimedEvent { offset: 0, event: off(1000, 60) });
    let ids: Vec<_> = out.events().map(|e| e.event.note_id).collect();
    assert_eq!(&ids[MIDI_FX_BLOCK - 3..], &[509, 511, 1000]);
}

#[test]
fn release_only_flood_has_bounded_channel_safety_releases() {
    let mut out = Output::new();
    for i in 0..4096u32 {
        let mut e = off(i as i32, (i % 128) as u8);
        e.channel = (i % 16) as u8;
        assert!(out.push(TimedEvent { offset: i, event: e }));
    }
    assert!(!out.push(TimedEvent { offset: 4096, event: on(5000, 60, 100) }));
    let events: Vec<_> = out.events().collect();
    assert_eq!(events.len(), 16 * 7);
    for channel in 0..16 {
        for cc in [64, 66, 67, 69, 120, 121, 123] {
            assert!(events.iter().any(|e| e.offset == 4095 && e.event.channel == channel && e.event.message == Message::Control { cc, value: 0 }));
        }
    }
    assert_eq!((out.counts.coalesced_releases, out.counts.overflow, out.counts.refused_ons), (4096, 1, 1));
}

#[test]
fn carried_ons_are_evicted_for_this_blocks_releases() {
    let t = track(&[("midi_velocity", &[])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    host.begin(&mut rec, &t, None, 0, &Transport::default(), 48_000.0, 0, FRAMES);
    let stage = host.stages[0].as_mut().unwrap();
    for (i, c) in stage.carry.iter_mut().enumerate() {
        *c = Carried { due: 0, event: on(i as i32, 60, 100) };
    }
    stage.carry_len = MIDI_FX_CARRY;
    for i in 0..MIDI_FX_BLOCK {
        let e = if i < MIDI_FX_BLOCK - MIDI_FX_CARRY { ev(-1, Message::Control { cc: 1, value: 1 }) } else { off(i as i32, 60) };
        assert!(host.take(&mut rec, 0, &e));
    }
    host.flush(&mut rec);
    assert_eq!(rec.heard.iter().filter(|(_, _, e)| matches!(e.message, Message::NoteOff { .. })).count(), MIDI_FX_CARRY);
    assert!(rec.heard.iter().all(|(_, _, e)| !matches!(e.message, Message::NoteOn { .. })));
    assert_eq!(host.counts.evicted_ons, MIDI_FX_CARRY as u64);
    assert_eq!(host.counts.overflow, MIDI_FX_CARRY as u64);
}

#[test]
fn overflow_on_the_audio_thread_allocates_nothing() {
    crate::mark_audio_thread();
    let t = track(&[("random_sequencer", &[("rate", 20.0), ("play", 4.0), ("model", 4.0), ("gate_bias", 100.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    rec.heard.reserve(8192);
    let ((), stats) = transit_rt::testing::measure(|| {
        for block in 0..8 {
            let pos = block * 16384;
            let transport =
                Transport { playing: true, samples: pos, quarters: pos as f64 * 1000.0 / (60.0 * 8_000.0), tempo: 1000.0, ..Transport::default() };
            host.begin(&mut rec, &t, None, pos, &transport, 8_000.0, pos as u64, 16384);
            host.notes.clear();
            host.flush(&mut rec);
            rec.heard.clear();
        }
    });
    assert_eq!(stats, transit_rt::testing::AllocStats::default());
    assert!(host.counts.overflow > 0);
    assert!(host.counts.coalesced_releases > 0);
}

#[test]
fn release_flood_ends_native_notes_even_when_controllers_are_ignored() {
    let t = track(&[("midi_velocity", &[])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    block(&mut host, &mut rec, &t, &[(0, on(1, 60, 100))]);
    rec.heard.clear();
    host.begin(&mut rec, &t, None, 256, &Transport::default(), 48_000.0, 256, FRAMES);
    let stage = host.stages[0].as_mut().unwrap();
    for c in &mut stage.carry {
        *c = Carried { due: 256, event: off(2, 61) };
    }
    stage.carry[MIDI_FX_CARRY - 2] = Carried { due: 512, event: on(777, 70, 100) };
    stage.carry[MIDI_FX_CARRY - 1] = Carried { due: 600, event: off(777, 70) };
    stage.carry_len = MIDI_FX_CARRY;
    for _ in 0..MIDI_FX_BLOCK {
        assert!(host.take(&mut rec, 0, &off(2, 61)));
    }
    host.flush(&mut rec);
    // A notes-only CLAP port or unmapped VST3 controller returns success for CCs
    // without affecting its voices. Only the explicit native off proves termination.
    assert!(rec.heard.iter().any(|(_, offset, e)| *offset == 255 && *e == off(1, 60)));
    assert_eq!(host.counts.released_on_overflow, 1);
    assert!(!host.pending());
    assert!(host.out.iter().all(|n| !n.used));
    rec.heard.clear();
    host.begin(&mut rec, &t, None, 512, &Transport::default(), 48_000.0, 512, FRAMES);
    host.flush(&mut rec);
    assert!(rec.heard.is_empty(), "the canceled future on and off must not escape next block");
}

#[test]
fn recovery_delivers_pedal_up_even_when_the_next_effect_delays_it() {
    let t = track(&[("midi_velocity", &[]), ("midi_test_delay", &[("samples", 1000.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    host.begin(&mut rec, &t, None, 0, &Transport::default(), 48_000.0, 0, FRAMES);
    host.deliver(&mut rec, 0, &ev(-1, Message::Control { cc: 64, value: 127 }));
    host.deliver(&mut rec, 0, &on(1, 60, 100));
    let stage = host.stages[0].as_mut().unwrap();
    stage.carry.fill(Carried { due: 0, event: off(2, 61) });
    stage.carry_len = MIDI_FX_CARRY;
    for _ in 0..MIDI_FX_BLOCK {
        assert!(host.take(&mut rec, 0, &off(2, 61)));
    }
    host.flush(&mut rec);
    for e in [off(1, 60), ev(-1, Message::Control { cc: 64, value: 0 }), ev(-1, Message::Control { cc: 120, value: 0 })] {
        assert!(rec.heard.iter().any(|(_, offset, heard)| *offset == 255 && *heard == e));
    }
    assert!(!host.pending());
}

#[test]
fn an_on_that_cannot_be_tracked_is_never_emitted() {
    let t = track(&[("midi_velocity", &[])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    let input: Vec<_> = (0..=MIDI_FX_NOTES).map(|i| (0, on(i as i32, 60, 100))).collect();
    block(&mut host, &mut rec, &t, &input);
    assert_eq!(rec.heard.len(), MIDI_FX_NOTES);
    assert!(rec.heard.iter().all(|(_, _, e)| e.note_id != MIDI_FX_NOTES as i32));
    assert_eq!((host.counts.overflow, host.counts.refused_ons), (1, 1));
    host.panic(&mut rec);
    assert_eq!(rec.heard.iter().filter(|(_, _, e)| matches!(e.message, Message::NoteOff { .. })).count(), MIDI_FX_NOTES);
}
