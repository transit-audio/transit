//! Audio device selection: which audio system (cpal host), output and input device, sample rate
//! and buffer size to open. The choice is per machine (the app keeps it with its preferences, not
//! in sessions). Anything that cannot be honoured falls back to the system default: a device that
//! is unplugged, a rate or buffer size the device does not offer.
//!
//! The cpal-typed helpers (sample-format preference, device description and lookup, the shared
//! hosts, the buffer-size rule) live in the cpal adapter, `backend::cpal` (ADR 0046); this module
//! keeps the platform-neutral types and the enumeration of audio systems.

use crate::backend::cpal::{describe, device_name, shared_host};
use cpal::traits::HostTrait;

/// Rates offered when a device supports them (any rate inside a supported range is possible, but
/// these are the ones interfaces actually run at).
pub const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered when the device reports a range that contains them.
pub const STANDARD_BUFFERS: [u32; 9] = [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

/// Which implementation talks to the audio system (ADR 0016). On Windows the saved value does not
/// decide: the audio system does ([`Backend::effective`], ADR 0046).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// The app's choice: on Windows the native backend of the audio system; elsewhere cpal (native
    /// JACK on Linux when a server answers, see below).
    #[default]
    Auto,
    /// cpal. Off Windows only; kept on every platform so a saved `"cpal"` still parses, and on
    /// Windows a loaded `Cpal` reads as `Auto` ([`Backend::loaded`], ADR 0046 decision 2).
    Cpal,
    /// The native WASAPI backend (shared mode by default, exclusive on request; ADR 0016). On
    /// Windows it serves every audio system but ASIO, whatever was saved; a failed open is an error,
    /// never another backend (ADR 0046).
    NativeWasapi,
    /// The native ASIO backend (`transit-audio-asio`, draft ADR 0016 amendment). On 64-bit Windows it
    /// serves the ASIO system, whatever was saved; a failed open is an error (ADR 0046).
    NativeAsio,
    /// The native JACK backend (`transit-audio-jack`, ADR 0021): JACK 2 or PipeWire through the JACK
    /// client API. Used only on the JACK system of a Linux build with the `native-jack` feature;
    /// otherwise, or if it fails to open, cpal. `Auto` opens it too when a JACK or PipeWire-JACK
    /// server answers, on the JACK system or the untouched default (ADR 0021, slice 2 amendment);
    /// otherwise `Auto` is cpal (ALSA) on Linux.
    NativeJack,
}

impl Backend {
    pub const ALL: [Backend; 5] = [Backend::Auto, Backend::Cpal, Backend::NativeWasapi, Backend::NativeAsio, Backend::NativeJack];

    /// The label of the Audio Devices selector.
    pub const fn label(self) -> &'static str {
        match self {
            Backend::Auto => "Auto",
            Backend::Cpal => "cpal (legacy)",
            Backend::NativeWasapi => "Native WASAPI",
            Backend::NativeAsio => "Native ASIO",
            Backend::NativeJack => "Native JACK",
        }
    }

    /// The choices the Backend selector offers on `system`: Auto, cpal, and the native backend of
    /// that system where this build has it.
    pub fn offered(system: Option<&str>) -> Vec<Backend> {
        Self::ALL.into_iter().filter(|b| !b.is_native() || b.native_on(system)).collect()
    }

    /// The backend that serves `system` (ADR 0046, decision 1): on Windows `NativeAsio` for the ASIO
    /// system and `NativeWasapi` for any other (the untouched default, `None`, too), for every saved
    /// value; elsewhere the saved value itself (ADR 0021's JACK rule stays in `factory_for`). Every
    /// site that asks which native backend runs reads this, not the saved value.
    pub fn effective(self, system: Option<&str>) -> Backend {
        self.effective_on(system, cfg!(windows))
    }

    /// [`Backend::effective`] with the platform as a parameter (`windows`), for tests on every platform.
    pub fn effective_on(self, system: Option<&str>, windows: bool) -> Backend {
        match (windows, is_asio_system(system)) {
            (false, _) => self,
            (true, true) => Backend::NativeAsio,
            (true, false) => Backend::NativeWasapi,
        }
    }

    /// How a saved value loads (ADR 0046, decision 2's one migration): on Windows `Cpal` reads as
    /// `Auto`, and is saved as `auto` on the next save; every other value, and every value off
    /// Windows, as saved. The saved value is never rewritten to a native variant.
    pub fn loaded(self) -> Backend {
        self.loaded_on(cfg!(windows))
    }

    /// [`Backend::loaded`] with the platform as a parameter, for tests on every platform.
    pub fn loaded_on(self, windows: bool) -> Backend {
        if windows && self == Backend::Cpal { Backend::Auto } else { self }
    }

    /// Whether this is one of the native backends.
    pub const fn is_native(self) -> bool {
        matches!(self, Backend::NativeWasapi | Backend::NativeAsio | Backend::NativeJack)
    }

    /// Whether this choice opens a native backend on `system` in this build.
    pub fn native_on(self, system: Option<&str>) -> bool {
        match self {
            Backend::NativeWasapi => native_wasapi_offered(system),
            Backend::NativeAsio => native_asio_offered(system),
            Backend::NativeJack => native_jack_offered(system),
            Backend::Auto | Backend::Cpal => false,
        }
    }
}

