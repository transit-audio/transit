//! Audio › Audio Devices: the audio system, output and input device, sample rate and buffer size,
//! kept as `UiState.audio` (saved in `ui.json` with the other preferences). Every choice starts at
//! "System default".
//!
//! The lists come from [`transit_playback::devices::cpal_audio_systems`] and, in a build with
//! the native ASIO backend, [`transit_playback::devices::native_asio_system`], read when the
//! window opens and on Rescan, never per frame. The ASIO system is listed on a worker thread (it
//! probes the driver that runs, which can take seconds): the window says "Listing the ASIO
//! drivers…" until it answers and stops waiting after [`ASIO_LISTING_LIMIT`]. A driver only chosen
//! in the window is never probed (that opens it): its lists are stale until Apply, then listed again. Without a player (`--no-audio`, tests) nothing is enumerated:
//! the choices are only saved, for the next start.
//!
//! Apply reopens the devices through [`StudioApp::reconnect_audio`] (plug-in editors close first,
//! on the main thread, and the old device closes before the new one opens: an ASIO driver is
//! process-wide). It stops playback first and is refused while recording. `ui.set {audio}` takes
//! the same path. A choice that cannot be honored falls back to the default (the playback crate
//! logs it); the fallback is said on the status line and in the window.

mod period;
mod window;
pub use window::{Section, window};

use crate::StudioApp;
use crate::arrange::{R4, r4};
use crate::icons::{self, Icon};
use crate::mixer::MenuItem;
use crate::theme::{SPACE, TOKENS};
use transit_playback::devices::{self, AudioSystem, DeviceInfo};
use transit_playback::telemetry::{StatsBackend, StreamStats};
use transit_playback::{AudioConfig, Backend};

/// Said when Apply (or `ui.set {audio}`) is asked while recording.
pub const REFUSED_RECORDING: &str = "Stop recording before changing the audio devices.";
/// The label of every "no choice" entry.
/// Why Reconnect, Apply and Rescan wait while a native ASIO driver's settings panel is open: the
/// panel holds the driver's host thread, which each of them would wait on.
pub const PANEL_OPEN_FIRST: &str = "Close the driver settings panel first";
pub const SYSTEM_DEFAULT: &str = "System default";
/// Said under the Output list while the ASIO drivers are listed (off the UI thread).
pub const ASIO_LISTING: &str = "Listing the ASIO drivers…";
/// How long the window waits for an ASIO listing before it stops waiting: each request to the
/// driver's host thread waits at most ten seconds, and a listing makes a few.
pub const ASIO_LISTING_LIMIT: std::time::Duration = std::time::Duration::from_secs(25);

/// Lists the ASIO system probing the driver given (`devices::native_asio_system`), on a worker.
pub(crate) type AsioLister = std::sync::Arc<dyn Fn(Option<String>) -> Option<AudioSystem> + Send + Sync>;

/// An ASIO listing running on a worker thread: its answer, and when it started.
pub(crate) struct AsioListing {
    rx: std::sync::mpsc::Receiver<Option<AudioSystem>>,
    started: std::time::Instant,
}
/// Said under the rate and buffer lists where the device must be open to be described (an ASIO
/// driver other than the one that runs): the lists are the standard choices until Apply.
/// Said under the buffer list on the JACK system: Apply asks the server.
pub const JACK_BUFFER_NOTE: &str = "Changes the JACK server's buffer size for all clients.";
pub const STALE_NOTE: &str = "updates after Apply";
const CHOOSE_ANOTHER: &str = "Choose another in Audio › Audio Devices.";

/// Where the window and its controls were last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AudioDevicesLayout {
    pub open: bool,
    pub window: R4,
    /// The choices being edited (Apply makes them `ui.audio`).
    pub draft: AudioConfig,
    /// The audio systems listed.
    pub systems: Vec<String>,
    /// The controls: label is the text shown, value the control (`system`, `output`, `input`,
    /// `backend` (only where offered), `paired_input` (native WASAPI only), `rate`, `buffer`, `apply`, `reconnect`, `rescan`,
    /// `panel` (native ASIO's "Driver settings")).
    pub controls: Vec<MenuItem>,
    /// The entries of the open list: label is the text shown, value the control it belongs to.
    pub items: Vec<MenuItem>,
    /// What is open now.
    pub opened: Vec<String>,
    /// Fallbacks and notes, as shown with the warning icon.
    pub notes: Vec<String>,
    /// The path table the I/O section showed (the engine's, read this frame).
    pub io: Option<transit_model::IoMap>,
    /// The Calibration line (only under a native backend: under the Paired input for WASAPI).
    pub calibration: Option<String>,
    /// The native ASIO Calibration line's Measure, as drawn.
    pub calibration_measure: CalibrationLayout,
    /// The drivers of the chosen system that failed to load, as said under the Output list
    /// (native ASIO's per-driver diagnostics).
    pub diagnostics: Vec<String>,
    /// The ASIO listing's state under the Output list: [`ASIO_LISTING`] while it runs, or why it
    /// stopped waiting.
    pub listing: Option<String>,
    /// The Hardware inserts subsection as drawn.
    pub hardware: crate::audio_devices_io::HardwareLayout,
    /// The pending-change line next to Apply (the window's choices differ from the running ones).
    pub pending: Option<String>,
    /// The section shown (`section_system`, `section_io`, `section_hardware`).
    pub section: String,
    /// Apply is drawn highlighted (something is pending).
    pub apply_highlighted: bool,
}

/// The native ASIO Calibration line's Measure as drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CalibrationLayout {
    /// Why Measure is off; empty when it can run (or when the line is not drawn).
    pub blocked: String,
    /// The output path the click plays on and the input path it is found on.
    pub send: String,
    pub ret: String,
    /// "Measuring… <n>/<total> clicks" while it runs; empty otherwise.
    pub progress: String,
    /// What the last measurement said.
    pub result: String,
}

/// What the Calibration line's Measure asks for this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CalibrationAction {
    Measure { send: String, ret: String },
    Cancel,
}

/// Devices standing in for the machine's (the control channel's `audio.fake_devices`, tests):
/// listed instead of enumerating, with device operations injected into the real reconnect path.
#[derive(Debug, Default)]
pub struct FakeDevices {
    pub systems: Vec<AudioSystem>,
    /// Every configuration a reopen was asked with, in order.
    pub reopened: Vec<AudioConfig>,
    pub(crate) connection: Option<crate::audio_connection::FakeConnection>,
    pub(crate) input: Option<crate::audio_connection::FakeInput>,
    pub(crate) trace: crate::audio_connection::Trace,
    pub(crate) enumerations: usize,
}

impl FakeDevices {
    pub(crate) fn input_channels_for(&self, cfg: &AudioConfig) -> usize {
        let Some(sys) = pick_system(&self.systems, cfg.system.as_deref()) else { return 0 };
        let named = cfg.input.as_deref().and_then(|w| sys.inputs.iter().find(|d| d.name == w));
        named.or_else(|| sys.inputs.iter().find(|d| d.is_default)).map_or(0, |d| usize::from(d.max_channels))
    }

    /// The output a real reopen would get for `cfg`: the named device when listed, else the
    /// system's default.
    fn output_for(&self, cfg: &AudioConfig) -> Option<String> {
        let sys = pick_system(&self.systems, cfg.system.as_deref())?;
        let named = cfg.output.as_deref().and_then(|w| sys.outputs.iter().find(|d| d.name == w));
        named.or_else(|| sys.outputs.iter().find(|d| d.is_default)).map(|d| d.name.clone())
    }
}

