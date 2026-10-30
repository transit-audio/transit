//! The native backends from the Player's and the settings window's side: which backend a
//! configuration selects ([`factory_for`]), and the Audio Devices listing of a native system built
//! from `Backend::endpoints` ([`system_from_endpoints`]; the first part of issue #2: the native ASIO
//! system is listed through its backend, not cpal).

use crate::devices::{self, AudioConfig, AudioSystem, DeviceInfo};
use transit_audio_backend as ab;

/// Which native backend serves an open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeKind {
    Wasapi,
    Asio,
    Jack,
}

impl NativeKind {
    /// How the status line names it.
    pub const fn label(self) -> &'static str {
        match self {
            NativeKind::Wasapi => "Native WASAPI",
            NativeKind::Asio => "Native ASIO",
            NativeKind::Jack => "Native JACK",
        }
    }

    /// What the Player says when this backend did not open and cpal did. cpal has no ASIO host,
    /// so a failed native ASIO open falls to cpal's default host, WASAPI shared.
    pub fn fallback_note(self, reason: &str) -> String {
        let opened = match self {
            NativeKind::Asio => "WASAPI shared",
            NativeKind::Wasapi | NativeKind::Jack => "the cpal backend",
        };
        format!("{} failed: {reason}; using {opened}", self.label())
    }
}

/// The native backend `cfg` selects on its audio system in this build, else `None` (cpal serves
/// it): native WASAPI on the WASAPI system, native ASIO on the ASIO system, native JACK on the JACK
/// system.
pub fn factory_for(cfg: &AudioConfig) -> Option<(Box<dyn ab::BackendFactory>, NativeKind)> {
    #[cfg(all(target_os = "linux", feature = "native-jack"))]
    if auto_jack_candidate(cfg) && matches!(transit_audio_jack::JackBackend::shared().probe(), transit_audio_jack::ServerState::Running(_)) {
        return Some((Box::new(transit_audio_jack::JackFactory), NativeKind::Jack));
    }
    if !cfg.backend.native_on(cfg.system.as_deref()) {
        return None;
    }
    match cfg.backend {
        #[cfg(all(windows, feature = "native-wasapi"))]
        devices::Backend::NativeWasapi => {
            Some((Box::new(transit_audio_wasapi::WasapiFactory { input_pairing: cfg.input_pairing.clone() }), NativeKind::Wasapi))
        }
        #[cfg(all(windows, target_pointer_width = "64", feature = "native-asio"))]
        devices::Backend::NativeAsio => Some((Box::new(transit_audio_asio::AsioFactory), NativeKind::Asio)),
        #[cfg(all(target_os = "linux", feature = "native-jack"))]
        devices::Backend::NativeJack => Some((Box::new(transit_audio_jack::JackFactory), NativeKind::Jack)),
        _ => None,
    }
}

/// Whether `Auto` would open the native JACK backend for `cfg` when a JACK or PipeWire-JACK server
/// answers (ADR 0021, slice 2 amendment): the JACK system, or the platform default with no device
/// named (a saved ALSA device name keeps cpal). [`factory_for`] asks this only in a Linux build
/// with the backend, and then probes for a server (never starting one); with none, cpal's ALSA
/// host serves as before.
pub fn auto_jack_candidate(cfg: &AudioConfig) -> bool {
    cfg.backend == devices::Backend::Auto
        && (devices::is_jack_system(cfg.system.as_deref()) || (cfg.system.is_none() && cfg.output.is_none() && cfg.input.is_none()))
}

/// The Audio Devices note of the JACK system, once for all its port groups (#146): the server (name
/// and version), its rate (the server's) and the buffer it runs. Picking another buffer size asks
/// the server to change it, for all its clients; the server can also be pinned (PipeWire's
/// `pw-metadata`).
pub fn jack_note(server: &str, rate: u32, buffer: u32) -> String {
    format!(
        "Native JACK ({server}) runs at {rate} Hz (the server's rate) with a {buffer}-sample buffer; a size picked here is requested of the server on Apply. To pin one instead (PipeWire: pw-metadata -n settings 0 clock.force-quantum {buffer}; JACK 2: jackd -p {buffer})"
    )
}

