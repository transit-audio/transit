//! What the replacement's first service holds to after an audio device reopen (#760, review round
//! 2): one boundary for notes, transport buttons, MIDI Learn and mapped controls; a bind that a full
//! worker queue delays is kept and sent again; failed inputs are tried again.
use super::*;
use crate::player::MapMsg;
use crate::player::midi_map::MapSpec;
use transit_model::{MapMode, SourceKind};

fn reopened() -> EngineEpoch {
    EngineEpoch(EPOCH.0 + 1)
}

/// The replacement Player of `carried`, not serviced yet.
fn replacement(session: Session, fake: &Fake, carried: Native) -> Fixture {
    Fixture::on(session, fake.io(), ingress::IDLE, reopened(), |fresh| Native::adopt(fresh, carried))
}

/// The button configuration the interface hands the Player every frame.
fn buttons(f: &mut Fixture) {
    f.native.configure_buttons(Default::default(), Vec::new());
    f.native.sync_buttons();
}

fn cc(port: &str, number: u8) -> MapSpec {
    MapSpec {
        port: port.into(),
        channel: Some(0),
        kind: SourceKind::Cc,
        number,
        pass_through: false,
        mode: MapMode::Jump,
        min: 0.0,
        max: 1.0,
        toggle: true,
        button: false,
        target: None,
    }
}

/// Finding 1: the first service meets a full worker update queue. The bind is kept and sent again,
/// and no route of the new engine is active before it went through.
#[test]
fn a_full_update_queue_at_the_first_service_delays_the_bind_and_loses_nothing() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    let t = track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let carried = old.hand_over();
    carried.worker.stall(true);
    while carried.worker.set_buttons(Vec::new()).is_ok() {}
    let mut new = replacement(session, &fake, carried);
    for _ in 0..3 {
        new.tick();
        assert_eq!(new.native.status().routes, 0, "no route of the new engine before its bind went through");
    }
    new.native.worker.stall(false);
    new.ready(1, 1);
    assert!(fake.input_port(&key).unwrap().deliver(10_000, &[0x90, 60, 100]));
    let packets = new.packets(1);
    assert_eq!((packets[0].engine, packets[0].target.0), (reopened(), t));
    new.finish();
}

/// Finding 2: buttons share the boundary of notes, the replacement's first service. A button offered
/// in the gap, or after the adoption and before that service, is not admitted.
#[test]
fn a_button_offered_before_the_replacements_first_service_is_not_admitted() {
    let fake = Fake::default();
    let key = fake.add_input("Controller");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    until(|| {
        buttons(&mut old);
        old.tick();
        old.native.status().open_inputs == 1
    });
    let port = fake.input_port(&key).unwrap();
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xfa]));
        buttons(&mut old);
        old.tick();
        got.extend(old.native.take_buttons().1);
        !got.is_empty()
    });
    let carried = old.hand_over();
    assert!(port.deliver(0, &[0xfa]), "Start in the gap");
    let mut new = replacement(session, &fake, carried);
    assert!(port.deliver(0, &[0xfb]), "Continue after the adoption, before the first service");
    let mut got = Vec::new();
    let mut sent = false;
    until(|| {
        buttons(&mut new);
        new.tick();
        if !sent && new.native.unbound.is_none() {
            assert!(port.deliver(0, &[0xfc]), "Stop after the boundary");
            sent = true;
        }
        got.extend(new.native.take_buttons().1);
        sent && !got.is_empty()
    });
    assert_eq!(got.iter().map(|e| e.raw.bytes[0]).collect::<Vec<_>>(), [0xfc], "only what followed the boundary");
    new.finish();
}

/// Finding 3: an armed Learn stays armed across the reopen and takes the first message after the
/// boundary, not one from the gap.
#[test]
fn an_armed_learn_takes_the_first_message_after_the_boundary_not_one_from_the_gap() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    old.native.set_learn(true);
    old.ready(1, 0);
    let port = fake.input_port(&key).unwrap();
    let carried = old.hand_over();
    assert!(port.deliver(0, &[0xB0, 74, 1]), "a CC in the gap");
    let mut new = replacement(session, &fake, carried);
    new.ready(1, 0);
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 75, 1]));
        new.tick();
        got.extend(new.native.take_map_events());
        !got.is_empty()
    });
    assert!(matches!(got[0], MapMsg::Candidate { number: 75, .. }), "{got:?}");
    new.finish();
}

/// Finding 3: a mapped control moved in the gap has no effect after the reopen.
#[test]
fn a_mapped_control_moved_in_the_gap_has_no_effect() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    old.native.set_mappings(vec![cc("Keys", 7)]);
    old.ready(1, 0);
    let port = fake.input_port(&key).unwrap();
    // The worker matches only once it holds the mapping: a move gets through before the reopen.
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 7, 64]));
        old.tick();
        got.extend(old.native.take_map_events());
        !got.is_empty()
    });
    let carried = old.hand_over();
    assert!(port.deliver(0, &[0xB0, 7, 127]), "a move in the gap");
    let mut new = replacement(session, &fake, carried);
    new.native.set_mappings(vec![cc("Keys", 7)]);
    new.ready(1, 0);
    let mut got = Vec::new();
    until(|| {
        assert!(port.deliver(0, &[0xB0, 7, 0]));
        new.tick();
        got.extend(new.native.take_map_events());
        !got.is_empty()
    });
    assert!(got.iter().all(|m| matches!(m, MapMsg::Move { value, .. } if *value == 0.0)), "{got:?}");
    new.finish();
}

