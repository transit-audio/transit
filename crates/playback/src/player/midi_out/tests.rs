#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The MIDI output thread against the fake backend. No hardware MIDI is ever opened.
use super::*;
use transit_midi_io::fake::{Fake, FakeSend};

// A deadline is only a deadlock guard; success always needs the observed state.
fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "MIDI output thread did not reach: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn port_of(key: &PortKey) -> MidiOutPort {
    MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() }
}

fn destination(track: u64, key: &PortKey, channel: MidiOutChannel, offset_ms: i64) -> Destination {
    Destination { track: TrackId(track), port: port_of(key), channel, offset_ns: offset_ms * 1_000_000, echo: true, device: false }
}

fn message(track: u64, at_ns: u64, bytes: [u8; 3]) -> OutEvent {
    OutEvent { kind: Kind::Message, track, at_ns, bytes, len: 3, delay_ns: 0 }
}

struct Rig {
    fake: Fake,
    key: PortKey,
    origin: HostOrigin,
    out: Out,
    playback: Producer,
    echo: Producer,
}

impl Rig {
    fn new(destinations: impl FnOnce(&PortKey) -> Vec<Destination>) -> Self {
        let fake = Fake::default();
        let key = fake.add_output("Synth OUT");
        let origin = HostOrigin::default();
        let (mut out, playback, echo) = Out::new();
        out.start(fake.io(), origin).unwrap();
        out.configure(destinations(&key));
        let rig = Self { fake, key, origin, out, playback, echo };
        until("the destination opens", || rig.out.status().tracks.iter().all(|t| t.state == DestinationState::Connected));
        rig
    }

    fn now(&self) -> u64 {
        self.origin.now_ns().unwrap()
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.fake.sent(&self.key)
    }

    fn wait_sent(&self, count: usize) -> Vec<FakeSend> {
        until(&format!("{count} sends"), || self.fake.sends(&self.key).len() >= count);
        self.fake.sends(&self.key)
    }
}

#[test]
fn events_go_out_in_order_at_their_instant_plus_the_offset_and_never_from_the_audio_thread() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 15)]);
    let base = rig.now() + 20_000_000;
    // The producer runs on a thread marked as the audio thread, as the callback is.
    soundcraft_mix::mark_audio_thread();
    let audio = std::thread::current().id();
    let (pushed, stats) = transit_rt::testing::measure(|| {
        [
            message(1, base, [0x90, 60, 100]),
            message(1, base + 5_000_000, [0x90, 64, 100]),
            message(1, base + 10_000_000, [0x80, 60, 64]),
            message(1, base + 10_000_000, [0x80, 64, 64]),
        ]
        .into_iter()
        .all(|e| rig.playback.push(e))
    });
    soundcraft_mix::unmark_audio_thread();
    assert!(pushed);
    assert!(stats.is_zero(), "pushing allocated: {stats:?}");
    let sends = rig.wait_sent(4);
    assert_eq!(
        sends.iter().map(|s| s.message.clone()).collect::<Vec<_>>(),
        [vec![0x90, 60, 100], vec![0x90, 64, 100], vec![0x80, 60, 64], vec![0x80, 64, 64]]
    );
    let origin = rig.origin.instant();
    for (send, at) in sends.iter().zip([base, base + 5_000_000, base + 10_000_000, base + 10_000_000]) {
        let due = origin + Duration::from_nanos(at + 15_000_000);
        assert!(send.at >= due, "sent {:?} before its instant plus the 15 ms offset", due - send.at);
        // A stall bound, not a latency measurement: send jitter on a shared runner is load-dependent
        // and the exact lateness is not the property (#87, E49). The deterministic checks are the
        // thread's own counters below; a send that waited 250 ms is a stuck output thread.
        assert!(send.at - due < Duration::from_millis(250), "sent {:?} late", send.at - due);
        assert_ne!(send.thread, audio, "a send ran on the audio thread");
        assert_ne!(send.thread, std::thread::current().id());
    }
    let status = rig.out.status();
    assert_eq!(status.audio_thread_sends, 0);
    assert_eq!(status.sent, 4);
    // `late` counts sends over 2 ms past their deadline, which host jitter can trip; it is not
    // asserted. Nothing may be dropped or left unsent.
    assert_eq!((status.dropped, status.unsent), (0, 0), "{status:?}");
}

