//! ADR 0059, 3b: arrival order at one frame, and at VST3 (`Delivery::new(true)`) the separation
//! of controller-borne voice events onto successive frames with bounded service. A VST3 plug-in
//! reads a frame's parameter points (one per parameter and offset, the last) either before or
//! after that frame's note events; both readings must give the voice state of arrival order.

use super::delivery::{Delivery, Item};
use super::tests::{Fake, block, cc, off, on, play};
use crate::event::Message;
use std::collections::BTreeMap;

/// The state a VST3 plug-in reaches from the delivered log, reading each frame's points (the last
/// per controller) before or after its note events.
fn vst3_reading(log: &[(usize, u64, Item)], points_first: bool) -> Fake {
    let mut fake = Fake::default();
    let mut frames: BTreeMap<u64, Vec<Item>> = BTreeMap::new();
    for (_, f, it) in log {
        frames.entry(*f).or_default().push(*it);
    }
    for items in frames.values() {
        let mut points: Vec<Item> = Vec::new();
        for it in items.iter().filter(|it| matches!(it.event.message, Message::Control { .. })) {
            let key = |x: &Item| match x.event.message {
                Message::Control { cc, .. } => (x.event.channel, cc),
                _ => (0, 0),
            };
            points.retain(|p| key(p) != key(it));
            points.push(*it);
        }
        let notes: Vec<Item> = items.iter().filter(|it| !matches!(it.event.message, Message::Control { .. })).copied().collect();
        let (a, b) = if points_first { (&points, &notes) } else { (&notes, &points) };
        for it in a.iter().chain(b.iter()) {
            fake.act(it);
        }
    }
    fake
}

/// The state of applying the log in arrival order (what CLAP and the Audio Unit give).
fn arrival(log: &[(usize, u64, Item)]) -> Fake {
    let mut items: Vec<Item> = log.iter().map(|x| x.2).collect();
    items.sort_by_key(|it| it.seq);
    let mut fake = Fake::default();
    for it in &items {
        fake.act(it);
    }
    fake
}

fn frames_of(log: &[(usize, u64, Item)]) -> Vec<u64> {
    log.iter().map(|x| x.1).collect()
}

/// Runs `script` at every block size, separated and not; checks that VST3's two readings and
/// arrival order agree on the final voice state, and that the separated frames are the same at
/// every partition. Returns the separated log at block size 512.
fn agree(script: &[(u64, crate::event::PluginEvent)], total: u64) -> Vec<(usize, u64, Item)> {
    let mut first: Option<Vec<u64>> = None;
    let mut out = Vec::new();
    for size in [1u32, 7, 64, 512] {
        let mut q = Delivery::new(false);
        let mut plain = Fake::default();
        assert!(play(&mut q, &mut plain, script, size, total).is_empty());
        let want = arrival(&plain.log);
        assert_eq!(plain.keys(), want.keys(), "one frame's events in arrival order at size {size}");
        let mut q = Delivery::new(true);
        let mut sep = Fake::default();
        assert!(play(&mut q, &mut sep, script, size, total).is_empty());
        for points_first in [true, false] {
            let got = vst3_reading(&sep.log, points_first);
            assert_eq!(got.keys(), want.keys(), "VST3 state, points first: {points_first}, size {size}");
            assert_eq!(got.pedal.iter().filter(|(_, d)| **d).count(), want.pedal.iter().filter(|(_, d)| **d).count());
        }
        let frames = frames_of(&sep.log);
        if let Some(f) = &first {
            assert_eq!(&frames, f, "the same frames at size {size}");
        } else {
            first = Some(frames);
        }
        out = sep.log;
    }
    out
}

#[test]
fn a_safety_controller_and_a_note_on_at_one_frame_keep_their_order() {
    // CC 123 then a note-on with no voices: the note sounds. CC 120 the same.
    for safety in [123, 120] {
        let log = agree(&[(10, cc(0, safety, 0)), (10, on(0, 60, -1))], 32);
        assert_eq!(frames_of(&log), [10, 11], "separated onto successive frames");
        assert_eq!(vst3_reading(&log, false).keys(), [(0, 60)], "the note sounds when notes are read first");
        // The note-on first: CC 123 ends it.
        let log = agree(&[(10, on(0, 60, -1)), (10, cc(0, safety, 0))], 32);
        assert_eq!(frames_of(&log), [10, 10, 11], "the on and its implied off share a frame, the controller follows");
        for points_first in [true, false] {
            assert!(vst3_reading(&log, points_first).keys().is_empty(), "the note ends, points first: {points_first}");
        }
    }
}