/// One endpoint as the settings window lists it: its channel count in `direction`, the standard
/// rates and buffer range its formats cover (none when its sample type is not supported).
fn describe(endpoint: &ab::Endpoint, direction: ab::Direction, is_default: bool) -> DeviceInfo {
    let max_channels = match direction {
        ab::Direction::Output => endpoint.outputs,
        ab::Direction::Input => endpoint.inputs,
    };
    let formats = &endpoint.formats;
    let sample_rates = devices::STANDARD_RATES.iter().copied().filter(|r| formats.iter().any(|f| f.min_rate <= *r && *r <= f.max_rate)).collect();
    let buffer_min = formats.iter().filter_map(|f| f.period.map(|p| p.0)).min();
    let buffer_max = formats.iter().filter_map(|f| f.period.map(|p| p.1)).max();
    DeviceInfo { name: endpoint.name.clone(), is_default, max_channels, sample_rates, buffer_min, buffer_max }
}

/// The settings window's entry of a native audio system: its endpoints per direction (the first
/// one is the default: a native plan opens it when no device is named), then every `installed`
/// device the backend could not describe (another driver held the process's one ASIO slot), offered
/// with unknown counts as `add_unlisted` does for cpal, and the diagnostics of those that failed.
pub fn system_from_endpoints(
    name: &str,
    outputs: &[ab::Endpoint],
    inputs: &[ab::Endpoint],
    installed: &[String],
    diagnostics: Vec<String>,
) -> AudioSystem {
    let list = |endpoints: &[ab::Endpoint], direction| -> Vec<DeviceInfo> {
        let mut list: Vec<DeviceInfo> = endpoints.iter().enumerate().map(|(i, e)| describe(e, direction, i == 0)).collect();
        devices::add_unlisted(&mut list, installed);
        list
    };
    AudioSystem {
        name: name.to_owned(),
        is_default: false,
        outputs: list(outputs, ab::Direction::Output),
        inputs: list(inputs, ab::Direction::Input),
        diagnostics,
    }
}

/// The ASIO system as the native backend lists it: every installed driver by its registry name,
/// none of them instantiated, and only `selected` (the driver chosen in Audio Devices, else the one
/// running) probed through the backend for its channels, rates and buffer range (the driver this
/// process has open is queried in place); the others are offered with unknown counts. A selected
/// driver that failed to load (or is not installed) is the one diagnostic. While cpal's ASIO host
/// holds a driver nothing is probed (two loaded drivers in one process is what ASIO forbids): the
/// registry names only. This can take seconds on a misbehaving driver: the app calls it off its UI
/// thread.
#[cfg(all(windows, target_pointer_width = "64", feature = "native-asio"))]
pub fn asio_system(selected: Option<&str>) -> AudioSystem {
    use transit_audio_asio::AsioBackend;
    let backend = match AsioBackend::shared() {
        Ok(backend) => backend,
        Err(e) => return system_from_endpoints(devices::ASIO_SYSTEM, &[], &[], &[], vec![format!("the native ASIO backend did not start: {e}")]),
    };
    let mut diagnostics = Vec::new();
    let installed: Vec<String> = match backend.driver_names() {
        Ok(names) => names,
        Err(e) => {
            diagnostics.push(format!("the ASIO drivers could not be listed: {e}"));
            Vec::new()
        }
    };
    let Some(selected) = selected else {
        return system_from_endpoints(devices::ASIO_SYSTEM, &[], &[], &installed, diagnostics);
    };
    if !installed.iter().any(|n| n == selected) {
        diagnostics.push(format!("{selected}: this ASIO driver is not installed"));
        return system_from_endpoints(devices::ASIO_SYSTEM, &[], &[], &installed, diagnostics);
    }
    if let Err(e) = backend.probe(selected) {
        log::info!("ASIO: listing \"{selected}\": {e}");
    }
    let described = |direction| -> Vec<ab::Endpoint> {
        ab::Backend::endpoints(&backend, direction).unwrap_or_default().into_iter().filter(|e| e.name == selected).collect()
    };
    let (outputs, inputs) = (described(ab::Direction::Output), described(ab::Direction::Input));
    if let Some(Err(problem)) = backend.probed(selected).filter(|p| p.as_ref().is_err_and(|d| !d.blocked)) {
        diagnostics.push(problem.to_string());
    }
    system_from_endpoints(devices::ASIO_SYSTEM, &outputs, &inputs, &installed, diagnostics)
}

