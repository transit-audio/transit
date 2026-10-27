#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn through the real Player on the fake MIDI backend (ADR 0028): ports open for
//! mappings and for a learn, events carry the right epoch and index, nothing opens a device.
use super::*;
use crate::player::MapMsg;
use crate::player::midi_map::MapSpec;
use std::time::{Duration, Instant};
use transit_midi_io::fake::Fake;
use transit_model::{MapMode, SourceKind};

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "did not reach: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn player(fake: &Fake) -> Player {
    let (mut player, _access, _clock) = Player::prepare(Arc::new(Session::default()), 48_000, transit_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    player
}

fn spec(port: &str, number: u8) -> MapSpec {
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
    }
}

#[test]
fn a_mapped_port_opens_without_any_route_and_its_cc_reaches_main() {
    let fake = Fake::default();
    let keys = fake.add_input("Keys");
    let other = fake.add_input("Unmapped pads");
    let player = player(&fake);
    player.set_midi_mappings(vec![spec("Keys", 74)]);
    let epoch = player.midi_map_epoch();
    until("the mapped port opens", || {
        player.service().unwrap();
        player.midi_status().unwrap().open_inputs == 1
    });
    assert!(!fake.input_port(&other).unwrap().deliver(0, &[0xB0, 74, 99]), "the unmapped port is not opened for mappings");
    // The worker takes the mapping a moment after the port opens, and a message that arrived
    // before it is not matched, so the controller keeps moving until the first move comes through.
    let quiet = player.midi_map_activity();
    player.service().unwrap();
    assert_eq!(player.midi_map_activity(), quiet, "no controller traffic, no news");
    let mut got = Vec::new();
    until("the CC reaches Main as a move", || {
        assert!(fake.input_port(&keys).unwrap().deliver(0, &[0xB0, 74, 127]));
        player.service().unwrap();
        got.extend(player.take_midi_map_events());
        !got.is_empty()
    });
    assert!(player.midi_map_activity() > quiet, "a published value is news for the UI");
    assert!(matches!(got[0], MapMsg::Move { epoch: e, index: 0, value, .. } if e == epoch && value == 1.0), "{got:?}");
    // Removing the mappings closes the port again (no route, no monitor, nothing wants it).
    player.set_midi_mappings(Vec::new());
    until("the port closes", || {
        player.service().unwrap();
        player.midi_status().unwrap().open_inputs == 0
    });
}

#[test]
fn an_armed_learn_opens_every_port_and_reports_the_next_message_with_its_port_name() {
    let fake = Fake::default();
    let _keys = fake.add_input("Keys");
    let pads = fake.add_input("Pads");
    let player = player(&fake);
    player.set_midi_learn(true);
    until("both ports open", || {
        player.service().unwrap();
        player.midi_status().unwrap().open_inputs == 2
    });
    assert!(fake.input_port(&pads).unwrap().deliver(0, &[0x92, 36, 100]));
    assert!(fake.input_port(&pads).unwrap().deliver(0, &[0x92, 37, 100]));
    let mut got = Vec::new();
    until("the candidate", || {
        player.service().unwrap();
        got.extend(player.take_midi_map_events());
        !got.is_empty()
    });
    player.service().unwrap();
    got.extend(player.take_midi_map_events());
    assert_eq!(
        got,
        vec![MapMsg::Candidate { port: "Pads".into(), channel: 2, kind: SourceKind::Note, number: 36 }],
        "once, and the second note is not forwarded"
    );
    player.set_midi_learn(false);
    until("ports close when the learn is over", || {
        player.service().unwrap();
        player.midi_status().unwrap().open_inputs == 0
    });
}
