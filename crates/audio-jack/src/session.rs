//! The backend over a [`JackApi`]: enumeration through a probe client, and one client "transit" per
//! open session (ADR 0021, decision 4).
//!
//! - The first `open_capture` or `open_render` opens the client and registers one port per channel
//!   (`in_1..` or `out_1..`); the user callback waits with the session. The other direction joins
//!   the same client while neither has started (the Player opens capture first, as on ASIO). An
//!   open after a start is refused, as is a second open of one direction.
//! - The open makes no rate or buffer size read under PipeWire before activation (an inactive
//!   client reports its own request or the server's default, not what the graph runs) and asks for
//!   a chosen buffer size on every client it opens; under JACK 2 it asks only for a size other than
//!   the server's, and reads both, as before (ADR 0021, amendment for #760, rules 1 and 2).
//!   `Achieved` holds the plan's provisional figures.
//! - The first `start` builds the process engine from both directions' callbacks, installs the
//!   JACK callbacks and activates the client, settles the active client's rate and buffer size
//!   (`crate::settle`: the publication, the request window under PipeWire), and, when the graph
//!   runs the rate the caller prepared for, connects that direction's ports to its device's
//!   ports (`out_i` to the i-th playback port, the i-th capture port to `in_i`; not when the backend
//!   was made without auto-connect); the other direction's `start` connects its own. Connections a
//!   user makes in a patchbay later are left alone, and a reopen brings them back: the reopen
//!   reads them before the old client goes and restores them on the new one (`crate::links`,
//!   [`ab::Stream::snapshot_links`]). A stop is for good (it returns once no process
//!   callback is inside that direction); when no direction runs, the client is deactivated.
//!   Dropping the last stream deactivates and closes it, and only then frees the engine. Under
//!   another rate the start latches a rebuild instead: neither direction starts, no port connects,
//!   and the stream says so ([`ab::Stream::needs_rebuild`]); the caller reopens. Under PipeWire the
//!   backend remembers the last rate an active session observed and plans the next open at it (a
//!   hint: a server shutdown clears it).
//! - The watcher thread (`transit-jack-watch`, every 20 ms while the client is active) reads the
//!   flags the callbacks set ([`Shared`]) and calls the streams' error callbacks: a server
//!   shutdown is [`ab::StreamError::DeviceNotAvailable`] once, and the dead client is closed there
//!   (jack.h: `jack_client_close` "cannot be called directly in the context of the thread that calls
//!   the shutdown callback"); the watcher then polls for the server (a probe client every 2 s for
//!   30 s) and, when one answers, requests a reset ([`ab::StreamError::ResetRequested`]), which the
//!   Player's reopen path serves. A sample rate or a quantum other than the settled ones is a reset
//!   request once per new pair (the Player reopens at the server's new settings). A user callback
//!   that panicked is [`ab::StreamError::Other`] once to its direction. Xruns are counted. Error
//!   callbacks run with no lock held, so one may stop its stream.

use crate::api::{JackApi, JackClient};
use crate::devices::{self, JackDevice, Server, ServerState};
use crate::engine::{self, Engine, EngineSide, Shared};
use crate::links;
use crate::settle::{self, Init};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use transit_audio_backend as ab;

/// How often the watcher reads the callbacks' flags.
pub(crate) const WATCH_PERIOD: Duration = Duration::from_millis(20);
/// The watcher thread's name.
pub(crate) const WATCHER_NAME: &str = "transit-jack-watch";
/// After a server shutdown: how often the watcher looks for the server again, and for how long.
pub(crate) const RECONNECT_PERIOD: Duration = Duration::from_secs(2);
pub(crate) const RECONNECT_WINDOW: Duration = Duration::from_secs(30);
/// How long a stop waits for a process callback to leave the stopped direction.
const QUIESCE_LIMIT: Duration = Duration::from_secs(1);
/// The environment variable that turns the automatic connections off (`0`, `no`, `off`, `false`):
/// transit's ports are then left for a patchbay to connect.
pub const AUTO_CONNECT_ENV: &str = "TRANSIT_JACK_AUTOCONNECT";

type Slot = Arc<Mutex<Option<Session>>>;
type SharedError = Arc<Mutex<ab::ErrorCallback>>;

fn lock(slot: &Mutex<Option<Session>>) -> MutexGuard<'_, Option<Session>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The timing knobs of the watcher (shorter in tests) and of the start's settling.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) watch: Duration,
    pub(crate) reconnect_period: Duration,
    pub(crate) reconnect_window: Duration,
    /// The start's request window and its step (the watcher period, which tests that drive the
    /// watcher by hand lengthen; the window keeps its own).
    pub(crate) window: Duration,
    pub(crate) window_step: Duration,
    pub(crate) init: Init,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            watch: WATCH_PERIOD,
            reconnect_period: RECONNECT_PERIOD,
            reconnect_window: RECONNECT_WINDOW,
            window: settle::REQUEST_WINDOW,
            window_step: WATCH_PERIOD,
            init: Init::default(),
        }
    }
}

/// The native JACK audio system. `Send` and `Sync`; clones share the session.
#[derive(Clone)]
pub struct JackBackend {
    api: Arc<dyn JackApi>,
    slot: Slot,
    next_generation: Arc<AtomicU64>,
    timing: Timing,
    auto_connect: bool,
    /// Under PipeWire, the last rate an active session observed (0: none): the next plan's rate.
    remembered: Arc<AtomicU32>,
}

/// Whether the value of [`AUTO_CONNECT_ENV`] keeps the automatic connections (unset or anything
/// but `0`, `no`, `off`, `false`).
pub fn auto_connect_from(value: Option<&str>) -> bool {
    !value.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "no" | "off" | "false"))
}

/// The server as the library names itself: "PipeWire ..." for PipeWire's libjack, "JACK 1.9.20"
/// for JACK 2, "JACK" when the library does not say.
pub fn server_name(version: Option<&str>) -> String {
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.to_ascii_lowercase().contains("pipewire") => {
            let at = v.to_ascii_lowercase().find("pipewire").unwrap_or(0);
            let name = v[at..].trim_end_matches(')').trim();
            format!("PipeWire{}", &name["pipewire".len().min(name.len())..])
        }
        Some(v) => format!("JACK {v}"),
        None => "JACK".to_owned(),
    }
}

