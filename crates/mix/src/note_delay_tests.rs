#![allow(clippy::unwrap_used)]
use super::*;
use transit_dsp::event::{Message, NoteId, PluginEvent};

struct Reader {
    events: Vec<TimedEvent>,
    held: [bool; 128],
}
impl Reader {
    fn new() -> Self {
        Self { events: Vec::new(), held: [false; 128] }
    }
}
impl Plugin for Reader {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    fn event(&mut self, offset: u32, event: &PluginEvent) -> bool {
        self.events.push(TimedEvent { offset, event: *event });
        match event.message {
            Message::NoteOn { key, .. } => self.held[key as usize] = true,
            Message::NoteOff { key, .. } => self.held[key as usize] = false,
            Message::Control { cc: 123, .. } => self.held.fill(false),
            _ => {}
        }
        true
    }
}

fn note(offset: u32, on: bool) -> TimedEvent {
    let message =
        if on { Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE } };
    TimedEvent { offset, event: PluginEvent { channel: 0, note_id: -1, message } }
}

#[test]
fn zero_delay_preserves_the_original_delivery_order_and_offsets() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    let input = [note(12, true), note(2, false), note(12, true)];
    assert_eq!(delay.deliver(&input, 0, 64, &mut reader), 0);
    assert_eq!(reader.events, input);
}

#[test]
fn block_boundary_and_larger_than_plan_latency_are_carried_exactly() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    let latency = crate::MAX_LATENCY + 17;
    assert_eq!(delay.deliver(&[note(1, true)], latency, 64, &mut reader), 0);
    assert!(reader.events.is_empty());
    assert_eq!(delay.deliver(&[], latency, latency - 64 + 1, &mut reader), 0);
    assert!(reader.events.is_empty(), "an event at the block end belongs to the next block");
    assert_eq!(delay.deliver(&[], latency, 64, &mut reader), 0);
    assert_eq!(reader.events, [note(0, true)]);
}

#[test]
fn a_latency_reduction_delivers_a_dense_due_stream_late_without_dropping_it() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    let notes = vec![note(0, true); 400];
    for _ in 0..4 {
        assert_eq!(delay.deliver(&notes, 2048, 64, &mut reader), 0);
    }
    assert_eq!(delay.deliver(&[note(0, false)], 0, 64, &mut reader), 0);
    assert_eq!(reader.events.len(), NOTE_VIEW_CAP - RELEASE_RESERVE - 1);
    while delay.len > 0 {
        assert_eq!(delay.deliver(&[], 0, 64, &mut reader), 0);
    }
    assert_eq!(reader.events.len(), 1601);
    assert!(reader.events[..1600].iter().all(|e| e.offset == 0));
    assert_eq!(reader.events[1600].offset, 1, "the final release cannot collapse onto the starts");
    assert!(!reader.held[60], "the release follows every delayed start");
}

#[test]
fn a_full_carry_reserves_releases_and_stands_in_all_notes_off() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    let mut dropped = 0;
    // Feed legal block views, keeping everything pending at maximum plan latency.
    let notes = vec![note(0, true); 400];
    for _ in 0..42 {
        dropped += delay.deliver(&notes, crate::MAX_LATENCY, 64, &mut reader);
    }
    let open = CARRY_CAP - RELEASE_RESERVE;
    assert_eq!(delay.len, open);
    assert_eq!(dropped, (42 * 400 - open + 1) as u64);
    assert!(delay.carry[..delay.len].iter().any(|p| p.event == ALL_OFF));
    let releases = vec![note(0, false); RELEASE_RESERVE];
    assert_eq!(delay.deliver(&releases, crate::MAX_LATENCY, 64, &mut reader), 0);
    assert_eq!(delay.len, CARRY_CAP);
    assert_eq!(delay.deliver(&[note(0, false)], crate::MAX_LATENCY, 64, &mut reader), 1);
    while delay.len > 0 {
        assert_eq!(delay.deliver(&[], 0, 64, &mut reader), 0);
    }
    assert!(!reader.held[60]);
    assert_eq!(reader.events.len(), CARRY_CAP);
    assert_eq!(reader.events.iter().filter(|e| e.event == ALL_OFF).count(), 1);
}

#[test]
fn release_only_overflow_keeps_the_final_slot_for_the_stand_in() {
    let mut delay = NoteDelay::new();
    for _ in 0..CARRY_CAP - 1 {
        assert_eq!(delay.record(&note(0, false)), 0);
    }
    assert_eq!(delay.record(&note(0, false)), 1);
    assert_eq!(delay.carry[CARRY_CAP - 1].event, ALL_OFF);
    assert_eq!(delay.record(&note(0, false)), 1);
    delay.clear();
    assert_eq!(delay.record(&note(0, true)), 0);
    assert_eq!(delay.len, 1);
}

