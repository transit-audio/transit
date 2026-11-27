//! ADR 0059 R1 through the CLAP adapter's native translation and processing boundary (review
//! round 3): the events plug-in logs every event it applies through its record channel (the
//! parameter log, read with the saved state), apart from its audio output, so blocks down to one
//! frame can be inspected. CLAP delivers in one time-sorted list in arrival order at one offset, so
//! nothing is separated or delayed: every event arrives at its own frame, and the expected native
//! sequences below are exact. Notes are CLAP note events with the entry's note id; controllers are
//! MIDI messages on the note port.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};

mod common;

const EVENTS: &str = "clap:org.soundcraft.test.events";

fn plugin(block: usize) -> Box<dyn Plugin> {
    common::events_fixture_dir();
    let mut p = transit_clap_host::create(EVENTS).unwrap();
    p.prepare(48_000.0, block, 2);
    assert!(p.load_state(b"CMD:log-start"), "the record channel");
    let _ = p.save_state();
    p
}

/// One applied event at its absolute frame: (frame, code, a, b) as the fixture logs it.
type Rec = (u64, u32, i64, i64);

/// The events applied since the last read, the current call starting at frame `at`.
fn log(p: &mut Box<dyn Plugin>, at: u64) -> Vec<Rec> {
    let state = p.save_state().unwrap();
    String::from_utf8_lossy(state.get(16..).unwrap_or(&[]))
        .lines()
        .filter_map(|l| l.strip_prefix("REC "))
        .map(|l| {
            let f: Vec<i64> = l.split(' ').map(|x| x.parse().unwrap()).collect();
            (at + f[0] as u64, f[1] as u32, f[2], f[3])
        })
        .collect()
}