#[test]
fn reset_controllers_interleaved_with_pedal_edges_and_a_note_off() {
    let script = [
        (0, on(0, 60, -1)),
        (0, on(0, 62, -1)),
        (4, cc(0, 64, 127)),
        (4, off(0, 60, -1)),
        (4, cc(0, 121, 0)),
        (4, cc(0, 66, 127)),
        (4, off(0, 62, -1)),
        (4, cc(0, 64, 127)),
    ];
    let log = agree(&script, 64);
    // The note-on pair shares frame 0; every event at frame 4 that changes voice state has its own
    // frame, the CC 121's implied sustain release included.
    let at4: Vec<u64> = frames_of(&log).into_iter().filter(|f| *f >= 4).collect();
    let mut sorted = at4.clone();
    sorted.dedup();
    assert!(at4.len() >= 6 && sorted.len() >= 5, "separated: {at4:?}");
}

#[test]
fn every_pair_of_controller_borne_voice_events_goes_on_successive_frames() {
    let kinds: [(u8, u8); 7] = [(64, 127), (66, 127), (67, 127), (69, 127), (120, 0), (121, 0), (123, 0)];
    for a in kinds {
        for b in kinds {
            if a.0 == b.0 {
                continue;
            }
            let mut q = Delivery::new(true);
            let mut fake = Fake::default();
            assert!(play(&mut q, &mut fake, &[(3, cc(2, a.0, a.1)), (3, cc(2, b.0, b.1))], 16, 16).is_empty());
            // Every controller-borne event (the implied releases included) on its own frame, from
            // 3 on, and the two controllers in arrival order.
            let got: Vec<(u64, u8, u8)> = fake
                .log
                .iter()
                .filter_map(|(_, f, it)| if let Message::Control { cc, value } = it.event.message { Some((*f, cc, value)) } else { None })
                .collect();
            let frames: Vec<u64> = got.iter().map(|g| g.0).collect();
            assert_eq!(frames, (3..3 + got.len() as u64).collect::<Vec<_>>(), "{a:?} then {b:?}: {got:?}");
            let ia = got.iter().position(|g| (g.1, g.2) == a).expect("first controller");
            let ib = got.iter().rposition(|g| (g.1, g.2) == b).expect("second controller");
            assert!(ia < ib, "{a:?} then {b:?}: {got:?}");
        }
    }
}

#[test]
fn a_press_a_note_off_and_a_value_at_one_frame_sustain_the_note() {
    // With the pedal up: CC 64 = 127, note-off A, CC 64 = 100 sustains A; the same with CC 66.
    for pedal in [64, 66] {
        let log = agree(&[(0, on(0, 60, -1)), (8, cc(0, pedal, 127)), (8, off(0, 60, -1)), (8, cc(0, pedal, 100))], 32);
        assert_eq!(arrival(&log).keys(), [(0, 60)], "A sustains under CC {pedal}");
        let got: Vec<u64> = frames_of(&log);
        assert_eq!(got, [0, 8, 9, 9], "the press at 8, note-off A and the value at 9");
    }
    // Note-off A then CC 64 = 127 then CC 66 = 127: A ends.
    let log = agree(&[(0, on(0, 60, -1)), (8, off(0, 60, -1)), (8, cc(0, 64, 127)), (8, cc(0, 66, 127))], 32);
    assert!(arrival(&log).keys().is_empty());
    assert_eq!(frames_of(&log), [0, 8, 9, 10]);
}

#[test]
fn a_sustain_press_and_a_sostenuto_release_at_one_frame_in_both_orders() {
    let base = [(0, on(0, 60, -1)), (1, cc(0, 66, 127)), (2, off(0, 60, -1))];
    for press_first in [true, false] {
        let mut script = base.to_vec();
        if press_first {
            script.extend([(9, cc(0, 64, 127)), (9, cc(0, 66, 0))]);
        } else {
            script.extend([(9, cc(0, 66, 0)), (9, cc(0, 64, 127))]);
        }
        let log = agree(&script, 32);
        let want = if press_first { vec![(0, 60)] } else { vec![] };
        assert_eq!(arrival(&log).keys(), want, "press first: {press_first}");
    }
}

