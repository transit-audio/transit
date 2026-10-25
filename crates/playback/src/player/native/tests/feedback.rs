//! Feedback protection (#99) through the native MIDI layer and the output thread, on the fake
//! backend: a controller whose MIDI thru sends every message transit echoes to it straight back.
use super::*;
use crate::player::midi_out::DestinationState;
use soundcraft_model::{MidiOutChannel, MidiOutPort, MidiOutput};
use std::sync::atomic::AtomicBool;

fn port_of(key: &PortKey) -> MidiOutPort {
    MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() }
}

/// An armed MIDI track (no instrument) listening to `input` and sending to `output`.
fn armed(input: &PortKey, output: &PortKey, allow: bool) -> (Session, TrackId) {
    let mut session = Session::default();
    let t = session.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let track = session.track_mut(t).unwrap();
    track.mixer.record_arm = true;
    track.midi_input = selected(input, None);
    track.midi_output = MidiOutput::External { port: port_of(output), channel: MidiOutChannel::AsRecorded, offset_ms: 0.0 };
    track.allow_echo_to_device = allow;
    (session, t)
}

impl Fixture {
    /// One round of Main's work with the audio side kept consuming: packets taken (credits renew)
    /// and revoked sources acknowledged.
    fn pump(&mut self) {
        self.tick();
        self.midi.service(&mut action_lane::callback_scope(), &mut self.ports.plans, &mut |_, _| ActionResult::MidiBuffered);
        self.release_sources();
    }

    fn connected(&mut self, track: TrackId) {
        until(|| {
            self.pump();
            let status = self.native.status();
            status.routes == 1 && status.output.track(track) == Some(&DestinationState::Connected)
        });
    }
}

/// The device's MIDI thru: everything sent to its output comes back on its input, as the
/// KeyStep Pro in #99 did.
struct Thru {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Thru {
    fn start(fake: &Fake, output: &PortKey, input: &PortKey) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (fake, output, input, flag) = (fake.clone(), output.clone(), input.clone(), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            let mut forwarded = 0;
            while !flag.load(Ordering::Acquire) {
                let sent = fake.sent(&output);
                if let Some(port) = fake.input_port(&input) {
                    for message in sent.iter().skip(forwarded) {
                        port.deliver(0, message);
                    }
                }
                forwarded = sent.len();
                std::thread::yield_now();
            }
        });
        Self { stop, thread: Some(thread) }
    }
}