#[test]
fn a_reset_discards_pending_starts_and_releases() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    delay.deliver(&[note(1, true), note(2, false)], 768, 64, &mut reader);
    delay.clear();
    delay.deliver(&[], 0, 1024, &mut reader);
    assert!(reader.events.is_empty());
}

#[test]
fn overdue_pairs_keep_a_sample_of_envelope_even_in_one_frame_blocks() {
    for block in [1, 64] {
        let mut delay = NoteDelay::new();
        let mut filter = transit_dsp::create("filter").unwrap();
        assert!(filter.set_param("amp_on", 1.0));
        assert!(filter.set_param("amp_attack", 0.5));
        assert!(filter.set_param("amp_release", 1.0));
        filter.prepare(48_000.0, 64, 2);
        assert_eq!(delay.deliver(&[note(25, true), note(50, false)], 769, 64, &mut *filter), 0);
        let mut io = vec![vec![0.0; 64]; 2];
        filter.process(&mut io, 64);
        io.iter_mut().for_each(|c| c.fill(1.0));
        assert_eq!(delay.deliver(&[], 0, block, &mut *filter), 0);
        filter.process(&mut io, block);
        assert!(io[0][0] > 0.0, "the amp envelope advances before its release, block {block}");
        if block == 1 {
            assert_eq!(delay.len, 1, "the release waits for the next sample");
            assert_eq!(delay.deliver(&[], 0, 1, &mut *filter), 0);
            filter.process(&mut io, 1);
        }
        assert_eq!(delay.len, 0);
        for _ in 0..100 {
            io.iter_mut().for_each(|c| c.fill(1.0));
            filter.process(&mut io, 64);
        }
        assert!(io.iter().flatten().all(|v| *v == 0.0), "the release closes the amp");
    }
}

#[test]
fn splitting_an_overdue_pair_preserves_every_subsequent_offset() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    delay.deliver(&[note(1, true), note(2, false), note(3, true), note(4, false)], 769, 64, &mut reader);
    delay.deliver(&[], 0, 64, &mut reader);
    assert_eq!(reader.events.iter().map(|e| e.offset).collect::<Vec<_>>(), [0, 1, 1, 2]);
    assert!(!reader.held[60]);
}

#[test]
fn out_of_order_views_leave_the_carry_sorted_like_a_full_sort() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..40 {
        let view: Vec<TimedEvent> = (0..(next() % 90) + 1).map(|_| note(next() % 64, next() % 2 == 0)).collect();
        delay.deliver(&view, crate::MAX_LATENCY, 64, &mut reader);
        let keys: Vec<_> = delay.carry[..delay.len].iter().map(|p| (p.source, p.order)).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "the carry stays strictly sorted by (source, order)");
    }
}

#[test]
fn starts_are_accepted_again_once_the_stand_in_has_left_the_carry() {
    let mut delay = NoteDelay::new();
    let mut reader = Reader::new();
    // Overflow while the delay is at its maximum, with legal 400-event views.
    let burst = vec![note(0, true); 400];
    let mut dropped = 0;
    for _ in 0..42 {
        dropped += delay.deliver(&burst, crate::MAX_LATENCY, 64, &mut reader);
    }
    // The delay then drops to a short one: the flood drains at the per-block limit, and the sparse
    // notes always leave something pending, so the carry is never empty (the old latch needed that).
    let latency = 640;
    assert!(dropped > 0 && delay.stand_in.is_some());
    // Sparse playing while the flood drains: refused (and counted) until the stand-in goes out.
    let (mut refused, mut accepted_after, mut stand_in_gone) = (0u64, 0, false);
    for _ in 0..200 {
        let before = delay.stand_in.is_some();
        let lost = delay.deliver(&[note(3, true), note(9, false)], latency, 64, &mut reader);
        if before && delay.stand_in.is_some() {
            assert_eq!(lost, 1, "the start is refused and counted; its release is queued behind the stand-in");
            refused += lost;
        } else if !before {
            assert_eq!(lost, 0, "no refusal after the stand-in left");
            accepted_after += 1;
        }
        stand_in_gone |= delay.stand_in.is_none();
    }
    assert!(stand_in_gone && refused > 0 && accepted_after > 0);
    // Drain: every sparse note after the stand-in sounds, and nothing is left held.
    while delay.len > 0 {
        delay.deliver(&[], 0, 64, &mut reader);
    }
    assert!(!reader.held[60], "nothing stays held");
    let stand_in_at = reader.events.iter().position(|e| e.event == ALL_OFF).unwrap();
    let ons_after = reader.events[stand_in_at..].iter().filter(|e| matches!(e.event.message, Message::NoteOn { .. })).count();
    assert_eq!(ons_after, accepted_after, "every start accepted after the stand-in is delivered after it");
}