#[derive(Default)]
pub struct AudioDevicesState {
    pub open: bool,
    pub draft: AudioConfig,
    /// The `ui.audio` the draft was taken from: when it changes elsewhere (`ui.set`), the draft follows.
    base: Option<AudioConfig>,
    pub(crate) systems: Option<Vec<AudioSystem>>,
    pub fake: Option<FakeDevices>,
    /// Fallbacks found at the last reopen (or at startup).
    pub notes: Vec<String>,
    startup_checked: bool,
    /// The (wanted, opened) input already reported, so it is said once.
    input_reported: Option<(String, String)>,
    pub layout: AudioDevicesLayout,
    verification: Option<Verification>,
    io_editor: crate::audio_devices_io::IoEditor,
    /// The measurement the Hardware inserts Measure started and has not collected yet.
    pub(crate) measuring: Option<crate::hardware_measure::Pending>,
    /// What to show as the last measurement's result, set where the editor is not at hand, with
    /// the kind of measurement it was (a hardware insert's, or the ASIO calibration's).
    pub(crate) measure_note: Option<(crate::hardware_measure::MeasureKind, String)>,
    /// The output and input paths picked for the ASIO calibration's Measure (`None`: the
    /// remembered pair, else the main output and the first input).
    calibration_send: Option<String>,
    calibration_return: Option<String>,
    /// What the last ASIO calibration measurement said.
    calibration_result: String,
    #[cfg(test)]
    pub(crate) fake_probe: Option<crate::hardware_measure::FakeProbe>,
    /// How long the window's measurement counts as having run (tests): the waits of
    /// `poll_measure` read this instead of the wall clock.
    #[cfg(test)]
    pub(crate) measure_elapsed: Option<std::time::Duration>,
    /// Scripted device-loop counters (output, input) standing in for the streams, so tests can
    /// render the Backend lines without a native backend.
    #[cfg(test)]
    pub(crate) injected_backend: Option<(Option<transit_playback::telemetry::StreamStats>, Option<transit_playback::telemetry::StreamStats>)>,
    /// The wake counters sampled at the last panel refreshes (output, input), for the rates.
    wake_rates: (WakeRate, WakeRate),
    /// A native ASIO driver's settings panel that is open; its result arrives when it closes.
    pub(crate) panel: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// The driver whose settings the window's "Driver settings" opened with fake devices (tests).
    pub(crate) panel_opened: Option<String>,
    /// Channel names standing in for the open device's (tests without a player).
    pub(crate) fake_channel_names: Option<crate::audio_devices_io::ChannelNames>,
    /// The ASIO driver the listing probed (the running one; `None`: none),
    /// so choosing another driver lists again for it.
    asio_listed_for: Option<Option<String>>,
    /// The ASIO listing in flight (off the UI thread), if any.
    pub(crate) asio_listing: Option<AsioListing>,
    /// Why the last ASIO listing was given up (it did not answer in time).
    asio_listing_note: Option<String>,
    /// How long to wait for an ASIO listing ([`ASIO_LISTING_LIMIT`]; tests shorten it).
    pub(crate) asio_listing_limit: Option<std::time::Duration>,
    /// The machine's listing standing in for the real one (tests): the systems listed at once, and
    /// the ASIO lister the worker runs.
    #[cfg(test)]
    pub(crate) machine: Option<(Vec<AudioSystem>, AsioLister)>,
}

/// The device loop's wake counter at the last sample, and the rate it gave.
#[derive(Default, Clone, Copy)]
pub(crate) struct WakeRate {
    sample: Option<(f64, u64)>,
    rate: Option<u64>,
}

impl WakeRate {
    /// The shortest span between samples a rate is computed over, in seconds; a refresh sooner
    /// than that shows the previous rate.
    const MIN_SPAN: f64 = 0.5;

    /// Take the counter `wakes` read at UI time `now` (seconds); returns the wakes per second
    /// since the previous sample, `None` until there are two. A counter that went down is a new
    /// stream: start over.
    pub(crate) fn update(&mut self, now: f64, wakes: u64) -> Option<u64> {
        match self.sample {
            Some((at, before)) if wakes >= before && now > at => {
                if now - at >= Self::MIN_SPAN {
                    self.rate = Some(((wakes - before) as f64 / (now - at)).round() as u64);
                    self.sample = Some((now, wakes));
                }
            }
            _ => {
                self.rate = None;
                self.sample = Some((now, wakes));
            }
        }
        self.rate
    }
}

impl AudioDevicesState {
    pub(crate) fn set_fake(&mut self, fake: Option<FakeDevices>) {
        self.systems = fake.as_ref().map(|f| f.systems.clone());
        self.fake = fake;
    }
    pub(crate) fn begin_verification(&mut self) {
        self.verification = Some(Verification::default());
    }
    /// `ui.audio` changed here (a measurement stored a calibration or remembered its paths): the
    /// draft already has the same change, so it is not reset to `ui.audio` on the next frame.
    pub(crate) fn adopt_audio(&mut self, audio: &AudioConfig) {
        self.base = Some(audio.clone());
    }

    /// List the devices again on the next frame the window is drawn (Rescan, a new fake).
    pub fn forget_listing(&mut self) {
        // A listing probes the drivers on the host thread, which the open settings panel holds:
        // keep showing the last list until the panel closes.
        if self.panel.is_some() {
            return;
        }
        self.systems = None;
    }
}

pub(crate) fn validate(cfg: &AudioConfig) -> Result<(), String> {
    if cfg.sample_rate == Some(0) || cfg.buffer_size == Some(0) {
        return Err("Audio sample rate and buffer size must be greater than zero (or null for default).".into());
    }
    Ok(())
}

/// A new stream must deliver two seconds of audio AND remain healthy for two seconds of UI
/// time. A silent clock, a callback burst followed by a stall, or any error cannot qualify.
#[derive(Default)]
struct Verification {
    first: Option<(f64, u64)>,
    last_progress: Option<(f64, u64)>,
}

impl Verification {
    fn ready(&mut self, now: f64, rate: u32, t: transit_playback::telemetry::CallbackTelemetry) -> bool {
        if rate == 0 || t.callbacks == 0 || !now.is_finite() {
            return false;
        }
        let progressed = self.last_progress.is_none_or(|(_, callbacks)| t.callbacks > callbacks);
        if self.last_progress.is_some_and(|(at, _)| now - at > 2.0) {
            self.first = None;
        }
        if progressed {
            self.last_progress = Some((now, t.callbacks));
        }
        let (at, frames) = *self.first.get_or_insert((now, t.frames));
        progressed && now - at >= 2.0 && t.frames.saturating_sub(frames) >= u64::from(rate) * 2
    }
}

