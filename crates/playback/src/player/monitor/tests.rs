#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;

fn note(n: u8) -> MonitorEvent {
    MonitorEvent::message(Dir::In, 1, u64::from(n), &[0x90, n, 100])
}

#[test]
fn ring_push_drain_and_drop_counting() {
    let (hub, mut tap_in, mut tap_out) = Hub::new();
    for i in 0..RING {
        tap_in.push(note((i % 100) as u8));
    }
    assert_eq!(hub.dropped(), (0, 0));
    tap_in.push(note(1));
    tap_in.push(note(2));
    tap_out.push(MonitorEvent::message(Dir::Out, 0, 5, &[0x80, 60, 0]));
    assert_eq!(hub.dropped(), (2, 0), "a full ring drops and counts per direction");
    let mut out = Vec::new();
    hub.drain(&mut out, 10);
    assert_eq!(out.len(), 11, "max per ring");
    hub.drain(&mut out, RING * 2);
    assert_eq!(out.len(), RING + 1, "the rest, none of the dropped");
    let mut again = Vec::new();
    hub.drain(&mut again, 10);
    assert!(again.is_empty());
    tap_in.push(note(3));
    assert_eq!(hub.dropped(), (2, 0), "space is free again");
}

#[test]
fn element_layout() {
    assert_eq!(std::mem::size_of::<MonitorEvent>(), 24);
    let e = MonitorEvent::message(Dir::In, 7, 9, &[0xB3, 64, 127]);
    assert_eq!((e.channel, e.status, e.data1, e.data2, e.port, e.host_ns), (3, 0xB3, 64, 127, 7, 9));
    assert_eq!(MonitorEvent::message(Dir::In, 0, 0, &[0xF8]).channel, NO_CHANNEL);
    assert_eq!(MonitorEvent::sysex(Dir::In, 0, 0, 300).sysex_len, 300);
}

fn pulses(count: u64, interval_ns: u64, jitter: impl Fn(u64) -> i64) -> (ClockRate, u64) {
    let mut rate = ClockRate::default();
    let mut t = 1_000_000;
    let mut last = t;
    for i in 0..count {
        last = (t as i64 + jitter(i)) as u64;
        rate.pulse(last);
        t += interval_ns;
    }
    (rate, last)
}

#[test]
fn clock_rate_120_bpm() {
    // 120 BPM at 24 PPQN is 48 pulses a second.
    let (rate, last) = pulses(100, 1_000_000_000 / 48, |_| 0);
    let bpm = rate.bpm(last).unwrap();
    assert!((bpm - 120.0).abs() < 0.1, "{bpm}");
}

#[test]
fn clock_rate_smooths_jitter() {
    let (rate, last) = pulses(400, 1_000_000_000 / 48, |i| if i % 2 == 0 { 800_000 } else { -800_000 });
    let bpm = rate.bpm(last).unwrap();
    assert!((bpm - 120.0).abs() < 3.0, "{bpm}");
}

#[test]
fn clock_rate_stops_after_one_second() {
    let (rate, last) = pulses(50, 1_000_000_000 / 48, |_| 0);
    assert!(rate.bpm(last + CLOCK_TIMEOUT_NS).is_some());
    assert!(rate.bpm(last + CLOCK_TIMEOUT_NS + 1).is_none());
    assert!(ClockRate::default().bpm(0).is_none());
    let mut one = ClockRate::default();
    one.pulse(5);
    assert!(one.bpm(6).is_none(), "one pulse is not a rate");
}

#[test]
fn clock_rate_restarts_after_a_gap() {
    let mut rate = ClockRate::default();
    for i in 0..20 {
        rate.pulse(i * 20_833_333);
    }
    // 60 BPM after a long pause.
    let base = 10_000_000_000;
    let mut last = base;
    for i in 0..60 {
        last = base + i * 41_666_666;
        rate.pulse(last);
    }
    let bpm = rate.bpm(last).unwrap();
    assert!((bpm - 60.0).abs() < 0.2, "{bpm}");
}

#[test]
fn slots_publish_and_expire() {
    let (hub, tap, _out) = Hub::new();
    let shared = tap.shared();
    let slot = shared.claim(5).unwrap();
    assert_ne!(shared.claim(6).unwrap(), slot, "claims take different slots");
    shared.publish_rate(slot, Some((120.0, 1_000)));
    shared.publish_sensing(slot, 1_000);
    assert_eq!(hub.input_rate(5, 2_000), (Some(120.0), true));
    assert_eq!(hub.input_rate(5, 1_000 + CLOCK_TIMEOUT_NS + 1).0, None);
    assert!(!hub.input_rate(5, 1_000 + SENSING_TIMEOUT_NS + 1).1);
    shared.release(slot);
    assert_eq!(hub.input_rate(5, 2_000), (None, false));
}

