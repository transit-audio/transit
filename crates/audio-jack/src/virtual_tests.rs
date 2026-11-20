//! ADR 0015, amendment for #431, slice C over the fake server: virtual devices. The probe lists,
//! beside today's physical port groups, every eligible virtual device (audio output ports the
//! server does not mark physical, grouped by client), and a chosen input resolves among both
//! whatever the preference (which only filters the Input list, in the app). A client with physical
//! output ports, transit's own clients (by the names the server assigned) and MIDI ports are never
//! virtual devices. A chosen input that is not there opens no input (rule 4); with no choice the
//! input stays physical (rule 1).

use crate::JackBackend;
use crate::devices::{JackDevice, ServerState};
use crate::fake::FakeServer;
use std::sync::Arc;
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend, Stream};

const HOUR: Duration = Duration::from_secs(3600);

fn backend(server: &FakeServer) -> JackBackend {
    JackBackend::with_api(Arc::new(server.clone())).with_timing(HOUR, HOUR, HOUR)
}

/// A PipeWire desktop: a Scarlett that plays and records (its ports physical, plus a monitor port
/// PipeWire does not mark physical), built-in audio that only plays physically and shows its
/// monitor ports under its own name, a browser's stream, a virtual source, a MIDI port, and an
/// application's input ports.
fn desktop() -> FakeServer {
    let server = FakeServer::running(
        48_000,
        256,
        &[
            ("Scarlett:capture_AUX0", true),
            ("Scarlett:capture_AUX1", true),
            ("Scarlett:playback_AUX0", false),
            ("Scarlett:playback_AUX1", false),
            ("Built-in Audio:playback_FL", false),
            ("Built-in Audio:playback_FR", false),
            ("USB Mic:capture_MONO", true),
        ],
    );
    server.add_port("Built-in Audio:monitor_FL", true);
    server.add_port("Built-in Audio:monitor_FR", true);
    server.add_port("Firefox:output_FL", true);
    server.add_port("Firefox:output_FR", true);
    server.add_port("Virtual Mic:capture_MONO", true);
    server.add_port("Scarlett:monitor_AUX0", true);
    server.add_midi_port("Midi Through:capture_1", true);
    server.add_port("OBS:input_FL", false);
    server
}

fn running(backend: &JackBackend) -> crate::devices::Server {
    let ServerState::Running(server) = backend.probe() else { panic!("not running") };
    server
}

fn names(devices: &[JackDevice]) -> Vec<&str> {
    devices.iter().map(|d| d.name.as_str()).collect()
}

/// The Player's duplex order: the input resolved, capture opened, render opened, both started.
fn open(backend: &JackBackend, output: &str, input: Option<&str>) -> Result<(Box<dyn Stream>, Box<dyn Stream>, ab::Endpoint), String> {
    let plan = backend.plan_render(&ab::OutputWish { device: Some(output.into()), ..Default::default() })?;
    let on = backend.duplex_capture_endpoint(&plan.endpoint, &ab::InputWish { device: input.map(str::to_owned), ..Default::default() })?;
    let request = ab::OpenRequest { format: backend.capture_format(&on, None)?, ..plan.attempts[0] };
    let (capture, _) = backend.open_capture(&on, &request, Box::new(|_| {}), Box::new(|_| {})).map_err(|e| e.to_string())?;
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {})).map_err(|e| e.to_string())?;
    capture.start().map_err(|e| e.to_string())?;
    render.start().map_err(|e| e.to_string())?;
    Ok((capture, render, on))
}

/// The links transit's client made to its own inputs.
fn input_links(server: &FakeServer) -> Vec<(String, String)> {
    server
        .clients(|c| c.iter().rfind(|c| !c.closed).map(|c| c.connections.iter().filter(|(_, to)| to.contains(":in_")).cloned().collect()))
        .unwrap_or_default()
}

#[test]
fn virtual_devices_are_the_non_physical_output_groups_and_todays_lists_are_unchanged() {
    let server = desktop();
    let backend = backend(&server);
    let found = running(&backend);
    assert_eq!(names(&found.devices), ["Scarlett", "USB Mic", "Built-in Audio"], "the physical groups, as before");
    assert_eq!(names(&found.virtual_devices), ["Built-in Audio", "Firefox", "Virtual Mic"]);
    let monitor = &found.virtual_devices[0];
    assert_eq!(monitor.capture, ["Built-in Audio:monitor_FL", "Built-in Audio:monitor_FR"], "the server's order");
    assert!(monitor.playback.is_empty() && monitor.virtual_device, "an input only");
    assert!(found.devices.iter().all(|d| !d.virtual_device));
    let listed = |direction| backend.endpoints(direction).unwrap().into_iter().map(|e| e.name).collect::<Vec<_>>();
    assert_eq!(listed(ab::Direction::Input), ["Scarlett", "USB Mic"], "the backend's input list is today's");
    assert_eq!(listed(ab::Direction::Output), ["Scarlett", "Built-in Audio"], "the output list lists physical playback ports only");
    assert!(!names(&found.virtual_devices).iter().any(|n| n.starts_with("Midi") || *n == "OBS"), "no MIDI port, no input port");
}

