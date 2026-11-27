//! The audio device handover (#760), review round 3: renewed mappings start from the controls'
//! values, a transport press before the worker resumed is neither acted on nor bypasses Learn, and
//! a close the device thread's full queue refuses is kept until it is taken.
use super::*;
use crate::player::MapMsg;
use crate::player::midi_map::MapSpec;
use transit_model::{MapMode, Mapping, MidiSource, SourceKind, TransportControl, TransportPrefs};

fn replacement(session: Session, fake: &Fake, carried: Native) -> Fixture {
    Fixture::on(session, fake.io(), ingress::IDLE, EngineEpoch(EPOCH.0 + 1), |fresh| Native::adopt(fresh, carried))
}

fn pickup(port: &str, number: u8) -> MapSpec {
    MapSpec {
        port: port.into(),
        channel: Some(0),
        kind: SourceKind::Cc,
        number,
        pass_through: false,
        mode: MapMode::Pickup,
        min: 0.0,
        max: 1.0,
        toggle: true,
        button: false,
    }
}

/// Finding A: a pickup mapping on a control at 0.8 does not catch a knob at zero after the reopen
/// (the renewed cell starts from the control's value, not from 0).
#[test]
fn renewed_mappings_start_from_the_controls_values() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    old.native.set_mappings(vec![pickup("Keys", 7)]);
    old.native.sync_map_values(&[(0.8, 0)]);
    old.ready(1, 0);
    until(|| {
        old.tick();
        old.native.map_ports_open() == Some(vec![true])
    });
    let port = fake.input_port(&key).unwrap();
    let carried = old.hand_over();
    let mut new = replacement(session, &fake, carried);
    new.native.set_mappings(vec![pickup("Keys", 7)]);
    new.ready(1, 0);
    until(|| {
        new.tick();
        new.native.map_ports_open() == Some(vec![true])
    });
    assert!(port.deliver(0, &[0xB0, 7, 0]));
    until(|| {
        new.tick();
        new.native.map_heard()[0].count == 1
    });
    let got = new.native.take_map_events();
    assert!(got.is_empty(), "the knob at zero is far from the control at 0.8: {got:?}");
    assert!(port.deliver(0, &[0xB0, 7, 102]));
    let mut got = Vec::new();
    until(|| {
        new.tick();
        got.extend(new.native.take_map_events());
        !got.is_empty()
    });
    assert!(matches!(got[..], [MapMsg::Move { value, .. }] if (value - 102.0 / 127.0).abs() < 1e-6), "{got:?}");
    new.finish();
}

/// Finding B: the boundary is drawn but the worker's queue refuses the resume. A mapped transport
/// press after the cutoff, with transport Learn armed, is not forwarded as a transport action; once
/// the worker resumes, Learn takes it.
#[test]
fn a_transport_press_before_the_worker_resumed_is_not_acted_on_and_learn_takes_it() {
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let session = Session::default();
    let source = MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 20 };
    let profiles: transit_model::TransportProfiles =
        [("Controller".to_string(), TransportPrefs { mappings: vec![Mapping::new(source, TransportControl::Play.target())], ..Default::default() })]
            .into();
    let buttons = |f: &mut Fixture| {
        f.native.configure_buttons(profiles.clone(), Vec::new());
        f.native.sync_buttons();
    };
    let mut old = Fixture::new(session.clone(), fake.io());
    until(|| {
        buttons(&mut old);
        old.tick();
        old.native.status().open_inputs == 1
    });
    let port = fake.input_port(&key).unwrap();
    // The worker holds the button configuration (a press comes through), so nothing more of it is
    // sent after the handover.
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 20, 127]));
        buttons(&mut old);
        old.tick();
        got.extend(old.native.take_buttons().1);
        !got.is_empty()
    });
    assert!(port.deliver(0, &[0xB0, 20, 0]));
    let carried = old.hand_over();
    let mut new = replacement(session, &fake, carried);
    let token = new.native.handovers;
    until(|| new.native.worker.suspend_seen() >= token);
    new.native.worker.stall(true);
    // Room for the lane, the echo producer and the monitor tap; none for the resume. The filler is
    // the button configuration the worker already holds.
    for _ in 0..sources::PORTS - 3 {
        assert!(new.native.worker.set_buttons(new.native.buttons.sent.clone()).is_ok());
    }
    new.native.set_monitor(true, false);
    new.native.set_learn(true);
    new.native.set_transport_learn(true);
    buttons(&mut new);
    new.tick();
    assert!(new.native.unbound.is_some(), "the resume waits behind the full queue");
    new.native.worker.stall(false);
    let seen = new.native.monitor_activity().0.count;
    assert!(port.deliver(0, &[0xB0, 20, 127]), "a mapped Play press after the cutoff");
    until(|| new.native.monitor_activity().0.count > seen);
    let mut pressed = Vec::new();
    let mut learned = Vec::new();
    until(|| {
        buttons(&mut new);
        new.tick();
        pressed.extend(new.native.take_buttons().1);
        learned.extend(new.native.take_map_events());
        new.native.unbound.is_none() && !learned.is_empty()
    });
    pressed.extend(new.native.take_buttons().1);
    assert!(pressed.is_empty(), "forwarded as a transport action: {pressed:?}");
    assert!(matches!(learned[..], [MapMsg::Candidate { number: 20, .. }]), "{learned:?}");
    new.finish();
}