impl StudioApp {
    pub(crate) fn save_good_audio(&mut self) {
        if self.audio_devices.verification.is_none() {
            return;
        }
        let (rate, telemetry, failed) = if let Some(p) = self.audio_devices.fake.as_ref().and_then(|f| f.connection.as_ref()) {
            (p.config.sample_rate.unwrap_or(48_000), p.telemetry, p.failed)
        } else if let Some(p) = &self.player {
            (p.device_rate, p.callback_telemetry(), p.silent || p.trouble().is_some() || self.recorder_failed)
        } else {
            return;
        };
        if failed || telemetry.backend_errors != 0 || telemetry.device_lost_errors != 0 || telemetry.fault_silence_frames != 0 {
            self.audio_devices.verification = None;
            return;
        }
        let now = self.last_frame.unwrap_or_default();
        if !self.audio_devices.verification.as_mut().is_some_and(|v| v.ready(now, rate, telemetry)) {
            return;
        }
        self.audio_devices.verification = None;
        // The desktop host supplies the settings directory through its Autosave sibling. Tests
        // without that service never write preferences. Honor the host's no-preferences switches.
        if std::env::var_os("TRANSIT_NO_PREFS").is_some() || std::env::var_os("SOUNDCRAFT_NO_PREFS").is_some() {
            return;
        }
        let Some(dir) = self.autosave_dir.as_deref().and_then(std::path::Path::parent) else { return };
        if let Err(e) = persist_audio(&dir.join("ui.json"), &self.ui) {
            self.status = format!("Audio is running, but its settings could not be saved: {e}");
        }
    }
}

/// Preserve the other saved preferences and atomically replace only the known-good audio block.
fn persist_audio(path: &std::path::Path, ui: &crate::UiState) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut value = match std::fs::File::open(path) {
        Ok(f) => {
            let mut bytes = Vec::new();
            f.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err("Settings file exceeds the 4 MiB limit".into());
            }
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut saved = ui.clone();
            saved.plugin_windows.clear();
            serde_json::to_value(saved).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    let obj = value.as_object_mut().ok_or("Settings must be an object")?;
    obj.insert("audio".into(), serde_json::to_value(&ui.audio).map_err(|e| e.to_string())?);
    obj.insert("io_maps".into(), serde_json::to_value(&ui.io_maps).map_err(|e| e.to_string())?);
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let tmp = path.with_extension(format!("audio.{}.{nonce}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

/// The chosen system in `systems`, else the default one.
fn pick_system<'a>(systems: &'a [AudioSystem], name: Option<&str>) -> Option<&'a AudioSystem> {
    name.and_then(|n| systems.iter().find(|s| s.name.eq_ignore_ascii_case(n))).or_else(|| systems.iter().find(|s| s.is_default))
}

/// A device as a list shows it: its name and channel count (unknown until an ASIO driver is open).
pub fn device_label(d: &DeviceInfo) -> String {
    match d.max_channels {
        0 => d.name.clone(),
        1 => format!("{} · 1 ch", d.name),
        n => format!("{} · {n} ch", d.name),
    }
}

/// What the player opened, for comparing with what was asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    pub output: Option<String>,
    pub rate: Option<u32>,
    pub buffer: Option<u32>,
}

/// The choices in `cfg` that were not honored: a system not on this machine, an output that did
/// not open (unplugged, renamed), an input not listed, a rate or buffer size the device refused.
pub fn fallback_notes(cfg: &AudioConfig, systems: Option<&[AudioSystem]>, opened: &Opened) -> Vec<String> {
    let mut notes = Vec::new();
    if let (Some(want), Some(list)) = (cfg.system.as_deref(), systems)
        && !list.iter().any(|s| s.name.eq_ignore_ascii_case(want))
    {
        notes.push(format!("Audio system “{want}” is not available; using the default."));
    }
    if let (Some(want), Some(got)) = (cfg.output.as_deref(), opened.output.as_deref())
        && want != got
    {
        notes.push(format!("Output “{want}” is not available; using “{got}”."));
    }
    let asio = cfg.system.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(devices::ASIO_SYSTEM));
    if let (Some(want), Some(list)) = (cfg.input.as_deref(), systems)
        && !asio
        && let Some(sys) = pick_system(list, cfg.system.as_deref())
        && !sys.inputs.iter().any(|d| d.name == want)
    {
        notes.push(format!("Input “{want}” is not available; recording uses the default input."));
    }
    if let (Some(want), Some(got)) = (cfg.sample_rate, opened.rate)
        && want != got
    {
        notes.push(format!("{want} Hz is not available; running at {got} Hz."));
    }
    if let Some(want) = cfg.buffer_size
        && opened.output.is_some()
        && opened.buffer != Some(want)
    {
        notes.push(match opened.buffer {
            Some(got) => format!("A {want}-sample buffer is outside the device's range; using {got}."),
            None => format!("A {want}-sample buffer is not available; using the device's default."),
        });
    }
    notes
}