/// Rule 1: physical and non-physical ports are never mixed; a client with physical output ports is
/// listed as today, and its non-physical outputs give no virtual device.
#[test]
fn a_client_with_physical_output_ports_gets_no_virtual_entry() {
    let server = desktop();
    let backend = backend(&server);
    let found = running(&backend);
    assert!(!names(&found.virtual_devices).contains(&"Scarlett"), "{:?}", names(&found.virtual_devices));
    let scarlett = found.input("Scarlett").expect("the Scarlett records");
    assert_eq!(scarlett.capture, ["Scarlett:capture_AUX0", "Scarlett:capture_AUX1"], "its physical ports only");
}

/// Rule 1's third condition: transit's own session client and its probe client are never offered,
/// by the names the server assigned them, which here differ from the names transit asked for.
/// Clients of other programs that happen to carry the requested names are offered.
#[test]
fn transits_own_clients_are_never_offered_under_the_names_the_server_assigned() {
    let server = desktop();
    let backend = backend(&server);
    let mut foreign_session = backend.open_other_client(crate::CLIENT_NAME).unwrap();
    foreign_session.register_port("out_1", false).unwrap();
    let mut foreign_probe = backend.open_other_client(crate::PROBE_NAME).unwrap();
    foreign_probe.register_port("out_1", false).unwrap();
    // Enumeration with no session opens a probe; the server names it "transit-probe-02", and a
    // port under that name stands for one of its own.
    server.add_port("transit-probe-02:out_1", true);
    let found = running(&backend);
    let probe = server.clients(|c| c.last().unwrap().name.clone());
    assert_eq!(probe, "transit-probe-02", "the probe's assigned name differs from the one asked for");
    assert!(!names(&found.virtual_devices).contains(&probe.as_str()), "{:?}", names(&found.virtual_devices));
    let wish = ab::InputWish { device: Some(probe.clone()), ..Default::default() };
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Scarlett".into()), ..Default::default() }).unwrap();
    assert_eq!(backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap_err(), ab::absent_input(&probe), "the probe is not an input");
    // That probe closed with the enumeration (the scripted port stood for it).
    server.remove_port("transit-probe-02:out_1");
    // With the session open, enumeration reads through the session's client (no probe, #760 rule
    // 4) and excludes it by its assigned name.
    let (_capture, _render, _) = open(&backend, "Scarlett", None).unwrap();
    let ours = backend.client_name().unwrap();
    assert_ne!(ours, crate::CLIENT_NAME, "the server made the session's name unique");
    let clients = server.clients(<[_]>::len);
    let found = running(&backend);
    assert_eq!(server.clients(<[_]>::len), clients, "no probe while the session runs");
    let offered = names(&found.virtual_devices);
    assert!(!offered.contains(&ours.as_str()), "{offered:?}");
    assert!(offered.contains(&crate::CLIENT_NAME) && offered.contains(&crate::PROBE_NAME), "other programs' clients are offered: {offered:?}");
    // A saved choice naming either of transit's clients is not an input: it is absent (rule 2, 4).
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Scarlett".into()), ..Default::default() }).unwrap();
    let wish = ab::InputWish { device: Some(ours.clone()), ..Default::default() };
    assert_eq!(backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap_err(), ab::absent_input(&ours));
    let wish = ab::InputWish { device: Some("Midi Through".into()), ..Default::default() };
    assert_eq!(backend.duplex_capture_endpoint(&plan.endpoint, &wish).unwrap_err(), ab::absent_input("Midi Through"), "MIDI is never an input");
}