/// The JACK system as the native backend lists it (ADR 0021, decision 3): one device per port group,
/// through a short-lived probe client that never starts a server; with no library or no server, no
/// device and the reason as the system's diagnostic.
#[cfg(all(target_os = "linux", feature = "native-jack"))]
pub fn jack_system() -> AudioSystem {
    let backend = transit_audio_jack::JackBackend::shared();
    let state = backend.probe();
    match &state {
        transit_audio_jack::ServerState::Running(server) => {
            let name = backend.server_name();
            let mut notes = vec![jack_note(&name, server.rate, server.buffer)];
            notes.extend(backend.library_note());
            let mut system = system_from_endpoints(
                devices::JACK_SYSTEM,
                &server.endpoints(ab::Direction::Output),
                &server.endpoints(ab::Direction::Input),
                &[],
                notes,
            );
            // Under PipeWire the buffer list is the server's quantum range (#230).
            if name.starts_with("PipeWire")
                && let Some(clock) = crate::pipewire::clock()
            {
                apply_server_clock(&mut system, &clock);
            }
            system
        }
        _ => {
            let mut diagnostics: Vec<String> = state.diagnostic().into_iter().collect();
            diagnostics.extend(backend.library_note());
            system_from_endpoints(devices::JACK_SYSTEM, &[], &[], &[], diagnostics)
        }
    }
}

/// The JACK system under a PipeWire server (#230): every port group offers the quanta the server
/// allows, and the server's line (its default quantum, range and a forced one) is a system note.
pub fn apply_server_clock(system: &mut AudioSystem, clock: &crate::pipewire::ServerClock) {
    if let Some(range) = clock.quantum_range() {
        for device in system.outputs.iter_mut().chain(system.inputs.iter_mut()) {
            crate::pipewire::clamp_range(device, range);
        }
    }
    system.diagnostics.push(clock.device_note("Server", false));
}