impl StudioApp {
    /// The one-line warning about the path table (`audio_devices_io::warning_text`).
    ///
    /// Cached by (document revision, I/O map revision): the status bar asks every frame, and
    /// finding the unresolved routes plans the sinks.
    pub fn io_warning(&mut self) -> Option<String> {
        let key = (self.engine.revision, self.engine.io_revision());
        if self.io_warning_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let text = crate::audio_devices_io::warning_text(&self.engine.unresolved_routes(), &self.engine.io_map().overlaps());
            self.io_warning_cache = Some((key, text));
        }
        self.io_warning_cache.as_ref().and_then(|(_, t)| t.clone())
    }

    /// Save `ui.audio` with the preferences now (a calibration was measured): the same file and
    /// switches as the save after a verified reopen. A failure is said on the status line.
    pub(crate) fn save_audio_preferences(&mut self) {
        if std::env::var_os("TRANSIT_NO_PREFS").is_some() || std::env::var_os("SOUNDCRAFT_NO_PREFS").is_some() {
            return;
        }
        let Some(dir) = self.autosave_dir.as_deref().and_then(std::path::Path::parent) else { return };
        if let Err(e) = persist_audio(&dir.join("ui.json"), &self.ui) {
            self.status = format!("The calibration is in use, but the settings could not be saved: {e}");
        }
    }

    /// Why the native ASIO Calibration line's Measure is off, `None` when it can run: it times
    /// the driver as it is open, so the window's choices must be the open ones.
    pub(crate) fn calibration_blocker(&self, draft: &AudioConfig) -> Option<&'static str> {
        if self.measuring_kind() == Some(crate::hardware_measure::MeasureKind::Insert) {
            return Some("A hardware insert measurement is running.");
        }
        #[cfg(test)]
        if self.audio_devices.fake_probe.is_some() {
            return None;
        }
        let Some(player) = self.player.as_ref().filter(|p| !p.silent) else { return Some("Measuring needs the open audio device.") };
        if !(player.native_backend && devices::is_asio_system(Some(&player.device_system))) {
            return Some("Apply Native ASIO first: Measure times the open driver.");
        }
        if player.calibration_key.is_none() {
            return Some("The driver's input is not open with its output: there is no loop to time.");
        }
        if !self.draft_is_open(draft) {
            return Some("Apply the changes first: Measure times the driver as it is open.");
        }
        if self.is_playing() {
            return Some("Stop the transport to measure.");
        }
        None
    }

    /// Whether the window's choices are the ones the device was opened with (the open driver's
    /// buffer is then the one a calibration of it is keyed on).
    pub(crate) fn draft_is_open(&self, draft: &AudioConfig) -> bool {
        let applied = &self.ui.audio;
        draft.system == applied.system
            && draft.backend == applied.backend
            && draft.output == applied.output
            && draft.sample_rate == applied.sample_rate
            && draft.buffer_size == applied.buffer_size
    }

    /// Audio › Audio Devices: open the window with the saved choices.
    pub fn open_audio_devices(&mut self) {
        let s = &mut self.audio_devices;
        s.open = true;
        s.draft = self.ui.audio.clone();
        s.base = Some(self.ui.audio.clone());
        s.forget_listing();
    }

    /// The window's Apply: keep `cfg` as `ui.audio` and reopen the devices with it. Playback stops
    /// first; refused while recording (the take would be cut), leaving `ui.audio` as it was.
    pub fn apply_audio(&mut self, cfg: AudioConfig) -> Result<(), String> {
        validate(&cfg)?;
        if self.recording_active() {
            self.status = REFUSED_RECORDING.into();
            return Err(REFUSED_RECORDING.into());
        }
        let old = std::mem::replace(&mut self.ui.audio, cfg);
        if let Err(error) = self.reopen_audio() {
            self.ui.audio = old;
            return Err(error);
        }
        self.audio_devices.base = Some(self.ui.audio.clone());
        self.audio_devices.draft = self.ui.audio.clone();
        Ok(())
    }

    /// Reopen the devices with `ui.audio` (Apply, `ui.set {audio}`). Without a player
    /// (`--no-audio`) the choice is only saved.
    pub(crate) fn reopen_audio(&mut self) -> Result<(), String> {
        let was_playing = self.is_playing();
        let cfg = self.ui.audio.clone();
        self.audio_devices.input_reported = None;
        let (opened, captured) = if self.player.is_some() || self.audio_devices.fake.is_some() {
            let captured = match self.reconnect_with_player(true) {
                Ok(captured) => {
                    self.reopen.reconnected();
                    captured
                }
                Err(error) => {
                    self.player_requests.fail(&mut self.status, error.clone());
                    return Err(error);
                }
            };
            (self.opened(), captured)
        } else {
            self.audio_devices.notes.clear();
            self.status = "Audio devices saved. Audio is off in this run (--no-audio); they open at the next start.".into();
            return Ok(());
        };
        let mut notes = fallback_notes(&cfg, self.audio_devices.systems.as_deref(), &opened);
        notes.extend(self.player.as_ref().and_then(|p| p.backend_note.clone()));
        notes.extend(self.not_opened_note());
        let stopped = if was_playing { "Playback stopped to change devices. " } else { "" };
        self.status = if notes.is_empty() {
            let name = opened.output.unwrap_or_else(|| "no audio device".into());
            format!("{stopped}Audio devices reopened: {name}.")
        } else {
            format!("{stopped}{} {CHOOSE_ANOTHER}", notes.join(" "))
        };
        if !captured {
            self.status.push_str(" Audio recovered from the document state; live plug-in state was unavailable after the audio fault.");
        }
        self.audio_devices.notes = notes;
        Ok(())
    }

    /// The window's Reconnect: reopen the same choices (a device came back, a driver changed).
    pub fn reconnect_audio_devices(&mut self) {
        if self.recording_active() {
            self.status = REFUSED_RECORDING.into();
            return;
        }
        if self.audio_devices.panel.is_some() {
            self.status = PANEL_OPEN_FIRST.into();
            return;
        }
        let _ = self.reopen_audio();
    }

    /// Why the device did not open, when the Player runs silent with an error (a saved ASIO
    /// driver that is no longer installed, say): said, never covered by another device.
    fn not_opened_note(&self) -> Option<String> {
        let why = self.player.as_ref().filter(|p| p.silent).and_then(transit_playback::Player::initial_error)?;
        Some(format!("The audio device did not open: {why}."))
    }

    fn opened(&self) -> Opened {
        if let Some(f) = &self.audio_devices.fake
            && let Some(p) = &f.connection
        {
            return Opened { output: f.output_for(&p.config), rate: Some(p.config.sample_rate.unwrap_or(48_000)), buffer: p.config.buffer_size };
        }
        match &self.player {
            Some(p) if !p.silent => Opened { output: Some(p.device_name.clone()), rate: Some(p.device_rate), buffer: p.device_buffer },
            _ => Opened::default(),
        }
    }

    /// The report for the Copy diagnostics buttons, except what a worker thread adds: the machine
    /// facts and the log lines. The device list is the window's (cached, or the fake's); empty when
    /// the window has not listed yet, in which case the worker lists it (enumeration only, no
    /// stream), so Callback Telemetry's button does not block the UI thread.
    /// Each track's name and plug-in display names by track index (the strip index of the
    /// callback's phase marker), read from the session on the UI thread (#162).
    pub(crate) fn strip_names(&mut self) -> Vec<crate::telemetry::StripName> {
        let tracks: Vec<(String, Vec<(String, bool)>)> = self
            .engine
            .session()
            .tracks
            .iter()
            .map(|t| {
                let plugins = match &t.instrument {
                    Some(i) => vec![(i.plugin.clone(), true)],
                    None => t.mixer.inserts.iter().flatten().filter(|i| i.active && !i.bypass).map(|i| (i.plugin.clone(), false)).collect(),
                };
                (t.name.clone(), plugins)
            })
            .collect();
        tracks
            .into_iter()
            .map(|(track, plugins)| {
                let names: Vec<String> = plugins.iter().map(|(id, instrument)| self.plugin_name(id, *instrument)).collect();
                crate::telemetry::StripName { track, plugin: (!names.is_empty()).then(|| names.join(", ")) }
            })
            .collect()
    }

    pub(crate) fn diagnostics_report(&mut self, now: f64) -> crate::diagnostics::Report {
        let systems = match (&self.audio_devices.systems, &self.audio_devices.fake) {
            (Some(s), _) => s.clone(),
            (None, Some(f)) => f.systems.clone(),
            (None, None) => Vec::new(), // listed by the worker, see `request_diagnostics`
        };
        // The lines the window shows, as last drawn; computed when the window has not drawn them.
        let opened = if self.audio_devices.layout.opened.is_empty() { opened_lines(self, now) } else { self.audio_devices.layout.opened.clone() };
        let telemetry = match (&self.player, self.telemetry.injected) {
            (Some(p), _) => Some(p.callback_telemetry()),
            (None, injected) => injected,
        };
        let version = if self.about.app_version.is_empty() { env!("CARGO_PKG_VERSION") } else { self.about.app_version.as_str() };
        crate::diagnostics::Report {
            version: version.to_owned(),
            build: self.about.build.clone(),
            systems,
            settings: self.ui.audio.clone(),
            opened,
            preview: Some(self.preview_line()),
            telemetry,
            midi: self.diagnostics_midi(),
            open_asio_driver: self.running_native_asio(),
            strips: self.strip_names(),
            last_status: self.status_age.last().cloned(),
            plugin_host: self.plugin_host_lines(),
            deferred_services: transit_dsp::host_call::deferred(),
            note_view_dropped: transit_mix::note_view_dropped_total(),
            unplaced_takes: self.engine.session().unplaced_takes.clone(),
            log_dir: self.services.config_dir.as_ref().map(|d| d.join("logs")),
            ..Default::default()
        }
    }

    /// The devices to offer: the fake's, else the machine's (only with a player). The native ASIO
    /// system names every installed driver and probes only [`Self::asio_selection`], on a worker
    /// thread ([`Self::start_asio_listing`]): until it answers the ASIO system is listed empty and
    /// the window says [`ASIO_LISTING`].
    fn listing(&mut self) -> Vec<AudioSystem> {
        match (&mut self.audio_devices.fake, &self.player) {
            (Some(f), _) => {
                f.enumerations += 1;
                f.systems.clone()
            }
            (None, Some(_)) => {
                let (mut systems, lister) = self.machine_listing();
                if let Some(lister) = lister {
                    systems.retain(|s| s.name != devices::ASIO_SYSTEM);
                    systems.push(AudioSystem {
                        name: devices::ASIO_SYSTEM.into(),
                        is_default: false,
                        outputs: Vec::new(),
                        inputs: Vec::new(),
                        diagnostics: Vec::new(),
                    });
                    self.start_asio_listing(lister);
                }
                systems
            }
            (None, None) => Vec::new(),
        }
    }

    /// The systems listed at once (cpal's) and the native ASIO lister, when this build has one.
    fn machine_listing(&self) -> (Vec<AudioSystem>, Option<AsioLister>) {
        #[cfg(test)]
        if let Some((systems, lister)) = &self.audio_devices.machine {
            return (systems.clone(), Some(std::sync::Arc::clone(lister)));
        }
        let lister: Option<AsioLister> = devices::NATIVE_ASIO_COMPILED
            .then(|| std::sync::Arc::new(|selected: Option<String>| devices::native_asio_system(selected.as_deref())) as AsioLister);
        let mut systems = devices::cpal_audio_systems();
        systems.extend(devices::native_jack_system());
        (systems, lister)
    }

    /// Lists the ASIO system for [`Self::asio_selection`] on a worker thread (probing a driver can
    /// take seconds); [`Self::poll_asio_listing`] takes the answer. A listing already in flight is
    /// dropped (its answer is not waited for).
    fn start_asio_listing(&mut self, lister: AsioLister) {
        let selected = self.asio_selection();
        self.audio_devices.asio_listed_for = Some(selected.clone());
        self.audio_devices.asio_listing_note = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("transit-asio-listing".into()).spawn(move || {
            let _ = tx.send(lister(selected));
        });
        match spawned {
            Ok(_) => self.audio_devices.asio_listing = Some(AsioListing { rx, started: std::time::Instant::now() }),
            Err(e) => {
                self.audio_devices.asio_listing = None;
                self.audio_devices.asio_listing_note = Some(format!("The ASIO drivers could not be listed: {e}."));
            }
        }
    }

    /// Lists the ASIO system again for another chosen driver, keeping the rest of the listing.
    fn relist_asio(&mut self) {
        if let (None, Some(_)) = (&self.audio_devices.fake, &self.player)
            && let (_, Some(lister)) = self.machine_listing()
        {
            self.start_asio_listing(lister);
        }
    }

    /// Takes the ASIO listing's answer when it has come (never waits): it replaces the ASIO system
    /// of the listing. Past the limit the window stops waiting and says so.
    pub(crate) fn poll_asio_listing(&mut self) {
        let Some(listing) = &self.audio_devices.asio_listing else { return };
        let limit = self.audio_devices.asio_listing_limit.unwrap_or(ASIO_LISTING_LIMIT);
        match listing.rx.try_recv() {
            Ok(system) => {
                self.audio_devices.asio_listing = None;
                if let (Some(systems), Some(system)) = (self.audio_devices.systems.as_mut(), system) {
                    match systems.iter_mut().find(|s| s.name == system.name) {
                        Some(slot) => *slot = system,
                        None => systems.push(system),
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if listing.started.elapsed() < limit => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.audio_devices.asio_listing = None;
                self.audio_devices.asio_listing_note =
                    Some(format!("The ASIO drivers did not answer within {} s. Use Rescan Devices to try again.", limit.as_secs()));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.audio_devices.asio_listing = None;
                self.audio_devices.asio_listing_note = Some("The ASIO drivers could not be listed. Use Rescan Devices to try again.".into());
            }
        }
    }

    /// The ASIO driver the listing probes: the native ASIO driver that runs; `None` probes none
    /// (names only). A driver chosen in the window is never probed before Apply: probing opens it.
    pub(crate) fn asio_selection(&self) -> Option<String> {
        self.running_native_asio()
    }

    /// Whether the driver that runs moved off the one the listing probed (after Apply: it lists
    /// again, probing the new one).
    fn asio_selection_changed(&self) -> bool {
        self.audio_devices.asio_listed_for.as_ref().is_some_and(|listed| *listed != self.asio_selection())
    }

    fn running_asio(&self) -> bool {
        let cfg = self.audio_devices.fake.as_ref().and_then(|f| f.connection.as_ref()).map_or(&self.ui.audio, |p| &p.config);
        self.opened().output.is_some() && cfg.system.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(devices::ASIO_SYSTEM))
    }

    /// The ASIO driver the native backend has open, if it serves the running device (with fake
    /// devices: the running configuration chose it and this build has it).
    fn running_native_asio(&self) -> Option<String> {
        if !self.running_asio() {
            return None;
        }
        if let Some(f) = &self.audio_devices.fake
            && let Some(p) = &f.connection
        {
            return (p.config.backend.native_on(p.config.system.as_deref())).then(|| f.output_for(&p.config)).flatten();
        }
        self.player.as_ref().filter(|p| p.native_backend && devices::is_asio_system(Some(&p.device_system))).map(|p| p.device_name.clone())
    }

    /// Opens the settings panel of the native ASIO driver that runs (the window's "Driver
    /// settings"): on the backend's host thread, from a thread of its own, so a modal panel does
    /// not hold the UI; the result is said when it closes.
    fn open_native_asio_panel(&mut self, driver: &str) {
        if self.audio_devices.fake.is_some() {
            self.audio_devices.panel_opened = Some(driver.to_owned());
            return;
        }
        match devices::native_asio_control_panel(driver) {
            Ok(rx) => {
                self.audio_devices.panel = Some(rx);
                self.status = format!(
                    "“{driver}” driver settings are open. A driver that asks for a reset is reopened when they close; otherwise Reconnect after changing the buffer size or the rate."
                );
            }
            Err(e) => self.status = format!("ASIO driver settings: {e}"),
        }
    }

    /// Says how the driver settings panel ended, once it has.
    fn poll_native_asio_panel(&mut self) {
        let Some(rx) = &self.audio_devices.panel else { return };
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.audio_devices.panel = None;
                self.status = if self.reopen.pending() {
                    "ASIO driver settings closed. The driver asked for a reset: reopening the device.".into()
                } else {
                    "ASIO driver settings closed. If you changed the buffer size or the rate, use Reconnect.".into()
                };
            }
            Ok(Err(e)) => {
                self.audio_devices.panel = None;
                self.status = format!("ASIO driver settings: {e}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.audio_devices.panel = None,
        }
    }
}