#[test]
fn sysex_prefixes_follow_bounded_drains_without_allocating_on_producers() {
    let (hub, mut input, mut output) = Hub::new();
    let bytes = [0x31; 100];
    let ((), allocations) = transit_rt::testing::measure(|| {
        input.push_sysex(7, 10, 100, &bytes);
        input.push_sysex(7, 11, 2, &[0xF0, 0xF7]);
        output.push_sysex(1, 12, 3, &[0xF0, 1, 0xF7]);
    });
    assert!(allocations.is_zero(), "{allocations:?}");
    let mut events = Vec::new();
    hub.drain(&mut events, 1);
    let captured = hub.take_sysex();
    assert_eq!(events.len(), 2);
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].len, 64);
    assert_eq!(captured[0].bytes, [0x31; 64]);
    assert_eq!(events[0].sysex_len, 100);
    assert_eq!((captured[0].dir, captured[0].id), (events[0].dir, events[0].sysex_id));
    assert_eq!(captured[1].dir, Dir::Out);
    assert!(hub.take_sysex().is_empty());
    events.clear();
    hub.drain(&mut events, 1);
    let captured = hub.take_sysex();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].len, 2);
    assert_eq!(&captured[0].bytes[..2], &[0xF0, 0xF7]);
    assert_eq!(captured[0].id, events[0].sysex_id);
    assert_ne!(captured[0].id, 1);
}

#[test]
fn mixed_notes_and_sysex_keep_every_payload_with_its_event_under_a_bounded_drain() {
    let (hub, mut input, _output) = Hub::new();
    // Pattern: note, note, sysex, note, sysex, sysex, note ... so a bounded drain splits runs.
    let mut expected = Vec::new();
    for i in 0..30u8 {
        let host = u64::from(i) * 10;
        if matches!(i % 7, 2 | 4 | 5) {
            input.push_sysex(1, host, 3, &[0xF0, i, 0xF7]);
            expected.push(i);
        } else {
            input.push(MonitorEvent::message(Dir::In, 1, host, &[0x90, 60, 100]));
        }
    }
    let mut seen = Vec::new();
    let mut total_events = 0;
    loop {
        let mut events = Vec::new();
        hub.drain(&mut events, 3);
        if events.is_empty() {
            break;
        }
        total_events += events.len();
        let payloads = hub.take_sysex();
        let with_id: Vec<_> = events.iter().filter(|e| e.sysex_id != 0).collect();
        assert_eq!(with_id.len(), payloads.len(), "no payload lost or taken by a non-SysEx event");
        for (event, payload) in with_id.iter().zip(&payloads) {
            assert_eq!(event.sysex_id, payload.id);
            assert_eq!((payload.bytes[0], payload.len), (0xF0, 3));
            assert_eq!(u64::from(payload.bytes[1]) * 10, event.host_ns, "payload belongs to its event");
            seen.push(payload.bytes[1]);
        }
    }
    assert_eq!(total_events, 30);
    assert_eq!(seen, expected);
}

#[test]
fn sysex_full_ring_drops_both_halves_and_recovers_without_payload_mismatch() {
    let (hub, mut input, _output) = Hub::new();
    for _ in 0..RING {
        input.push(note(1));
    }
    input.push_sysex(1, 1, 3, &[0xF0, 5, 0xF7]);
    assert_eq!(hub.dropped(), (1, 0));
    let mut events = Vec::new();
    hub.drain(&mut events, RING);
    assert!(hub.take_sysex().is_empty());
    for i in 0..RING {
        input.push_sysex(1, i as u64, 3, &[0xF0, (i % 128) as u8, 0xF7]);
    }
    input.push_sysex(1, 1, 3, &[0xF0, 5, 0xF7]);
    assert_eq!(hub.dropped(), (2, 0));
    events.clear();
    hub.drain(&mut events, RING);
    let captured = hub.take_sysex();
    assert_eq!(captured.len(), RING);
    for (i, (event, payload)) in events.iter().zip(captured.iter()).enumerate() {
        assert_eq!(event.sysex_id, payload.id);
        assert_eq!(payload.bytes[1], (i % 128) as u8);
    }
    input.push_sysex(1, 2, 2, &[0xF0, 0xF7]);
    events.clear();
    hub.drain(&mut events, 1);
    assert_eq!(hub.take_sysex()[0].id, events[0].sysex_id);
}