#[test]
fn three_sustain_values_at_one_offset_end_released() {
    // The pedal down before; CC 64 values 0, 127, 0 at one offset.
    let script = [(0, cc(0, 64, 127)), (5, cc(0, 64, 0)), (5, cc(0, 64, 127)), (5, cc(0, 64, 0))];
    let log = agree(&script, 16);
    assert_eq!(frames_of(&log), [0, 5, 6, 7], "two edges at one frame are separated");
    for points_first in [true, false] {
        assert!(!vst3_reading(&log, points_first).pedal.values().any(|d| *d), "the pedal ends released");
    }
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    play(&mut q, &mut fake, &script, 16, 16);
    assert_eq!(frames_of(&fake.log), [0, 5, 5, 5], "arrival order at one offset elsewhere");
    assert!(!fake.pedal.values().any(|d| *d));
}

#[test]
fn an_edits_pedal_up_meets_a_clip_press_at_one_frame() {
    let script = [(0, cc(0, 64, 127)), (20, cc(0, 64, 0)), (20, cc(0, 64, 127))];
    let log = agree(&script, 64);
    assert_eq!(frames_of(&log), [0, 20, 21], "at VST3 the press one frame later");
    let mut q = Delivery::new(false);
    let mut fake = Fake::default();
    play(&mut q, &mut fake, &script, 7, 64);
    assert_eq!(frames_of(&fake.log), [0, 20, 20], "the up and the press at their frame elsewhere");
}

#[test]
fn a_separation_at_every_frame_falls_behind_with_bounded_service() {
    // A pedal edge and a note event at every frame: press and note-on, then release and note-off.
    for total in [1204u64, 4000] {
        let mut script = Vec::new();
        for f in 0..total {
            let key = ((f / 2) % 128) as u8;
            let ch = 0u8;
            if f % 2 == 0 {
                script.push((f, cc(ch, 64, 127)));
                script.push((f, on(ch, key, -1)));
            } else {
                script.push((f, cc(ch, 64, 0)));
                script.push((f, off(ch, key, -1)));
            }
        }
        let mut frames_at: Option<Vec<u64>> = None;
        for size in [1u32, 7, 64, 512] {
            let mut q = Delivery::new(true);
            let mut fake = Fake::default();
            let mut refused = 0usize;
            let mut at = 0u64;
            let mut next = 0usize;
            let end = total + 2 * 2480;
            while at < end {
                let n = u64::from(size).min(end - at);
                while let Some(&(f, ev)) = script.get(next) {
                    if f >= at + n {
                        break;
                    }
                    if !q.admit((f - at) as u32, &ev) {
                        refused += 1;
                    }
                    next += 1;
                }
                assert!(q.ledger().outstanding() <= super::ledger::ENTRIES);
                block(&mut q, &mut fake, at, n as u32);
                at += n;
            }
            assert_eq!(q.refusals.starts as usize, refused, "every refusal counted");
            assert!(q.items().is_empty(), "everything delivered in the end");
            // Never overtaken: delivery order is arrival order.
            let seqs: Vec<u64> = fake.log.iter().map(|x| x.2.seq).collect();
            assert!(seqs.windows(2).all(|w| w[0] < w[1]), "no waiting event overtaken at size {size}");
            // At least one event served every frame while one waits, and no delay above 2,480.
            let mut by_frame: BTreeMap<u64, usize> = BTreeMap::new();
            for (_, f, _) in &fake.log {
                *by_frame.entry(*f).or_default() += 1;
            }
            let last = fake.log.last().map_or(0, |x| x.1);
            assert!((0..=last).all(|f| by_frame.contains_key(&f)), "a frame served nothing at size {size}");
            let max_delay = arrival_delay(&script, &fake.log);
            assert!(max_delay <= 2480, "delay {max_delay} at size {size}");
            if total == 1204 && size == 1 {
                assert_eq!(refused, 0, "604 entries by frame 1,204 at block size 1");
            }
            if total == 4000 {
                assert!(refused > 0, "a sustained stream refuses starts at size {size}");
            }
            let frames = frames_of(&fake.log);
            if refused == 0 {
                if let Some(f) = &frames_at {
                    assert_eq!(&frames, f);
                } else {
                    frames_at = Some(frames);
                }
            }
        }
    }
}

/// The largest delivery delay: each delivered event's frame against its scripted frame (matched
/// by arrival order among the admitted events).
fn arrival_delay(script: &[(u64, crate::event::PluginEvent)], log: &[(usize, u64, Item)]) -> u64 {
    let mut delays = 0u64;
    let mut it = script.iter();
    for (_, f, item) in log {
        // Skip scripted events that were refused (never delivered): match on the event itself.
        for (sf, ev) in it.by_ref() {
            if ev.message == item.event.message && ev.channel == item.event.channel {
                delays = delays.max(f.saturating_sub(*sf));
                break;
            }
        }
    }
    delays
}