/// Whether this build can open the native WASAPI backend: every Windows build (ADR 0046).
pub const NATIVE_WASAPI_COMPILED: bool = cfg!(windows);
/// Whether this build can open the native ASIO backend: every 64-bit Windows build (ADR 0046).
pub const NATIVE_ASIO_COMPILED: bool = cfg!(all(windows, target_pointer_width = "64"));
/// Whether this build can open the native JACK backend (Linux with the `native-jack` feature).
pub const NATIVE_JACK_COMPILED: bool = cfg!(all(target_os = "linux", feature = "native-jack"));
/// The native JACK backend's audio system (JACK 2, or PipeWire through its JACK library).
pub const JACK_SYSTEM: &str = "JACK";

/// Whether `system` (the choice's audio system; `None` is the platform default) is WASAPI.
pub fn is_wasapi_system(system: Option<&str>) -> bool {
    system.map_or(cfg!(windows), |s| s.eq_ignore_ascii_case("WASAPI"))
}

/// Whether `system` is ASIO (never the default system).
pub fn is_asio_system(system: Option<&str>) -> bool {
    system.is_some_and(|s| s.eq_ignore_ascii_case(ASIO_SYSTEM))
}

/// Whether `system` is JACK (never the default system).
pub fn is_jack_system(system: Option<&str>) -> bool {
    system.is_some_and(|s| s.eq_ignore_ascii_case(JACK_SYSTEM))
}

/// Whether the native WASAPI choice applies: the feature is compiled in and the system is WASAPI.
pub fn native_wasapi_offered(system: Option<&str>) -> bool {
    NATIVE_WASAPI_COMPILED && is_wasapi_system(system)
}

/// Whether the native ASIO choice applies: the feature is compiled in and the system is ASIO.
pub fn native_asio_offered(system: Option<&str>) -> bool {
    NATIVE_ASIO_COMPILED && is_asio_system(system)
}

/// Whether the native JACK choice applies: the feature is compiled in and the system is JACK.
pub fn native_jack_offered(system: Option<&str>) -> bool {
    NATIVE_JACK_COMPILED && is_jack_system(system)
}

/// Whether the Backend selector is shown for `system`: a native backend can serve it in this build.
pub fn native_offered(system: Option<&str>) -> bool {
    native_wasapi_offered(system) || native_asio_offered(system) || native_jack_offered(system)
}

/// The user's audio device choice. Every field is optional: `None` means "the system default".
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Which backend opens the device: on Windows the audio system decides ([`Backend::effective`]);
    /// elsewhere a native one where it serves the chosen system ([`Backend::native_on`]), else cpal.
    /// A saved `cpal` loads as `Auto` on Windows ([`Backend::loaded`]).
    #[serde(deserialize_with = "load_backend")]
    pub backend: Backend,
    /// Audio system (cpal host name: "WASAPI", "ASIO", "CoreAudio", "ALSA", "JACK", ...).
    pub system: Option<String>,
    pub output: Option<String>,
    pub input: Option<String>,
    /// The native WASAPI backend opens this input (endpoint id or name) together with the output
    /// for duplex recording, for interfaces whose endpoints share no container id. `None` pairs
    /// by container id alone.
    pub input_pairing: Option<String>,
    /// Device sample rate; the mix resamples when it differs from the session's.
    pub sample_rate: Option<u32>,
    /// Device buffer in frames.
    pub buffer_size: Option<u32>,
    /// Loopback calibrations of device pairs (`audio-bench --loopback --settings`, and the
    /// Calibration line's Measure over native ASIO). Only the native backends apply one, and only
    /// on an exact key match.
    pub calibrations: Vec<LoopbackCalibration>,
    /// The output path the native ASIO calibration's Measure plays its click on (the user's last
    /// choice; `None` is the main output's path).
    pub calibration_send: Option<String>,
    /// The input path it listens on (`None` is the first input path).
    pub calibration_return: Option<String>,
    /// The native WASAPI backend opens the device shared (through the Windows mixer), which is its
    /// default; false asks for exclusive mode (ADR 0016, amendment 2 of 2026-10-22). Missing in a
    /// saved config reads as true: shared.
    pub share_device: bool,
    /// System Audio (#268): what the computer plays, as a stereo input path ("System Audio")
    /// beside the interface's inputs. Windows only (a WASAPI loopback capture); off by default.
    pub system_audio: bool,
    /// The output System Audio captures (its name or id); `None` is the system's default output.
    pub system_audio_output: Option<String>,
}

/// The saved backend through [`Backend::loaded`].
fn load_backend<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Backend, D::Error> {
    <Backend as serde::Deserialize>::deserialize(deserializer).map(Backend::loaded)
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            system: None,
            output: None,
            input: None,
            input_pairing: None,
            sample_rate: None,
            buffer_size: None,
            calibrations: Vec::new(),
            calibration_send: None,
            calibration_return: None,
            share_device: true,
            system_audio: false,
            system_audio_output: None,
        }
    }
}