/// Rule 2: a virtual device opens like a port group: one input port per port, linked in the
/// server's order; its identity is the group name and its ports guard its paths (rule 5).
#[test]
fn a_chosen_virtual_device_opens_and_links_its_ports_in_the_servers_order() {
    let server = desktop();
    let backend = backend(&server);
    let (capture, render, on) = open(&backend, "Scarlett", Some("Firefox")).unwrap();
    assert_eq!((on.name.as_str(), on.inputs), ("Firefox", 2));
    let transit = backend.client_name().unwrap();
    assert_eq!(
        input_links(&server),
        [("Firefox:output_FL".to_string(), format!("{transit}:in_1")), ("Firefox:output_FR".to_string(), format!("{transit}:in_2"))]
    );
    assert_eq!(backend.guarded_ports(&on), ["output_FL", "output_FR"]);
    assert_eq!(backend.channel_names(&on, ab::Direction::Input), ["output_FL", "output_FR"]);
    drop((capture, render));
    let (c2, r2, scarlett) = open(&backend, "Scarlett", Some("Scarlett")).unwrap();
    assert!(backend.guarded_ports(&scarlett).is_empty(), "a physical group's ports guard nothing");
    // A stream that came back with another serial: the next resolution reports its new name.
    drop((c2, r2));
    server.rename_port("Firefox:output_FR", "Firefox:output_FR-57");
    let (_c, _r, again) = open(&backend, "Scarlett", Some("Firefox")).unwrap();
    assert_eq!(backend.guarded_ports(&again), ["output_FL", "output_FR-57"]);
}

/// Rule 1, "No choice made": with no chosen input the input resolves among physical groups only,
/// the output's own and then the first; a virtual device of the output's name is not its own group.
#[test]
fn with_no_chosen_input_only_a_physical_group_opens() {
    let server = desktop();
    let backend = backend(&server);
    let (_c, _r, on) = open(&backend, "Built-in Audio", None).unwrap();
    assert_eq!(on.name, "Scarlett", "the first physical group that records, not the output's monitor");
    assert!(backend.guarded_ports(&on).is_empty());
    let only_virtual = FakeServer::running(48_000, 256, &[("HDMI:playback_FL", false)]);
    only_virtual.add_port("Firefox:output_FL", true);
    let backend = self::backend(&only_virtual);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let error = backend.duplex_capture_endpoint(&plan.endpoint, &ab::InputWish::default()).unwrap_err();
    assert!(error.contains("no port group has Input ports"), "a virtual device is never the default input: {error}");
}

/// Rule 4: a chosen input that is not there, virtual or hardware, opens no input, links no other
/// group and says so by name; the output still opens. When it is back, the next open (Reconnect)
/// opens it.
#[test]
fn a_chosen_input_that_is_absent_opens_no_input_and_the_next_open_finds_it_back() {
    let server = desktop();
    let backend = backend(&server);
    server.remove_port("Firefox:output_FL");
    server.remove_port("Firefox:output_FR");
    server.remove_physical("USB Mic:capture_MONO");
    for absent in ["Firefox", "USB Mic"] {
        assert_eq!(open(&backend, "Scarlett", Some(absent)).err(), Some(ab::absent_input(absent)));
        assert_eq!(server.open_clients(), 0, "no input opened, nothing was linked");
    }
    // The output opens alone, as the Player's soft input failure keeps it.
    let plan = backend.plan_render(&ab::OutputWish { device: Some("Scarlett".into()), ..Default::default() }).unwrap();
    let (render, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], Box::new(|_| {}), Box::new(|_| {})).unwrap();
    render.start().unwrap();
    assert!(input_links(&server).is_empty(), "no other group is linked to transit's inputs");
    drop(render);
    server.add_port("Firefox:output_FL", true);
    server.add_port("Firefox:output_FR", true);
    let (_c, _r, on) = open(&backend, "Scarlett", Some("Firefox")).unwrap();
    assert_eq!(on.name, "Firefox", "back: the next open opens it");
}

/// Rule 5 with rule 6: the same virtual device comes back at the same width with its ports in
/// another order. transit links the new order (its own links follow the server), and a link
/// another program made to an input whose port moved is dropped with the diagnostic, never moved
/// onto another source.
#[test]
fn a_same_width_reorder_drops_the_outside_links_of_the_inputs_that_moved() {
    let server = desktop();
    server.add_port("Player:out_L", true);
    let backend = backend(&server);
    let (capture, render, _) = open(&backend, "Scarlett", Some("Firefox")).unwrap();
    let transit = backend.client_name().unwrap();
    server.outside_link("Player:out_L", &format!("{transit}:in_1"));
    let snapshot = render.snapshot_links().expect("a snapshot");
    drop((capture, render));
    server.remove_port("Firefox:output_FL");
    server.add_port("Firefox:output_FL", true);
    let (_capture, render, on) = open(&backend, "Scarlett", Some("Firefox")).unwrap();
    assert_eq!(backend.guarded_ports(&on), ["output_FR", "output_FL"], "the server's new order");
    let transit = backend.client_name().unwrap();
    assert!(server.linked("Firefox:output_FR", &format!("{transit}:in_1")));
    let note = render.restore_links(snapshot).expect("a diagnostic");
    assert!(note.contains("in_1 from Player:out_L (in_1 now stands for another channel)"), "{note}");
    assert!(!server.linked("Player:out_L", &format!("{transit}:in_1")));
}