impl JackBackend {
    pub(crate) fn with_api(api: Arc<dyn JackApi>) -> Self {
        Self {
            api,
            slot: Arc::default(),
            next_generation: Arc::new(AtomicU64::new(1)),
            timing: Timing::default(),
            auto_connect: true,
            remembered: Arc::default(),
        }
    }

    /// Whether the library is PipeWire's (its version string names it, as [`server_name`] reads it).
    fn pipewire(&self) -> bool {
        self.server_name().starts_with("PipeWire")
    }

    /// The rate a plan opens at for a group the probe listed at `listed`: under PipeWire the
    /// remembered rate when there is one (rule 1: a hint, used alike for the render request, the
    /// capture format and the provisional `Achieved`), else the listing's.
    fn planned_rate(&self, listed: u32) -> u32 {
        match self.remembered.load(Ordering::Relaxed) {
            0 => listed,
            rate if self.pipewire() => rate,
            _ => listed,
        }
    }

    /// The rate the backend remembers (tests).
    #[cfg(test)]
    pub(crate) fn remembered_rate(&self) -> Option<u32> {
        Some(self.remembered.load(Ordering::Relaxed)).filter(|r| *r != 0)
    }

    /// This backend without the automatic connections: transit's ports are registered and left for
    /// a patchbay (or the caller) to connect.
    pub fn without_auto_connect(mut self) -> Self {
        self.auto_connect = false;
        self
    }

    /// The server as the library names itself ([`server_name`]).
    pub fn server_name(&self) -> String {
        server_name(self.api.version().as_deref())
    }

    /// The Audio Devices line about the library (#230): "JACK library: <path>" once one connected,
    /// else every library tried with what it did; `None` before any probe.
    pub fn library_note(&self) -> Option<String> {
        self.api.library().map(|selection| selection.summary())
    }

    /// "Native JACK (<server>)": how the Audio Devices panel names this backend over its server.
    pub fn label(&self) -> String {
        format!("Native JACK ({})", self.server_name())
    }

    /// Opens a short-lived probe client (`JackNoStartServer`: no server is ever started), reads the
    /// server's rate, quantum, physical ports and eligible virtual devices, and closes it. A
    /// control-thread call, never under the session's lock.
    pub fn probe(&self) -> ServerState {
        let client = match self.api.open_client(crate::PROBE_NAME) {
            Ok(client) => client,
            Err(error) => {
                if let crate::api::ClientError::LibraryMissing(text) = &error {
                    log::info!("JACK: libjack did not load: {text}");
                }
                return ServerState::from_error(error);
            }
        };
        let (rate, buffer) = (client.sample_rate(), client.buffer_size());
        let physical = client.physical_ports();
        let devices = devices::group(&physical, rate, buffer);
        // The clients transit owns, by the names the server assigned (rule 1: either may have
        // been made unique): this probe, and the session's client while one is open.
        let mut owned = vec![client.name()];
        owned.extend(lock(&self.slot).as_ref().and_then(|s| s.client.as_ref().map(|c| c.name())));
        let virtual_devices = devices::virtual_groups(&client.output_ports(), &physical, &owned, rate, buffer);
        if let Err(e) = client.close() {
            log::warn!("JACK: closing the probe client: {e}");
        }
        ServerState::Running(Server { rate, buffer, devices, virtual_devices })
    }

    fn running(&self) -> Result<Server, String> {
        match self.probe() {
            ServerState::Running(server) => Ok(server),
            other => Err(other.diagnostic().unwrap_or_default()),
        }
    }

    /// The group named `device`, else the first physical group with ports in `direction`. An input
    /// is named among every eligible group, physical or virtual, whatever the preference (rule 2);
    /// a named input that is not there is absent (rule 4).
    fn find(&self, direction: ab::Direction, device: Option<&str>) -> Result<ab::Endpoint, String> {
        let server = self.running()?;
        let usable = |d: &JackDevice| !d.ports(direction).is_empty();
        let found = match device {
            Some(name) if direction == ab::Direction::Input => server.input(name).ok_or_else(|| ab::absent_input(name))?,
            Some(name) => {
                let d = server.devices.iter().find(|d| d.name == name).ok_or_else(|| format!("JACK: no port group named \"{name}\""))?;
                if !usable(d) {
                    return Err(format!("JACK: \"{name}\" has no {direction:?} ports"));
                }
                d
            }
            None => server.devices.iter().find(|d| usable(d)).ok_or_else(|| format!("JACK: no port group has {direction:?} ports"))?,
        };
        Ok(found.endpoint(direction))
    }

    /// The one request to try: `f32`, every port of the group, at the planned rate.
    fn plan(&self, endpoint: ab::Endpoint, direction: ab::Direction, period: Option<u32>) -> Result<ab::Plan, String> {
        let device = endpoint.handle::<JackDevice>().ok_or("the endpoint is not a JACK port group")?;
        let format = ab::Format::new(self.planned_rate(device.rate), clamp(device.ports(direction).len()), ab::SampleFormat::F32);
        let request = ab::OpenRequest {
            format,
            // A chosen buffer size is asked of the server when the client opens (JACK 2: the
            // quantum of every client of the server; PipeWire: a force on transit's own node while
            // it is active); none keeps the server's.
            period: period.map_or(ab::Period::Default, ab::Period::Frames),
            mode: ab::Mode::Shared,
            origin: ab::HostOrigin::default(),
            calibration: None,
        };
        Ok(ab::Plan { endpoint, attempts: vec![request] })
    }