/// The `mode` key of a shared-mode calibration.
pub const MODE_SHARED: &str = "shared";
/// The `mode` key of an exclusive-mode calibration (`audio-bench --mode exclusive`).
pub const MODE_EXCLUSIVE: &str = "exclusive";
/// The `mode` key of a calibration of a native ASIO driver (its device is the driver's registry
/// name; ASIO has one mode).
pub const MODE_ASIO: &str = "asio";

/// The `mode` key a stream of `system` opened in `mode` is calibrated under: [`MODE_ASIO`] on ASIO,
/// else the WASAPI mode; `None` for a mode no calibration names.
pub fn calibration_mode(system: &str, mode: transit_audio_backend::Mode) -> Option<&'static str> {
    if system.eq_ignore_ascii_case(ASIO_SYSTEM) {
        return Some(MODE_ASIO);
    }
    match mode {
        transit_audio_backend::Mode::Shared => Some(MODE_SHARED),
        transit_audio_backend::Mode::Exclusive => Some(MODE_EXCLUSIVE),
        _ => None,
    }
}

/// One measured loopback of an output/input pair. The key is `(device, input, mode, rate,
/// period)`; a calibration applies to a stream pair only when every part of the key is equal,
/// because the round trip changes with any of them. Under [`MODE_ASIO`] the round trip is the
/// whole loop in the driver's sample positions (the app's Measure, `audio-bench --backend asio`),
/// under the WASAPI modes the delay the stamps do not account for (see
/// [`transit_audio_backend::Calibration`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoopbackCalibration {
    /// The output device name, as the backend lists it.
    pub device: String,
    /// The paired input: the endpoint name the run named (`audio-bench --in-device`), or `None`
    /// when it named none. It matches a config's pairing by [`input_matches`]: the same name
    /// (case-insensitive), both unnamed, or an automatic pairing that resolved to this endpoint.
    pub input: Option<String>,
    /// How the device was opened ([`MODE_SHARED`], [`MODE_EXCLUSIVE`] or [`MODE_ASIO`]).
    pub mode: String,
    pub rate: u32,
    /// The requested period in frames; 0 is the driver's default.
    pub period: u32,
    /// The median round trip, in frames at `rate` (a loop that comes up in two phases, one
    /// period apart, keeps the median of the run; the spread shows the other).
    pub round_trip_frames: u32,
    /// The uncertainty of `round_trip_frames`, in frames: the measured spread plus one frame of
    /// position granularity.
    pub spread_frames: u32,
    /// UTC date of the measurement (`YYYY-MM-DD`).
    pub measured_at: String,
}

impl LoopbackCalibration {
    /// Whether `other` measured the same device pair under the same conditions.
    pub fn same_key(&self, other: &Self) -> bool {
        self.device.eq_ignore_ascii_case(&other.device)
            && self.input.as_deref().map(str::to_lowercase) == other.input.as_deref().map(str::to_lowercase)
            && self.mode == other.mode
            && self.rate == other.rate
            && self.period == other.period
    }

    /// The calibration the backend applies: the round trip and its bound in nanoseconds.
    pub fn to_backend(&self) -> Option<transit_audio_backend::Calibration> {
        if self.rate == 0 {
            return None;
        }
        let ns = |frames: u32| u64::from(frames) * 1_000_000_000 / u64::from(self.rate);
        Some(transit_audio_backend::Calibration { round_trip_ns: ns(self.round_trip_frames), uncertainty_ns: ns(self.spread_frames) })
    }
}

/// The key a calibration of an open stream pair is looked up and stored under (the parts of
/// [`LoopbackCalibration`] before its figures): what the Player matched, or would match, at the
/// open.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CalibrationKey {
    pub device: String,
    pub input: Option<String>,
    pub mode: String,
    pub rate: u32,
    /// The requested period in frames; 0 is the driver's default.
    pub period: u32,
}

impl CalibrationKey {
    /// The key of the pair of audio system `system` opened on output `device` with `request`,
    /// paired with `input` (`None` on ASIO, whose input is the driver's own); `None` for a mode
    /// no calibration names.
    pub fn of(system: &str, device: &str, input: Option<&str>, request: &transit_audio_backend::OpenRequest) -> Option<Self> {
        Some(Self {
            device: device.to_owned(),
            input: input.map(str::to_owned),
            mode: calibration_mode(system, request.mode)?.to_owned(),
            rate: request.format.rate,
            period: request.period.frames().unwrap_or(0),
        })
    }

    /// The record of a loop measured under this key.
    pub fn record(&self, round_trip_frames: u32, spread_frames: u32, measured_at: &str) -> LoopbackCalibration {
        LoopbackCalibration {
            device: self.device.clone(),
            input: self.input.clone(),
            mode: self.mode.clone(),
            rate: self.rate,
            period: self.period,
            round_trip_frames,
            spread_frames,
            measured_at: measured_at.to_owned(),
        }
    }
}