fn ev(channel: u8, message: Message) -> PluginEvent {
    PluginEvent { channel, note_id: PluginEvent::NO_ID, message }
}
fn on(channel: u8, key: u8) -> PluginEvent {
    ev(channel, Message::NoteOn { key, velocity: 100, id: NoteId::NONE })
}
fn off(channel: u8, key: u8) -> PluginEvent {
    ev(channel, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
}
fn cc(channel: u8, n: u8, v: u8) -> PluginEvent {
    ev(channel, Message::Control { cc: n, value: v })
}
/// A controller as the MIDI record's `a`.
fn midi(channel: u8, n: u8, v: u8) -> i64 {
    i64::from(0xB0 | channel) << 16 | i64::from(n) << 8 | i64::from(v)
}

/// Plays `script` through `p` in blocks of `size` for `total` frames: the records and the refused.
fn play(p: &mut Box<dyn Plugin>, script: &[(u64, PluginEvent)], size: usize, total: u64) -> (Vec<Rec>, usize) {
    let (mut recs, mut refused, mut at, mut next) = (Vec::new(), 0, 0u64, 0);
    let mut io = vec![vec![0.0f32; size], vec![0.0f32; size]];
    while at < total {
        let n = (size as u64).min(total - at);
        while let Some(&(f, e)) = script.get(next) {
            if f >= at + n {
                break;
            }
            refused += usize::from(!p.event((f - at) as u32, &e));
            next += 1;
        }
        p.process(&mut io, n as usize);
        recs.extend(log(p, at));
        at += n;
    }
    assert_eq!(transit_clap_host::UNTRANSLATED.load(std::sync::atomic::Ordering::Relaxed), 0);
    (recs, refused)
}

/// Held notes (by note id), sounding under the sustain pedal, and pedals down, after `recs`.
#[derive(Default, Debug, PartialEq)]
struct Voices {
    held: BTreeSet<i64>,
    sustained: BTreeSet<i64>,
    down: BTreeSet<(i64, i64)>,
}

fn replay(recs: &[Rec]) -> Voices {
    let mut v = Voices::default();
    for &(_, code, a, b) in recs {
        match code {
            1 => {
                v.held.insert(b);
            }
            2 => {
                if v.held.remove(&b) && v.down.contains(&(a / 1000, 64)) {
                    v.sustained.insert(b);
                }
            }
            5 => {
                let (ch, n, val) = ((a >> 16) & 0x0F, (a >> 8) & 0x7F, a & 0x7F);
                match n {
                    64 | 66 | 67 | 69 if val >= 64 => {
                        v.down.insert((ch, n));
                    }
                    64 | 66 | 67 | 69 => {
                        v.down.remove(&(ch, n));
                        if n == 64 {
                            v.sustained.clear();
                        }
                    }
                    121 => v.down.retain(|d| d.0 != ch),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    v
}

const SIZES: [usize; 4] = [1, 7, 64, 512];

#[test]
fn five_press_cycles_sustain_a_note_to_the_third_release_at_every_size_and_through_skipped_calls() {
    let mut script = Vec::new();
    for c in 0..5u64 {
        script.push((c * 20, cc(0, 64, 127)));
        script.push((c * 20 + 10, cc(0, 64, 0)));
    }
    script.push((5, on(0, 60)));
    script.push((45, off(0, 60)));
    script.sort_by_key(|(f, _)| *f);
    for size in SIZES {
        let mut p = plugin(size);
        let (recs, refused) = play(&mut p, &script, size, 128);
        assert_eq!(refused, 0);
        // Every event at its own frame, in order: CLAP separates nothing.
        let frames: Vec<u64> = recs.iter().map(|r| r.0).collect();
        assert_eq!(frames, script.iter().map(|s| s.0).collect::<Vec<_>>(), "size {size}");
        let id = recs.iter().find(|r| r.1 == 1).unwrap().3;
        // The note ends at the third release (frame 50), not at its off (45).
        let upto = |f: u64| replay(&recs.iter().filter(|r| r.0 <= f).copied().collect::<Vec<_>>());
        assert!(upto(49).sustained.contains(&id), "sustained through frame 49 at size {size}");
        assert_eq!(upto(50), Voices::default(), "released by the third pedal-up at size {size}");
    }
    // Three skipped calls: each cycle arrives at offset 0 of the next call, in arrival order.
    let mut p = plugin(64);
    let mut io = vec![vec![0.0f32; 64]; 2];
    for _ in 0..3 {
        for e in [cc(0, 64, 127), cc(0, 64, 0), cc(0, 66, 127), cc(0, 66, 0)] {
            assert!(p.event(0, &e));
        }
        p.process(&mut io, 0);
    }
    p.process(&mut io, 64);
    let recs = log(&mut p, 0);
    let want: Vec<Rec> = [midi(0, 64, 127), midi(0, 64, 0), midi(0, 66, 127), midi(0, 66, 0)].repeat(3).into_iter().map(|a| (0, 5, a, 0)).collect();
    assert_eq!(recs, want, "every edge of the three skipped cycles at offset 0, in arrival order");
}

#[test]
fn every_pair_of_controller_borne_voice_events_arrives_at_its_frame_in_arrival_order() {
    let kinds: [(u8, u8); 7] = [(64, 127), (66, 127), (67, 127), (69, 127), (120, 0), (121, 0), (123, 0)];
    let pedal = |n: u8| matches!(n, 64 | 66 | 67 | 69);
    for a in kinds {
        for b in kinds {
            if a.0 == b.0 {
                continue;
            }
            let mut p = plugin(64);
            let (recs, refused) = play(&mut p, &[(3, cc(2, a.0, a.1)), (3, cc(2, b.0, b.1))], 64, 64);
            assert_eq!(refused, 0);
            // CC 121 after a press also releases that pedal: the implied release, then the reset.
            let mut want = vec![midi(2, a.0, a.1)];
            if b.0 == 121 && pedal(a.0) {
                want.push(midi(2, a.0, 0));
            }
            want.push(midi(2, b.0, b.1));
            let got: Vec<i64> = recs.iter().map(|r| r.2).collect();
            assert!(recs.iter().all(|r| r.0 == 3 && r.1 == 5), "{a:?} {b:?}: {recs:?}");
            assert_eq!(got, want, "{a:?} then {b:?}");
        }
    }
}

#[test]
fn capacity_cases_deliver_every_release_reset_and_start_in_order_at_every_size() {
    for size in SIZES {
        // 200 sounding notes; 200 offs over frames 1..=200, then 200 ons over 201..=400.
        let mut p = plugin(size);
        let mut script = Vec::new();
        let key = |k: u16| ((k % 16) as u8, (k / 16) as u8);
        for k in 0..200u16 {
            script.push((0, on(key(k).0, key(k).1)));
        }
        for k in 0..200u16 {
            script.push((1 + u64::from(k), off(key(k).0, key(k).1)));
        }
        for k in 0..200u16 {
            script.push((201 + u64::from(k), on(key(k).0, 20 + key(k).1)));
        }
        let (recs, refused) = play(&mut p, &script, size, 512);
        assert_eq!(refused, 0, "200/200 at size {size}");
        let first: Vec<Rec> = recs.iter().filter(|r| r.0 == 0).copied().collect();
        let offs: Vec<Rec> = recs.iter().filter(|r| r.1 == 2).copied().collect();
        let later: Vec<Rec> = recs.iter().filter(|r| r.1 == 1 && r.0 > 0).copied().collect();
        assert_eq!(first.len(), 200);
        for k in 0..200usize {
            let (ch, kk) = key(k as u16);
            assert_eq!((offs[k].0, offs[k].2), (1 + k as u64, i64::from(ch) * 1000 + i64::from(kk)), "off {k} at size {size}");
            assert_eq!(offs[k].3, first[k].3, "off {k} carries its on's note id");
            assert_eq!((later[k].0, later[k].2), (201 + k as u64, i64::from(ch) * 1000 + 20 + i64::from(kk)), "on {k}");
        }
        assert_eq!(replay(&recs).held.len(), 200, "the replacement notes sound");
        // 256 notes and 64 pedals held; 96 x CC 121 on channel 0, each followed by presses of its
        // four pedals, one event a frame.
        let mut p = plugin(size);
        let mut script = Vec::new();
        for k in 0..256u16 {
            script.push((0, on((k % 16) as u8, (k / 16) as u8)));
        }
        for ch in 0..16u8 {
            for n in [64, 66, 67, 69] {
                script.push((0, cc(ch, n, 127)));
            }
        }
        let mut f = 1u64;
        let mut want: Vec<(u64, i64)> = Vec::new();
        for _ in 0..96 {
            script.push((f, cc(0, 121, 0)));
            for n in [64, 66, 67, 69] {
                want.push((f, midi(0, n, 0))); // the implied releases, oldest first, before the reset
            }
            want.push((f, midi(0, 121, 0)));
            f += 1;
            for n in [64, 66, 67, 69] {
                script.push((f, cc(0, n, 127)));
                want.push((f, midi(0, n, 127)));
                f += 1;
            }
        }
        let (recs, refused) = play(&mut p, &script, size, 1024);
        assert_eq!(refused, 0, "96 x CC 121 at size {size}");
        let got: Vec<(u64, i64)> = recs.iter().filter(|r| r.0 > 0 && r.1 == 5 && (r.2 >> 16) & 0x0F == 0).map(|r| (r.0, r.2)).collect();
        assert_eq!(got, want, "channel 0's resets, releases and presses at size {size}");
        assert_eq!(recs.iter().filter(|r| r.0 == 0).count(), 256 + 64, "the held notes and pedals");
    }
}

#[test]
fn an_edge_and_a_note_at_every_frame_arrive_at_their_frames_with_nothing_refused() {
    for total in [1204u64, 4000] {
        let mut script = Vec::new();
        for f in 0..total {
            let key = ((f / 2) % 128) as u8;
            if f % 2 == 0 {
                script.push((f, cc(0, 64, 127)));
                script.push((f, on(0, key)));
            } else {
                script.push((f, cc(0, 64, 0)));
                script.push((f, off(0, key)));
            }
        }
        for size in SIZES {
            let mut p = plugin(size);
            let (recs, refused) = play(&mut p, &script, size, total);
            // CLAP orders the pair in one list at its frame: nothing waits, nothing is refused.
            assert_eq!(refused, 0, "{total} frames at size {size}");
            assert_eq!(recs.len(), script.len());
            for (r, (f, e)) in recs.iter().zip(&script) {
                assert_eq!(r.0, *f, "{total} at size {size}");
                let kind = match e.message {
                    Message::Control { cc, value } => (5, midi(0, cc, value)),
                    Message::NoteOn { key, .. } => (1, i64::from(key)),
                    Message::NoteOff { key, .. } => (2, i64::from(key)),
                    m => panic!("{m:?}"),
                };
                assert_eq!((r.1, r.2), kind, "frame {f}");
            }
            assert_eq!(replay(&recs), Voices::default());
        }
    }
}
