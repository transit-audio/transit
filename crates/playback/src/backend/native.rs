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

    /// Whether a failed open of this backend falls back to cpal: native JACK only (off Windows, ADR
    /// 0021). Native WASAPI and native ASIO never do: their failure is the visible error with the
    /// device closed (ADR 0046).
    pub const fn falls_back_to_cpal(self) -> bool {
        matches!(self, NativeKind::Jack)
    }

    /// What the Player says when native JACK did not open and cpal did. Never produced for native
    /// WASAPI or native ASIO ([`Self::falls_back_to_cpal`]).
    pub fn fallback_note(self, reason: &str) -> String {
        format!("{} failed: {reason}; using the cpal backend", self.label())
    }
}

/// The native backend of the effective backend of `cfg` (`Backend::effective`, ADR 0046), else
/// `None` (cpal serves it, off Windows only). On Windows the audio system decides: native ASIO on
/// the ASIO system (64-bit builds), native WASAPI on any other, whatever backend was saved.
/// Elsewhere the saved choice where it serves the system: native JACK on the JACK system, or `Auto`
/// when a JACK server answers (ADR 0021).
pub fn factory_for(cfg: &AudioConfig) -> Option<(Box<dyn ab::BackendFactory>, NativeKind)> {
    #[cfg(test)]
    super::asks::count();
    #[cfg(all(target_os = "linux", feature = "native-jack"))]
    if auto_jack_candidate(cfg) && matches!(transit_audio_jack::JackBackend::shared().probe(), transit_audio_jack::ServerState::Running(_)) {
        return Some((Box::new(transit_audio_jack::JackFactory), NativeKind::Jack));
    }
    let system = cfg.system.as_deref();
    let backend = cfg.backend.effective(system);
    if !cfg!(windows) && !backend.native_on(system) {
        return None;
    }
    match backend {
        #[cfg(windows)]
        devices::Backend::NativeWasapi => {
            Some((Box::new(transit_audio_wasapi::WasapiFactory { input_pairing: cfg.input_pairing.clone() }), NativeKind::Wasapi))
        }
        #[cfg(all(windows, target_pointer_width = "64"))]
        devices::Backend::NativeAsio => Some((Box::new(transit_audio_asio::AsioFactory), NativeKind::Asio)),
        #[cfg(all(target_os = "linux", feature = "native-jack"))]
        devices::Backend::NativeJack => Some((Box::new(transit_audio_jack::JackFactory), NativeKind::Jack)),
        _ => None,
    }
}

/// The configuration a native open uses for `cfg` (ADR 0046): on Windows a saved audio system no
/// native backend serves (a configuration from another platform, "ALSA" say) opens the default
/// system, native WASAPI, as cpal opened its default host for an unknown name; the app's note says
/// "Audio system “ALSA” is not available; using the default." Otherwise `cfg` as it is.
pub fn open_config(cfg: &AudioConfig) -> std::borrow::Cow<'_, AudioConfig> {
    let system = cfg.system.as_deref();
    if cfg!(windows) && system.is_some() && !devices::is_asio_system(system) && !devices::is_wasapi_system(system) {
        std::borrow::Cow::Owned(AudioConfig { system: None, ..cfg.clone() })
    } else {
        std::borrow::Cow::Borrowed(cfg)
    }
}

/// The native factory of the effective backend of `cfg` for an input that opens by itself
/// (`Recorder::open_with`, a silent runtime's input): on Windows the one [`factory_for`] picks;
/// `None` elsewhere, where cpal opens inputs.
pub fn input_factory(cfg: &AudioConfig) -> Option<Box<dyn ab::BackendFactory>> {
    if cfg!(windows) { factory_for(cfg).map(|(factory, _)| factory) } else { None }
}

/// Why nothing opens `cfg` on Windows when no native backend serves its system (the ASIO system of
/// a build without native ASIO): said instead of opening another backend (ADR 0046).
pub fn unserved(cfg: &AudioConfig) -> String {
    format!("no native backend serves the audio system “{}” in this build", cfg.system.as_deref().unwrap_or(devices::WASAPI_SYSTEM))
}