impl Drop for Thru {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn count(fake: &Fake, key: &PortKey, message: &[u8]) -> usize {
    fake.sent(key).iter().filter(|m| m.as_slice() == message).count()
}

/// The repro: echo allowed to the controller's own port and the detector off. One key press comes
/// back through the thru and goes out again, without end.
#[test]
fn a_looped_device_with_echo_allowed_and_no_detector_storms() {
    let fake = Fake::default();
    let input = fake.add_input("KeyStep Pro");
    let output = fake.add_output("KeyStep Pro");
    let (session, t) = armed(&input, &output, true);
    let mut f = Fixture::new(session, fake.io());
    f.native.disable_storm_detector();
    f.connected(t);
    let thru = Thru::start(&fake, &output, &input);
    assert!(fake.input_port(&input).unwrap().deliver(0, &[0x90, 60, 100]));
    // One press, hundreds of sends: the loop formed.
    until(|| {
        f.pump();
        count(&fake, &output, &[0x90, 60, 100]) >= 300
    });
    assert!(f.native.status().muted_inputs.is_empty());
    drop(thru);
    f.finish();
}

/// The default: the controller's own port is the same device, so its input is not echoed there.
/// Recording still gets the input (the route stays, capture only).
#[test]
fn by_default_input_is_not_echoed_to_its_own_device_so_no_loop_forms() {
    let fake = Fake::default();
    let input = fake.add_input("KeyStep Pro");
    let output = fake.add_output("KeyStep Pro");
    let (session, t) = armed(&input, &output, false);
    let mut f = Fixture::new(session, fake.io());
    // The guard alone, without the detector behind it.
    f.native.disable_storm_detector();
    f.connected(t);
    let route = f.native.inputs[0].routes[0].route;
    assert!(!route.echo && !route.deliver, "the route captures for a recording and does not echo: {route:?}");
    assert_eq!(f.native.status().self_echo, vec![(t, false)], "the UI is told why");
    let thru = Thru::start(&fake, &output, &input);
    assert!(fake.input_port(&input).unwrap().deliver(0, &[0x90, 60, 100]));
    let packets = f.packets(1);
    assert_eq!(packets[0].message, soundcraft_mix::Message::NoteOn { key: 60, velocity: 100, id: soundcraft_mix::NoteId::NONE });
    // timing: sleep-ok negative check, an echo would go out within the output thread's 1 ms poll
    std::thread::sleep(Duration::from_millis(50));
    f.pump();
    assert_eq!(fake.sent(&output), Vec::<Vec<u8>>::new(), "nothing is echoed to the controller's own port");
    drop(thru);
    f.finish();
}

/// Belt and braces: echo allowed, detector on. The port mutes, the note the echo held is released
/// on the output, the flag is up for the UI, and the port stays muted until re-enabled.
#[test]
fn with_echo_allowed_the_storm_detector_mutes_the_port_and_releases_its_notes() {
    let fake = Fake::default();
    let input = fake.add_input("KeyStep Pro");
    let output = fake.add_output("KeyStep Pro");
    let (session, t) = armed(&input, &output, true);
    let mut f = Fixture::new(session, fake.io());
    f.connected(t);
    assert!(f.native.inputs[0].routes[0].route.echo, "allowed: the route echoes");
    assert_eq!(f.native.status().self_echo, vec![(t, true)]);
    let thru = Thru::start(&fake, &output, &input);
    assert!(fake.input_port(&input).unwrap().deliver(0, &[0x90, 60, 100, 0xB0, 64, 127]));
    until(|| {
        f.pump();
        !f.native.status().muted_inputs.is_empty()
    });
    let status = f.native.status();
    assert_eq!(status.counts.storms, 1);
    assert_eq!((status.muted_inputs[0].name.as_str(), status.muted_inputs[0].tracks.as_slice()), ("KeyStep Pro", [t].as_slice()));
    // The route is revoked, so the echo's held note and pedal are released on the port.
    until(|| {
        f.pump();
        let sent = fake.sent(&output);
        sent.contains(&vec![0x80, 60, 64]) && sent.contains(&vec![0xB0, 64, 0])
    });
    assert_eq!(f.native.status().routes, 0);
    let looped = count(&fake, &output, &[0x90, 60, 100]);
    // The bound is the detector's 200 ms at the loop's rate (one pass per output poll); this is a
    // stall bound on that, the windows themselves are proven on stamped times in `storm.rs`.
    assert!(looped < 2_000, "{looped} looped note-ons before the mute");
    // The release came back through the thru: the muted port admits nothing.
    // timing: sleep-ok negative check, the muted port must not echo the returning release
    std::thread::sleep(Duration::from_millis(50));
    f.pump();
    assert_eq!(count(&fake, &output, &[0x90, 60, 100]), looped);
    assert_eq!(count(&fake, &output, &[0x80, 60, 64]), 1, "the release is not echoed back");
    // Re-enabled: routed again, with a fresh count.
    f.native.status().muted_inputs[0].reenable();
    until(|| {
        f.pump();
        let status = f.native.status();
        status.muted_inputs.is_empty() && status.routes == 1
    });
    drop(thru);
    f.finish();
}

/// A sequencer at 1,000 messages a second for 1.5 s (arrival stamps 1 ms apart) to another
/// device: every message is echoed and the port is never muted.
#[test]
fn a_dense_sequencer_stream_is_echoed_and_never_muted() {
    let fake = Fake::default();
    let input = fake.add_input("Sequencer");
    let output = fake.add_output("Synth");
    let (session, t) = armed(&input, &output, false);
    let mut f = Fixture::new(session, fake.io());
    f.connected(t);
    assert!(f.native.inputs[0].routes[0].route.echo);
    assert!(f.native.status().self_echo.is_empty(), "another device");
    let port = fake.input_port(&input).unwrap();
    let start = transit_midi_io::host_now_ns();
    let total = 1_500u64;
    // In chunks the audio side's note budget absorbs (the stamps, not the delivery, set the rate).
    const CHUNK: u64 = 20;
    for chunk in 0..total / CHUNK {
        for i in chunk * CHUNK..(chunk + 1) * CHUNK {
            let key = 36 + (i / 2 % 48) as u8;
            let channel = (i / 96 % 16) as u8;
            let message = if i.is_multiple_of(2) { [0x90 | channel, key, 100] } else { [0x80 | channel, key, 0] };
            assert!(port.deliver_at(i * 1_000, start + i * 1_000_000, &message));
        }
        until(|| {
            f.pump();
            f.native.status().counts.offered == (chunk + 1) * CHUNK && fake.sent(&output).len() as u64 == (chunk + 1) * CHUNK
        });
    }
    assert_eq!(fake.sent(&output).len(), total as usize);
    let status = f.native.status();
    assert_eq!(status.counts.shed, 0);
    assert_eq!((status.counts.storms, status.muted_inputs.len(), status.routes), (0, 0, 1));
    f.finish();
}

/// Channel-mode messages (CC 120 to 127) in are never echoed, and an incoming All Notes Off does
/// not make transit send its own releases (note-offs, Sustain off, All Notes Off) to the port.
#[test]
fn channel_mode_messages_are_never_echoed_and_all_notes_off_in_sends_no_release() {
    let fake = Fake::default();
    let input = fake.add_input("Keys");
    let output = fake.add_output("Synth");
    let (session, t) = armed(&input, &output, false);
    let mut f = Fixture::new(session, fake.io());
    f.connected(t);
    let mut bytes = vec![0x90, 60, 100];
    for cc in 120u8..=127 {
        bytes.extend([0xB0, cc, if cc == 122 { 127 } else { 0 }]);
    }
    // A plain controller last: once its echo is out, every earlier message was decided.
    bytes.extend([0xB0, 7, 100]);
    assert!(fake.input_port(&input).unwrap().deliver(0, &bytes));
    until(|| {
        f.pump();
        fake.sent(&output).contains(&vec![0xB0, 7, 100])
    });
    assert_eq!(fake.sent(&output), vec![vec![0x90, 60, 100], vec![0xB0, 7, 100]]);
    f.finish();
}
