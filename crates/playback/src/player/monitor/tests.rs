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
