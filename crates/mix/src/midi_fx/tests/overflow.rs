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
    // ADR 0057, slice P3b: past 384 non-releases the output refuses note-ons, and a release that
    // finds it full makes the reset barrier (the old sink evicted note-ons instead).
    assert!(host.counts.refused_ons > 0 && host.counts.safety_resets > 0, "counts={:?}", host.counts);
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
fn an_accepted_note_on_is_never_evicted() {
    // ADR 0057, decision 13, P3b's first case: 512 note-ons offered to one stage buffer, 384
    // accepted and reported, 128 refused and reported; 100 releases after them all accepted; none
    // of the 384 lost (the old sink reported 512 accepted and then evicted 100). Each release kind.
    let releases: [fn(i32) -> PluginEvent; 4] = [
        |i| off(i, 60),
        |_| ev(-1, Message::Control { cc: 120, value: 0 }),
        |_| ev(-1, Message::Control { cc: 123, value: 0 }),
        |_| ev(-1, Message::Control { cc: 64, value: 63 }),
    ];
    for release in releases {
        let mut out = Output::new();
        let verdicts: Vec<bool> = (0..MIDI_FX_BLOCK).map(|i| out.push(TimedEvent { offset: 10, event: on(i as i32, 60, 100) })).collect();
        assert_eq!(verdicts.iter().filter(|v| **v).count(), MIDI_FX_NON_RELEASE);
        assert!(verdicts[..MIDI_FX_NON_RELEASE].iter().all(|v| *v), "the first 384 accepted, the rest refused");
        for i in 0..100 {
            assert!(out.push(TimedEvent { offset: 11, event: release(i) }), "release {i}");
        }
        let emitted: Vec<_> = out.events().collect();
        assert_eq!(emitted.len(), MIDI_FX_NON_RELEASE + 100);
        let ons: Vec<i32> = emitted.iter().filter(|e| matches!(e.event.message, Message::NoteOn { .. })).map(|e| e.event.note_id).collect();
        assert_eq!(ons, (0..MIDI_FX_NON_RELEASE as i32).collect::<Vec<_>>(), "none of the 384 lost, in write order");
        assert!(emitted[MIDI_FX_NON_RELEASE..].iter().all(|e| e.event.is_release()));
        assert_eq!((out.counts.overflow, out.counts.refused_ons, out.release_refused), (128, 128, false));
    }
}

#[test]
fn a_release_that_finds_the_buffer_full_is_refused_and_marks_the_reset() {
    let mut out = Output::new();
    for i in 0..MIDI_FX_NON_RELEASE {
        assert!(out.push(TimedEvent { offset: 0, event: on(i as i32, 60, 100) }));
    }
    for i in MIDI_FX_NON_RELEASE..MIDI_FX_BLOCK {
        assert!(out.push(TimedEvent { offset: 0, event: off(i as i32, 60) }));
    }
    assert!(!out.release_refused);
    assert!(!out.push(TimedEvent { offset: 0, event: off(1000, 60) }));
    assert!(out.release_refused, "the block's reset barrier");
    let ids: Vec<_> = out.events().map(|e| e.event.note_id).collect();
    assert_eq!(ids, (0..MIDI_FX_BLOCK as i32).collect::<Vec<_>>(), "nothing evicted");
    assert_eq!((out.counts.overflow, out.counts.refused_ons), (1, 0));
}