    /// Opens one direction on the session's client (opening the client first when there is none).
    pub(crate) fn open_side(
        &self,
        endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        callback: Pending,
        error: ab::ErrorCallback,
    ) -> Result<(JackStream, ab::Achieved), ab::OpenError> {
        let device = endpoint.handle::<JackDevice>().ok_or_else(|| ab::OpenError::Other("the endpoint is not a JACK port group".into()))?;
        let channels = request.format.channels;
        if channels == 0 {
            return Err(ab::OpenError::Other("JACK: a stream needs at least one channel".into()));
        }
        let direction = callback.direction();
        let mut guard = lock(&self.slot);
        if guard.is_none() {
            let pipewire = self.pipewire();
            let mut client = self.api.open_client(crate::CLIENT_NAME).map_err(|e| ab::OpenError::DeviceNotAvailable(e.to_string()))?;
            // A chosen buffer size is asked for as the client opens, here on the opening thread
            // (never a JACK callback). A refusal is not an error: the stream opens at the server's
            // quantum and the stream says the request was declined. Under JACK 2 it changes the
            // server's quantum for every client, so it is asked only when it differs. Under
            // PipeWire it is a force on transit's own node that holds only while this client is
            // active, so every client asks, with no read of an inactive client to compare with
            // (an inactive client reports its own request or the server's default, not what the
            // graph runs); it arrives after activation, inside the start's request window.
            let requested = request.period.frames();
            let current = if pipewire { None } else { Some(client.buffer_size()) };
            let declined = match requested.filter(|frames| current != Some(*frames)) {
                Some(frames) => {
                    client.set_buffer_size(frames).inspect_err(|e| log::warn!("JACK: the server did not take a {frames}-frame buffer: {e}")).is_err()
                }
                None => false,
            };
            // The provisional figures: the plan's under PipeWire; under JACK 2 the client's, as
            // before. The start settles the active client's (rule 1).
            let opened = if pipewire {
                Opened { rate: request.format.rate, buffer: device.buffer, pipewire, requested, declined }
            } else {
                Opened { rate: client.sample_rate(), buffer: client.buffer_size(), pipewire, requested, declined }
            };
            let id = self.next_generation.fetch_add(1, Ordering::Relaxed);
            *guard = Some(Session::new(client, opened, request.format.rate, id, Arc::clone(&self.remembered)));
        }
        let Some(session) = guard.as_mut() else { return Err(ab::OpenError::Other("JACK: no session".into())) };
        // The provisional rate (PipeWire: the plan's; JACK 2: the client's, whatever was asked, as
        // before). The start rebuilds when the active graph runs another than the prepared one.
        let format = ab::Format::new(session.rate, channels, ab::SampleFormat::F32);
        let waiting = Waiting { callback, format, origin: request.origin };
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let opened = match session.attach(device, channels, waiting, error, self.auto_connect, generation) {
            Ok(generation) => generation,
            Err(e) => {
                // A client opened for this request alone is closed again.
                if session.sides.is_empty() {
                    let closing = guard.take();
                    drop(guard);
                    if let Some(session) = closing {
                        session.close();
                    }
                }
                return Err(e);
            }
        };
        let achieved = ab::Achieved {
            format,
            period: ab::Period::Frames(session.buffer),
            latency: None,
            device_position: None,
            min_period: None,
            buffer_frames: Some(session.buffer),
        };
        let stream = JackStream {
            slot: Arc::clone(&self.slot),
            api: Arc::clone(&self.api),
            session: session.id,
            generation: opened,
            direction,
            shared: Arc::clone(&session.shared),
            timing: self.timing,
        };
        Ok((stream, achieved))
    }

    /// The session's client name, while one is open (tests and the device report).
    #[cfg(test)]
    pub(crate) fn client_name(&self) -> Option<String> {
        lock(&self.slot).as_ref().and_then(|s| s.client.as_ref().map(|c| c.name()))
    }

    /// The open session's callback state (tests against a real server).
    #[cfg(all(test, target_os = "linux", feature = "libjack"))]
    pub(crate) fn session_shared(&self) -> Option<Arc<Shared>> {
        lock(&self.slot).as_ref().map(|s| Arc::clone(&s.shared))
    }

    /// Whether a session is open.
    #[cfg(test)]
    pub(crate) fn has_session(&self) -> bool {
        lock(&self.slot).is_some()
    }

    /// Runs `f` on the session's client (tests: connect transit's own ports, read connections).
    #[cfg(test)]
    pub(crate) fn with_client<R>(&self, f: impl FnOnce(&mut dyn JackClient) -> R) -> Option<R> {
        let mut guard = lock(&self.slot);
        let client = guard.as_mut()?.client.as_mut()?;
        Some(f(client.as_mut()))
    }

    /// Opens a client of its own on the backend's library (tests: a second client that changes the
    /// server's quantum).
    #[cfg(test)]
    #[cfg_attr(not(all(target_os = "linux", feature = "libjack")), allow(dead_code))]
    pub(crate) fn open_other_client(&self, name: &str) -> Result<Box<dyn JackClient>, crate::api::ClientError> {
        self.api.open_client(name)
    }

