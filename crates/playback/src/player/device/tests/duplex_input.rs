//! #682: a duplex open takes its input from the endpoint `Backend::duplex_capture_endpoint` gives.
//! On JACK that is the chosen input's port group; the input's channel count, the I/O table's inputs,
//! the input channel names and the recorder's device name all follow it, and a chosen input that
//! is missing falls back without failing the output open. The scripted backend stands in for JACK's
//! one client (`with_capture_device`); the JACK backend's own choice is tested over its fake server.

use super::*;
use crate::backend::native::NativeKind;

/// A JACK-like duplex system: the output group `output` with `own` inputs, and a mono "Mic" group.
fn jack(output: &str, own: u16) -> ab::fake::FakeBackend {
    fake_factory(false).named("JACK", output).with_capture(own, true).with_capture_device("Mic", 1)
}

/// What the open's input is: (recorder device name, its channels, the I/O table's inputs, the
/// input channel names).
fn input_of(player: &Player) -> (String, usize, u16, Vec<String>) {
    let (name, channels) = match &*player.input.borrow() {
        HeldInput::Held(Some(recorder)) => (recorder.device_name.clone(), recorder.device_channels()),
        HeldInput::Held(None) => ("taken".into(), 0),
        HeldInput::No => ("none".into(), 0),
        HeldInput::Failed(why) => (format!("failed: {why}"), 0),
    };
    (name, channels, player.io_map().inputs, player.input_channel_names.clone())
}

fn open(native: &ab::fake::FakeBackend, input: Option<&str>) -> Player {
    let cfg = AudioConfig { input: input.map(str::to_owned), ..Default::default() };
    Player::open_native_kind(Arc::new(Session::default()), &cfg, native, &fake_factory(false), NativeKind::Jack).expect("the output opens")
}

#[test]
fn a_chosen_input_on_another_group_is_the_one_opened_with_its_channel_count() {
    let native = jack("Headset", 2);
    let player = open(&native, Some("Mic"));
    assert_eq!(native.capture_opens(), ["Mic"], "the input opened on the chosen group, not the output's");
    assert_eq!(input_of(&player), ("Mic".into(), 1, 1, vec!["Mic:capture_1".into()]));
    assert_eq!(player.io_map().key(), ("JACK", "Headset"), "the table stays keyed by the output");
    assert!(native.running(ab::Direction::Input) && native.running(ab::Direction::Output));
}

#[test]
fn an_output_without_inputs_opens_the_chosen_input() {
    let native = jack("HDMI", 0);
    let player = open(&native, Some("Mic"));
    assert_eq!(native.capture_opens(), ["Mic"]);
    assert_eq!(input_of(&player).0, "Mic");
    assert!(!player.silent && native.running(ab::Direction::Input));
}

#[test]
fn no_chosen_input_keeps_the_outputs_own_inputs() {
    let native = jack("Headset", 2);
    let player = open(&native, None);
    assert_eq!(native.capture_opens(), ["Headset"]);
    assert_eq!(input_of(&player).0, "Headset");
    assert_eq!((input_of(&player).1, input_of(&player).2), (2, 2));
}

#[test]
fn a_chosen_input_that_is_missing_falls_back_and_the_output_still_opens() {
    let native = jack("Headset", 2);
    let player = open(&native, Some("Second Mic"));
    assert!(!player.silent && native.running(ab::Direction::Output), "the output open does not fail on the input");
    assert_eq!(native.capture_opens(), ["Headset"]);
    assert_eq!(input_of(&player).0, "Headset", "the recorder names the device that opened, so the app can say the choice is missing");
}