/// Say once what the saved choices did not get: the output at startup, the input when it opens.
fn check(app: &mut StudioApp) {
    app.poll_native_asio_panel();
    if !app.audio_devices.startup_checked && app.player.is_some() {
        app.audio_devices.startup_checked = true;
        let mut notes = fallback_notes(&app.ui.audio, None, &app.opened());
        notes.extend(app.player.as_ref().and_then(|p| p.backend_note.clone()));
        notes.extend(app.not_opened_note());
        if !notes.is_empty() {
            app.status = format!("{} {CHOOSE_ANOTHER}", notes.join(" "));
            app.audio_devices.notes = notes;
        }
    }
    let (Some(want), Some(r)) = (app.ui.audio.input.as_deref(), &app.recorder) else { return };
    let pair = (want.to_string(), r.device_name.clone());
    if pair.0 == pair.1 || app.audio_devices.input_reported.as_ref() == Some(&pair) {
        return;
    }
    let asio = app.ui.audio.system.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(devices::ASIO_SYSTEM));
    let note = if asio {
        format!("On ASIO the input is the output's device: recording from “{}”, not “{want}”.", pair.1)
    } else {
        format!("Input “{want}” is not available; recording from “{}”.", pair.1)
    };
    app.status = format!("{note} {CHOOSE_ANOTHER}");
    app.audio_devices.notes.retain(|n| !n.starts_with("Input ") && !n.starts_with("On ASIO"));
    app.audio_devices.notes.push(note);
    app.audio_devices.input_reported = Some(pair);
}

