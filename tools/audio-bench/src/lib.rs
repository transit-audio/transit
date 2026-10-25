//! The audio benchmark harness (ADR 0016 slice b). It drives any [`Backend`] through the trait
//! only and measures by host arrival time at callback entry: achieved period, arrival jitter,
//! xruns (steps in the arrival drift and render callbacks longer than a period), callback time and, with an output-to-input cable, the
//! loopback round trip. Results are a Markdown table (`docs/benchmarks/`).
//!
//! The callbacks (`probe`) never allocate or lock; `analysis` and `report` run after the streams
//! stop. `fake` is the device-free mode used by tests and CI.

pub mod analysis;
pub mod calibration;
pub mod cli;
pub mod fake;
pub mod fit;
pub mod midi_roundtrip;
pub mod onset;
pub mod placement;
pub mod probe;
pub mod report;
pub mod session;

use transit_audio_backend as ab;

/// Which backend a run measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The cpal adapter (WASAPI shared or ASIO, chosen with `--system`).
    Cpal,
    /// The native WASAPI backend (shared mode through `IAudioClient3`, or exclusive with
    /// `--mode exclusive`); needs the `native-wasapi` feature.
    Wasapi,
    /// The native ASIO backend (`transit-audio-asio`); needs the `native-asio` feature.
    Asio,
    /// The scripted [`ab::fake::FakeBackend`] in virtual time.
    Fake,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Cpal => "cpal",
            Kind::Wasapi => "wasapi",
            Kind::Asio => "asio",
            Kind::Fake => "fake",
        }
    }
}

/// Everything a run needs.
#[derive(Debug, Clone)]
pub struct Config {
    pub kind: Kind,
    pub system: Option<String>,
    pub request: session::Request,
    pub seconds: f64,
    pub fake: fake::Script,
    pub out: Option<std::path::PathBuf>,
    /// `ui.json` to merge the measured loopback calibration into (`--settings`).
    pub settings: Option<std::path::PathBuf>,
}

/// The period the fake mode pumps when none is requested.
pub const FAKE_DEFAULT_PERIOD: u32 = 64;

/// The round trip a run of `kind` calibrates with, as the app would: on native ASIO the click
/// loop in the driver's sample positions (what the app's Calibration Measure stores; #20, known
/// issue K31), on native WASAPI and the fake backend the round trip in the stamps' own time;
/// none on cpal, whose two streams do not stamp one clock.
pub fn app_trip(kind: Kind, measurements: &analysis::Measurements) -> Option<analysis::FrameTrip> {
    match kind {
        Kind::Asio => measurements.position_trip,
        Kind::Wasapi | Kind::Fake => measurements.frame_trip,
        Kind::Cpal => None,
    }
}

/// Run one measurement and return its table row.
pub fn run(config: &Config) -> Result<report::Row, String> {
    let shared = probe::Shared::new(config.request.rate, config.seconds);
    let (system, opened) = match config.kind {
        Kind::Fake => {
            let period = config.request.period.unwrap_or(FAKE_DEFAULT_PERIOD);
            let fake_backend = fake::backend(&config.request, period);
            let script = fake::Script { seconds: config.seconds, ..config.fake.clone() };
            // Virtual time: the body time of a callback is not a measurement here.
            shared.measure_body.store(false, std::sync::atomic::Ordering::Relaxed);
            let handle = fake_backend.clone();
            let opened = session::run(&fake_backend, &config.request, &shared, &mut || fake::drive(&handle, &config.request, period, &script))?;
            (ab::Backend::system(&fake_backend).to_string(), opened)
        }
        Kind::Asio => {
            let backend = asio_backend()?;
            let request = resolve_asio_device(&config.request)?;
            let seconds = config.seconds;
            let opened = session::run(backend.as_ref(), &request, &shared, &mut || {
                std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                Ok(())
            })?;
            (backend.system().to_string(), opened)
        }
        Kind::Wasapi => {
            let backend = wasapi_backend()?;
            let seconds = config.seconds;
            let opened = session::run(backend.as_ref(), &config.request, &shared, &mut || {
                std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                Ok(())
            })?;
            (backend.system().to_string(), opened)
        }
        Kind::Cpal => {
            let backend = cpal_backend(config.system.as_deref())?;
            let seconds = config.seconds;
            let opened = session::run(backend.as_ref(), &config.request, &shared, &mut || {
                std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                Ok(())
            })?;
            (backend.system().to_string(), opened)
        }
    };
    let measurements = analysis::measure(&shared, opened.render_rate, opened.capture_rate);
    let date = report::today();
    // Frame times of the two streams are comparable only when one backend stamps both (the native
    // one, in the shared host origin); a cpal run measures no calibration. Over native ASIO the
    // calibration is the loop by sample position ([`app_trip`]).
    let calibration = (opened.capture_rate == Some(opened.render_rate))
        .then(|| app_trip(config.kind, &measurements))
        .flatten()
        .map(|trip| calibration::figures(&trip, opened.render_rate, &measurements.input_peaks));
    let mut stored_calibration = None;
    if let Some(path) = &config.settings {
        let figures = calibration.clone().ok_or("--settings needs a native loopback run (--backend wasapi or asio, --loopback) at one rate")??;
        // On ASIO the input is the driver's own: the app's key names none.
        let input = if config.kind == Kind::Asio { None } else { config.request.in_device.as_deref() };
        let mode = calibration::mode_key(config.kind, config.request.mode);
        // On ASIO the loop belongs to the buffer the driver runs (its panel's size, a fixed-size
        // driver's one size), which the app keys on too; elsewhere to the requested period.
        let period = if config.kind == Kind::Asio { opened.render.period.frames().or(config.request.period) } else { config.request.period };
        let record = calibration::record(&figures, &opened.device, input, mode, period.unwrap_or(0), &date);
        stored_calibration = Some(calibration::stored(path, &record));
        calibration::merge_settings(path, &record)?;
    }
    Ok(report::Row {
        backend: config.kind.name().to_string(),
        system,
        device: opened.device,
        requested: config.request.period,
        mode: (config.request.mode == ab::Mode::Exclusive).then(|| report::exclusive_note(&opened.render)),
        measurements,
        calibration,
        stored_calibration,
        render_stats: opened.render_stats,
        capture_stats: opened.capture_stats,
        render_achieved: Some(opened.render),
        capture_achieved: opened.capture,
        machine: report::machine(),
        date,
    })
}