#[test]
fn stop_drops_scheduled_playback_and_releases_held_notes_and_pedals() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::Channel(3), 0)]);
    assert!(rig.playback.push(message(1, 0, [0x93, 60, 100])));
    assert!(rig.playback.push(message(1, 0, [0xB3, 64, 127])));
    rig.wait_sent(2);
    // A note scheduled a second ahead belongs to audio that Stop discards.
    assert!(rig.playback.push(message(1, rig.now() + 1_000_000_000, [0x93, 72, 100])));
    assert!(rig.playback.push(OutEvent { kind: Kind::Stop, ..OutEvent::default() }));
    // The held note's off and the pedal it held up, then Sustain off and All Notes Off on the
    // channel the port was used on.
    let sends = rig.wait_sent(6);
    let tail: Vec<_> = sends.iter().skip(2).map(|s| s.message.clone()).collect();
    assert_eq!(tail, [vec![0x83, 60, 64], vec![0xB3, 64, 0], vec![0xB3, 64, 0], vec![0xB3, 123, 0]]);
    std::thread::sleep(Duration::from_millis(30));
    assert!(!rig.sent().contains(&vec![0x93, 72, 100]), "a note scheduled past Stop was sent");
}

#[test]
fn a_wrap_ends_held_notes_at_the_wrap_instant_and_drops_discarded_playback() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 0)]);
    // The wrap lies far enough ahead that a loaded host cannot reach it before all four events are
    // queued; a late discard would otherwise let the "discarded" note fall due first (E49, #110).
    let wrap = rig.now() + 300_000_000;
    assert!(rig.playback.push(message(1, 0, [0x90, 60, 100])));
    // Rendered ahead past the wrap, then discarded by a locate: must never sound.
    assert!(rig.playback.push(message(1, wrap + 5_000_000, [0x90, 61, 100])));
    assert!(rig.playback.push(OutEvent { kind: Kind::Release, at_ns: wrap, ..OutEvent::default() }));
    // The next lap restarts the note (the mix side chases it) at the wrap instant.
    assert!(rig.playback.push(message(1, wrap, [0x90, 60, 100])));
    // A later sentinel: once it is sent, everything due before it was either sent or dropped, so
    // the absence check below needs no sleep.
    assert!(rig.playback.push(message(1, wrap + 50_000_000, [0x90, 62, 100])));
    let sends = rig.wait_sent(4);
    assert_eq!(
        sends.iter().map(|s| s.message.clone()).collect::<Vec<_>>(),
        [vec![0x90, 60, 100], vec![0x80, 60, 64], vec![0x90, 60, 100], vec![0x90, 62, 100]]
    );
    let due = rig.origin.instant() + Duration::from_nanos(wrap);
    assert!(sends[1].at >= due, "the release went out before the wrap");
    assert!(sends[1].at <= sends[2].at);
    assert!(!rig.sent().contains(&vec![0x90, 61, 100]), "a discarded note sounded");
}

#[test]
fn a_destination_change_releases_on_the_old_port_only_on_the_tracks_channels() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::Channel(0), 0), destination(2, key, MidiOutChannel::Channel(1), 0)]);
    let other = rig.fake.add_output("Other OUT");
    assert!(rig.playback.push(message(1, 0, [0x90, 60, 100])));
    assert!(rig.playback.push(message(2, 0, [0x91, 62, 100])));
    rig.wait_sent(2);
    rig.out.configure(vec![destination(1, &other, MidiOutChannel::Channel(0), 0), destination(2, &rig.key, MidiOutChannel::Channel(1), 0)]);
    let sends = rig.wait_sent(5);
    assert_eq!(sends.iter().skip(2).map(|s| s.message.clone()).collect::<Vec<_>>(), [vec![0x80, 60, 64], vec![0xB0, 64, 0], vec![0xB0, 123, 0]]);
    until("the new port opens", || rig.out.status().tracks.iter().all(|t| t.state == DestinationState::Connected));
    assert!(rig.playback.push(message(1, 0, [0x90, 67, 100])));
    until("the new port receives", || rig.fake.sent(&other).contains(&vec![0x90, 67, 100]));
    assert!(!rig.sent().iter().any(|m| m[0] & 0x0F == 1 && m[0] & 0xF0 != 0x90), "track 2 on the old port kept its note");
}