/// The "no choice" entry of the Paired input list.
pub const PAIRED_AUTOMATIC: &str = "Automatic (same device)";
/// The button that opens the running native ASIO driver's own settings panel.
pub const DRIVER_SETTINGS: &str = "Driver settings";

/// The `ui.json` the running app saves to, as `audio-bench --settings` should name it.
fn settings_path(app: &StudioApp) -> String {
    app.autosave_dir
        .as_deref()
        .and_then(std::path::Path::parent)
        .map_or_else(|| r"%APPDATA%\transit\ui.json".to_string(), |dir| dir.join("ui.json").display().to_string())
}

/// The checkbox under the Backend selector (native WASAPI): unchecked is shared, the default;
/// checked is exclusive (`share_device` false).
pub const SHARE_LABEL: &str = "Take exclusive control of the device";

/// The one-line hint under the checkbox, for the state it is in.
pub fn share_hint(shared: bool) -> &'static str {
    if shared {
        "Shared: Windows mixes other apps in with transit; the most dependable choice."
    } else {
        "Exclusive: other apps are silenced while transit runs, and latency is only as good as the driver; some drivers open silent."
    }
}

/// The Calibration line under the Paired input: the saved loopback of this exact pair, rate,
/// buffer and mode (the checkbox's: shared or exclusive), or the command that measures it
/// (`device` is the output's name, `settings` the `ui.json` to merge into; close the app before
/// running it). `paired_input` is the input the automatic pairing resolved to (the open Player's,
/// else the open recorder's), `None` when none is open for `device`: a record naming it matches an
/// automatic pairing (see [`devices::input_matches`]).
pub fn calibration_line(cfg: &AudioConfig, device: &str, paired_input: Option<&str>, rate: u32, settings: &str) -> String {
    let period = cfg.buffer_size.unwrap_or(0);
    let mode = if cfg.share_device { devices::MODE_SHARED } else { devices::MODE_EXCLUSIVE };
    if let Some(c) = cfg.find_calibration_paired(device, paired_input, mode, rate, period) {
        return format!("measured {} frames \u{b1}{} ({}) \u{b7} {mode}", c.round_trip_frames, c.spread_frames, c.measured_at);
    }
    let mut command = format!("audio-bench --backend wasapi --loopback --out-device \"{device}\"");
    if !cfg.share_device {
        command.push_str(" --mode exclusive");
    }
    if let Some(input) = cfg.input_pairing.as_deref().or(paired_input) {
        command.push_str(&format!(" --in-device \"{input}\""));
    }
    if period != 0 {
        command.push_str(&format!(" --period {period}"));
    }
    command.push_str(&format!(" --rate {rate} --settings \"{settings}\""));
    format!("not calibrated \u{b7} {mode}: run {command} (with this app closed)")
}

/// "256 / 48 kHz": the period and rate a calibration was measured at ("driver buffer" for the
/// driver's default period).
pub fn period_rate_text(period: u32, rate: u32) -> String {
    let khz = f64::from(rate) / 1000.0;
    let khz = if rate.is_multiple_of(1000) { format!("{khz:.0}") } else { format!("{khz:.1}") };
    let period = if period == 0 { "driver buffer".to_string() } else { period.to_string() };
    format!("{period} / {khz} kHz")
}

/// "1196 frames (24.9 ms) \u{b1}7 at 256 / 48 kHz": a measured loop with its spread and conditions.
pub fn calibrated_text(c: &transit_playback::LoopbackCalibration) -> String {
    format!("{} \u{b1}{} at {}", crate::hardware_ui::frames_text(c.round_trip_frames, c.rate), c.spread_frames, period_rate_text(c.period, c.rate))
}

/// The Calibration line under the native ASIO backend (#20, known issue K31): the loop of this
/// driver (its registry name) measured at the buffer it runs (`running`: the open driver's buffer
/// and rate, `None` when the window's choices are not open yet), else the driver's own figures.
/// The driver's figures are close by sample position on the drivers measured, but unverified per
/// driver and phase; a measured loop replaces them. It belongs to the buffer the driver actually
/// runs, which it may choose itself (its panel's size, a fixed-size driver), so the line keys on
/// that, not on the size asked for.
pub fn asio_calibration_line(cfg: &AudioConfig, driver: &str, running: Option<(u32, u32)>) -> String {
    let elsewhere: Vec<String> = cfg.asio_calibrations(driver).map(|c| period_rate_text(c.period, c.rate)).collect();
    if let Some((period, rate)) = running {
        if let Some(c) = cfg.find_calibration(driver, devices::MODE_ASIO, rate, period) {
            return format!("calibrated: {} ({})", calibrated_text(c), c.measured_at);
        }
    } else if !elsewhere.is_empty() {
        return format!("measured at {}: whether it applies is known once the driver runs these choices (Apply)", elsewhere.join(", "));
    }
    if elsewhere.is_empty() {
        "not calibrated: the driver's own latency figures are used; Measure checks the loop through a cable from an output to an input".into()
    } else {
        format!(
            "not calibrated at the buffer size and rate the driver runs (measured at {}): the driver's own latency figures are used; Measure checks it again",
            elsewhere.join(", ")
        )
    }
}

/// The line next to Apply while the window's choices differ from the running ones, `None` when
/// they are the same. A different audio system or backend is a driver change; a different sample
/// rate says what reopens. Nothing is opened to say it.
pub fn pending_text(draft: &AudioConfig, running: &AudioConfig) -> Option<String> {
    if draft == running {
        return None;
    }
    if draft.system != running.system || draft.backend != running.backend {
        let system = draft.system.as_deref().unwrap_or(SYSTEM_DEFAULT);
        let name = if draft.backend.is_native() { format!("{system} ({})", draft.backend.label()) } else { system.to_owned() };
        return Some(format!("Driver change pending: press Apply to switch to {name}."));
    }
    if draft.sample_rate != running.sample_rate {
        let rate = draft.sample_rate.map_or_else(|| SYSTEM_DEFAULT.to_owned(), |r| format!("{r} Hz"));
        return Some(format!("Sample rate change pending: press Apply to reopen at {rate}."));
    }
    Some("Changes pending: press Apply to reopen the devices.".into())
}

/// The pick of `pick` if it still names one of `names`, else `remembered` if it does, else
/// `fallback` if it does, else the first name.
fn pick_path(pick: Option<&str>, remembered: Option<&str>, fallback: Option<&str>, names: &[&str]) -> Option<String> {
    [pick, remembered, fallback].into_iter().flatten().find(|n| names.contains(n)).or(names.first().copied()).map(str::to_owned)
}