/// Today's date in UTC as `YYYY-MM-DD`, for [`LoopbackCalibration::measured_at`].
pub fn utc_today() -> String {
    utc_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// The UTC date of `secs` since the Unix epoch, as `YYYY-MM-DD`.
pub fn utc_date(secs: u64) -> String {
    let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Whether a calibration whose run named `recorded` measured the pair a config pairs as `pairing`:
/// both unnamed, both naming the same endpoint (case-insensitive), or an automatic pairing
/// (`pairing` is `None`) that resolved to the endpoint `recorded` names (`resolved`).
pub fn input_matches(recorded: Option<&str>, pairing: Option<&str>, resolved: Option<&str>) -> bool {
    match (recorded, pairing) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (Some(a), None) => resolved.is_some_and(|r| a.eq_ignore_ascii_case(r)),
        (None, Some(_)) => false,
    }
}

impl AudioConfig {
    /// Keep `calibration`, replacing a saved one with the same key.
    pub fn store_calibration(&mut self, calibration: LoopbackCalibration) {
        match self.calibrations.iter_mut().find(|c| c.same_key(&calibration)) {
            Some(saved) => *saved = calibration,
            None => self.calibrations.push(calibration),
        }
    }

    /// The saved calibrations of the native ASIO driver `driver` at any rate or period (for "not
    /// calibrated at this buffer size").
    pub fn asio_calibrations<'a>(&'a self, driver: &'a str) -> impl Iterator<Item = &'a LoopbackCalibration> + 'a {
        self.calibrations.iter().filter(move |c| c.mode == MODE_ASIO && c.device.eq_ignore_ascii_case(driver))
    }

    /// The saved calibration of the pair opened on output `device` in `mode` ([`MODE_SHARED`] or
    /// [`MODE_EXCLUSIVE`]) at `rate` and the requested `period` (0 is the driver's default), under
    /// this config's paired input.
    pub fn find_calibration(&self, device: &str, mode: &str, rate: u32, period: u32) -> Option<&LoopbackCalibration> {
        self.find_calibration_paired(device, None, mode, rate, period)
    }

    /// [`Self::find_calibration`] when the automatic pairing is known to resolve to the input
    /// `paired_input` (see [`input_matches`]).
    pub fn find_calibration_paired(
        &self,
        device: &str,
        paired_input: Option<&str>,
        mode: &str,
        rate: u32,
        period: u32,
    ) -> Option<&LoopbackCalibration> {
        self.calibrations.iter().find(|c| {
            c.device.eq_ignore_ascii_case(device)
                && input_matches(c.input.as_deref(), self.input_pairing.as_deref(), paired_input)
                && c.mode == mode
                && c.rate == rate
                && c.period == period
        })
    }

    /// The calibration for the pair opened on output `device` of audio system `system` with
    /// `request` (the mode key is [`calibration_mode`]'s); `None` when no saved calibration has
    /// exactly that key.
    pub fn calibration_for(
        &self,
        system: &str,
        device: &str,
        paired_input: Option<&str>,
        request: &transit_audio_backend::OpenRequest,
    ) -> Option<transit_audio_backend::Calibration> {
        let mode = calibration_mode(system, request.mode)?;
        self.find_calibration_paired(device, paired_input, mode, request.format.rate, request.period.frames().unwrap_or(0))?.to_backend()
    }
}

/// One device as the settings window lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Most channels any usable configuration offers.
    pub max_channels: u16,
    /// Standard rates the device accepts (in a format the streams convert).
    pub sample_rates: Vec<u32>,
    /// Buffer range the device reports (`None` when it does not say).
    pub buffer_min: Option<u32>,
    pub buffer_max: Option<u32>,
}

impl DeviceInfo {
    /// Standard buffer sizes inside the device's range (all of them when the range is unknown).
    pub fn buffer_sizes(&self) -> Vec<u32> {
        STANDARD_BUFFERS.iter().copied().filter(|b| self.buffer_min.is_none_or(|m| *b >= m) && self.buffer_max.is_none_or(|m| *b <= m)).collect()
    }
}

/// What the listing of an ASIO system knows of one driver in one direction ([`asio_description`]).
/// Only the driver that runs is probed (probing opens a driver, and ASIO allows one per process,
/// #108); every other installed driver is listed by name with the standard choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsioDescription {
    /// Probed: the channels, rates and buffer range are the driver's.
    Probed,
    /// Probed, and the driver has no channels in this direction (an output-only driver).
    NoChannels,
    /// Its probe failed; the system's diagnostics say why.
    ProbeFailed,
    /// Never opened: the standard choices stand in for its rates and buffers.
    NotProbed,
}

/// How `system`'s listing describes `device` when it is an ASIO system (`None` for any other):
/// the one rule the Audio Devices window's "updates after Apply" and the diagnostics report share
/// (#127). A device with channels was probed; one without was probed when the same driver has
/// channels in the other direction, failed when a diagnostic names it ("<driver>: ..."), and was
/// never opened otherwise.
pub fn asio_description(system: &AudioSystem, device: &DeviceInfo) -> Option<AsioDescription> {
    if !is_asio_system(Some(&system.name)) {
        return None;
    }
    let named = |d: &&DeviceInfo| d.name == device.name && d.max_channels > 0;
    let failed = system.diagnostics.iter().any(|n| n.strip_prefix(device.name.as_str()).is_some_and(|rest| rest.starts_with(": ")));
    Some(if device.max_channels > 0 {
        AsioDescription::Probed
    } else if system.outputs.iter().chain(&system.inputs).any(|d| named(&d)) {
        AsioDescription::NoChannels
    } else if failed {
        AsioDescription::ProbeFailed
    } else {
        AsioDescription::NotProbed
    })
}