#[test]
fn device_loss_marks_the_destination_missing_and_the_returning_port_reconnects() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 0)]);
    assert!(rig.playback.push(message(1, 0, [0x90, 60, 100])));
    rig.wait_sent(1);
    let (name, id) = (rig.key.name.clone(), rig.key.id.clone());
    rig.fake.remove_output(&rig.key);
    until("the destination is missing", || rig.out.status().track(TrackId(1)) == Some(&DestinationState::Missing));
    assert!(rig.playback.push(message(1, 0, [0x90, 62, 100])));
    until("the event is counted unsent", || rig.out.status().unsent >= 1);
    let key = rig.fake.add_output_with_id(&name, &id);
    until("the destination reconnects", || rig.out.status().track(TrackId(1)) == Some(&DestinationState::Connected));
    // A reconnect starts clean on the channels the port was used on.
    until("the reconnect resets", || rig.fake.sent(&key).len() >= 2);
    assert_eq!(rig.fake.sent(&key), [vec![0xB0, 64, 0], vec![0xB0, 123, 0]]);
    assert!(rig.playback.push(message(1, 0, [0x90, 64, 100])));
    until("the reconnected port receives", || rig.fake.sent(&key).contains(&vec![0x90, 64, 100]));
}

#[test]
fn a_port_absent_at_load_is_missing_and_sends_nothing_until_it_appears() {
    let fake = Fake::default();
    let origin = HostOrigin::default();
    let (mut out, mut playback, _echo) = Out::new();
    out.start(fake.io(), origin).unwrap();
    let absent = MidiOutPort { backend: "fake".into(), name: "Synth OUT".into(), ordinal: 0, id: "usb-synth".into() };
    let foreign = MidiOutPort { backend: "coremidi".into(), name: "Synth OUT".into(), ordinal: 0, id: String::new() };
    out.configure(vec![
        Destination { track: TrackId(1), port: absent, channel: MidiOutChannel::AsRecorded, offset_ns: 0, echo: true, device: false },
        Destination { track: TrackId(2), port: foreign, channel: MidiOutChannel::AsRecorded, offset_ns: 0, echo: true, device: false },
    ]);
    until("both are missing", || {
        let s = out.status();
        s.track(TrackId(1)) == Some(&DestinationState::Missing) && s.track(TrackId(2)) == Some(&DestinationState::Missing)
    });
    assert!(playback.push(message(1, 0, [0x90, 60, 100])));
    until("unsent", || out.status().unsent >= 1);
    let key = fake.add_output_with_id("Synth OUT", "usb-synth");
    until("it opens when it appears", || out.status().track(TrackId(1)) == Some(&DestinationState::Connected));
    assert_eq!(out.status().track(TrackId(2)), Some(&DestinationState::Missing), "a port of another backend never opens");
    assert!(playback.push(message(1, 0, [0x90, 61, 100])));
    until("sent", || fake.sent(&key).contains(&vec![0x90, 61, 100]));
    assert!(!fake.sent(&key).contains(&vec![0x90, 60, 100]));
    assert!(out.status().ports.iter().any(|p| p.key == key), "the picker lists the output");
}

#[test]
fn echo_goes_out_at_once_on_the_destination_channel_with_bounded_latency() {
    let mut rig = Rig::new(|key| vec![destination(5, key, MidiOutChannel::Channel(9), 250)]);
    let pushed = Instant::now();
    // Echo ignores the offset: live playing is never delayed for alignment.
    assert!(rig.echo.push(message(5, rig.now(), [0x90, 60, 100])));
    let sends = rig.wait_sent(1);
    assert_eq!(sends[0].message, vec![0x99, 60, 100]);
    let latency = sends[0].at - pushed;
    // Protects "echo ignores the 250 ms offset", not a latency figure: a delayed echo cannot pass,
    // host jitter can (#87, E49).
    assert!(latency < Duration::from_millis(200), "echo took {latency:?}");
    // Stop ends notes that live input holds too.
    assert!(rig.playback.push(OutEvent { kind: Kind::Stop, ..OutEvent::default() }));
    let sends = rig.wait_sent(4);
    assert_eq!(sends.iter().skip(1).map(|s| s.message.clone()).collect::<Vec<_>>(), [vec![0x89, 60, 64], vec![0xB9, 64, 0], vec![0xB9, 123, 0]]);
}