/// Finding 4: an input another program held when it was opened is tried again by the reopen.
#[test]
fn a_reopen_tries_a_failed_input_again() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let held = fake.io().open_input(&key, transit_midi_io::InputConfig::default()).unwrap();
    let mut old = Fixture::new(session.clone(), fake.io());
    until(|| {
        old.tick();
        old.native.inputs.iter().any(|input| input.failed)
    });
    let carried = old.hand_over();
    drop(held);
    let mut new = replacement(session, &fake, carried);
    new.ready(1, 1);
    new.finish();
}

/// Two reopens before the first replacement was serviced: the second takes the inputs on.
#[test]
fn two_reopens_before_the_first_service_keep_the_input_open() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    let t = track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let middle = replacement(session.clone(), &fake, old.hand_over());
    let last = EngineEpoch(EPOCH.0 + 2);
    let mut new = Fixture::on(session, fake.io(), ingress::IDLE, last, |fresh| Native::adopt(fresh, middle.hand_over()));
    new.ready(1, 1);
    assert!(fake.input_port(&key).unwrap().deliver(10_000, &[0x90, 60, 100]));
    let packets = new.packets(1);
    assert_eq!((packets[0].engine, packets[0].target.0), (last, t));
    assert_eq!(fake.input_opens(&key), 1);
    new.finish();
}

/// A port plugged in and one pulled out while no runtime was open.
#[test]
fn hot_plug_in_the_gap_is_followed() {
    let fake = Fake::default();
    let keys = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let carried = old.hand_over();
    let pads = fake.add_input("Pads");
    fake.remove_input(&keys);
    let mut new = replacement(session, &fake, carried);
    until(|| {
        new.tick();
        let status = new.native.status();
        status.open_input_ports.len() == 1 && same_port(&status.open_input_ports[0], &pads) && status.routes == 1
    });
    assert_eq!(fake.input_opens(&pads), 1);
    assert!(fake.input_port(&pads).unwrap().deliver(10_000, &[0x90, 64, 100]));
    new.packets(1);
    new.finish();
}

/// The carried device thread ended before the adoption: the replacement opens the input itself.
#[test]
fn carried_threads_that_ended_leave_the_replacement_to_open_the_input() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let mut session = Session::default();
    track(&mut session, MidiInput::default());
    let mut old = Fixture::new(session.clone(), fake.io());
    old.ready(1, 1);
    let carried = old.hand_over();
    let io = carried.io.as_ref().unwrap();
    io.stop.store(true, Ordering::Release);
    io.thread.thread().unpark();
    let finished = Arc::clone(&io.finished);
    until(|| finished.load(Ordering::Acquire));
    let mut new = replacement(session, &fake, carried);
    new.ready(1, 1);
    assert_eq!(fake.input_opens(&key), 2, "the replacement's own open");
    new.finish();
}

/// Review round 1 (#760): the old worker never matched (it took the mapping only once suspended),
/// so its matcher counts as off. A mapped control moved after the replacement's boundary and before
/// the worker resumed is still matched when it resumes: the renewed mapping starts at the boundary,
/// so it sees what follows it whatever the matcher did before.
#[test]
fn a_mapped_control_after_the_boundary_is_matched_when_the_matcher_was_off_before_the_reopen() {
    let fake = Fake::default();
    let key = fake.add_input("Keys");
    let session = Session::default();
    let mut old = Fixture::new(session.clone(), fake.io());
    // The monitor opens the port with no route and no mapping.
    old.native.set_monitor(true, false);
    old.ready(1, 0);
    old.native.worker.stall(true);
    old.native.set_mappings(vec![cc("Keys", 7)]);
    old.tick();
    let port = fake.input_port(&key).unwrap();
    let carried = old.hand_over();
    carried.worker.stall(false);
    let mut new = replacement(session, &fake, carried);
    let token = new.native.handovers;
    until(|| new.native.worker.suspend_seen() >= token);
    // Room for the lane, the echo producer and the monitor tap; none for the resume.
    new.native.worker.stall(true);
    for _ in 0..sources::PORTS - 3 {
        assert!(new.native.worker.set_buttons(Vec::new()).is_ok());
    }
    new.native.set_mappings(vec![cc("Keys", 7)]);
    new.tick();
    assert!(new.native.unbound.is_some() && !new.native.before_boundary(), "the boundary is drawn, the resume waits");
    assert!(port.deliver(0, &[0xB0, 7, 127]), "a move after the boundary, before the resume");
    new.native.worker.stall(false);
    let mut got = Vec::new();
    until(|| {
        new.tick();
        got.extend(new.native.take_map_events());
        new.native.worker.resumed() >= token
    });
    // Resumed: the move is published by the step that applied the resume, or not at all.
    until(|| {
        new.tick();
        got.extend(new.native.take_map_events());
        !got.is_empty()
    });
    assert!(matches!(got[..], [MapMsg::Move { value, .. }] if (value - 1.0).abs() < 1e-6), "{got:?}");
    new.finish();
}