/// Opens the control panel of the ASIO driver `driver` on the backend's host thread (where every
/// call to the driver is made), from a thread of its own so a modal panel does not hold the caller.
/// The receiver gets the panel's result when it closes. The driver of a running stream is shared
/// with the panel; another driver is loaded for the panel only while no stream holds one.
#[cfg(all(windows, target_pointer_width = "64", feature = "native-asio"))]
pub fn asio_control_panel(driver: &str) -> Result<std::sync::mpsc::Receiver<Result<(), String>>, String> {
    let backend = transit_audio_asio::AsioBackend::shared().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    let driver = driver.to_owned();
    std::thread::Builder::new()
        .name("transit-asio-panel".into())
        .spawn(move || {
            let result = backend.load(&driver).and_then(|loaded| {
                let shown = loaded.control_panel();
                let closed = loaded.close();
                shown.and(closed)
            });
            let _ = tx.send(result.map_err(|e| e.to_string()));
        })
        .map_err(|e| format!("could not start the control panel thread: {e}"))?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devices::Backend;
    use std::sync::Arc;

    fn endpoint(name: &str, inputs: u16, outputs: u16, rates: &[u32]) -> ab::Endpoint {
        let channels = outputs.max(inputs);
        let formats = rates
            .iter()
            .map(|r| ab::FormatRange { sample: ab::SampleFormat::I24, channels, min_rate: *r, max_rate: *r, period: Some((32, 2048)) })
            .collect();
        ab::Endpoint::new(name.into(), name.into(), "ASIO".into(), inputs, outputs, formats, Arc::new(()))
    }

    /// Native JACK under PipeWire (#230): each port group offers the quanta the server allows
    /// (64-2048 on the tester's machine) instead of the backend's standard list, and the server's
    /// line, with a forced quantum, is a system note.
    #[test]
    fn the_jack_system_under_pipewire_offers_the_servers_quantum_range() {
        let group = endpoint("alsa_output.pci", 0, 2, &[48_000]);
        let mut system = system_from_endpoints(devices::JACK_SYSTEM, std::slice::from_ref(&group), &[], &[], Vec::new());
        let clock = crate::pipewire::ServerClock {
            rate: Some(48_000),
            quantum: Some(128),
            min_quantum: Some(64),
            max_quantum: Some(1024),
            force_quantum: Some(256),
        };
        apply_server_clock(&mut system, &clock);
        assert_eq!(system.outputs[0].buffer_sizes(), [64, 128, 256, 512, 1024]);
        assert_eq!(
            system.system_notes().collect::<Vec<_>>(),
            [
                "Server: PipeWire server, default quantum 128, allowed 64-1024, forced to 256 (transit cannot change it). The buffer size here is the quantum."
            ]
        );
    }

    #[test]
    fn a_native_system_lists_its_endpoints_then_the_drivers_it_could_not_describe() {
        let model12 = endpoint("Model Mixer ASIO Driver", 12, 10, &[44_100, 48_000]);
        let installed = ["Model Mixer ASIO Driver".to_string(), "Focusrite USB ASIO".to_string()];
        let system = system_from_endpoints(
            "ASIO",
            std::slice::from_ref(&model12),
            std::slice::from_ref(&model12),
            &installed,
            vec!["Broken: init failed".into()],
        );
        assert_eq!(system.name, "ASIO");
        let names: Vec<(&str, u16, bool)> = system.outputs.iter().map(|d| (d.name.as_str(), d.max_channels, d.is_default)).collect();
        assert_eq!(names, [("Model Mixer ASIO Driver", 10, true), ("Focusrite USB ASIO", 0, false)], "the registry names, the probed one first");
        assert_eq!((system.inputs[0].max_channels, system.inputs[1].max_channels), (12, 0));
        let m = &system.outputs[0];
        assert_eq!(m.sample_rates, [44_100, 48_000]);
        assert_eq!((m.buffer_min, m.buffer_max), (Some(32), Some(2048)));
        assert_eq!(m.buffer_sizes(), [32, 64, 128, 256, 512, 1024, 2048]);
        assert_eq!(system.outputs[1].sample_rates, devices::STANDARD_RATES, "an unprobed driver offers the standard choices");
        assert_eq!(system.diagnostics, ["Broken: init failed"]);
    }

    #[test]
    fn an_unsupported_sample_type_lists_the_driver_with_no_rate() {
        let msb = endpoint("Big Endian", 2, 2, &[]);
        let system = system_from_endpoints("ASIO", &[msb], &[], &[], Vec::new());
        assert_eq!((system.outputs[0].max_channels, system.outputs[0].sample_rates.len()), (2, 0));
        assert!(system.inputs.is_empty());
    }

    #[test]
    fn a_native_backend_is_selected_only_where_it_is_built_and_serves_the_system() {
        let asio = AudioConfig { backend: Backend::NativeAsio, system: Some("ASIO".into()), ..Default::default() };
        assert_eq!(factory_for(&asio).map(|(_, kind)| kind), devices::NATIVE_ASIO_COMPILED.then_some(NativeKind::Asio));
        let wasapi = AudioConfig { backend: Backend::NativeWasapi, system: Some("WASAPI".into()), ..Default::default() };
        assert_eq!(factory_for(&wasapi).map(|(_, kind)| kind), devices::NATIVE_WASAPI_COMPILED.then_some(NativeKind::Wasapi));
        let jack = AudioConfig { backend: Backend::NativeJack, system: Some("JACK".into()), ..Default::default() };
        assert_eq!(factory_for(&jack).map(|(_, kind)| kind), devices::NATIVE_JACK_COMPILED.then_some(NativeKind::Jack));
        let auto_jack = AudioConfig { backend: Backend::Auto, system: Some("JACK".into()), ..Default::default() };
        assert!(auto_jack_candidate(&auto_jack), "Auto on the JACK system opens native JACK when a server answers");
        assert!(auto_jack_candidate(&AudioConfig::default()), "so does the untouched default");
        let alsa_device = AudioConfig { output: Some("hw:CARD=USB,DEV=0".into()), ..Default::default() };
        assert!(!auto_jack_candidate(&alsa_device), "a saved ALSA device keeps cpal");
        assert!(!auto_jack_candidate(&AudioConfig { system: Some("ALSA".into()), ..Default::default() }));
        assert!(!auto_jack_candidate(&AudioConfig { backend: Backend::Cpal, ..auto_jack.clone() }), "cpal is cpal");
        if !devices::NATIVE_JACK_COMPILED {
            assert!(factory_for(&auto_jack).is_none(), "a build without the backend stays cpal");
        }
        assert_eq!(
            jack_note("JACK 1.9.20", 48_000, 128),
            "Native JACK (JACK 1.9.20) runs at 48000 Hz (the server's rate) with a 128-sample buffer; a size picked here is requested of the server on Apply. To pin one instead (PipeWire: pw-metadata -n settings 0 clock.force-quantum 128; JACK 2: jackd -p 128)"
        );
        let crossed = AudioConfig { backend: Backend::NativeAsio, system: Some("WASAPI".into()), ..Default::default() };
        assert!(factory_for(&crossed).is_none(), "native ASIO never serves the WASAPI system");
        assert!(factory_for(&AudioConfig { backend: Backend::Cpal, ..asio }).is_none());
        assert_eq!(NativeKind::Asio.fallback_note("no driver"), "Native ASIO failed: no driver; using WASAPI shared");
        assert_eq!(
            NativeKind::Jack.fallback_note("JACK/PipeWire server not running"),
            "Native JACK failed: JACK/PipeWire server not running; using the cpal backend"
        );
    }
}