#[test]
fn exit_releases_every_held_note_and_the_panic_releases_now() {
    let fake = Fake::default();
    let key = fake.add_output("Synth OUT");
    let (mut out, mut playback, _echo) = Out::new();
    out.start(fake.io(), HostOrigin::default()).unwrap();
    out.configure(vec![destination(1, &key, MidiOutChannel::AsRecorded, 0)]);
    assert!(playback.push(message(1, 0, [0x92, 60, 100])));
    until("sent", || fake.sent(&key).len() == 1);
    out.panic();
    until("panic", || fake.sent(&key).len() == 4);
    assert_eq!(fake.sent(&key)[1..], [vec![0x82, 60, 64], vec![0xB2, 64, 0], vec![0xB2, 123, 0]]);
    assert!(playback.push(message(1, 0, [0x92, 62, 100])));
    until("sent", || fake.sent(&key).len() == 5);
    drop(out);
    assert_eq!(fake.sent(&key)[5..], [vec![0x82, 62, 64], vec![0xB2, 64, 0], vec![0xB2, 123, 0]]);
}

#[test]
fn a_full_ring_keeps_room_for_releases() {
    let (out, mut playback, _echo) = Out::new();
    let mut accepted = 0;
    while playback.push(message(1, 0, [0x90, 60, 100])) {
        accepted += 1;
    }
    assert_eq!(accepted, QUEUE - RELEASE_RESERVE);
    assert!(playback.push(message(1, 0, [0x80, 60, 0])));
    assert!(playback.push(OutEvent { kind: Kind::Stop, ..OutEvent::default() }));
    assert_eq!(out.status().dropped, 1);
    assert_eq!(out.status().dropped_releases, 0);
}

#[test]
fn disarming_ends_what_the_echo_holds_and_a_late_echo_cannot_start_a_note() {
    // Refutation B1: the key-up and pedal-up of an echo cannot arrive after the route goes.
    let mut rig = Rig::new(|key| vec![destination(3, key, MidiOutChannel::AsRecorded, 0)]);
    assert!(rig.echo.push(message(3, 0, [0x90, 60, 100])));
    assert!(rig.echo.push(message(3, 0, [0xB0, 64, 127])));
    rig.wait_sent(2);
    let disarmed = |key: &PortKey| vec![Destination { echo: false, ..destination(3, key, MidiOutChannel::AsRecorded, 0) }];
    rig.out.configure(disarmed(&rig.key));
    let sends = rig.wait_sent(4);
    assert_eq!(sends.iter().skip(2).map(|s| s.message.clone()).collect::<Vec<_>>(), [vec![0x80, 60, 64], vec![0xB0, 64, 0]]);
    // An echo still in flight from the revoked route may end things but not start them.
    assert!(rig.echo.push(message(3, 0, [0x90, 62, 100])));
    assert!(rig.echo.push(message(3, 0, [0x80, 60, 0])));
    rig.wait_sent(5);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rig.sent().last(), Some(&vec![0x80, 60, 0]));
    assert!(!rig.sent().contains(&vec![0x90, 62, 100]));
}

#[test]
fn a_revoked_echo_route_releases_its_notes_and_pedal_even_when_the_destination_is_unchanged() {
    // Input change, channel filter change or the controller unplugged: still armed.
    let mut rig = Rig::new(|key| vec![destination(4, key, MidiOutChannel::Channel(2), 0)]);
    assert!(rig.echo.push(message(4, 0, [0x90, 60, 100])));
    assert!(rig.echo.push(message(4, 0, [0xB0, 64, 127])));
    rig.out.release_echo(TrackId(4));
    // The note-on pushed before the release goes out before it, never after.
    let sends = rig.wait_sent(4);
    assert_eq!(
        sends.iter().map(|s| s.message.clone()).collect::<Vec<_>>(),
        [vec![0x92, 60, 100], vec![0xB2, 64, 127], vec![0x82, 60, 64], vec![0xB2, 64, 0]]
    );
}