    /// One watcher tick of the open session, on the caller's thread (tests drive the ticks
    /// explicitly with a watcher period long enough that its own thread never ticks).
    #[cfg(test)]
    pub(crate) fn tick(&self) {
        let id = lock(&self.slot).as_ref().map(|s| s.id);
        if let Some(id) = id {
            let _ = watch_once(&Arc::downgrade(&self.slot), id);
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timing(mut self, watch: Duration, reconnect_period: Duration, reconnect_window: Duration) -> Self {
        self.timing = Timing { watch, reconnect_period, reconnect_window, ..self.timing };
        self
    }

    /// This backend settling its first figures from the first process cycle (rule 1's fallback).
    #[cfg(test)]
    pub(crate) fn with_first_cycle_init(mut self) -> Self {
        self.timing.init = Init::FirstCycle;
        self
    }
}

fn clamp(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

impl ab::Backend for JackBackend {
    fn system(&self) -> &str {
        crate::SYSTEM
    }

    /// The port groups with ports in `direction`; a missing library or server is an error with
    /// the panel's text.
    fn endpoints(&self, direction: ab::Direction) -> Result<Vec<ab::Endpoint>, ab::OpenError> {
        self.running().map(|server| server.endpoints(direction)).map_err(ab::OpenError::DeviceNotAvailable)
    }

    fn plan_render(&self, wish: &ab::OutputWish) -> Result<ab::Plan, String> {
        self.plan(self.find(ab::Direction::Output, wish.device.as_deref())?, ab::Direction::Output, wish.period)
    }

    fn plan_capture(&self, wish: &ab::InputWish) -> Result<ab::Plan, String> {
        self.plan(self.find(ab::Direction::Input, wish.device.as_deref())?, ab::Direction::Input, wish.period)
    }

    /// One client, one process callback for both directions.
    fn capture_with_render(&self, render: &ab::Endpoint) -> bool {
        render.handle::<JackDevice>().is_some()
    }

    /// The port group the duplex input links `in_N` to (#682): the one client records from any
    /// group, so a chosen input is the eligible group of that name, physical or virtual (ADR 0015,
    /// amendment for #431, rule 2). A chosen input that is not there opens no input and says so by
    /// name; no other group is substituted (rule 4). With no choice, the output's own group when it
    /// has capture ports, else the first physical group that has some: never a virtual device
    /// (rule 1, "No choice made").
    fn duplex_capture_endpoint(&self, render: &ab::Endpoint, wish: &ab::InputWish) -> Result<ab::Endpoint, String> {
        let device = render.handle::<JackDevice>().ok_or("the endpoint is not a JACK port group")?;
        if let Some(name) = wish.device.as_deref() {
            return self.find(ab::Direction::Input, Some(name)).inspect_err(|why| log::warn!("JACK: {why}"));
        }
        if !device.capture.is_empty() {
            return Ok(render.clone());
        }
        self.find(ab::Direction::Input, None)
    }

    /// One process callback runs the capture callback, then the render callback (K22: the live
    /// input ring aligns with no slack).
    fn capture_precedes_render(&self, endpoint: &ab::Endpoint) -> bool {
        endpoint.handle::<JackDevice>().is_some()
    }

    /// `f32`, every capture port of the group, at the planned rate, the render plan's (a requested
    /// rate is not applied: the server dictates it).
    fn capture_format(&self, endpoint: &ab::Endpoint, _rate: Option<u32>) -> Result<ab::Format, String> {
        let device = endpoint.handle::<JackDevice>().ok_or("the endpoint is not a JACK port group")?;
        Ok(ab::Format::new(self.planned_rate(device.rate), clamp(device.capture.len()), ab::SampleFormat::F32))
    }

    /// The ports' short names.
    fn channel_names(&self, endpoint: &ab::Endpoint, direction: ab::Direction) -> Vec<String> {
        endpoint.handle::<JackDevice>().map(|d| d.channel_names(direction)).unwrap_or_default()
    }

    /// A virtual device's capture ports' short names, in order (rule 5's port-mapping guard);
    /// nothing for a physical group, whose ports do not change between opens.
    fn guarded_ports(&self, endpoint: &ab::Endpoint) -> Vec<String> {
        endpoint.handle::<JackDevice>().filter(|d| d.virtual_device).map(|d| d.channel_names(ab::Direction::Input)).unwrap_or_default()
    }

    /// Registers `out_1..out_n` on the session's client (opening it when there is none). The
    /// stream opens at the server's rate and quantum whatever was requested; `Achieved` says which.
    fn open_render(
        &self,
        endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        render: ab::RenderCallback,
        error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        let (stream, achieved) = self.open_side(endpoint, request, Pending::Render(render), error)?;
        Ok((Box::new(stream), achieved))
    }

    /// Registers `in_1..in_n` on the session's client, before either direction starts.
    fn open_capture(
        &self,
        endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        capture: ab::CaptureCallback,
        error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        let (stream, achieved) = self.open_side(endpoint, request, Pending::Capture(capture), error)?;
        Ok((Box::new(stream), achieved))
    }
}

/// A user callback waiting for the client's activation.
pub(crate) enum Pending {
    Render(ab::RenderCallback),
    Capture(ab::CaptureCallback),
}

impl Pending {
    fn direction(&self) -> ab::Direction {
        match self {
            Pending::Render(_) => ab::Direction::Output,
            Pending::Capture(_) => ab::Direction::Input,
        }
    }
}

/// A user callback with its format and origin, until the engine is built.
struct Waiting {
    callback: Pending,
    format: ab::Format,
    origin: ab::HostOrigin,
}

/// One direction of the session.
struct Side {
    generation: u64,
    direction: ab::Direction,
    /// The user callback until the first start moves it into the engine.
    waiting: Option<Waiting>,
    /// (source, destination) pairs to connect at this side's start: transit's own links.
    connections: Vec<(String, String)>,
    /// The full names of this side's ports, by index, and what each stands for (#431, rule 6).
    names: Vec<String>,
    meaning: links::Meaning,
    error: SharedError,
    /// A panic of this side's user callback was reported.
    panic_reported: bool,
}

impl Side {
    fn output(&self) -> bool {
        self.direction == ab::Direction::Output
    }
}

/// The watcher thread of an active client.
struct Watcher {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Watcher {
    /// Stops the thread and joins it, unless this is the watcher itself (an error callback that
    /// stopped its stream): then it ends after the current tick.
    fn finish(self) {
        self.stop.store(true, Ordering::Release);
        if std::thread::current().id() == self.thread.thread().id() {
            return;
        }
        self.thread.thread().unpark();
        if self.thread.join().is_err() {
            log::warn!("JACK: the watcher thread panicked");
        }
    }
}

/// What the open of the session's client found and asked for.
struct Opened {
    /// The provisional figures, until the start settles the active client's.
    rate: u32,
    buffer: u32,
    pipewire: bool,
    requested: Option<u32>,
    declined: bool,
}

/// The open client and its directions.
pub(crate) struct Session {
    id: u64,
    /// `None` once the server shut the client down (the watcher closed it).
    client: Option<Box<dyn JackClient>>,
    /// The session's rate and buffer size: provisional until the first start, then settled from
    /// the active client (rule 1).
    rate: u32,
    buffer: u32,
    /// The rate the caller (the Player and its recorder) prepared for: the plan's.
    prepared_rate: u32,
    pipewire: bool,
    requested: Option<u32>,
    declined: bool,
    /// The start settled the figures.
    settled: bool,
    /// The reset latch: the graph runs another rate than the prepared one; nothing starts.
    rebuild: bool,
    /// The backend's remembered rate (PipeWire), shared with it.
    remembered: Arc<AtomicU32>,
    /// The watcher logged the first process cycle's figures.
    first_cycle_logged: bool,
    /// Under the latch: the link snapshot a restore handed this client, for the rebuilt one.
    held: Option<links::Snapshot>,
    shared: Arc<Shared>,
    sides: Vec<Side>,
    active: bool,
    watcher: Option<Watcher>,
    /// The shutdown was reported to the error callbacks.
    shutdown_reported: bool,
    /// The last (rate, quantum) a reset was requested for.
    reset_for: (u32, u32),
}

impl Session {
    fn new(client: Box<dyn JackClient>, opened: Opened, prepared_rate: u32, id: u64, remembered: Arc<AtomicU32>) -> Self {
        let Opened { rate, buffer, pipewire, requested, declined } = opened;
        Self {
            id,
            client: Some(client),
            rate,
            buffer,
            prepared_rate,
            pipewire,
            requested,
            declined,
            settled: false,
            rebuild: false,
            remembered,
            first_cycle_logged: false,
            held: None,
            // Unknown until the client is active (rule 1, "Publication").
            shared: Arc::new(Shared::new(engine::UNKNOWN, engine::UNKNOWN)),
            sides: Vec::new(),
            active: false,
            watcher: None,
            shutdown_reported: false,
            reset_for: (rate, buffer),
        }
    }

    fn side_mut(&mut self, generation: u64) -> Option<&mut Side> {
        self.sides.iter_mut().find(|s| s.generation == generation)
    }

    /// Registers `channels` ports of `waiting`'s direction and plans their connections to `device`.
    fn attach(
        &mut self,
        device: &JackDevice,
        channels: u16,
        waiting: Waiting,
        error: ab::ErrorCallback,
        auto_connect: bool,
        generation: u64,
    ) -> Result<u64, ab::OpenError> {
        let direction = waiting.callback.direction();
        if self.active || self.sides.iter().any(|s| self.shared.side(s.output()).state() != engine::OPENED) {
            return Err(ab::OpenError::Other("JACK: open both directions before starting either; reopen the device to add a direction".into()));
        }
        if self.sides.iter().any(|s| s.direction == direction) {
            return Err(ab::OpenError::InUse(format!("JACK: transit's {direction:?} ports are already open")));
        }
        let client = self.client.as_mut().ok_or_else(|| ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into()))?;
        let input = direction == ab::Direction::Input;
        // A direction dropped before any start left its state detached; this open starts it afresh.
        self.shared.side(!input).set_state(engine::OPENED);
        let device_ports = device.ports(direction);
        let mut connections = Vec::with_capacity(usize::from(channels));
        let mut names = Vec::with_capacity(usize::from(channels));
        for i in 0..usize::from(channels) {
            let short = if input { format!("in_{}", i + 1) } else { format!("out_{}", i + 1) };
            let ours = client.register_port(&short, input).map_err(|e| ab::OpenError::Other(format!("JACK: registering {short}: {e}")))?;
            if let Some(theirs) = device_ports.get(i).filter(|_| auto_connect) {
                connections.push(if input { (theirs.clone(), ours.clone()) } else { (ours.clone(), theirs.clone()) });
            }
            names.push(ours);
        }
        // Port i stands for the device's i-th port, linked or not (auto-connect off).
        let channels = (0..names.len()).map(|i| device_ports.get(i).cloned().unwrap_or_default()).collect();
        let meaning = links::Meaning { endpoint: device.name.clone(), channels };
        let error = Arc::new(Mutex::new(error));
        self.sides.push(Side { generation, direction, waiting: Some(waiting), connections, names, meaning, error, panic_reported: false });
        Ok(generation)
    }

    /// The process engine over every waiting user callback.
    fn build_engine(&mut self) -> Engine {
        let mut engine = Engine::new(Arc::clone(&self.shared));
        for side in &mut self.sides {
            match side.waiting.take() {
                Some(Waiting { callback: Pending::Render(callback), format, origin }) => {
                    engine.render = Some(EngineSide::new(callback, format, origin));
                }
                Some(Waiting { callback: Pending::Capture(callback), format, origin }) => {
                    engine.capture = Some(EngineSide::new(callback, format, origin));
                }
                None => {}
            }
        }
        engine
    }

    /// Starts one direction: activates the client at the first start and settles its figures, then
    /// connects its ports; under a latched rebuild neither.
    fn start(&mut self, generation: u64, slot: &Slot, api: &Arc<dyn JackApi>, timing: Timing) -> Result<(), ab::OpenError> {
        let output = self.side_mut(generation).map(|s| s.output()).ok_or_else(|| ab::OpenError::Other("JACK: the stream was closed".into()))?;
        match self.shared.side(output).state() {
            engine::STARTED => return Ok(()),
            engine::OPENED => {}
            _ => return Err(ab::OpenError::Other("JACK: a stopped stream is reopened, not restarted".into())),
        }
        if self.client.is_none() {
            return Err(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into()));
        }
        if !self.active {
            let engine = self.build_engine();
            let shared = Arc::clone(&self.shared);
            let Some(client) = self.client.as_mut() else { return Err(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())) };
            client.activate(shared, engine).map_err(|e| ab::OpenError::Other(format!("JACK: activating the client: {e}")))?;
            self.active = true;
            self.settle(api.as_ref(), timing)?;
            if self.rebuild {
                return Ok(());
            }
            match spawn_watcher(Arc::downgrade(slot), self.id, Arc::clone(api), timing) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(e) => log::warn!("JACK: the watcher thread did not start ({e}); a server shutdown will not be reported"),
            }
        }
        if self.rebuild {
            return Ok(());
        }
        let connections = self.side_mut(generation).map(|s| s.connections.clone()).unwrap_or_default();
        if let Some(client) = self.client.as_mut() {
            for (source, destination) in &connections {
                if let Err(e) = client.connect(source, destination) {
                    log::warn!("JACK: connecting {source} to {destination}: {e}");
                }
            }
        }
        self.shared.side(output).set_state(engine::STARTED);
        Ok(())
    }