/// Finding C: an open still in flight at the handover, with the device thread's queue full. Since
/// an open in flight is carried (not closed), the carried device thread completes it once the
/// queue drains and the replacement routes the port.
#[test]
fn a_close_the_full_device_queue_refuses_is_kept_until_taken() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    let io = old.native.io.as_ref().unwrap();
    io.stall.store(true, Ordering::Release);
    let stall = Arc::clone(&io.stall);
    until(|| {
        old.tick();
        !old.native.inputs.is_empty()
    });
    // The open is queued and not taken; fill the rest of the device thread's queue.
    let io = old.native.io.as_ref().unwrap();
    while io.request(Request::Close(u64::MAX)).is_ok() {}
    let carried = old.hand_over();
    stall.store(false, Ordering::Release);
    let mut new = replacement(session, &fake, carried);
    // While the device thread works its queue off a request can be refused for a moment (the
    // status says so); only the port open and routed counts.
    until(|| {
        new.tick();
        let status = new.native.status();
        status.open_inputs == 1 && status.routes == 1
    });
    assert!(fake.input_port(&key).unwrap().deliver(10_000, &[0x90, 60, 100]));
    new.packets(1);
    new.finish();
}

/// The refused close (review round 1): an input whose open failed (another program holds the
/// port) and whose close the device thread's full queue refused is kept by the handover, sent again
/// before any open, and no open is made while it waits; once the queue takes it, the replacement
/// opens the port.
#[test]
fn a_failed_inputs_refused_close_is_kept_and_sent_again_before_the_replacement_opens() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let holder = fake.io().open_input(&key, transit_midi_io::InputConfig::default()).unwrap();
    let mut old = Fixture::new(session.clone(), fake.io());
    until(|| {
        old.tick();
        !old.native.inputs.is_empty()
    });
    // The device thread tried the open (refused: the port is held), then takes no more requests;
    // its failure reply is out by the pass that acknowledges the stall.
    // (The holder's own open is the first attempt.)
    until(|| fake.input_open_attempts(&key) == 2);
    let io = old.native.io.as_ref().unwrap();
    let (stall, held) = (Arc::clone(&io.stall), Arc::clone(&io.held));
    stall.store(true, Ordering::Release);
    until(|| held.load(Ordering::Acquire));
    while io.request(Request::Close(u64::MAX)).is_ok() {}
    until(|| {
        old.tick();
        old.native.inputs.iter().any(|input| input.failed)
    });
    let id = old.native.inputs[0].id;
    assert!(!old.native.inputs[0].closed, "the full queue refused the failed input's close");
    let carried = old.hand_over();
    assert_eq!(carried.closing, [id], "the handover keeps the refused close");
    let mut new = replacement(session, &fake, carried);
    for _ in 0..3 {
        new.tick();
        assert_eq!(new.native.closing, [id], "kept while the queue is full");
        assert!(new.native.inputs.is_empty(), "no open while the close waits");
    }
    drop(holder);
    stall.store(false, Ordering::Release);
    until(|| {
        new.tick();
        let status = new.native.status();
        new.native.closing.is_empty() && status.open_inputs == 1 && status.routes == 1
    });
    assert_eq!(fake.input_open_attempts(&key), 3, "the holder's, one refused open, then the replacement's");
    new.finish();
}

/// #760: the same window with Learn not armed. A mapped transport press after the cutoff, while the
/// worker has not resumed yet, is acted on once (nothing would capture it after the resume).
#[test]
fn a_transport_press_before_the_worker_resumed_is_acted_on_when_learn_is_not_armed() {
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let session = Session::default();
    let source = MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 20 };
    let profiles: transit_model::TransportProfiles =
        [("Controller".to_string(), TransportPrefs { mappings: vec![Mapping::new(source, TransportControl::Play.target())], ..Default::default() })]
            .into();
    let buttons = |f: &mut Fixture| {
        f.native.configure_buttons(profiles.clone(), Vec::new());
        f.native.sync_buttons();
    };
    let mut old = Fixture::new(session.clone(), fake.io());
    until(|| {
        buttons(&mut old);
        old.tick();
        old.native.status().open_inputs == 1
    });
    let port = fake.input_port(&key).unwrap();
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 20, 127]));
        buttons(&mut old);
        old.tick();
        got.extend(old.native.take_buttons().1);
        !got.is_empty()
    });
    assert!(port.deliver(0, &[0xB0, 20, 0]));
    let carried = old.hand_over();
    let mut new = replacement(session, &fake, carried);
    let token = new.native.handovers;
    until(|| new.native.worker.suspend_seen() >= token);
    new.native.worker.stall(true);
    for _ in 0..sources::PORTS - 3 {
        assert!(new.native.worker.set_buttons(new.native.buttons.sent.clone()).is_ok());
    }
    new.native.set_monitor(true, false);
    buttons(&mut new);
    new.tick();
    assert!(new.native.unbound.is_some(), "the resume waits behind the full queue");
    new.native.worker.stall(false);
    let seen = new.native.monitor_activity().0.count;
    assert!(port.deliver(0, &[0xB0, 20, 127]), "a mapped Play press after the cutoff");
    until(|| new.native.monitor_activity().0.count > seen);
    let mut pressed = Vec::new();
    until(|| {
        buttons(&mut new);
        new.tick();
        pressed.extend(new.native.take_buttons().1);
        new.native.unbound.is_none() && !pressed.is_empty()
    });
    pressed.extend(new.native.take_buttons().1);
    assert_eq!(pressed.iter().map(|e| (e.raw.bytes[1], e.raw.bytes[2])).collect::<Vec<_>>(), [(20, 127)], "acted on once: {pressed:?}");
    new.finish();
}