#[test]
fn an_external_instrument_tracks_destination_has_no_offset() {
    use soundcraft_model::{ChannelFormat, Insert, MidiOutput, Session, TrackKind};
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let track = s.track_mut(t).unwrap();
    // A hand-edited file with an offset beside the device: the device's latency is the one knob.
    track.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::AsRecorded, offset_ms: 12.0 };
    assert_eq!(Destination::of_session(&s)[0].offset_ns, 12_000_000);
    s.track_mut(t).unwrap().instrument = Some(Insert::external("In 1"));
    assert_eq!(Destination::of_session(&s)[0].offset_ns, 0, "ADR 0020: the offset is held at 0");
}

/// B2 of the ADR 0020 slice 1 review: the plan latency A drops mid-note (a latent insert
/// removed, a manual latency lowered, another External Instrument's port lost). The note-on was
/// stamped P(R) + A_old, the note-off P(R + len) + A_new; for len < A_old - A_new the off would
/// fall due first and the synth would hold the note. A track's deadlines never go backwards, so
/// the off waits for its on and follows it.
#[test]
fn a_note_off_stamped_before_its_note_on_after_a_drops_still_follows_it() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 0)]);
    let base = rig.now() + 20_000_000;
    let a_old = 22_000_000u64; // C = 1061 frames at 48 kHz, about 22 ms
    let len = 5_000_000u64;
    assert!(rig.playback.push(message(1, base + a_old, [0x90, 60, 100])));
    assert!(rig.playback.push(message(1, base + len, [0x80, 60, 64]))); // A_new = 0
    let sends = rig.wait_sent(2);
    let order: Vec<Vec<u8>> = sends.iter().map(|s| s.message.clone()).collect();
    assert_eq!(order, [vec![0x90, 60, 100], vec![0x80, 60, 64]], "the off overtook its on: the note would hang");
    let due = rig.origin.instant() + Duration::from_nanos(base + a_old);
    assert!(sends[1].at >= due, "the off went out before its on's instant");
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(rig.sent().len(), 2);
}

/// ADR 0020 decision 2: a bare MIDI-out destination's notes carry the mix's delay A, an External
/// Instrument's do not, so a wrap releases each on its own clock. The device's next lap, which
/// starts before the bare track's release instant, keeps its note.
#[test]
fn a_wrap_releases_a_device_track_without_the_mixs_delay_and_a_bare_track_with_it() {
    let mut rig = Rig::new(|key| {
        vec![destination(1, key, MidiOutChannel::AsRecorded, 0), Destination { device: true, ..destination(2, key, MidiOutChannel::AsRecorded, 0) }]
    });
    let wrap = rig.now() + 20_000_000;
    let delay = 30_000_000u64;
    assert!(rig.playback.push(message(1, 0, [0x90, 60, 100])));
    assert!(rig.playback.push(message(2, 0, [0x91, 62, 100])));
    rig.wait_sent(2);
    assert!(rig.playback.push(OutEvent { kind: Kind::Release, at_ns: wrap, delay_ns: delay, ..OutEvent::default() }));
    // The device's next lap restarts its note at the wrap; the bare track's at the wrap plus A.
    assert!(rig.playback.push(message(2, wrap, [0x91, 62, 100])));
    assert!(rig.playback.push(message(1, wrap + delay, [0x90, 60, 100])));
    let sends = rig.wait_sent(6);
    assert_eq!(
        sends.iter().map(|s| s.message.clone()).collect::<Vec<_>>(),
        [vec![0x90, 60, 100], vec![0x91, 62, 100], vec![0x81, 62, 64], vec![0x91, 62, 100], vec![0x80, 60, 64], vec![0x90, 60, 100]]
    );
    let origin = rig.origin.instant();
    assert!(sends[2].at >= origin + Duration::from_nanos(wrap));
    assert!(sends[4].at >= origin + Duration::from_nanos(wrap + delay), "the bare track ended before its notes do");
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(rig.sent().len(), 6, "the device's restarted note was released with the bare track");
}