/// An audio system and its devices.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AudioSystem {
    pub name: String,
    pub is_default: bool,
    pub outputs: Vec<DeviceInfo>,
    pub inputs: Vec<DeviceInfo>,
    /// Devices of the system that could not be listed, with why ("<driver>: <problem>"): the
    /// native ASIO backend's per-driver diagnostics. Empty for cpal's systems.
    pub diagnostics: Vec<String>,
}

impl AudioSystem {
    /// The diagnostics that name no device of this system (no "<device>: " prefix): notes about
    /// the system as a whole, such as the JACK server's rate and buffer (#146).
    pub fn system_notes(&self) -> impl Iterator<Item = &str> {
        self.diagnostics
            .iter()
            .map(String::as_str)
            .filter(|n| !self.outputs.iter().chain(&self.inputs).any(|d| n.strip_prefix(d.name.as_str()).is_some_and(|rest| rest.starts_with(": "))))
    }
}

/// The name of the ASIO audio system, served by the native ASIO backend (cpal has no ASIO host here).
pub const ASIO_SYSTEM: &str = "ASIO";

/// The name of the WASAPI audio system: the same string in cpal's listing and the native backend's.
pub const WASAPI_SYSTEM: &str = "WASAPI";

/// Opens the control panel ("Driver settings") of the ASIO driver `driver` through the native
/// backend: the call runs on its host thread, from a thread of its own, and the receiver gets the
/// result when the panel closes. Drivers do not reliably report what changed there: reopen the
/// device afterwards. Fails at once in a build without the native ASIO backend.
pub fn native_asio_control_panel(driver: &str) -> Result<std::sync::mpsc::Receiver<Result<(), String>>, String> {
    #[cfg(all(windows, target_pointer_width = "64"))]
    return crate::backend::native::asio_control_panel(driver);
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    {
        let _ = driver;
        Err("this build has no native ASIO backend".into())
    }
}

/// Every audio system on this machine with its devices. Enumerating can take a moment (some
/// systems probe each device): call it when the settings window opens, not every frame. In a build
/// with the native ASIO backend, the ASIO system is listed through that backend
/// ([`native_asio_system`], with no driver selected: the registry names only), not cpal's ASIO
/// host, whichever backend then opens it: the driver names are the same registry names.
pub fn audio_systems() -> Vec<AudioSystem> {
    let mut systems = audio_systems_without_asio();
    systems.extend(native_asio_system(None));
    systems
}

/// [`audio_systems`] without the ASIO system: what the Audio Devices window lists at once (it lists
/// ASIO on a worker thread, probing the selected driver). In a Windows build with the native WASAPI
/// backend the WASAPI system is the native one (`backend::native::wasapi_system`, ADR 0046
/// decision 2); elsewhere cpal's systems, then native JACK where built.
pub fn audio_systems_without_asio() -> Vec<AudioSystem> {
    #[cfg(windows)]
    let mut systems = vec![crate::backend::native::wasapi_system()];
    #[cfg(not(windows))]
    let mut systems = cpal_audio_systems();
    systems.extend(native_jack_system());
    systems
}

/// What listed `system` in [`audio_systems`] in this build, for the diagnostics report: the native
/// backend's enumerator where one lists it, else cpal.
pub fn enumerator_of(system: &str) -> &'static str {
    if NATIVE_WASAPI_COMPILED && system == WASAPI_SYSTEM {
        "native WASAPI (IMMDeviceEnumerator)"
    } else if NATIVE_ASIO_COMPILED && system == ASIO_SYSTEM {
        "native ASIO (driver registry)"
    } else if NATIVE_JACK_COMPILED && system == JACK_SYSTEM {
        "native JACK (port groups through a probe client)"
    } else {
        "cpal"
    }
}

/// The MIDI ports present now as `(name, input)`, inputs first, named as the MIDI menus name them:
/// enumeration only, no port is opened (`transit --report`, #127). `Err` says why they could not
/// be listed.
pub fn midi_ports() -> Result<Vec<(String, bool)>, String> {
    let io = transit_midi_io::MidiIo::new("transit");
    let inputs = io.list_inputs().map_err(|e| e.to_string())?;
    let outputs = io.list_outputs().map_err(|e| e.to_string())?;
    Ok(inputs.into_iter().map(|p| (p.display_name, true)).chain(outputs.into_iter().map(|p| (p.display_name, false))).collect())
}

/// The JACK system as the native JACK backend lists it, `None` in a build without it: its port
/// groups (one per device), or no device and why (the library did not load, or no server answers).
/// Lists through a short-lived probe client; never starts a server.
pub fn native_jack_system() -> Option<AudioSystem> {
    #[cfg(all(target_os = "linux", feature = "native-jack"))]
    return Some(crate::backend::native::jack_system());
    #[cfg(not(all(target_os = "linux", feature = "native-jack")))]
    None
}

