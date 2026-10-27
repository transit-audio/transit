//! Unit tests of the fan-out's own pieces; the worker set itself is exercised end to end in
//! `crates/mix/tests/render_workers.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use transit_dsp::event::NoteId;

/// Serializes the unit tests that flip the process-wide serial switch.
pub(crate) static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn event(message: Message, channel: u8) -> InstrumentEvent {
    InstrumentEvent {
        engine: EngineEpoch(1),
        target: (TrackId(1), INSTRUMENT_SLOT, SlotGen::VACANT),
        due: Due::Next,
        transport: None,
        source: Source { port: 0, channel, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    }
}

#[test]
fn the_ring_sees_the_classes_its_rules_need() {
    let id = NoteId::NONE;
    let on = to_event(&event(Message::NoteOn { key: 60, velocity: 90, id }, 3), 7);
    assert!(on.is_note_on() && on.channel() == 3 && on.offset == 7 && on.data1 == 60);
    let off = to_event(&event(Message::NoteOff { key: 60, velocity: 0, id }, 3), 9);
    assert!(off.is_note_off() && off.is_release());
    assert!(to_event(&event(Message::Control { cc: 64, value: 0 }, 0), 0).is_release(), "sustain off");
    assert!(!to_event(&event(Message::Control { cc: 64, value: 127 }, 0), 0).is_release(), "sustain on");
    assert!(!to_event(&event(Message::PitchBend { value: 100 }, 0), 0).is_release());
    assert!(!to_event(&event(Message::Program { program: 4 }, 0), 0).is_release());
    assert_eq!(on.tag, Event::NO_TAG, "the caller tags it");
}

#[test]
fn a_block_period_follows_frames_and_rate() {
    assert_eq!(block_period(128, 48_000), Duration::from_nanos(2_666_666));
    assert_eq!(block_period(64, 48_000), Duration::from_nanos(1_333_333));
    assert_eq!(block_period(64, 0), Duration::from_millis(1), "an unknown rate has a floor");
}

#[test]
fn sending_a_strip_away_keeps_its_routing_state_and_identity_in_the_table() {
    let mut table = Strip::new();
    table.latency = 64;
    table.pdc_out = 99;
    table.pdc_pos = 3;
    table.generation = SlotGen(7);
    table.track = 2;
    table.id = TrackId(42);
    table.sink_routes = vec![Some(1)];
    table.meter.peaks = vec![0.5];
    table.buf = vec![vec![0.25; 4]];
    let mut cell = Strip::new();
    send_away(&mut table, &mut cell);
    assert_eq!((table.latency, table.pdc_out, table.pdc_pos), (64, 99, 3), "the delay walk reads the table");
    assert_eq!((table.generation, table.track, table.id), (SlotGen(7), 2, TrackId(42)), "event lookups find the strip");
    assert_eq!(table.sink_routes, [Some(1)]);
    assert_eq!(table.meter.peaks, [0.5]);
    assert!(table.buf.is_empty(), "the render state went with the strip");
    assert_eq!(cell.buf, [vec![0.25; 4]]);
    table.pdc_out = 100;
    take_home(&mut table, &mut cell);
    assert_eq!((table.pdc_out, table.latency, table.buf.len()), (100, 64, 1), "home again with what the table updated");
    assert_eq!(table.sink_routes, [Some(1)]);
    assert_eq!(cell.generation, SlotGen::VACANT, "the spare carries no identity");
    assert!(cell.sink_routes.is_empty() && cell.buf.is_empty());
}
