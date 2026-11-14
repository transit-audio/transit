//! #682: the duplex input follows the chosen input device. The one client links `in_N` to the
//! capture ports of the port group the input wish names, not to the output's group; a missing or
//! absent choice falls back to the output's group, then to the first group that records.

use crate::JackBackend;
use crate::fake::FakeServer;
use std::sync::Arc;
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend};

/// PipeWire with three nodes: a mono microphone (listed first, so "the first group that records"
/// and "the output's own group" differ), a headset that plays and records (stereo), and an HDMI
/// output that only plays.
fn pipewire() -> FakeServer {
    FakeServer::running(
        48_000,
        256,
        &[
            ("Mic:capture_MONO", true),
            ("Headset:playback_FL", false),
            ("Headset:playback_FR", false),
            ("Headset:capture_FL", true),
            ("Headset:capture_FR", true),
            ("HDMI:playback_FL", false),
            ("HDMI:playback_FR", false),
        ],
    )
}

fn backend(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(Duration::from_millis(2), Duration::from_millis(5), Duration::from_secs(2))
}

/// What a duplex open on `output` with the input wish `input` linked: the capture group's name,
/// the input channel count, and every connection made to transit's inputs (source, destination).
struct Opened {
    group: String,
    channels: u16,
    linked: Vec<(String, String)>,
}

/// The Player's duplex order over the seam: resolve the input, open capture on it, then render on
/// the output, then start both.
fn open(server: &FakeServer, output: &str, input: Option<&str>) -> Opened {
    let backend = backend(server);
    let plan = backend.plan_render(&ab::OutputWish { device: Some(output.into()), ..Default::default() }).expect("render plan");
    assert!(backend.capture_with_render(&plan.endpoint), "one client serves both directions");
    let wish = ab::InputWish { device: input.map(str::to_owned), ..Default::default() };
    let capture_on = backend.duplex_capture_endpoint(&plan.endpoint, &wish).expect("an input to open");
    let format = backend.capture_format(&capture_on, None).expect("capture format");
    let request = ab::OpenRequest { format, ..plan.attempts[0] };
    let (capture, _) = backend.open_capture(&capture_on, &request, Box::new(|_| {}), Box::new(|_| {})).expect("capture");
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {})).expect("render");
    render.start().unwrap();
    capture.start().unwrap();
    let linked = server.clients(|c| c.last().unwrap().connections.iter().filter(|(_, to)| to.starts_with("transit:in_")).cloned().collect());
    assert_eq!(server.open_clients(), 1, "both directions on one client");
    drop((capture, render));
    Opened { group: capture_on.name, channels: format.channels, linked }
}

fn pair(source: &str, destination: &str) -> (String, String) {
    (source.to_owned(), destination.to_owned())
}

#[test]
fn a_chosen_input_on_another_group_is_linked_and_the_outputs_capture_ports_are_not() {
    let opened = open(&pipewire(), "Headset", Some("Mic"));
    assert_eq!(opened.group, "Mic");
    assert_eq!(opened.channels, 1, "the channel count is the chosen group's");
    assert_eq!(opened.linked, [pair("Mic:capture_MONO", "transit:in_1")]);
    assert!(!opened.linked.iter().any(|(from, _)| from.starts_with("Headset:")), "none of the output group's capture ports: {:?}", opened.linked);
}

#[test]
fn an_output_group_without_capture_ports_opens_the_chosen_input() {
    let opened = open(&pipewire(), "HDMI", Some("Mic"));
    assert_eq!((opened.group.as_str(), opened.channels), ("Mic", 1));
    assert_eq!(opened.linked, [pair("Mic:capture_MONO", "transit:in_1")]);
}

#[test]
fn no_chosen_input_keeps_the_outputs_group_and_without_one_takes_the_first_that_records() {
    let opened = open(&pipewire(), "Headset", None);
    assert_eq!((opened.group.as_str(), opened.channels), ("Headset", 2), "as before #682: the output's own group");
    assert_eq!(opened.linked, [pair("Headset:capture_FL", "transit:in_1"), pair("Headset:capture_FR", "transit:in_2")]);
    let hdmi = open(&pipewire(), "HDMI", None);
    assert_eq!((hdmi.group.as_str(), hdmi.channels), ("Mic", 1), "the first group that records, as a plan with no device chosen");
}

#[test]
fn a_chosen_input_that_does_not_exist_falls_back_and_the_endpoint_names_the_group_opened() {
    let opened = open(&pipewire(), "Headset", Some("Second Mic"));
    assert_eq!((opened.group.as_str(), opened.channels), ("Headset", 2), "the output's group, named so the app can say so");
    let hdmi = open(&pipewire(), "HDMI", Some("Second Mic"));
    assert_eq!(hdmi.group, "Mic", "an output without capture ports falls back to the first group that records");
    // A group that only plays is not an input either.
    assert_eq!(open(&pipewire(), "Headset", Some("HDMI")).group, "Headset");
}

#[test]
fn with_no_group_that_records_there_is_no_input_to_open() {
    let server = FakeServer::running(48_000, 256, &[("HDMI:playback_FL", false), ("HDMI:playback_FR", false)]);
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let wish = ab::InputWish { device: Some("Mic".into()), ..Default::default() };
    let error = backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap_err();
    assert!(error.contains("no port group has Input ports"), "{error}");
}