/// The ASIO system as the native ASIO backend lists it, `None` in a build without it: every
/// installed driver by name, and only `selected` probed for its channels, rates and buffers (see
/// `backend::native::asio_system`). Probing a misbehaving driver can take seconds (each host
/// request waits at most ten): call it off the UI thread.
pub fn native_asio_system(selected: Option<&str>) -> Option<AudioSystem> {
    #[cfg(all(windows, target_pointer_width = "64"))]
    return Some(crate::backend::native::asio_system(selected));
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    {
        let _ = selected;
        None
    }
}

/// The audio systems cpal lists (every one but the ASIO system in a build with the native ASIO
/// backend, which [`native_asio_system`] lists). In a Windows build with the native WASAPI backend
/// only the name-equality test calls it (ADR 0046, decision 2).
pub fn cpal_audio_systems() -> Vec<AudioSystem> {
    let default_id = cpal::default_host().id();
    cpal::available_hosts()
        .into_iter()
        .filter(|id| !(NATIVE_ASIO_COMPILED && id.name() == ASIO_SYSTEM))
        .filter_map(|id| {
            let host = shared_host(id)?;
            let def_out = host.default_output_device().as_ref().and_then(device_name);
            let def_in = host.default_input_device().as_ref().and_then(device_name);
            let outputs: Vec<DeviceInfo> =
                host.output_devices().map(|ds| ds.filter_map(|d| describe(&d, def_out.as_deref(), true)).collect()).unwrap_or_default();
            let inputs: Vec<DeviceInfo> =
                host.input_devices().map(|ds| ds.filter_map(|d| describe(&d, def_in.as_deref(), false)).collect()).unwrap_or_default();
            let mut system = AudioSystem { name: id.name().to_string(), is_default: id == default_id, outputs, inputs, diagnostics: Vec::new() };
            // PipeWire's ALSA plug-in (#230): its devices offer the buffers whose periods the
            // server allows, with the server's quantum under them.
            if system.name.eq_ignore_ascii_case("ALSA")
                && system.outputs.iter().chain(&system.inputs).any(|d| crate::pipewire::routed(&d.name))
                && let Some(clock) = crate::pipewire::server_clock()
            {
                crate::pipewire::apply_to_alsa(&mut system, &clock, crate::pipewire::default_is_pipewire());
            }
            Some(system)
        })
        .collect()
}

