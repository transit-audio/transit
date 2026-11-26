//! The MIDI inputs outlive an audio device reopen (#760), through the native MIDI layer on the fake
//! backend: the Player being replaced hands its MIDI side over, its runtime is retired, and the
//! replacement adopts it before its first service.
use super::*;

impl Fixture {
    /// The reopen's first half: the MIDI side handed over, then this runtime retired (the old
    /// Player drops after the handover).
    pub(super) fn hand_over(self) -> Native {
        let Self { mut native, owner, ports, midi, .. } = self;
        native.hand_off();
        drop(midi);
        owner.retire(ports);
        native
    }
}

/// The open input is neither closed nor opened again: the same open's sequence continues on the new
/// engine, and a note that arrived while no runtime was open is cancelled (counted), not played.
#[test]
fn an_audio_reopen_keeps_the_open_input_and_routes_it_to_the_new_engine() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    let t = track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let backend = fake.input_port(&key).unwrap();
    assert!(backend.deliver(10_000, &[0x90, 60, 100]));
    let packets = old.packets(1);
    assert_eq!((packets[0].engine, packets[0].raw.seq), (EPOCH, 0));
    let carried = old.hand_over();
    assert!(backend.deliver(20_000, &[0x80, 60, 0]), "the port is still open between the two runtimes");
    let reopened = EngineEpoch(EPOCH.0 + 1);
    let mut new = Fixture::on(session, fake.io(), ingress::IDLE, reopened, |fresh| Native::adopt(fresh, carried));
    new.ready(1, 1);
    assert!(backend.deliver(30_000, &[0x90, 62, 100]));
    let packets = new.packets(1);
    assert_eq!((packets[0].engine, packets[0].target.0, packets[0].raw.seq), (reopened, t, 2), "{packets:?}");
    assert_eq!(fake.input_opens(&key), 1, "the reopen closed and reopened the input");
    assert_eq!(new.native.status().counts.cancelled, 1, "the note from between the runtimes is cancelled and counted");
    new.finish();
}

/// Ports plugged in and pulled out after the handover are followed by the carried device thread.
#[test]
fn hot_plug_keeps_working_after_an_audio_reopen() {
    let fake = Fake::default();
    let keys = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let carried = old.hand_over();
    let mut new = Fixture::on(session, fake.io(), ingress::IDLE, EngineEpoch(EPOCH.0 + 1), |fresh| Native::adopt(fresh, carried));
    new.ready(1, 1);
    let pads = fake.add_input("Pads");
    new.ready(2, 2);
    assert_eq!((fake.input_opens(&keys), fake.input_opens(&pads)), (1, 1));
    fake.remove_input(&keys);
    new.ready(1, 1);
    assert!(fake.input_port(&pads).unwrap().deliver(10_000, &[0x90, 64, 100]));
    let packets = new.packets(1);
    assert_eq!(packets[0].raw.seq, 0, "the port plugged in after the reopen has its own open");
    new.finish();
}

/// A replacement that runs no MIDI of its own (not enabled, as for the silent diagnostic device)
/// takes nothing over: the carried inputs close, as every reopen closed them before.
#[test]
fn a_replacement_without_midi_closes_the_carried_inputs() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let carried = old.hand_over();
    let finished = Arc::clone(&carried.io.as_ref().unwrap().finished);
    let target = RenderTarget { device: DeviceGeneration(EPOCH.0 + 1), sample_rate: 48_000, max_block: 512, out_channels: 0, in_channels: 0 };
    let (owner, ports) = Owner::new(Arc::new(session), None, target, PlanLimits::default()).unwrap();
    let (lane, midi) = owner.midi_lane().unwrap();
    let silent = Native::new_with_idle(lane, HostOrigin::default(), ingress::IDLE).unwrap();
    let installed = Native::adopt(silent, carried);
    assert!(installed.io.is_none() && installed.inputs.is_empty(), "the replacement keeps its own (empty) MIDI side");
    until(|| finished.load(Ordering::Acquire));
    assert!(!fake.input_port(&key).unwrap().deliver(10_000, &[0x90, 60, 100]), "the carried input is closed");
    drop(installed);
    drop(midi);
    owner.retire(ports);
}

/// Starts `old`'s open of `key` and holds it in flight (the device thread takes no request), hands
/// the MIDI side over, then lets the device thread complete the open while no runtime is open.
fn hand_over_in_flight(mut old: Fixture, fake: &Fake, key: &PortKey) -> Native {
    let stall = Arc::clone(&old.native.io.as_ref().unwrap().stall);
    stall.store(true, Ordering::Release);
    until(|| {
        old.tick();
        !old.native.inputs.is_empty()
    });
    assert!(old.native.inputs[0].attached.is_none(), "the open is in flight");
    let carried = old.hand_over();
    stall.store(false, Ordering::Release);
    until(|| fake.input_opens(key) == 1);
    carried
}

/// An open still in flight at the handover is completed by the carried device thread: the port is
/// opened once, not closed and opened again, and a note it received before the replacement took it
/// over is cancelled (counted), not played.
#[test]
fn an_open_in_flight_at_the_handover_is_opened_once_and_its_gap_note_does_not_play() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    let t = track(&mut session, MidiInput::default());
    let old = Fixture::new(session.clone(), fake.io());
    let carried = hand_over_in_flight(old, &fake, &key);
    let port = fake.input_port(&key).unwrap();
    assert!(port.deliver(10_000, &[0x90, 60, 100]), "the open in flight stays open across the handover");
    let reopened = EngineEpoch(EPOCH.0 + 1);
    let mut new = Fixture::on(session, fake.io(), ingress::IDLE, reopened, |fresh| Native::adopt(fresh, carried));
    new.ready(1, 1);
    assert!(port.deliver(20_000, &[0x90, 62, 100]));
    let packets = new.packets(1);
    assert_eq!((packets[0].engine, packets[0].target.0, packets[0].raw.bytes[1]), (reopened, t, 62), "{packets:?}");
    assert_eq!(fake.input_opens(&key), 1, "the handover closed and reopened the input");
    assert_eq!(new.native.status().counts.cancelled, 1, "the note from the gap is cancelled and counted");
    new.finish();
}

/// The same open with MIDI Learn armed: a message it received in the gap is not the candidate; the
/// first message after the replacement took it over is.
#[test]
fn an_open_in_flight_at_the_handover_gives_learn_nothing_from_the_gap() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    old.native.set_learn(true);
    let carried = hand_over_in_flight(old, &fake, &key);
    let port = fake.input_port(&key).unwrap();
    assert!(port.deliver(0, &[0xB0, 74, 1]), "a CC in the gap");
    let mut new = Fixture::on(session, fake.io(), ingress::IDLE, EngineEpoch(EPOCH.0 + 1), |fresh| Native::adopt(fresh, carried));
    new.ready(1, 0);
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 75, 1]));
        new.tick();
        got.extend(new.native.take_map_events());
        !got.is_empty()
    });
    assert!(matches!(got[0], crate::player::MapMsg::Candidate { number: 75, .. }), "{got:?}");
    new.finish();
}