    /// After activation, on this thread under the session lock: settles the active client's rate
    /// and buffer size (`crate::settle`), commits them as the baseline the watcher compares with,
    /// and latches a rebuild when the rate is not the prepared one. A failed settle (a server
    /// shutdown in the request window, no cycle in first-cycle initialization) deactivates the
    /// client and fails the start.
    fn settle(&mut self, api: &dyn JackApi, timing: Timing) -> Result<(), ab::OpenError> {
        let ask = settle::Ask {
            pipewire: self.pipewire,
            requested: self.requested,
            declined: self.declined,
            prepared_rate: self.prepared_rate,
            init: timing.init,
        };
        let Some(client) = self.client.as_ref() else { return Err(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())) };
        let name = client.name();
        let settled = settle::settle(&self.shared, api, timing.window_step, timing.window, &ask, || (client.sample_rate(), client.buffer_size()));
        let settled = match settled {
            Ok(settled) => settled,
            Err(e) => {
                self.deactivate();
                return Err(e);
            }
        };
        (self.rate, self.buffer, self.settled) = (settled.rate, settled.buffer, true);
        self.reset_for = (settled.rate, settled.buffer);
        if self.pipewire {
            self.remembered.store(settled.rate, Ordering::Relaxed);
        }
        log::info!("JACK: client \"{name}\" active at {} Hz, quantum {}", self.rate, self.buffer);
        if settled.rebuild {
            self.rebuild = true;
            log::warn!("JACK: the graph runs {} Hz, not the {} Hz transit prepared for: rebuilding before any audio", self.rate, self.prepared_rate);
        }
        Ok(())
    }

    /// Ends one direction (`state`: stopped or detached) and waits for the process callback to
    /// leave it; when none runs, deactivates the client. Returns the watcher to finish with the lock
    /// released.
    fn end(&mut self, generation: u64, state: u8) -> Option<Watcher> {
        let output = self.side_mut(generation).map(|s| s.output())?;
        let side = self.shared.side(output);
        if side.state() == engine::STARTED || state == engine::DETACHED {
            side.set_state(state);
        }
        if !side.quiesce(QUIESCE_LIMIT) {
            log::warn!("JACK: a process callback did not leave the stopped direction within {QUIESCE_LIMIT:?}");
        }
        let running = [true, false].into_iter().any(|o| self.shared.side(o).state() == engine::STARTED);
        if running || !self.active {
            return None;
        }
        self.deactivate();
        self.watcher.take()
    }

    fn deactivate(&mut self) {
        if !std::mem::replace(&mut self.active, false) {
            return;
        }
        if let Some(client) = self.client.as_mut()
            && let Err(e) = client.deactivate()
        {
            log::warn!("JACK: deactivating the client: {e}");
        }
    }

    /// Ends the session: stops the watcher, deactivates and closes the client (which frees the
    /// engine and the user callbacks). Called with the session out of its slot (no lock held).
    fn close(mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.finish();
        }
        self.deactivate();
        if let Some(client) = self.client.take()
            && let Err(e) = client.close()
        {
            log::warn!("JACK: closing the client: {e}");
        }
    }

    /// One watcher tick: what to report and the dead client to close, read under the lock.
    fn watch(&mut self) -> Tick {
        if self.shared.shutdown() && !self.shutdown_reported {
            self.shutdown_reported = true;
            self.active = false;
            // The graph may come back at another rate: no hint survives a server shutdown.
            self.remembered.store(0, Ordering::Relaxed);
            for output in [true, false] {
                let side = self.shared.side(output);
                if side.state() == engine::STARTED {
                    side.set_state(engine::STOPPED);
                }
            }
            return Tick { reports: self.errors(ab::StreamError::DeviceNotAvailable), dead: self.client.take(), done: true };
        }
        let mut reports = Vec::new();
        for side in &mut self.sides {
            if !side.panic_reported && self.shared.side(side.output()).panicked() {
                side.panic_reported = true;
                reports.push((Arc::clone(&side.error), ab::StreamError::Other));
            }
        }
        // The figures of the first process cycle, once, for the tester's sitting (rule 9's Q1).
        if !self.first_cycle_logged && self.shared.cycles() > 0 {
            self.first_cycle_logged = true;
            log::info!("JACK: the first process cycle ran {} frames at {} Hz", self.shared.last_frames(), self.shared.sample_rate());
        }
        let now = (self.shared.sample_rate(), self.shared.buffer_size());
        if self.settled && now != (self.rate, self.buffer) && now != self.reset_for {
            self.reset_for = now;
            // An active session observed this rate: the reopen plans at it (PipeWire's hint).
            if self.pipewire && now.0 != engine::UNKNOWN {
                self.remembered.store(now.0, Ordering::Relaxed);
            }
            reports.extend(self.errors(ab::StreamError::ResetRequested));
        }
        Tick { reports, dead: None, done: false }
    }

    fn errors(&self, error: ab::StreamError) -> Vec<(SharedError, ab::StreamError)> {
        self.sides.iter().map(|s| (Arc::clone(&s.error), error)).collect()
    }

    /// Every direction's ports, what they stand for and transit's own links (#431, rule 6).
    fn ports(&self) -> Vec<links::Ports> {
        let side = |s: &Side| links::Ports { input: !s.output(), names: s.names.clone(), meaning: s.meaning.clone(), own: s.connections.clone() };
        self.sides.iter().map(side).collect()
    }

    /// The links of this active client's ports, read under the session lock before a reopen
    /// deactivates it. Nothing after a server shutdown: the client is gone (or going), and its
    /// links with it.
    ///
    /// A client that latched a rebuild holds the snapshot a restore handed it instead of its own
    /// links (it made none and restored none): the rebuilt client restores that one.
    fn snapshot(&mut self) -> Option<links::Snapshot> {
        if self.rebuild {
            return self.held.take();
        }
        if !self.active || self.shared.shutdown() {
            return None;
        }
        let client = self.client.as_ref()?;
        Some(links::take(client.as_ref(), &self.ports(), self.id, Arc::clone(&self.shared)))
    }

    /// Restores `snapshot`, taken from an earlier session, on this started client: what it did,
    /// with the links the server accepted and does not show yet left to [`Session::confirm`].
    fn restore(&mut self, snapshot: &links::Snapshot) -> Option<links::Restored> {
        if snapshot.from >= self.id {
            log::warn!("JACK: a link snapshot is restored only on a later client; discarded");
            return None;
        }
        if !self.active || self.shared.shutdown() {
            log::warn!("JACK: the client did not start; {} links made outside transit are not restored", snapshot.links.len());
            return None;
        }
        let ports = self.ports();
        let client = self.client.as_mut()?;
        Some(links::restore(client.as_mut(), &ports, snapshot))
    }

    /// One read of the links a restore still waits for (#431). `None` once this client is no
    /// longer started (stopped, or the server shut it down): nothing more will show.
    fn confirm(&self, done: &mut links::Restored) -> Option<bool> {
        if !self.active || self.shared.shutdown() {
            return None;
        }
        let client = self.client.as_ref()?;
        Some(links::confirm(client.as_ref(), done))
    }

    /// One period of the server's graph, the step of the restore's confirmation: at least 1 ms, at
    /// most the confirmation's whole limit.
    fn period(&self) -> Duration {
        let micros = u64::from(self.buffer) * 1_000_000 / u64::from(self.rate.max(1));
        Duration::from_micros(micros).clamp(Duration::from_millis(1), links::CONFIRM_LIMIT)
    }
}