#[test]
fn carried_events_enter_the_buffer_before_the_effect_writes() {
    // ADR 0057, slice P3b: 64 carried note-ons due in the block, then the effect writes 384
    // controllers and 64 note-offs of them. The note-ons take buffer room first, so they are
    // delivered, the controllers are refused past the non-release limit (320 accepted) and every
    // note-off ends its note.
    let t = track(&[("midi_velocity", &[])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    host.begin(&mut rec, &t, None, 0, &Transport::default(), 48_000.0, 0, FRAMES);
    let stage = host.stages[0].as_mut().unwrap();
    for (i, c) in stage.carry.iter_mut().enumerate() {
        *c = Carried { due: 0, event: on(i as i32, 60, 100) };
    }
    stage.carry_len = MIDI_FX_CARRY;
    for i in 0..MIDI_FX_BLOCK - MIDI_FX_CARRY {
        let e = if i < MIDI_FX_NON_RELEASE { ev(-1, Message::Control { cc: 1, value: 1 }) } else { off((i - MIDI_FX_NON_RELEASE) as i32, 60) };
        assert!(host.take(&mut rec, 0, &e));
    }
    host.flush(&mut rec);
    let kinds = |pick: fn(&Message) -> bool| rec.heard.iter().filter(|(_, _, e)| pick(&e.message)).count();
    assert_eq!(kinds(|m| matches!(m, Message::NoteOn { .. })), MIDI_FX_CARRY, "the carried note-ons");
    assert_eq!(kinds(|m| matches!(m, Message::NoteOff { .. })), MIDI_FX_CARRY);
    assert_eq!(kinds(|m| matches!(m, Message::Control { .. })), MIDI_FX_NON_RELEASE - MIDI_FX_CARRY);
    assert!(rec.heard.iter().take(MIDI_FX_CARRY).all(|(_, _, e)| matches!(e.message, Message::NoteOn { .. })), "ahead of the block's events");
    assert!(host.out.iter().all(|n| !n.used), "every note ended");
    assert_eq!((host.counts.overflow, host.counts.refused_ons, host.counts.safety_resets), (64, 0, 0));
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
    assert_eq!((host.counts.overflow, host.counts.refused_ons, host.counts.untracked), (1, 1, 1));
    host.panic(&mut rec);
    assert_eq!(rec.heard.iter().filter(|(_, _, e)| matches!(e.message, Message::NoteOff { .. })).count(), MIDI_FX_NOTES);
}

#[test]
fn a_release_in_a_full_carry_never_puts_its_off_before_its_on() {
    let t = track(&[("midi_test_delay", &[("samples", 1000.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    host.begin(&mut rec, &t, None, 0, &Transport::default(), 48_000.0, 0, FRAMES);
    let stage = host.stages[0].as_mut().unwrap();
    for (i, c) in stage.carry.iter_mut().enumerate() {
        let event = match i {
            0 => on(9, 62, 100),
            i if i == MIDI_FX_CARRY - 1 => on(7, 60, 100),
            _ => ev(-1, Message::Control { cc: 1, value: 1 }),
        };
        *c = Carried { due: 1000, event };
    }
    stage.carry_len = MIDI_FX_CARRY;
    assert!(host.take(&mut rec, 0, &off(7, 60)));
    host.flush(&mut rec);
    rec.process(&mut [], FRAMES);
    for _ in 0..3 {
        block(&mut host, &mut rec, &t, &[]);
    }
    // The release evicts its own on, which never sounded: nothing for note 60 reaches the instrument.
    let heard: Vec<_> = rec
        .heard
        .iter()
        .filter_map(|(_, _, e)| match e.message {
            Message::NoteOn { key: 60, .. } => Some("on"),
            Message::NoteOff { key: 60, .. } => Some("off"),
            _ => None,
        })
        .collect();
    assert_eq!(heard, Vec::<&str>::new());
    assert_eq!(host.counts.carry_overflow, 1);
}

#[test]
fn the_fuzz_seed_71271_through_two_delays_never_delivers_an_off_before_its_on() {
    // #281: zero-length notes through [Delay 64, Delay 37] fill the carry with releases and ons of
    // other notes, so a release that evicts from a full carry meets its own on still pending.
    let t = track(&[("midi_test_delay", &[("samples", 64.0)]), ("midi_test_delay", &[("samples", 37.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    let mut state: u32 = 71271;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut voice = 0;
    for _ in 0..200 {
        let mut events = Vec::new();
        for _ in 0..next() % 160 {
            voice += 1;
            let key = 60 + (next() % 6) as u8;
            let at = next() % FRAMES as u32;
            let off_at = if next() % 2 == 0 { at } else { (at + next() % 40).min(FRAMES as u32 - 1) };
            events.push((at, on(voice, key, 100)));
            events.push((off_at, off(voice, key)));
        }
        block(&mut host, &mut rec, &t, &events);
    }
    for _ in 0..3 {
        block(&mut host, &mut rec, &t, &[]);
    }
    // A voice whose off reaches the instrument before any on for it: the instrument would hang it.
    let mut off_first = std::collections::HashSet::new();
    let mut sounded = std::collections::HashSet::new();
    for (_, _, e) in &rec.heard {
        match e.message {
            Message::NoteOn { .. } => {
                assert!(!off_first.contains(&e.note_id), "voice {} got its off before its on", e.note_id);
                sounded.insert(e.note_id);
            }
            Message::NoteOff { .. } if !sounded.contains(&e.note_id) => {
                off_first.insert(e.note_id);
            }
            _ => {}
        }
    }
    assert!(!host.pending());
    assert!(rec.heard.iter().any(|(_, _, e)| matches!(e.message, Message::NoteOn { .. })));
}

#[test]
fn a_same_key_release_without_a_voice_id_evicts_its_newest_on_not_the_oldest() {
    // Repeated notes on one key without voice ids, through a 1000-sample delay: the block's events
    // fill the 64-slot carry and the last release overflows it.
    let t = track(&[("midi_test_delay", &[("samples", 1000.0)])]);
    let (mut host, mut rec) = (MidiFxHost::new(), Rec::default());
    let mut events = vec![(0, on(-1, 60, 100)), (1, off(-1, 60)), (2, on(-1, 60, 100))];
    for i in 0..61 {
        events.push((3 + i, ev(-1, Message::Control { cc: 1, value: 1 })));
    }
    events.push((100, off(-1, 60)));
    block(&mut host, &mut rec, &t, &events);
    for _ in 0..8 {
        block(&mut host, &mut rec, &t, &[]);
    }
    let heard: Vec<_> = rec
        .heard
        .iter()
        .filter_map(|(b, o, e)| match e.message {
            Message::NoteOn { key: 60, .. } => Some((*b, *o, "on")),
            Message::NoteOff { key: 60, .. } => Some((*b, *o, "off")),
            _ => None,
        })
        .collect();
    let mut sounding = false;
    for (_, _, k) in &heard {
        sounding = *k == "on";
    }
    assert!(host.counts.carry_overflow >= 1, "the carry must overflow");
    assert!(!sounding, "key 60 left sounding: {heard:?}");
}
