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