/// The index of the endpoint native WASAPI opens for a saved `device` among `candidates` (`(name,
/// id)` in default-first order): the first match by name or id, else 0, the default endpoint
/// (`transit_audio_wasapi::saved_endpoint`, ADR 0046 decision 2).
#[cfg(windows)]
pub fn wasapi_saved_endpoint(candidates: &[(&str, &str)], device: Option<&str>) -> Option<usize> {
    transit_audio_wasapi::saved_endpoint(candidates, device, "output")
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
/// `is_default` marks the platform's default system (WASAPI on Windows; never ASIO or JACK).
pub fn system_from_endpoints(
    name: &str,
    is_default: bool,
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
        is_default,
        outputs: list(outputs, ab::Direction::Output),
        inputs: list(inputs, ab::Direction::Input),
        diagnostics,
    }
}

/// One direction as an enumerator lists it: the endpoints and the id of the default one (`None`
/// when the system has no default).
pub type Listing = (Vec<ab::Endpoint>, Option<String>);

/// The WASAPI system as the native backend lists it (ADR 0046, decision 2): every active endpoint
/// per direction by its friendly name (`PKEY_Device_FriendlyName`, the property cpal names them by),
/// from `IMMDeviceEnumerator`, with the `eConsole` default first and marked. The default system on
/// Windows. An enumerator error is the system's diagnostic ("WASAPI: <error>") over an empty list.
#[cfg(windows)]
pub fn wasapi_system() -> AudioSystem {
    let backend = transit_audio_wasapi::WasapiBackend::default();
    wasapi_system_from(|direction| backend.listing(direction).map_err(|e| e.to_string()))
}

/// [`wasapi_system`] over any lister: `list` answers one direction. The endpoints are put in
/// default-first order (the order a native plan resolves in) and only the default one is marked;
/// a default the list does not hold marks none.
pub fn wasapi_system_from(list: impl Fn(ab::Direction) -> Result<Listing, String>) -> AudioSystem {
    let mut diagnostics = Vec::new();
    let mut side = |direction| -> (Vec<ab::Endpoint>, Option<String>) {
        match list(direction) {
            Ok((mut endpoints, default)) => {
                endpoints.sort_by_key(|e| Some(&e.id) != default.as_ref());
                (endpoints, default)
            }
            Err(e) => {
                let note = format!("{}: {e}", devices::WASAPI_SYSTEM);
                if !diagnostics.contains(&note) {
                    diagnostics.push(note);
                }
                (Vec::new(), None)
            }
        }
    };
    let (outputs, default_out) = side(ab::Direction::Output);
    let (inputs, default_in) = side(ab::Direction::Input);
    let mut system = system_from_endpoints(devices::WASAPI_SYSTEM, true, &outputs, &inputs, &[], diagnostics);
    for (listed, endpoints, default) in [(&mut system.outputs, &outputs, default_out), (&mut system.inputs, &inputs, default_in)] {
        for (device, endpoint) in listed.iter_mut().zip(endpoints) {
            device.is_default = default.as_ref() == Some(&endpoint.id);
            // The buffer range is reported as unknown, as cpal's WASAPI rows did, so every standard
            // size stays offered and a saved size still resolves. The endpoint's shared-mode engine
            // period (often 480-480, and `STANDARD_BUFFERS` has no 480 or 441) would leave only
            // "System default". The honest native offer is an open decision of ADR 0046 (slice 3
            // left it as it was).
            device.buffer_min = None;
            device.buffer_max = None;
        }
    }
    system
}

