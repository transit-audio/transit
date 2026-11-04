#![allow(clippy::unwrap_used)]
use super::*;
use transit_midi_io::{InputConfig, fake::Fake};

#[test]
fn complete_transport_reaches_control_without_routes_monitor_or_learn() {
    let (control, _plans) = super::tests::bridge();
    let (lane, _audio) = action_lane::pair_with_result(&control);
    let (send, mut pump) = super::tests::pump(lane);
    let (tx, rx) = transport::pair();
    pump.transport_out = tx;
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let (_handle, consumer) = fake.io().open_input(&key, InputConfig::default()).unwrap();
    send.send(Update::Input(Box::new(Input {
        id: 7,
        consumer,
        routes: Vec::new(),
        mon: Mon::default(),
        guard: storm::Guard::default(),
        live: false,
        buttons: Vec::new(),
        buttons_enabled: true,
    })))
    .ok()
    .unwrap();
    pump.step();
    let port = fake.input_port(&key).unwrap();
    port.deliver_at(0, 10, &[0xf8]);
    let worked = pump.step();
    let (lost, events) = rx.take();
    assert!(events.is_empty(), "Clock alone must not publish transport");
    assert!(!lost);
    assert!(!worked, "Clock alone must not keep the transport worker awake");
    port.deliver_at(0, 20, &[0xf0, 0x7f, 0, 6, 2, 0xf8]);
    port.deliver_at(0, 30, &[0xf7, 0xfc]);
    let (_, allocations) = transit_rt::testing::measure(|| pump.step());
    assert!(allocations.is_zero(), "{allocations:?}");
    let (lost, events) = rx.take();
    assert!(!lost);
    assert_eq!(events.iter().map(|(id, raw)| (*id, raw.message())).collect::<Vec<_>>(), [(7, &[0xf0, 0x7f, 0, 6, 2, 0xf7][..]), (7, &[0xfc][..])]);
    assert_eq!(events[0].1.host_arrival_ns, 20);
    pump.step();
    assert!(rx.take().1.is_empty(), "each sequence is forwarded once");
}

#[test]
fn clock_flood_never_publishes_to_control_or_signals_loss() {
    let (mut tx, rx) = transport::pair();
    let raw = transit_midi_io::TransportEnvelope { bytes: [0xf8; 13], len: 1, seq: 0, host_arrival_ns: 0 };
    tx.push(1, raw);
    let (lost, events) = rx.take();
    assert!(events.is_empty(), "Clock must not enter control publication");
    assert!(!lost);
    for _ in 0..transit_midi_io::TRANSPORT_CAPACITY * 2 {
        tx.push(1, raw);
    }
    let (lost, events) = rx.take();
    assert!(events.is_empty());
    assert!(!lost);
    let mut stop = raw;
    stop.bytes[0] = 0xfc;
    tx.push(1, stop);
    assert_eq!(rx.take(), (false, vec![(1, stop)]));
}

#[test]
fn full_control_lane_publishes_sticky_loss_instead_of_losing_stop() {
    let (mut tx, rx) = transport::pair();
    let raw = transit_midi_io::TransportEnvelope { bytes: [0xfa; 13], len: 1, seq: 0, host_arrival_ns: 0 };
    for _ in 0..transit_midi_io::TRANSPORT_CAPACITY {
        tx.push(1, raw);
    }
    let mut stop = raw;
    stop.bytes[0] = 0xfc;
    tx.push(1, stop);
    tx.push(1, raw);
    let (lost, events) = rx.take();
    assert!(lost);
    assert!(events.is_empty(), "old starts cannot escape a loss");
    assert!(!rx.take().0);
}

#[test]
fn learning_captures_once_and_releases_before_or_after_the_new_mapping_is_installed() {
    for note in [false, true] {
        let mut mon = Mon::default();
        let shared = Shared::default();
        shared.learn_transport.store(true, Ordering::Release);
        let (mut out, mut events) = rtrb::RingBuffer::new(8);
        shared.learn.store(true, Ordering::Release);
        let raw = |value| InputEvent { bytes: [if note { 0x90 } else { 0xb0 }, 20, value], len: 3, seq: 0, host_arrival_ns: 0, backend_time_us: 0 };
        assert_eq!(transport::capture(&mut mon, 1, raw(127), &mut out, &shared), Some(true));
        assert!(matches!(events.pop().unwrap(), MapEvent::Candidate { number: 20, .. }));
        assert_eq!(transport::capture(&mut mon, 1, raw(127), &mut out, &shared), Some(true), "held repeat remains consumed");
        assert!(events.pop().is_err());
        assert_eq!(transport::capture(&mut mon, 1, raw(0), &mut out, &shared), Some(true), "release is consumed without firing");
        assert_eq!(transport::capture(&mut mon, 1, raw(127), &mut out, &shared), None, "the next intentional press reaches the new mapping");
    }
}

#[test]
fn learn_chooses_first_wire_candidate_across_both_copies_and_never_replays_it() {
    let (control, _plans) = super::tests::bridge();
    let (lane, _audio) = action_lane::pair_with_result(&control);
    let (send, mut pump) = super::tests::pump(lane);
    let (out, mut candidates) = rtrb::RingBuffer::new(8);
    pump.map_out = out;
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let (_handle, consumer) = fake.io().open_input(&key, InputConfig::default()).unwrap();
    consumer.boundary().set_transport_sources(&[transit_midi_io::TransportSource { channel: None, note: false, number: 20 }]);
    send.send(Update::Input(Box::new(Input {
        id: 7,
        consumer,
        routes: Vec::new(),
        mon: Mon::default(),
        guard: storm::Guard::default(),
        live: false,
        buttons: Vec::new(),
        buttons_enabled: true,
    })))
    .ok()
    .unwrap();
    pump.step();
    pump.shared.learn.store(true, Ordering::Release);
    pump.step();
    let port = fake.input_port(&key).unwrap();
    port.deliver_at(0, 0, &[0xb0, 19, 127, 0xb0, 20, 127, 0xb0, 19, 0, 0xb0, 20, 0]);
    pump.step();
    assert!(matches!(candidates.pop().unwrap(), MapEvent::Candidate { number: 19, .. }));
    pump.shared.learn.store(true, Ordering::Release);
    pump.step();
    assert!(candidates.pop().is_err(), "a musical copy cannot be captured again");
    port.deliver_at(0, 1, &[0xb0, 20, 127, 0xb0, 20, 0]);
    pump.step();
    assert!(matches!(candidates.pop().unwrap(), MapEvent::Candidate { number: 20, .. }));
    pump.shared.learn.store(true, Ordering::Release);
    pump.step();
    assert!(candidates.pop().is_err());
}
