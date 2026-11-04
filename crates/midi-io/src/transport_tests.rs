//! Transport framing uses supplied stamps; no worker or wall-clock assertions.
#![allow(clippy::unwrap_used)]
use crate::{InputConfig, TRANSPORT_CAPACITY, ring::InputSink};

#[test]
fn complete_transport_is_independent_of_monitor_and_musical_capacity() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::new(1));
    sink.push_stamped(0, 100, &[0xf0, 0x7f, 0, 6, 2, 0xf8]);
    assert!(input.pop_transport().is_none(), "neither Clock nor a prefix enters the button lane");
    sink.push_stamped(0, 200, &[0xf7, 0xfa, 0xf2, 4, 0]);
    let mmc = input.pop_transport().unwrap();
    assert_eq!(mmc.message(), &[0xf0, 0x7f, 0, 6, 2, 0xf7]);
    assert_eq!(mmc.host_arrival_ns, 100);
    let start = input.pop_transport().unwrap();
    assert_eq!(start.message(), &[0xfa]);
    assert!(start.seq > mmc.seq);
    assert_eq!(input.pop_transport().unwrap().message(), &[0xf2, 4, 0]);
    assert!(input.pop_sysex().is_none(), "Monitor never owned this copy");
}

#[test]
fn overlong_aborted_and_truncated_envelopes_never_publish_a_prefix() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::default());
    sink.push_stamped(0, 0, &[0xf0, 0x7f, 0, 6, 2]);
    assert!(input.pop_transport().is_none());
    sink.push_stamped(0, 1, &[0x90, 60, 100, 0xf7]);
    assert!(input.pop_transport().is_none());
    sink.push_stamped(0, 2, &[0xf0, 0x7f, 0, 6, 0x44, 6, 1, 0, 0, 10, 0, 0, 0, 0xf8, 0xf7]);
    assert!(input.pop_transport().is_none());
    sink.push_stamped(0, 3, &[0xf0, 0x7f, 0, 6, 1, 0xf7]);
    assert_eq!(input.pop_transport().unwrap().message(), &[0xf0, 0x7f, 0, 6, 1, 0xf7]);
}

#[test]
fn transport_overflow_is_sticky_and_cancellation_covers_dropped_stop_and_starts() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::default());
    let boundary = input.boundary();
    let (_, allocations) = transit_rt::testing::measure(|| {
        for n in 0..TRANSPORT_CAPACITY {
            sink.push_stamped(0, n as u64, &[0xfa]);
        }
        sink.push_stamped(0, 3000, &[0xfc, 0xfa]);
    });
    assert!(allocations.is_zero(), "{allocations:?}");
    assert!(boundary.take_transport_loss(), "the full queue must not lose Stop");
    let cutoff = boundary.cutoff();
    while let Some(raw) = input.pop_transport() {
        assert!(raw.seq < cutoff);
    }
    assert!(!boundary.take_transport_loss());
    sink.push_stamped(0, 4000, &[0xfa]);
    assert!(input.pop_transport().unwrap().seq >= cutoff, "only a fresh Start can resume");
}

#[test]
fn clock_flood_never_publishes_a_transport_envelope_or_loss() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::default());
    for n in 0..TRANSPORT_CAPACITY * 2 {
        sink.push_stamped(0, n as u64, &[0xf8]);
    }
    assert!(input.pop_transport().is_none(), "Clock must not enter the button lane");
    assert!(!input.boundary().take_transport_loss());
    sink.push_stamped(0, 5000, &[0xfc]);
    assert_eq!(input.pop_transport().unwrap().message(), &[0xfc]);
}

#[test]
fn learned_stop_has_independent_capacity_and_keeps_order_with_mmc() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::new(1));
    input.boundary().set_transport_sources(&[crate::TransportSource { channel: Some(0), note: false, number: 20 }]);
    for _ in 0..20 {
        sink.push_stamped(0, 0, &[0x90, 60, 100]);
    }
    assert!(input.counters().dropped > 0);
    sink.push_stamped(0, 100, &[0xb0, 20, 127, 0xf0, 0x7f, 0, 6, 7, 0xf7, 0xb0, 20, 0]);
    let press = input.pop_transport().unwrap();
    let exit = input.pop_transport().unwrap();
    let release = input.pop_transport().unwrap();
    assert_eq!(press.message(), &[0xb0, 20, 127]);
    assert_eq!(exit.message(), &[0xf0, 0x7f, 0, 6, 7, 0xf7]);
    assert_eq!(release.message(), &[0xb0, 20, 0]);
    assert!(press.seq < exit.seq && exit.seq < release.seq);
    assert!(!input.boundary().take_transport_loss());
    input.boundary().set_transport_sources(&[]);
    sink.push_stamped(0, 200, &[0xb0, 20, 127]);
    assert!(input.pop_transport().is_none());
}

#[test]
fn disabled_transport_flood_cannot_signal_loss_or_suppress_musical_input() {
    let (mut sink, mut input, _) = InputSink::new(&InputConfig::default());
    input.boundary().set_transport_enabled(false);
    for _ in 0..TRANSPORT_CAPACITY * 2 {
        sink.push_stamped(0, 0, &[0xf0, 0x7f, 0, 6, 2, 0xf7]);
    }
    assert!(!input.boundary().take_transport_loss());
    assert!(input.pop_transport().is_none());
    sink.push_stamped(0, 1, &[0xb0, 20, 100]);
    assert_eq!(input.pop().unwrap().message(), &[0xb0, 20, 100]);
    input.boundary().set_transport_enabled(true);
    sink.push_stamped(0, 2, &[0xfa]);
    assert_eq!(input.pop_transport().unwrap().message(), &[0xfa]);
}

#[test]
fn merged_scan_waits_for_both_copies_completed_publication() {
    use std::sync::{Arc, Mutex};
    let (mut sink, input, _) = InputSink::new(&InputConfig::default());
    input.boundary().set_transport_sources(&[crate::TransportSource { channel: None, note: false, number: 20 }]);
    let input = Arc::new(Mutex::new(input));
    let delayed = Arc::clone(&input);
    sink.after_publish = Some(Box::new(move || {
        let mut count = 0;
        delayed.lock().unwrap().scan_transport(|_, _| count += 1);
        assert_eq!(count, 0, "a partially published callback must remain queued");
    }));
    sink.push_stamped(0, 0, &[0xb0, 19, 100, 0xb0, 20, 100]);
    let mut seen = Vec::new();
    input.lock().unwrap().scan_transport(|raw, independent| seen.push((raw.bytes[1], independent)));
    assert_eq!(seen, [(19, false), (20, true)]);
}