/// What one watcher tick found.
struct Tick {
    reports: Vec<(SharedError, ab::StreamError)>,
    dead: Option<Box<dyn JackClient>>,
    /// The client is gone: the watcher looks for the server, then ends.
    done: bool,
}

/// What a watcher tick decided.
enum Next {
    Continue,
    End,
    /// The server shut the client down: look for it again on behalf of these callbacks.
    Reconnect(Vec<SharedError>, Arc<Shared>),
}

fn spawn_watcher(slot: Weak<Mutex<Option<Session>>>, id: u64, api: Arc<dyn JackApi>, timing: Timing) -> std::io::Result<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new().name(WATCHER_NAME.into()).spawn(move || {
        while !flag.load(Ordering::Acquire) {
            std::thread::park_timeout(timing.watch);
            if flag.load(Ordering::Acquire) {
                break;
            }
            match watch_once(&slot, id) {
                Next::Continue => {}
                Next::End => break,
                Next::Reconnect(errors, shared) => {
                    reconnect(&slot, id, api.as_ref(), timing, &errors, &shared);
                    break;
                }
            }
        }
    })?;
    Ok(Watcher { stop, thread })
}

/// One tick of the watcher of session `id`.
fn watch_once(slot: &Weak<Mutex<Option<Session>>>, id: u64) -> Next {
    let Some(slot) = slot.upgrade() else { return Next::End };
    let mut guard = lock(&slot);
    let Some(session) = guard.as_mut().filter(|s| s.id == id) else { return Next::End };
    let shared = Arc::clone(&session.shared);
    let tick = session.watch();
    let mut errors = Vec::new();
    if tick.done {
        // This thread outlives the client: dropping its own handle detaches it.
        session.watcher.take();
        errors = session.sides.iter().map(|s| Arc::clone(&s.error)).collect();
    }
    drop(guard);
    // The dead client is closed here, on the watcher, never in the shutdown handler.
    if let Some(dead) = tick.dead
        && let Err(e) = dead.close()
    {
        log::warn!("JACK: closing the client after the server stopped: {e}");
    }
    let (mut faulted, mut reset) = (false, false);
    for (callback, error) in tick.reports {
        match error {
            ab::StreamError::DeviceNotAvailable | ab::StreamError::Other if !faulted => {
                faulted = true;
                shared.count_fault();
                if error == ab::StreamError::DeviceNotAvailable {
                    log::warn!("JACK: the server stopped");
                } else {
                    log::warn!("JACK: a user callback panicked; it is not called again");
                }
            }
            ab::StreamError::ResetRequested if !reset => {
                reset = true;
                shared.count_reset();
                log::info!("JACK: the server now runs {} Hz, quantum {}: reopening", shared.sample_rate(), shared.buffer_size());
            }
            _ => {}
        }
        let mut callback = callback.lock().unwrap_or_else(PoisonError::into_inner);
        (*callback)(error);
    }
    if tick.done { Next::Reconnect(errors, shared) } else { Next::Continue }
}