/// A locate backwards while A has dropped: the release (at P(R) + A_new) drops the old lap's
/// pending note and resets the track's deadline clamp to the release instant, so the next lap
/// goes out on time and is not held to the old lap's deadline.
#[test]
fn a_locate_with_a_lower_a_resets_the_deadline_clamp_so_the_next_lap_is_on_time() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 0)]);
    let base = rig.now() + 30_000_000;
    assert!(rig.playback.push(message(1, 0, [0x90, 50, 100])));
    rig.wait_sent(1);
    assert!(rig.playback.push(message(1, base + 200_000_000, [0x90, 60, 100]))); // the old lap, A_old large
    assert!(rig.playback.push(OutEvent { kind: Kind::Release, at_ns: base, delay_ns: 0, ..OutEvent::default() }));
    assert!(rig.playback.push(message(1, base + 1_000_000, [0x90, 62, 100])));
    let sends = rig.wait_sent(3);
    let order: Vec<Vec<u8>> = sends.iter().map(|s| s.message.clone()).collect();
    assert_eq!(order, [vec![0x90, 50, 100], vec![0x80, 50, 64], vec![0x90, 62, 100]]);
    let late = sends[2].at.saturating_duration_since(rig.origin.instant() + Duration::from_nanos(base + 1_000_000));
    // A lap held to the old deadline is ~200 ms late; host jitter is far below this (#87, E49).
    assert!(late < Duration::from_millis(100), "the next lap was held to the old lap's deadline: {late:?}");
    std::thread::sleep(Duration::from_millis(250));
    assert!(!rig.sent().contains(&vec![0x90, 60, 100]), "the discarded old-lap note went out");
}

/// One bogus far-future stamp does not stall the track: the clamp holds an event at most
/// `CLAMP_CAP_NS` past now.
#[test]
fn the_deadline_clamp_is_capped_so_a_far_future_stamp_cannot_stall_a_track() {
    let mut rig = Rig::new(|key| vec![destination(1, key, MidiOutChannel::AsRecorded, 0)]);
    let now = rig.now();
    assert!(rig.playback.push(message(1, now + 60_000_000_000, [0x90, 60, 100])));
    assert!(rig.playback.push(message(1, now + 5_000_000, [0x90, 62, 100])));
    let sends = rig.wait_sent(1);
    assert_eq!(sends[0].message, vec![0x90, 62, 100]);
    let late = sends[0].at.saturating_duration_since(rig.origin.instant() + Duration::from_nanos(now + 5_000_000));
    // Already a stall bound (#87, E49): the margin over the cap is far above host jitter.
    assert!(late <= Duration::from_nanos(CLAMP_CAP_NS) + Duration::from_millis(100), "held past the cap: {late:?}");
}

fn stall(rig: &Rig) {
    rig.out.shared.stall.store(true, Ordering::Release);
    until("the thread is held", || rig.out.shared.stalled.load(Ordering::Acquire));
}

fn unstall(rig: &Rig) {
    rig.out.shared.stall.store(false, Ordering::Release);
}

/// Fills the 64-slot control queue with harmless releases of a track nobody has.
fn fill_control_queue(rig: &Rig) {
    while rig.out.control.try_send(Control::ReleaseEcho { track: TrackId(99), generation: 1 }).is_ok() {}
}

#[test]
fn a_stale_echo_note_on_after_the_release_is_dropped_and_no_note_hangs() {
    // #69 item 2: the ingress worker checks the route, is preempted, Main revokes it (disarm) and
    // the release goes out, then the push lands. The key-up never comes: without the generation
    // the note would hang.
    let mut rig = Rig::new(|key| vec![destination(6, key, MidiOutChannel::AsRecorded, 0)]);
    assert!(rig.echo.push(message(6, 0, [0x90, 60, 100])));
    rig.wait_sent(1);
    let checked = rig.echo.echo_generation(); // the route check of the late note-on
    rig.out.release_echo(TrackId(6)); // Main revokes the route
    let sends = rig.wait_sent(2);
    assert_eq!(sends[1].message, vec![0x80, 60, 64]);
    // The preempted push lands after the release.
    assert!(rig.echo.push(OutEvent { delay_ns: checked, ..message(6, 0, [0x90, 62, 100]) }));
    until("the stale event is dropped", || rig.out.status().stale_echo == 1);
    // An event whose route check came after the revocation still goes out.
    let fresh = rig.echo.echo_generation();
    assert!(fresh > checked);
    assert!(rig.echo.push(OutEvent { delay_ns: fresh, ..message(6, 0, [0x90, 64, 100]) }));
    rig.wait_sent(3);
    // Nothing of the stale note: the later panic ends only the fresh one.
    rig.out.panic();
    let sent = rig.wait_sent(5);
    assert!(!sent.iter().any(|s| s.message == vec![0x90, 62, 100] || s.message == vec![0x80, 62, 64]));
    assert!(sent.iter().any(|s| s.message == vec![0x80, 64, 64]));
}