/// `--midi-roundtrip`: open the MIDI output and the native ASIO device and measure the
/// MIDI-to-audio onset latency ([`midi_roundtrip`]). A stop requested through `interrupt`
/// (Ctrl+C) ends it early, releasing the note.
pub fn run_midi_roundtrip(
    config: &Config,
    options: &midi_roundtrip::Options,
    interrupt: &midi_roundtrip::Interrupt,
) -> Result<midi_roundtrip::Report, String> {
    if config.kind != Kind::Asio {
        return Err("--midi-roundtrip needs --backend asio: it places the note and the onset by the driver's sample positions".into());
    }
    let backend = asio_backend()?;
    let request = resolve_asio_device(&config.request)?;
    let io = transit_midi_io::MidiIo::new("audio-bench");
    let mut port = midi_roundtrip::open_output(&io, &options.port)?;
    let mut driver = midi_roundtrip::RealTime { origin: ab::HostOrigin::default(), interrupt };
    midi_roundtrip::measure(backend.as_ref(), &request, &options.settings, &mut port, &mut driver, interrupt)
}

/// `--placement`: open the MIDI output and the native ASIO device and place a note pattern with
/// and without the compensation ([`placement`]). A stop requested through `interrupt` (Ctrl+C)
/// ends it early, releasing the note.
pub fn run_placement(config: &Config, options: &placement::Options, interrupt: &midi_roundtrip::Interrupt) -> Result<placement::Report, String> {
    if config.kind != Kind::Asio {
        return Err("--placement needs --backend asio: it schedules the notes and places the onsets by the driver's sample positions".into());
    }
    // Refused before any device or port opens.
    options.settings.check(config.request.rate)?;
    let backend = asio_backend()?;
    let request = resolve_asio_device(&config.request)?;
    let io = transit_midi_io::MidiIo::new("audio-bench");
    let mut port = midi_roundtrip::open_output(&io, &options.port)?;
    let mut driver = midi_roundtrip::RealTime { origin: ab::HostOrigin::default(), interrupt };
    placement::measure(backend.as_ref(), &request, &options.settings, &mut port, &mut driver, interrupt)
}

/// `args` as one shell line (an argument with a space, a quote or nothing in it quoted).
pub fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| if a.is_empty() || a.contains([' ', '"', '\t']) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The cpal backend of the audio system named (else the default); an unavailable name is an error
/// rather than the silent fallback the Player takes.
pub fn cpal_backend(system: Option<&str>) -> Result<Box<dyn ab::Backend>, String> {
    use ab::BackendFactory;
    let backend = soundcraft_playback::backend::cpal::CpalFactory.backend(system).ok_or("no audio system is available")?;
    match system {
        Some(want) if !backend.system().eq_ignore_ascii_case(want) => {
            Err(format!("audio system {want} is not available (got {}); ASIO needs a build with --features asio", backend.system()))
        }
        _ => Ok(backend),
    }
}

/// The native WASAPI backend (`--backend wasapi`).
#[cfg(all(windows, feature = "native-wasapi"))]
pub fn wasapi_backend() -> Result<Box<dyn ab::Backend>, String> {
    use ab::BackendFactory;
    transit_audio_wasapi::WasapiFactory::default().backend(None).ok_or_else(|| "the native WASAPI backend is not available".to_string())
}