/// After a server shutdown: a probe client every `reconnect_period` for `reconnect_window`, while
/// the session still exists; when the server answers, a reset request to `errors` (the Player's
/// reopen path opens a new client).
fn reconnect(slot: &Weak<Mutex<Option<Session>>>, id: u64, api: &dyn JackApi, timing: Timing, errors: &[SharedError], shared: &Shared) {
    let deadline = Instant::now() + timing.reconnect_window;
    while Instant::now() < deadline {
        std::thread::park_timeout(timing.reconnect_period);
        let alive = slot.upgrade().is_some_and(|slot| lock(&slot).as_ref().is_some_and(|s| s.id == id));
        if !alive {
            return;
        }
        if let Ok(probe) = api.open_client(crate::PROBE_NAME) {
            if let Err(e) = probe.close() {
                log::warn!("JACK: closing the probe client: {e}");
            }
            log::info!("JACK: the server is back: reopening");
            shared.count_reset();
            for callback in errors {
                let mut callback = callback.lock().unwrap_or_else(PoisonError::into_inner);
                (*callback)(ab::StreamError::ResetRequested);
            }
            return;
        }
    }
    log::warn!("JACK: no server answered within {:?}; reconnect by hand", timing.reconnect_window);
}

/// One direction of the session's client. Dropping it detaches the direction; the last one
/// deactivates and closes the client, and joins the watcher: drop it on a control thread.
pub(crate) struct JackStream {
    slot: Slot,
    api: Arc<dyn JackApi>,
    session: u64,
    generation: u64,
    direction: ab::Direction,
    shared: Arc<Shared>,
    timing: Timing,
}

