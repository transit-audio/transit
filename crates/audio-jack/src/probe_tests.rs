//! ADR 0021, amendment for #760, slice 3 (rule 4), over the fake server: one probe client per
//! open, resolved against that open's own listing; enumeration through the session's client while
//! it is active and from the open's listing while it is not (no probe, no rate getter); the
//! listing's ports validated against the session client's view before use.

use crate::JackBackend;
use crate::devices::ServerState;
use crate::fake::FakeServer;
use std::sync::Arc;
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend, Stream};

const HOUR: Duration = Duration::from_secs(3600);

fn backend(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(HOUR, HOUR, HOUR)
}

/// A desktop with a stereo interface, a mono microphone and a browser's stream.
fn desktop() -> FakeServer {
    let server = FakeServer::running(
        48_000,
        256,
        &[
            ("Scarlett:capture_AUX0", true),
            ("Scarlett:capture_AUX1", true),
            ("Scarlett:playback_AUX0", false),
            ("Scarlett:playback_AUX1", false),
            ("USB Mic:capture_MONO", true),
        ],
    );
    server.add_port("Firefox:output_FL", true);
    server.add_port("Firefox:output_FR", true);
    server
}

fn probes(server: &FakeServer) -> usize {
    server.clients(|c| c.iter().filter(|c| c.name.starts_with(crate::PROBE_NAME)).count())
}

struct Open {
    capture: Option<Box<dyn Stream>>,
    render: Box<dyn Stream>,
}

/// The Player's duplex open of the output "Scarlett" with the input `input`: plan, input resolved,
/// capture opened (when it resolved), render opened. Not started.
fn open(backend: &JackBackend, input: Option<&str>) -> Result<Open, ab::OpenError> {
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Scarlett".into()), ..Default::default() }).map_err(ab::OpenError::Other)?;
    let wish = ab::InputWish { device: input.map(str::to_owned), ..Default::default() };
    let capture = match backend.duplex_capture_endpoint(&plan.endpoint, &wish) {
        Ok(on) => {
            let request = ab::OpenRequest { format: backend.capture_format(&on, None).map_err(ab::OpenError::Other)?, ..plan.attempts[0] };
            Some(backend.open_capture(&on, &request, Box::new(|_| {}), Box::new(|_| {}))?.0)
        }
        Err(_) => None,
    };
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {}))?;
    Ok(Open { capture, render })
}

#[test]
fn one_open_probes_once_whatever_the_input() {
    for input in [Some("USB Mic"), Some("Firefox"), Some("Gone"), None] {
        let server = desktop();
        let backend = backend(&server);
        let opened = open(&backend, input).unwrap();
        assert_eq!(probes(&server), 1, "one probe for the open with input {input:?}");
        assert_eq!(opened.capture.is_some(), input != Some("Gone"), "an absent input opens none ({input:?})");
        // Apply: the device reopens with its choices (the old streams dropped first): one more.
        drop(opened);
        let _again = open(&backend, input).unwrap();
        assert_eq!(probes(&server), 2, "one probe per open ({input:?})");
    }
}

#[test]
fn enumeration_with_a_session_opens_no_probe_and_reads_no_rate_from_an_inactive_client() {
    let server = FakeServer::pipewire(48_000, 1024, (48_000, 1024));
    let backend = backend(&server);
    let opened = open(&backend, None).unwrap();
    assert_eq!(probes(&server), 1);
    let session = server.clients(|c| c.iter().position(|c| c.name == crate::CLIENT_NAME)).unwrap();
    // Not active yet: the open's own listing answers, with no rate getter on the session client.
    let ServerState::Running(listed) = backend.probe() else { panic!("not running") };
    assert!(listed.devices.iter().any(|d| d.name == "Scarlett"));
    assert_eq!((probes(&server), server.inactive_reads(session)), (1, 0));
    // Active: the session's client lists (a rescan with the device open).
    opened.capture.as_ref().unwrap().start().unwrap();
    opened.render.start().unwrap();
    server.add_port("Firefox:output_FL", true);
    let outputs = backend.endpoints(ab::Direction::Output).unwrap();
    assert_eq!(outputs.len(), 1);
    let ServerState::Running(listed) = backend.probe() else { panic!("not running") };
    assert_eq!(listed.virtual_devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Firefox"], "read now, through the session's client");
    assert!(!listed.virtual_devices.iter().any(|d| d.name.starts_with(crate::CLIENT_NAME)), "transit's own client is no input");
    assert_eq!(probes(&server), 1, "no probe while the session runs");
    drop(opened);
    let _ = backend.probe();
    assert_eq!(probes(&server), 2, "with no session, enumeration probes again");
}

#[test]
fn a_group_that_went_away_or_changed_between_the_listing_and_the_open_fails_visibly() {
    // The microphone goes away after the listing: the input fails as absent; nothing is substituted.
    let server = desktop();
    let backend = backend(&server);
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Scarlett".into()), ..Default::default() }).unwrap();
    let wish = ab::InputWish { device: Some("USB Mic".into()), ..Default::default() };
    let on = backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap();
    server.remove_physical("USB Mic:capture_MONO");
    let request = ab::OpenRequest { format: backend.capture_format(&on, None).unwrap(), ..plan.attempts[0] };
    let error = backend.open_capture(&on, &request, Box::new(|_| {}), Box::new(|_| {})).err().expect("refused");
    assert_eq!(error, ab::OpenError::DeviceNotAvailable(ab::absent_input("USB Mic")));
    assert_eq!(server.open_clients(), 0, "the client opened for it is closed again; no other group opened");
    // The browser's ports are reordered after the listing: the mapping no longer holds.
    let wish = ab::InputWish { device: Some("Firefox".into()), ..Default::default() };
    let on = backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap();
    server.remove_port("Firefox:output_FL");
    server.add_port("Firefox:output_FL", true);
    let request = ab::OpenRequest { format: backend.capture_format(&on, None).unwrap(), ..plan.attempts[0] };
    let error = backend.open_capture(&on, &request, Box::new(|_| {}), Box::new(|_| {})).err().expect("refused");
    assert_eq!(
        error,
        ab::OpenError::DeviceNotAvailable("The ports of input \"Firefox\" changed while it opened; no input opened. Use Reconnect.".into())
    );
    assert!(server.connects().is_empty(), "nothing linked");
    // The output's own group changed: the open fails, said by name.
    server.remove_physical("Scarlett:playback_AUX1");
    let error = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {})).err().expect("refused");
    assert_eq!(error, ab::OpenError::DeviceNotAvailable("JACK: the ports of \"Scarlett\" changed while the device opened".into()));
}
