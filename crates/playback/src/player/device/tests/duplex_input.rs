//! #682: a duplex open takes its input from the endpoint `Backend::duplex_capture_endpoint` gives.
//! On JACK that is the chosen input's port group; the input's channel count, the I/O table's inputs,
//! the input channel names and the recorder's device name all follow it, and a chosen input that
//! is missing opens no input without failing the output open (ADR 0015, amendment for #431, rule 4). The scripted backend stands in for JACK's
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

/// ADR 0015, amendment for #431, rule 4: a chosen input that is missing opens no input and says so
/// by name, with the output still open; no other group is substituted. Once it is back, the next
/// open (Reconnect) opens it.
#[test]
fn a_chosen_input_that_is_missing_opens_no_input_and_the_output_still_opens() {
    let native = jack("Headset", 2);
    let player = open(&native, Some("Second Mic"));
    assert!(!player.silent && native.running(ab::Direction::Output), "the output open does not fail on the input");
    assert!(native.capture_opens().is_empty(), "no other group was opened");
    let (name, ..) = input_of(&player);
    assert_eq!(name, format!("failed: the audio input did not open with the output ({})", ab::absent_input("Second Mic")));
    assert_eq!(player.io_map().inputs, 0);
    drop(player);
    native.set_input_ports("Second Mic", Some(&["capture_1"]));
    let back = open(&native, Some("Second Mic"));
    assert_eq!(input_of(&back).0, "Second Mic", "back: Reconnect opens it");
}

/// Rule 5: a virtual device's ordered port names reach the recorder (the guard of its paths) and
/// the calibration key; a hardware group carries none.
#[test]
fn a_virtual_input_carries_its_ports_and_a_hardware_group_none() {
    let native = jack("Headset", 2).with_virtual_device("Firefox", &["output_FL", "output_FR"]);
    let player = open(&native, Some("Firefox"));
    let ports = |player: &Player| match &*player.input.borrow() {
        HeldInput::Held(Some(recorder)) => recorder.input_ports.clone(),
        _ => vec!["no input".into()],
    };
    assert_eq!(ports(&player), ["output_FL", "output_FR"]);
    assert_eq!(player.calibration_key.as_ref().map(|k| k.input_ports.clone()), Some(vec!["output_FL".into(), "output_FR".into()]));
    assert_eq!(player.input_channel_names, ["output_FL", "output_FR"]);
    drop(player);
    let mic = open(&native, Some("Mic"));
    assert!(ports(&mic).is_empty() && mic.calibration_key.as_ref().is_some_and(|k| k.input_ports.is_empty()));
    drop(mic);
    let none = open(&native, None);
    assert_eq!(input_of(&none).0, "Headset", "with no choice the input is never a virtual device");
}

/// ADR 0015, amendment for #431, rule 8: the input's identity is the capture endpoint the backend
/// opened. Native WASAPI opens the capture endpoint paired with the render endpoint, resolved at
/// each open: two opens of one output with two different pairings give two identities, and
/// neither is the render endpoint's name. JACK's is the chosen group.
#[test]
fn the_input_identity_is_the_capture_endpoint_that_opened() {
    let wasapi = fake_factory(false).named("WASAPI", "Speakers (Interface)").with_capture(2, true);
    let cfg = AudioConfig::default();
    let open_wasapi = || Player::open_native_kind(Arc::new(Session::default()), &cfg, &wasapi, &fake_factory(false), NativeKind::Wasapi).unwrap();
    wasapi.pair_capture(Some(("{capture-a}", "Microphone (Interface)")));
    let a = open_wasapi();
    assert_eq!(a.held_input_identity().map(|e| (e.id, e.name)), Some(("{capture-a}".into(), "Microphone (Interface)".into())));
    assert_eq!(a.io_map().input_device, "{capture-a}");
    assert_eq!(a.io_map().key(), ("WASAPI", "Speakers (Interface)"), "the output's table is still keyed by the output");
    drop(a);
    wasapi.pair_capture(Some(("{capture-b}", "Line In (Interface)")));
    let b = open_wasapi();
    assert_eq!(b.io_map().input_device, "{capture-b}", "another pairing is another input");
    let jack = jack("Headset", 2);
    let player = open(&jack, Some("Mic"));
    assert_eq!(player.held_input_identity().map(|e| e.id), Some("Mic".into()));
    assert_eq!(player.io_map().input_device, "Mic");
}