#[test]
fn a_release_survives_a_full_control_queue_and_runs_when_the_thread_drains() {
    // #69 item 3: the thread is stuck in a driver; the release must not be skipped.
    let mut rig = Rig::new(|key| vec![destination(5, key, MidiOutChannel::AsRecorded, 0)]);
    assert!(rig.echo.push(message(5, 0, [0x90, 60, 100])));
    assert!(rig.echo.push(message(5, 0, [0xB0, 64, 127])));
    rig.wait_sent(2);
    stall(&rig);
    fill_control_queue(&rig);
    rig.out.release_echo(TrackId(5)); // queue full: pending, not skipped
    assert_eq!(rig.sent().len(), 2, "the stuck thread has sent nothing");
    unstall(&rig);
    let sends = rig.wait_sent(4);
    assert_eq!(sends[2].message, vec![0x80, 60, 64]);
    assert_eq!(sends[3].message, vec![0xB0, 64, 0]);
}

#[test]
fn a_track_release_survives_a_full_control_queue_and_runs_when_the_thread_drains() {
    // N2 of the slice review: a Measure's release after a device loss is never skipped.
    let mut rig = Rig::new(|key| vec![destination(5, key, MidiOutChannel::Channel(3), 0)]);
    assert!(rig.echo.push(message(5, 0, [0x90, 60, 100])));
    rig.wait_sent(1);
    stall(&rig);
    fill_control_queue(&rig);
    rig.out.release_track(TrackId(5)); // queue full: pending, not skipped
    rig.out.release_track(TrackId(5)); // one entry per track
    assert_eq!(rig.out.shared.track_releases.lock().unwrap().len(), 1);
    assert_eq!(rig.sent().len(), 1, "the stuck thread has sent nothing");
    unstall(&rig);
    let sends = rig.wait_sent(3);
    let sent: Vec<_> = sends.iter().map(|s| s.message.clone()).collect();
    assert!(sent.contains(&vec![0x83, 60, 64]), "{sent:?}");
    assert!(sent.contains(&vec![0xB3, 123, 0]), "{sent:?}");
}

#[test]
fn a_panic_survives_a_full_control_queue_and_runs_when_the_thread_drains() {
    let mut rig = Rig::new(|key| vec![destination(5, key, MidiOutChannel::Channel(3), 0)]);
    assert!(rig.echo.push(message(5, 0, [0x90, 60, 100])));
    rig.wait_sent(1);
    stall(&rig);
    fill_control_queue(&rig);
    rig.out.panic();
    unstall(&rig);
    let sends = rig.wait_sent(4);
    let sent: Vec<_> = sends.iter().map(|s| s.message.clone()).collect();
    assert!(sent.contains(&vec![0x83, 60, 64]), "{sent:?}");
    assert!(sent.contains(&vec![0xB3, 123, 0]), "{sent:?}");
}

#[test]
fn pending_releases_keep_the_newest_generation_per_track_without_growing() {
    let rig = Rig::new(|key| vec![destination(5, key, MidiOutChannel::AsRecorded, 0)]);
    stall(&rig);
    fill_control_queue(&rig);
    let capacity = rig.out.shared.releases.lock().unwrap().capacity();
    for _ in 0..1000 {
        rig.out.release_echo(TrackId(5));
        rig.out.panic();
    }
    let pending = rig.out.shared.releases.lock().unwrap().clone();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(rig.out.shared.releases.lock().unwrap().capacity(), capacity);
    unstall(&rig);
}