/// The ASIO system as the native backend lists it (#363): every installed driver by its registry
/// name, none of them instantiated. Only `selected` (the driver this process has open) is described,
/// queried in place for its channels, rates and buffer range ([`transit_audio_asio::AsioBackend::probe_open`]);
/// when it is not open (it closed meanwhile) it is named like the others, with unknown counts, and
/// nothing is loaded to describe it: a driver is instantiated only to open it. A selected driver
/// that is not installed, or whose query failed, is the one diagnostic. A query of a driver blocked
/// in a call can take seconds: the app calls this off its UI thread.
#[cfg(all(windows, target_pointer_width = "64"))]
pub fn asio_system(selected: Option<&str>) -> AudioSystem {
    use transit_audio_asio::AsioBackend;
    let backend = match AsioBackend::shared() {
        Ok(backend) => backend,
        Err(e) => {
            return system_from_endpoints(devices::ASIO_SYSTEM, false, &[], &[], &[], vec![format!("the native ASIO backend did not start: {e}")]);
        }
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
        return system_from_endpoints(devices::ASIO_SYSTEM, false, &[], &[], &installed, diagnostics);
    };
    if !installed.iter().any(|n| n == selected) {
        diagnostics.push(format!("{selected}: this ASIO driver is not installed"));
        return system_from_endpoints(devices::ASIO_SYSTEM, false, &[], &[], &installed, diagnostics);
    }
    let open = match backend.probe_open(selected) {
        Ok(Some(_)) => true,
        Ok(None) => {
            log::debug!("ASIO: listing \"{selected}\" by name: it is not open");
            false
        }
        Err(e) => {
            log::info!("ASIO: listing \"{selected}\": {e}");
            diagnostics.push(format!("{selected}: {e}"));
            false
        }
    };
    let described = |direction| -> Vec<ab::Endpoint> {
        if !open {
            return Vec::new();
        }
        ab::Backend::endpoints(&backend, direction).unwrap_or_default().into_iter().filter(|e| e.name == selected).collect()
    };
    let (outputs, inputs) = (described(ab::Direction::Output), described(ab::Direction::Input));
    system_from_endpoints(devices::ASIO_SYSTEM, false, &outputs, &inputs, &installed, diagnostics)
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
            // A PipeWire sink's monitor ports carry what that output plays (#268): said under it.
            notes.extend(server.devices.iter().filter_map(|d| {
                crate::system_audio::monitor_label(&d.name, &d.channel_names(ab::Direction::Input)).map(|label| format!("{}: {label}", d.name))
            }));
            let mut system = system_from_endpoints(
                devices::JACK_SYSTEM,
                false,
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
            system_from_endpoints(devices::JACK_SYSTEM, false, &[], &[], &[], diagnostics)
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
#[cfg(all(windows, target_pointer_width = "64"))]
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
        let mut system = system_from_endpoints(devices::JACK_SYSTEM, false, std::slice::from_ref(&group), &[], &[], Vec::new());
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
            false,
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

    fn names(list: &[DeviceInfo]) -> Vec<(&str, bool)> {
        list.iter().map(|d| (d.name.as_str(), d.is_default)).collect()
    }

    /// ADR 0046, decision 2: the native WASAPI list is the default system, default endpoint first
    /// and marked, whatever order the enumerator returned.
    #[test]
    fn the_wasapi_system_lists_the_default_endpoint_first_and_is_the_default_system() {
        let [a, b, c] = [endpoint("Speakers", 0, 2, &[48_000]), endpoint("Headphones", 0, 2, &[48_000]), endpoint("Monitors", 0, 8, &[48_000])];
        let mic = endpoint("Microphone", 1, 0, &[48_000]);
        let list = |default: &'static str| {
            let (outputs, inputs) = (vec![a.clone(), b.clone(), c.clone()], vec![mic.clone()]);
            move |direction| -> Result<Listing, String> {
                Ok(match direction {
                    ab::Direction::Output => (outputs.clone(), Some(default.to_owned())),
                    ab::Direction::Input => (inputs.clone(), Some("Microphone".to_owned())),
                })
            }
        };
        let system = wasapi_system_from(list("Monitors"));
        assert_eq!(system.name, "WASAPI");
        assert!(system.is_default, "WASAPI is the default system");
        assert_eq!(names(&system.outputs), [("Monitors", true), ("Speakers", false), ("Headphones", false)]);
        assert_eq!(system.outputs[0].max_channels, 8);
        let d = &system.outputs[0];
        assert_eq!((d.buffer_min, d.buffer_max), (None, None), "the engine period (32-2048 here) is not offered yet");
        assert_eq!(d.buffer_sizes(), devices::STANDARD_BUFFERS, "every standard size, as cpal's rows offered");
        assert_eq!(names(&system.inputs), [("Microphone", true)]);
        assert!(system.diagnostics.is_empty());
        // Control: another default reorders and marks that one, not the third.
        let other = wasapi_system_from(list("Headphones"));
        assert_eq!(names(&other.outputs), [("Headphones", true), ("Speakers", false), ("Monitors", false)]);
        // A default the list does not hold (its client did not activate) marks none.
        let gone = wasapi_system_from(list("Unplugged"));
        assert_eq!(names(&gone.outputs), [("Speakers", false), ("Headphones", false), ("Monitors", false)]);
    }

    /// An enumerator error is the WASAPI system's diagnostic over an empty list, never a panic or a
    /// silent empty list; a direction that lists keeps its endpoints.
    #[test]
    fn a_wasapi_enumerator_error_is_the_systems_diagnostic() {
        let failed = wasapi_system_from(|_| Err("EnumAudioEndpoints failed: 0x80070005".to_owned()));
        assert!(failed.outputs.is_empty() && failed.inputs.is_empty());
        assert_eq!(failed.diagnostics, ["WASAPI: EnumAudioEndpoints failed: 0x80070005"], "one note for both directions");
        assert_eq!(failed.system_notes().collect::<Vec<_>>(), ["WASAPI: EnumAudioEndpoints failed: 0x80070005"]);
        let speakers = endpoint("Speakers", 0, 2, &[48_000]);
        let half = wasapi_system_from(|direction| match direction {
            ab::Direction::Output => Ok((vec![speakers.clone()], Some("Speakers".to_owned()))),
            ab::Direction::Input => Err("GetCount failed".to_owned()),
        });
        assert_eq!(names(&half.outputs), [("Speakers", true)]);
        assert_eq!(half.diagnostics, ["WASAPI: GetCount failed"]);
    }

    #[test]
    fn an_unsupported_sample_type_lists_the_driver_with_no_rate() {
        let msb = endpoint("Big Endian", 2, 2, &[]);
        let system = system_from_endpoints("ASIO", false, &[msb], &[], &[], Vec::new());
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
        let jack_kind = if cfg!(windows) { Some(NativeKind::Wasapi) } else { devices::NATIVE_JACK_COMPILED.then_some(NativeKind::Jack) };
        assert_eq!(factory_for(&jack).map(|(_, kind)| kind), jack_kind, "on Windows any system but ASIO is native WASAPI's");
        let auto_jack = AudioConfig { backend: Backend::Auto, system: Some("JACK".into()), ..Default::default() };
        assert!(auto_jack_candidate(&auto_jack), "Auto on the JACK system opens native JACK when a server answers");
        assert!(auto_jack_candidate(&AudioConfig::default()), "so does the untouched default");
        let alsa_device = AudioConfig { output: Some("hw:CARD=USB,DEV=0".into()), ..Default::default() };
        assert!(!auto_jack_candidate(&alsa_device), "a saved ALSA device keeps cpal");
        assert!(!auto_jack_candidate(&AudioConfig { system: Some("ALSA".into()), ..Default::default() }));
        assert!(!auto_jack_candidate(&AudioConfig { backend: Backend::Cpal, ..auto_jack.clone() }), "cpal is cpal");
        if !cfg!(windows) && !devices::NATIVE_JACK_COMPILED {
            assert!(factory_for(&auto_jack).is_none(), "a build without the backend stays cpal");
        }
        assert_eq!(
            jack_note("JACK 1.9.20", 48_000, 128),
            "Native JACK (JACK 1.9.20) runs at 48000 Hz (the server's rate) with a 128-sample buffer; a size picked here is requested of the server on Apply. To pin one instead (PipeWire: pw-metadata -n settings 0 clock.force-quantum 128; JACK 2: jackd -p 128)"
        );
        let crossed = AudioConfig { backend: Backend::NativeAsio, system: Some("WASAPI".into()), ..Default::default() };
        // ADR 0046: on Windows the system decides, so the crossed configuration opens native WASAPI and
        // a saved cpal on ASIO opens native ASIO; elsewhere native ASIO never serves WASAPI and cpal is cpal.
        assert_eq!(factory_for(&crossed).map(|(_, kind)| kind), cfg!(windows).then_some(NativeKind::Wasapi));
        assert_eq!(
            factory_for(&AudioConfig { backend: Backend::Cpal, ..asio }).map(|(_, kind)| kind),
            devices::NATIVE_ASIO_COMPILED.then_some(NativeKind::Asio)
        );
        assert!(!NativeKind::Wasapi.falls_back_to_cpal() && !NativeKind::Asio.falls_back_to_cpal() && NativeKind::Jack.falls_back_to_cpal());
        assert_eq!(
            NativeKind::Jack.fallback_note("JACK/PipeWire server not running"),
            "Native JACK failed: JACK/PipeWire server not running; using the cpal backend"
        );
    }
}