impl JackStream {
    fn what(&self) -> &'static str {
        if self.direction == ab::Direction::Output { "render" } else { "capture" }
    }
}

impl ab::Stream for JackStream {
    /// Starts this direction (activating the client with the first one) and connects its ports.
    fn start(&self) -> Result<(), ab::OpenError> {
        let mut guard = lock(&self.slot);
        let session = guard
            .as_mut()
            .filter(|s| s.id == self.session)
            .ok_or_else(|| ab::OpenError::DeviceNotAvailable("JACK: the client of this stream is gone".into()))?;
        session.start(self.generation, &self.slot, &self.api, self.timing)
    }

    /// Stops this direction for good; returns once no process callback is inside it.
    fn stop(&self) -> Result<(), ab::OpenError> {
        let watcher = {
            let mut guard = lock(&self.slot);
            guard.as_mut().filter(|s| s.id == self.session).and_then(|s| s.end(self.generation, engine::STOPPED))
        };
        if let Some(watcher) = watcher {
            watcher.finish();
        }
        Ok(())
    }

    /// Port latencies are slice 3.
    fn latency(&self) -> Option<Duration> {
        None
    }

    /// The buffer size the start settled from the active client (ADR 0021, amendment for #760,
    /// rule 1); `None` before the start, or when it latched a rebuild.
    fn period(&self) -> Option<u32> {
        let guard = lock(&self.slot);
        guard.as_ref().filter(|s| s.id == self.session && s.settled && !s.rebuild).map(|s| s.buffer)
    }

    /// The start found the graph at another rate than the one the caller prepared for: no
    /// direction started, nothing connected; the caller reopens.
    fn needs_rebuild(&self) -> bool {
        lock(&self.slot).as_ref().is_some_and(|s| s.id == self.session && s.rebuild)
    }

    /// The server refused the buffer size the open asked for (`jack_set_buffer_size` failed).
    fn period_declined(&self) -> bool {
        lock(&self.slot).as_ref().is_some_and(|s| s.id == self.session && s.declined)
    }

    /// The process thread and its priority are the server's.
    fn thread_priority(&self) -> ab::WorkerPriority {
        ab::WorkerPriority::Unobserved
    }

    /// This direction's process callbacks as wakes, server xruns as overloads, rate and quantum
    /// changes and the server's return as resets, a server shutdown or a panic as a fault.
    fn stats(&self) -> Option<ab::StreamStats> {
        Some(self.shared.stats(self.direction == ab::Direction::Output))
    }

    /// The links of every port of this stream's client that are not transit's own (#431, rule 6),
    /// for the reopen operation to hold while this client closes. A control-thread call, before
    /// the reopen stops or drops anything; never a JACK callback or an error callback.
    fn snapshot_links(&self) -> Option<ab::LinkSnapshot> {
        let mut guard = lock(&self.slot);
        let snapshot = guard.as_mut().filter(|s| s.id == self.session)?.snapshot()?;
        Some(Box::new(snapshot))
    }

    /// Restores a snapshot of an earlier client of this backend on this stream's started client,
    /// once (it is consumed here). Another backend's snapshot is discarded.
    ///
    /// Links the server accepted but its graph does not show yet are read again once per period
    /// until they show or [`links::CONFIRM_LIMIT`] has passed (a graph switch the server postponed;
    /// see `links`). The wait is on this control thread with the session's lock released: the lock
    /// is held for the restore and then for each read only, as long as one read-back held it
    /// before.
    ///
    /// A client that latched a rebuild ([`ab::Stream::needs_rebuild`]) restores nothing: it is
    /// silent and about to be replaced. It keeps the snapshot, and the reopen that replaces it
    /// takes it back ([`ab::Stream::snapshot_links`]) for the rebuilt client to restore.
    fn restore_links(&self, snapshot: ab::LinkSnapshot) -> Option<String> {
        let snapshot = snapshot.downcast::<links::Snapshot>().ok()?;
        let (mut done, period) = {
            let mut guard = lock(&self.slot);
            let session = guard.as_mut().filter(|s| s.id == self.session)?;
            if session.rebuild {
                log::info!("JACK: the client latched a rebuild; {} links made outside transit wait for the rebuilt client", snapshot.links.len());
                session.held = Some(*snapshot);
                return None;
            }
            (session.restore(&snapshot)?, session.period())
        };
        let limit = u64::try_from(links::CONFIRM_LIMIT.as_micros()).unwrap_or(u64::MAX);
        let start = self.api.now_us();
        let mut why = None;
        while !done.pending.is_empty() && self.api.now_us().saturating_sub(start) < limit {
            self.api.await_graph(period);
            let guard = lock(&self.slot);
            if guard.as_ref().filter(|s| s.id == self.session).and_then(|s| s.confirm(&mut done)).is_none() {
                why = Some("transit's client stopped before the server showed it");
                break;
            }
        }
        links::finish(&mut done, why);
        done.note()
    }
}

impl Drop for JackStream {
    fn drop(&mut self) {
        let (closing, watcher) = {
            let mut guard = lock(&self.slot);
            let Some(session) = guard.as_mut().filter(|s| s.id == self.session) else { return };
            let watcher = session.end(self.generation, engine::DETACHED);
            session.sides.retain(|s| s.generation != self.generation);
            if session.sides.is_empty() { (guard.take(), watcher) } else { (None, watcher) }
        };
        if let Some(watcher) = watcher {
            watcher.finish();
        }
        if let Some(session) = closing {
            log::debug!("JACK: the last stream ({}) closed; closing the client", self.what());
            session.close();
        }
    }
}

const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<JackBackend>();
    shareable::<JackStream>();
};