/// Adds the installed ASIO drivers cpal could not list. cpal loads each driver to describe it, and
/// the SDK cannot load one while another is open: without this, only the open driver showed and
/// switching to another meant going through a different audio system first. Like REAPER, every
/// installed driver is offered; one that turns out to be absent falls back to the default.
pub(crate) fn add_unlisted(list: &mut Vec<DeviceInfo>, installed: &[String]) {
    for name in installed {
        if !list.iter().any(|d| &d.name == name) {
            list.push(DeviceInfo {
                name: name.clone(),
                is_default: false,
                max_channels: 0,
                // Not known until the driver is open: offer the standard choices.
                sample_rates: STANDARD_RATES.to_vec(),
                buffer_min: None,
                buffer_max: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_jack_backend_is_labeled_native_jack() {
        assert_eq!(Backend::NativeJack.label(), "Native JACK");
    }

    fn sample_calibration() -> LoopbackCalibration {
        LoopbackCalibration {
            device: "Speakers (Scarlett 2i2)".into(),
            input: Some("Line (Scarlett 2i2)".into()),
            mode: MODE_SHARED.into(),
            rate: 48_000,
            period: 128,
            round_trip_frames: 480,
            spread_frames: 25,
            measured_at: "2026-10-21".into(),
        }
    }

    fn request(rate: u32, period: transit_audio_backend::Period) -> transit_audio_backend::OpenRequest {
        use transit_audio_backend as ab;
        ab::OpenRequest {
            format: ab::Format::new(rate, 2, ab::SampleFormat::F32),
            period,
            mode: ab::Mode::Shared,
            origin: ab::HostOrigin::default(),
            calibration: None,
        }
    }

    #[test]
    fn a_calibration_applies_only_to_its_exact_key() {
        use transit_audio_backend::Period;
        let cal = sample_calibration();
        let cfg = AudioConfig { input_pairing: cal.input.clone(), calibrations: vec![cal.clone()], ..Default::default() };
        let found = cfg.calibration_for("WASAPI", "speakers (scarlett 2i2)", None, &request(48_000, Period::Frames(128))).expect("the key matches");
        assert_eq!((found.round_trip_ns, found.uncertainty_ns), (10_000_000, 520_833));
        let device = "Speakers (Scarlett 2i2)";
        assert!(cfg.calibration_for("WASAPI", device, None, &request(44_100, Period::Frames(128))).is_none(), "another rate");
        assert!(cfg.calibration_for("WASAPI", device, None, &request(48_000, Period::Frames(64))).is_none(), "another period");
        assert!(cfg.calibration_for("WASAPI", device, None, &request(48_000, Period::Default)).is_none(), "the default period is not 128");
        assert!(cfg.calibration_for("WASAPI", "Headphones", None, &request(48_000, Period::Frames(128))).is_none(), "another device");
        let other_input = AudioConfig { input_pairing: Some("Mic".into()), ..cfg.clone() };
        assert!(other_input.calibration_for("WASAPI", device, None, &request(48_000, Period::Frames(128))).is_none(), "another input");
        let unpaired = AudioConfig { input_pairing: None, ..cfg };
        assert!(
            unpaired.calibration_for("WASAPI", device, None, &request(48_000, Period::Frames(128))).is_none(),
            "container-id pairing is its own key"
        );
    }

    #[test]
    fn a_calibration_matches_the_pairing_by_name_or_by_the_resolved_automatic_input() {
        use transit_audio_backend::Period;
        let req = request(48_000, Period::Frames(128));
        let device = "Speakers (Scarlett 2i2)";
        let named = LoopbackCalibration { input: Some("Microphone (Model 12)".into()), ..sample_calibration() };
        let unnamed = LoopbackCalibration { input: None, ..sample_calibration() };
        let with = |cal: &LoopbackCalibration, pairing: Option<&str>| AudioConfig {
            input_pairing: pairing.map(str::to_owned),
            calibrations: vec![cal.clone()],
            ..Default::default()
        };
        assert!(with(&unnamed, None).calibration_for("WASAPI", device, None, &req).is_some(), "both unnamed");
        assert!(with(&named, Some("microphone (model 12)")).calibration_for("WASAPI", device, None, &req).is_some(), "the same name, any case");
        assert!(
            with(&named, None).calibration_for("WASAPI", device, Some("MICROPHONE (Model 12)"), &req).is_some(),
            "automatic resolved to the named input"
        );
        assert!(with(&named, None).calibration_for("WASAPI", device, Some("Line (Other)"), &req).is_none(), "automatic resolved to another input");
        assert!(with(&named, None).calibration_for("WASAPI", device, None, &req).is_none(), "automatic with nothing resolved");
        assert!(
            with(&unnamed, Some("Mic")).calibration_for("WASAPI", device, Some("Mic"), &req).is_none(),
            "a named pairing never matches an unnamed run"
        );
    }

    #[test]
    fn calibrations_survive_serde_and_old_files_have_none() {
        let cfg = AudioConfig { calibrations: vec![sample_calibration()], ..Default::default() };
        let text = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<AudioConfig>(&text).unwrap().calibrations, cfg.calibrations);
        assert!(serde_json::from_str::<AudioConfig>(r#"{"output":"X"}"#).unwrap().calibrations.is_empty());
    }

    #[test]
    fn config_round_trips_and_tolerates_missing_fields() {
        let c = AudioConfig {
            system: Some("WASAPI".into()),
            output: Some("Speakers".into()),
            input: None,
            input_pairing: Some("{0.0.1.00000000}.{abc}".into()),
            sample_rate: Some(48_000),
            buffer_size: Some(256),
            backend: Backend::NativeWasapi,
            calibrations: vec![sample_calibration()],
            calibration_send: Some("Out 3-4".into()),
            calibration_return: Some("In 2".into()),
            share_device: true,
            system_audio: true,
            system_audio_output: Some("Speakers".into()),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<AudioConfig>(&s).unwrap(), c);
        assert_eq!(serde_json::from_str::<AudioConfig>("{}").unwrap(), AudioConfig::default());
        assert_eq!(serde_json::from_str::<AudioConfig>("{}").unwrap().backend, Backend::Auto, "settings saved before the field exist stay on Auto");
        assert!(s.contains("\"native_wasapi\""));
        assert_eq!(serde_json::from_str::<AudioConfig>(r#"{"output":"X","unknown":1}"#).unwrap().output.as_deref(), Some("X"));
    }

    #[test]
    fn the_share_checkbox_defaults_to_shared_and_round_trips() {
        assert!(AudioConfig::default().share_device, "shared is the default");
        assert!(serde_json::from_str::<AudioConfig>(r#"{"output":"X"}"#).unwrap().share_device, "a config saved before the field exists is shared");
        let exclusive = AudioConfig { share_device: false, ..Default::default() };
        let text = serde_json::to_string(&exclusive).unwrap();
        assert!(!serde_json::from_str::<AudioConfig>(&text).unwrap().share_device);
    }

    #[test]
    fn a_calibration_is_keyed_by_the_requests_mode() {
        use transit_audio_backend::{Mode, Period};
        let shared = sample_calibration();
        let exclusive = LoopbackCalibration { mode: MODE_EXCLUSIVE.into(), round_trip_frames: 960, ..shared.clone() };
        let cfg = AudioConfig { input_pairing: shared.input.clone(), calibrations: vec![shared, exclusive], ..Default::default() };
        let device = "Speakers (Scarlett 2i2)";
        let mut req = request(48_000, Period::Frames(128));
        assert_eq!(cfg.calibration_for("WASAPI", device, None, &req).unwrap().round_trip_ns, 10_000_000, "shared request, shared calibration");
        req.mode = Mode::Exclusive;
        assert_eq!(cfg.calibration_for("WASAPI", device, None, &req).unwrap().round_trip_ns, 20_000_000, "exclusive request, exclusive calibration");
        let only_shared = AudioConfig { calibrations: vec![sample_calibration()], ..cfg };
        assert!(only_shared.calibration_for("WASAPI", device, None, &req).is_none(), "a shared calibration never applies to an exclusive open");
    }

    #[test]
    fn each_system_offers_only_its_own_native_backend() {
        use Backend::*;
        let native = |list: Vec<Backend>| list.into_iter().filter(|b| b.is_native()).collect::<Vec<_>>();
        assert_eq!(native(Backend::offered(Some("ASIO"))), if NATIVE_ASIO_COMPILED { vec![NativeAsio] } else { vec![] });
        assert_eq!(native(Backend::offered(Some("WASAPI"))), if NATIVE_WASAPI_COMPILED { vec![NativeWasapi] } else { vec![] });
        assert!(native(Backend::offered(Some("CoreAudio"))).is_empty());
        assert!(Backend::offered(Some("ASIO")).starts_with(&[Auto, Cpal]));
        assert!(!NativeAsio.native_on(Some("WASAPI")) && !NativeWasapi.native_on(Some("ASIO")) && !Cpal.native_on(Some("ASIO")));
        assert!(!is_asio_system(None), "the default system is never ASIO");
        assert_eq!(native_offered(Some("asio")), NATIVE_ASIO_COMPILED, "system names compare without case");
        let saved = AudioConfig { backend: NativeAsio, system: Some("ASIO".into()), ..Default::default() };
        let text = serde_json::to_string(&saved).unwrap();
        assert!(text.contains("\"native_asio\""), "{text}");
        assert_eq!(serde_json::from_str::<AudioConfig>(&text).unwrap(), saved);
    }

    #[test]
    fn an_asio_calibration_is_keyed_asio_whatever_the_request_mode() {
        use transit_audio_backend::{Mode, Period};
        assert_eq!(calibration_mode("ASIO", Mode::Shared), Some(MODE_ASIO));
        assert_eq!(calibration_mode("WASAPI", Mode::Exclusive), Some(MODE_EXCLUSIVE));
        let asio =
            LoopbackCalibration { device: "Focusrite USB ASIO".into(), input: None, mode: MODE_ASIO.into(), period: 64, ..sample_calibration() };
        let cfg = AudioConfig { calibrations: vec![asio], ..Default::default() };
        let req = request(48_000, Period::Frames(64));
        assert!(cfg.calibration_for("ASIO", "Focusrite USB ASIO", None, &req).is_some(), "the driver name and mode asio");
        assert!(cfg.calibration_for("WASAPI", "Focusrite USB ASIO", None, &req).is_none(), "a WASAPI open never takes it");
    }

    #[test]
    fn a_measured_asio_loop_is_stored_under_the_open_key_and_found_only_there() {
        use transit_audio_backend::{Mode, Period};
        let driver = "Focusrite USB ASIO";
        let mut req = request(48_000, Period::Frames(256));
        req.mode = Mode::Shared;
        let key = CalibrationKey::of("ASIO", driver, None, &req).expect("ASIO names a mode");
        assert_eq!((key.mode.as_str(), key.rate, key.period, key.input.as_deref()), (MODE_ASIO, 48_000, 256, None));
        let mut cfg = AudioConfig::default();
        cfg.store_calibration(key.record(1_192, 7, "2026-10-23"));
        cfg.store_calibration(key.record(1_196, 4, "2026-10-24"));
        assert_eq!(cfg.calibrations.len(), 1, "the same key is replaced");
        let found = cfg.calibration_for("ASIO", driver, None, &req).expect("the open key");
        assert_eq!(found.round_trip_ns, 1_196 * 1_000_000_000 / 48_000);
        assert!(cfg.calibration_for("ASIO", driver, None, &request(48_000, Period::Frames(64))).is_none(), "another buffer size");
        assert!(cfg.calibration_for("ASIO", driver, None, &request(44_100, Period::Frames(256))).is_none(), "another rate");
        cfg.store_calibration(CalibrationKey { period: 64, ..key.clone() }.record(333, 7, "2026-10-23"));
        assert_eq!(cfg.asio_calibrations(driver).count(), 2);
        assert_eq!(cfg.asio_calibrations("ModelMixer").count(), 0);
        assert_eq!(utc_today().len(), 10);
    }

    #[test]
    fn installed_asio_drivers_cpal_could_not_list_are_offered() {
        let open = DeviceInfo {
            name: "Open Driver".into(),
            is_default: false,
            max_channels: 2,
            sample_rates: vec![48_000],
            buffer_min: Some(64),
            buffer_max: Some(1024),
        };
        let mut list = vec![open.clone()];
        add_unlisted(&mut list, &["Open Driver".into(), "Other Driver".into()]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], open, "a described device keeps what cpal reported");
        assert_eq!(list[1].name, "Other Driver");
        assert_eq!(list[1].sample_rates, STANDARD_RATES);
        assert_eq!(list[1].buffer_sizes().len(), STANDARD_BUFFERS.len());
    }

    #[test]
    #[ignore = "hardware enumeration loads ASIO drivers: opt in only on a test audio machine"]
    fn enumeration_does_not_fail() {
        let systems = audio_systems();
        // Every listed system has a name; on a machine without audio the lists are just empty.
        assert!(systems.iter().all(|s| !s.name.is_empty()));
        assert!(systems.iter().filter(|s| s.is_default).count() <= 1);
    }
}