/// Without the feature (or off Windows) there is no native backend to measure.
#[cfg(not(all(windows, feature = "native-wasapi")))]
pub fn wasapi_backend() -> Result<Box<dyn ab::Backend>, String> {
    Err("--backend wasapi needs Windows and a build with --features native-wasapi".into())
}

/// The native ASIO backend (`--backend asio`): the process-wide one the app's factory hands out.
#[cfg(all(windows, target_pointer_width = "64", feature = "native-asio"))]
pub fn asio_backend() -> Result<Box<dyn ab::Backend>, String> {
    use ab::BackendFactory;
    transit_audio_asio::AsioFactory.backend(None).ok_or_else(|| "the native ASIO backend is not available".to_string())
}

/// `--device` of `--backend asio` as an installed driver's registry name: the exact name, else the
/// one that matches ignoring case or contains it ("model mixer"), said on stderr. An unknown or
/// ambiguous name is an error listing the installed drivers; the default driver never stands in.
#[cfg(all(windows, target_pointer_width = "64", feature = "native-asio"))]
pub fn resolve_asio_device(request: &session::Request) -> Result<session::Request, String> {
    let Some(wanted) = request.device.as_deref() else { return Ok(request.clone()) };
    let backend = transit_audio_asio::AsioBackend::shared().map_err(|e| format!("the native ASIO backend did not start: {e}"))?;
    let installed = backend.driver_names().map_err(|e| format!("the ASIO drivers could not be listed: {e}"))?;
    let resolved = transit_audio_asio::resolve_driver_name(&installed, wanted).map_err(|e| format!("--device: {e}"))?;
    if !resolved.exact {
        eprintln!("--device \"{wanted}\" is the ASIO driver \"{}\"", resolved.name);
    }
    Ok(session::Request { device: Some(resolved.name), ..request.clone() })
}

/// Without the native ASIO backend there is no driver to resolve (`asio_backend` says why).
#[cfg(not(all(windows, target_pointer_width = "64", feature = "native-asio")))]
pub fn resolve_asio_device(request: &session::Request) -> Result<session::Request, String> {
    Ok(request.clone())
}

/// Without the feature (or off 64-bit Windows) there is no native ASIO backend to measure.
#[cfg(not(all(windows, target_pointer_width = "64", feature = "native-asio")))]
pub fn asio_backend() -> Result<Box<dyn ab::Backend>, String> {
    Err("--backend asio needs 64-bit Windows and a build with --features native-asio".into())
}

/// The device names of a cpal system, for `--list-devices` (enumerates; opens no stream); the JACK
/// system through the native JACK backend ([`list_jack`]).
pub fn list_devices(system: Option<&str>) -> Result<String, String> {
    if soundcraft_playback::devices::is_jack_system(system) {
        return list_jack();
    }
    let backend = cpal_backend(system)?;
    let mut text = format!("system: {}\n", backend.system());
    for (label, direction) in [("outputs", ab::Direction::Output), ("inputs", ab::Direction::Input)] {
        text.push_str(&format!("{label}:\n"));
        for endpoint in backend.endpoints(direction).map_err(|e| e.to_string())? {
            text.push_str(&format!("  {} ({} ch)\n", endpoint.name, endpoint.inputs.max(endpoint.outputs)));
        }
    }
    Ok(text)
}

/// The JACK system as the native JACK backend lists it (`--backend jack --list-devices`): each port
/// group with its ports, rate and quantum, or why the system is unavailable. Opens a short-lived
/// probe client only; never starts a server and opens no stream.
pub fn list_jack() -> Result<String, String> {
    let system = soundcraft_playback::devices::native_jack_system()
        .ok_or("--backend jack needs Linux and a build with --features native-jack (cargo build -p audio-bench --features native-jack)")?;
    let mut text = format!("system: {} (native)\n", system.name);
    // Without a device the diagnostic says why the system is unavailable; with one, each group's
    // note names the server and where its rate and buffer are set.
    let tag = if system.outputs.is_empty() && system.inputs.is_empty() { "unavailable" } else { "note" };
    for diagnostic in &system.diagnostics {
        text.push_str(&format!("{tag}: {diagnostic}\n"));
    }
    for (label, devices) in [("outputs", &system.outputs), ("inputs", &system.inputs)] {
        text.push_str(&format!("{label}:\n"));
        for d in devices {
            let rate = d.sample_rates.first().map_or_else(|| "rate unknown".to_string(), |r| format!("{r} Hz"));
            let quantum = d.buffer_min.map_or_else(|| "quantum unknown".to_string(), |b| format!("quantum {b}"));
            text.push_str(&format!("  {} ({} ch, {rate}, {quantum})\n", d.name, d.max_channels));
        }
    }
    Ok(text)
}