/// The Measure row under the native ASIO Calibration line: an output path for the click (the
/// remembered one, else Main's), an input path to find it on (the remembered one, else the first),
/// Measure, then the progress with Cancel while it runs and what the last run said.
fn calibration_measure(
    ui: &mut egui::Ui,
    io: &transit_model::IoMap,
    (draft, send, ret): (&AudioConfig, &mut Option<String>, &mut Option<String>),
    blocked: Option<&str>,
    progress: Option<(u32, u32)>,
    result: &str,
    (controls, items): (&mut Vec<(String, &'static str, egui::Rect)>, &mut Vec<MenuItem>),
) -> (Option<CalibrationAction>, CalibrationLayout) {
    use transit_model::IoDirection;
    let outputs: Vec<&str> = io.paths_of(IoDirection::Output).map(|p| p.name.as_str()).collect();
    let inputs: Vec<&str> = io.paths_of(IoDirection::Input).map(|p| p.name.as_str()).collect();
    *send = pick_path(send.as_deref(), draft.calibration_send.as_deref(), Some(io.main.as_str()), &outputs);
    *ret = pick_path(ret.as_deref(), draft.calibration_return.as_deref(), None, &inputs);
    let running = progress.is_some();
    let mut action = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!running, |ui| {
            for (label, key, names, pick) in [("Out", "cal_send", &outputs, &mut *send), ("In", "cal_ret", &inputs, &mut *ret)] {
                ui.label(egui::RichText::new(label).color(TOKENS.text_dim).size(11.5));
                let combo = egui::ComboBox::from_id_salt(key).selected_text(pick.as_deref().unwrap_or("None")).show_ui(ui, |ui| {
                    for n in names {
                        let b = crate::menus::check_button(ui, pick.as_deref() == Some(*n), *n);
                        items.push(MenuItem::at(ui, (*n).to_string(), format!("{key}_entry"), b.rect));
                        if b.clicked() {
                            *pick = Some((*n).to_string());
                        }
                    }
                });
                controls.push((pick.clone().unwrap_or_default(), key, combo.response.rect));
            }
        });
        let ready = send.is_some() && ret.is_some();
        let b = ui.add_enabled(ready && blocked.is_none() && !running, egui::Button::new("Measure"));
        controls.push(("Measure".into(), "cal_measure", b.rect));
        let tip = blocked.unwrap_or(if running {
            "A measurement is running."
        } else {
            "Play a click on the output and time it back on the input: the loop at this buffer size and rate. Cable the output to the input first."
        });
        if b.on_hover_text(tip).on_disabled_hover_text(tip).clicked()
            && let (Some(send), Some(ret)) = (send.clone(), ret.clone())
        {
            action = Some(CalibrationAction::Measure { send, ret });
        }
    });
    let mut layout = CalibrationLayout {
        blocked: blocked.unwrap_or_default().to_owned(),
        send: send.clone().unwrap_or_default(),
        ret: ret.clone().unwrap_or_default(),
        result: result.to_owned(),
        ..Default::default()
    };
    if let Some((sent, total)) = progress {
        layout.progress = format!("Measuring\u{2026} {sent}/{total} clicks");
        ui.horizontal(|ui| {
            let r = ui.label(egui::RichText::new(&layout.progress).color(TOKENS.text).size(11.5));
            controls.push((layout.progress.clone(), "cal_progress", r.rect));
            let b = ui.button("Cancel");
            controls.push(("Cancel".into(), "cal_cancel", b.rect));
            if b.on_hover_text("Stop measuring; nothing is stored.").clicked() {
                action = Some(CalibrationAction::Cancel);
            }
        });
    } else if let Some(why) = blocked {
        ui.label(egui::RichText::new(why).color(TOKENS.text_dim).size(11.0));
    }
    if !result.is_empty() {
        let r = ui.add(egui::Label::new(egui::RichText::new(result).color(TOKENS.text).size(11.5)).wrap());
        controls.push((result.to_owned(), "cal_result", r.rect));
    }
    (action, layout)
}

/// The bench command inside a "not calibrated" [`calibration_line`] (`None` for a measured one).
pub fn calibration_command(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("not calibrated")?.split_once(": run ")?.1;
    Some(rest.strip_suffix(" (with this app closed)").unwrap_or(rest))
}

/// What is open now: the output with its rate, channels and buffer, and the input.
pub(crate) fn opened_lines(app: &mut StudioApp, now: f64) -> Vec<String> {
    let mut lines = Vec::new();
    // A native backend reports what it opened: the device sample format and its latency figure.
    let native = app.player.as_ref().is_some_and(|p| p.native_backend);
    match &app.player {
        Some(p) => {
            let buffer = p.device_buffer.map_or("default buffer".to_string(), |b| format!("{b}-sample buffer"));
            let host = if p.native_backend {
                format!(
                    " · {}{}{}",
                    crate::telemetry::host_name(p),
                    achieved_detail(p.output_sample, p.output_latency, "out"),
                    priority_detail(p.thread_priority())
                )
            } else {
                String::new()
            };
            lines.push(format!("Output open: {} · {} Hz · {} ch · {buffer}{host}", p.device_name, p.device_rate, p.device_channels));
            let asked = &app.ui.audio;
            lines.extend(requested_note(asked.buffer_size, p.device_buffer, asked.sample_rate, p.device_rate));
        }
        None => lines.push("Output open: none".into()),
    }
    // PipeWire through ALSA: the buffer, the period the callbacks see, the server's quantum (#230).
    lines.extend(period::stream_line(app));
    let output = app.player.as_ref().and_then(|p| p.backend_stats());
    // The input is the recorder's once a track takes it; until then a native duplex output holds
    // the one it opened with, and the panel reads that.
    let held = app.player.as_ref().and_then(|p| p.with_held_input(|r| (input_line(r, native), r.backend_stats())));
    let input = match &app.recorder {
        Some(r) => r.backend_stats(),
        None => held.as_ref().and_then(|(_, stats)| *stats),
    };
    #[cfg(test)]
    let (output, input) = app.audio_devices.injected_backend.unwrap_or((output, input));
    let (mut out_rate, mut in_rate) = app.audio_devices.wake_rates;
    let output_rate = output.and_then(|s| out_rate.update(now, s.wakes));
    let input_rate = input.and_then(|s| in_rate.update(now, s.wakes));
    let keep = |present: bool, rate: WakeRate| if present { rate } else { WakeRate::default() };
    app.audio_devices.wake_rates = (keep(output.is_some(), out_rate), keep(input.is_some(), in_rate));
    // Each backend's own fields. The two directions of a duplex ASIO driver share its events and
    // timeouts: one line under the output with each direction's figures as `out / in`, while
    // both run the same switches ([`same_cadence`]); two lines otherwise (#127).
    let kind = stats_backend(app);
    let shared = match (output, input) {
        (Some(o), Some(i)) if kind == StatsBackend::Asio && same_cadence(&o, &i, output_rate, input_rate) => Some((o, i)),
        _ => None,
    };
    match shared {
        Some((o, i)) => {
            let rate = |r: Option<u64>| r.map(|r| format!("{r}/s"));
            let wakes = rate(output_rate).zip(rate(input_rate));
            lines.push(format!(
                "Backend (output / input, one ASIO driver): {}",
                StreamStats::asio_duplex_line(&o, &i, wakes.as_ref().map(|(a, b)| (a.as_str(), b.as_str())))
            ));
        }
        None => lines.extend(output.map(|s| crate::telemetry::backend_line(&s, false, output_rate, kind))),
    }
    lines.push(match (&app.recorder, &held) {
        (Some(r), _) => input_line(r, native),
        (None, Some((line, _))) => line.clone(),
        (None, None) => "Input open: none yet (it opens when a track is armed or monitored)".into(),
    });
    lines.extend(input.filter(|_| shared.is_none()).map(|s| crate::telemetry::backend_line(&s, true, input_rate, kind)));
    // After a driver reset: what the automatic reopen did and does next, with its counts.
    lines.extend(app.reopen.status(app.audio_devices.panel.is_some()));
    lines
}

/// Whether the two directions of a duplex ASIO driver ran the same buffer switches, so one line can
/// carry both: their wake rates over the last panel samples within 1% (at least 2 a second) of each
/// other, or before there is a rate, their wake counts within 1% (at least 2). A direction that
/// stopped while the other runs (its rate 0, or its count behind) gets its own line.
pub fn same_cadence(output: &StreamStats, input: &StreamStats, output_rate: Option<u64>, input_rate: Option<u64>) -> bool {
    let close = |a: u64, b: u64| a.abs_diff(b) <= (a.max(b) / 100).max(2);
    match (output_rate, input_rate) {
        (Some(a), Some(b)) => close(a, b),
        _ => close(output.wakes, input.wakes),
    }
}

/// The backend whose counters the Backend lines show: the running native backend's, else (no
/// Player, a scripted snapshot) the configured audio system's.
fn stats_backend(app: &StudioApp) -> StatsBackend {
    let system = app.player.as_ref().map_or(app.ui.audio.system.as_deref(), |p| Some(p.device_system.as_str()));
    if devices::is_asio_system(system) {
        StatsBackend::Asio
    } else if devices::is_jack_system(system) {
        StatsBackend::Jack
    } else {
        StatsBackend::Wasapi
    }
}

/// "Requested 128 · running at 256": the buffer size or rate the window asked for next to the one
/// the device runs, only where they differ. PipeWire's ALSA plug-in runs a stream at the server's
/// quantum, and any backend can step down to what a device offers; the open stream's own figures
/// (the Player's device status) are the achieved ones, except that the rate half reflects the rate
/// of the open attempt, not a value the device reported. `None` where nothing was asked for or the
/// request was honored.
pub fn requested_note(asked_buffer: Option<u32>, buffer: Option<u32>, asked_rate: Option<u32>, rate: u32) -> Option<String> {
    let mut parts = Vec::new();
    if let (Some(asked), Some(got)) = (asked_buffer, buffer)
        && asked != got
    {
        parts.push(format!("Requested {asked} · running at {got}"));
    }
    if let Some(asked) = asked_rate
        && asked != rate
    {
        parts.push(format!("Requested {asked} Hz · running at {rate} Hz"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The "Input open:" line of an input, the recorder's or the player's held one.
fn input_line(r: &transit_playback::record::Recorder, native: bool) -> String {
    if native {
        let buffer = r.buffer_frames.map_or(String::new(), |b| format!(" · {b}-sample buffer"));
        let detail = achieved_detail(r.sample_format, r.latency, "in");
        let priority = priority_detail(r.thread_priority());
        format!("Input open: {} · {} Hz · {} ch{buffer}{detail}{priority}", r.device_name, r.sample_rate, r.channels)
    } else {
        format!("Input open: {} · {} Hz · {} ch", r.device_name, r.sample_rate, r.channels)
    }
}

/// The scheduling of a native stream's worker as its "open" line ends: " · MMCSS Pro Audio" (the
/// WASAPI worker, the ASIO buffer-switch thread once it switched), " · MMCSS pending" before that,
/// " · MMCSS by driver" where the ASIO driver registered its own thread, and so on; nothing where
/// the backend observes none.
pub fn priority_detail(priority: transit_playback::scheduling::WorkerPriority) -> String {
    use transit_playback::scheduling::WorkerPriority as P;
    let text = match priority {
        P::Pending => "MMCSS pending",
        P::MmcssAudio => "MMCSS Audio",
        P::MmcssProAudio => "MMCSS Pro Audio",
        P::DriverRegistered => "MMCSS by driver",
        P::RaisedPriority => "raised priority",
        P::PromotionFailed => "MMCSS refused",
        P::Released => "MMCSS released",
        P::ReleaseFailed => "MMCSS release failed",
        _ => return String::new(),
    };
    format!(" · {text}")
}

/// What a native stream reported at open, for its "open" line: " · i24 · 15.8 ms out" (each part
/// only where reported). `direction` is "out" or "in".
pub fn achieved_detail(sample: Option<impl std::fmt::Debug>, latency: Option<std::time::Duration>, direction: &str) -> String {
    let mut detail = String::new();
    if let Some(sample) = sample {
        detail.push_str(&format!(" · {}", format!("{sample:?}").to_lowercase()));
    }
    if let Some(latency) = latency {
        detail.push_str(&format!(" · {:.1} ms {direction}", latency.as_secs_f64() * 1000.0));
    }
    detail
}

/// "<driver>: <reason>" for a diagnostic, without the repeated `ASIO driver "<driver>": ` prefix.
fn short_problem(problem: &str) -> String {
    if let Some((driver, reason)) = problem.split_once(": ") {
        let prefix = format!("ASIO driver \"{driver}\": ");
        if let Some(rest) = reason.strip_prefix(&prefix) {
            return format!("{driver}: {rest}");
        }
    }
    problem.to_string()
}

/// `audio.fake_devices {systems: [{name, is_default?, outputs: [...], inputs: [...], diagnostics?: [...]}]}`:
/// stand-in devices for screenshot runs and tests (each device `{name, is_default?, max_channels?,
/// sample_rates?, buffer_min?, buffer_max?}`, each diagnostic a "<driver>: <problem>" text); `{systems: null}` removes them.
pub fn fake_from_json(p: &serde_json::Value) -> Result<Option<FakeDevices>, String> {
    use serde_json::Value;
    let Some(list) = p.get("systems") else { return Err("audio.fake_devices needs systems".into()) };
    if list.is_null() {
        return Ok(None);
    }
    let Some(list) = list.as_array() else { return Err("systems must be a list".into()) };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let flag = |v: &Value, k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let devs = |v: &Value, k: &str| -> Result<Vec<DeviceInfo>, String> {
        v.get(k)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|d| {
                Ok(DeviceInfo {
                    name: text(d, "name").ok_or("every device needs a name")?,
                    is_default: flag(d, "is_default"),
                    max_channels: d.get("max_channels").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()).unwrap_or(2),
                    sample_rates: d
                        .get("sample_rates")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|r| r.as_u64().and_then(|r| u32::try_from(r).ok())).collect())
                        .unwrap_or_else(|| vec![44_100, 48_000]),
                    buffer_min: Some(d.get("buffer_min").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(64)),
                    buffer_max: Some(d.get("buffer_max").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).unwrap_or(2048)),
                })
            })
            .collect()
    };
    let systems = list
        .iter()
        .map(|s| {
            Ok(AudioSystem {
                name: text(s, "name").ok_or("every system needs a name")?,
                is_default: flag(s, "is_default"),
                outputs: devs(s, "outputs")?,
                inputs: devs(s, "inputs")?,
                diagnostics: s
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|d| d.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut fake = FakeDevices { systems, ..Default::default() };
    fake.connection = Some(crate::audio_connection::FakeConnection::new(AudioConfig::default(), std::rc::Rc::clone(&fake.trace)));
    Ok(Some(fake))
}
