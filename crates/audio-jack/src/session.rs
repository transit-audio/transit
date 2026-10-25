//! The backend over a [`JackApi`]: enumeration through a probe client, and one client "transit" per
//! open session (ADR 0021, decision 4).
//!
//! - The first `open_capture` or `open_render` opens the client and registers one port per channel
//!   (`in_1..` or `out_1..`); the user callback waits with the session. The other direction joins
//!   the same client while neither has started (the Player opens capture first, as on ASIO). An
//!   open after a start is refused, as is a second open of one direction.
//! - The first `start` builds the process engine from both directions' callbacks, installs the
//!   JACK callbacks and activates the client, then connects that direction's ports to its device's
//!   ports (`out_i` to the i-th playback port, the i-th capture port to `in_i`; not when the backend
//!   was made without auto-connect); the other direction's `start` connects its own. Connections a
//!   user makes in a patchbay later are left alone. A stop is for good (it returns once no process
//!   callback is inside that direction); when no direction runs, the client is deactivated.
//!   Dropping the last stream deactivates and closes it, and only then frees the engine.
//! - The watcher thread (`transit-jack-watch`, every 20 ms while the client is active) reads the
//!   flags the callbacks set ([`Shared`]) and calls the streams' error callbacks: a server
//!   shutdown is [`ab::StreamError::DeviceNotAvailable`] once, and the dead client is closed there
//!   (jack.h: `jack_client_close` "cannot be called directly in the context of the thread that calls
//!   the shutdown callback"); the watcher then polls for the server (a probe client every 2 s for
//!   30 s) and, when one answers, requests a reset ([`ab::StreamError::ResetRequested`]), which the
//!   Player's reopen path serves. A sample rate or a quantum other than the open ones is a reset
//!   request once per new pair (the Player reopens at the server's new settings). A user callback
//!   that panicked is [`ab::StreamError::Other`] once to its direction. Xruns are counted. Error
//!   callbacks run with no lock held, so one may stop its stream.

use crate::api::{JackApi, JackClient};
use crate::devices::{self, JackDevice, Server, ServerState};
use crate::engine::{self, Engine, EngineSide, Shared};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// The timing knobs of the watcher (shorter in tests).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    pub(crate) watch: Duration,
    pub(crate) reconnect_period: Duration,
    pub(crate) reconnect_window: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { watch: WATCH_PERIOD, reconnect_period: RECONNECT_PERIOD, reconnect_window: RECONNECT_WINDOW }
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
        Self { api, slot: Arc::default(), next_generation: Arc::new(AtomicU64::new(1)), timing: Timing::default(), auto_connect: true }
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

    /// "Native JACK (<server>)": how the Audio Devices panel names this backend over its server.
    pub fn label(&self) -> String {
        format!("Native JACK ({})", self.server_name())
    }

    /// Opens a short-lived probe client (`JackNoStartServer`: no server is ever started), reads the
    /// server's rate, quantum and physical ports, and closes it. A control-thread call.
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
        let devices = devices::group(&client.physical_ports(), rate, buffer);
        if let Err(e) = client.close() {
            log::warn!("JACK: closing the probe client: {e}");
        }
        ServerState::Running(Server { rate, buffer, devices })
    }

    fn running(&self) -> Result<Server, String> {
        match self.probe() {
            ServerState::Running(server) => Ok(server),
            other => Err(other.diagnostic().unwrap_or_default()),
        }
    }

    /// The group named `device`, else the first with ports in `direction`.
    fn find(&self, direction: ab::Direction, device: Option<&str>) -> Result<ab::Endpoint, String> {
        let server = self.running()?;
        let usable = |d: &JackDevice| !d.ports(direction).is_empty();
        let found = match device {
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

    /// The one request to try: `f32`, every port of the group, at the server's rate and quantum.
    fn plan(endpoint: ab::Endpoint, direction: ab::Direction) -> Result<ab::Plan, String> {
        let device = endpoint.handle::<JackDevice>().ok_or("the endpoint is not a JACK port group")?;
        let format = ab::Format::new(device.rate, clamp(device.ports(direction).len()), ab::SampleFormat::F32);
        let request = ab::OpenRequest {
            format,
            // The server's quantum: setting it would change every client of the server.
            period: ab::Period::Default,
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
            let client = self.api.open_client(crate::CLIENT_NAME).map_err(|e| ab::OpenError::DeviceNotAvailable(e.to_string()))?;
            let (rate, buffer) = (client.sample_rate(), client.buffer_size());
            *guard = Some(Session::new(client, rate, buffer, self.next_generation.fetch_add(1, Ordering::Relaxed)));
        }
        let Some(session) = guard.as_mut() else { return Err(ab::OpenError::Other("JACK: no session".into())) };
        // The stream runs at the server's rate whatever was asked (the Player resamples).
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

    #[cfg(test)]
    pub(crate) fn with_timing(mut self, watch: Duration, reconnect_period: Duration, reconnect_window: Duration) -> Self {
        self.timing = Timing { watch, reconnect_period, reconnect_window };
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
        Self::plan(self.find(ab::Direction::Output, wish.device.as_deref())?, ab::Direction::Output)
    }

    fn plan_capture(&self, wish: &ab::InputWish) -> Result<ab::Plan, String> {
        Self::plan(self.find(ab::Direction::Input, wish.device.as_deref())?, ab::Direction::Input)
    }

    /// One client, one process callback for both directions.
    fn capture_with_render(&self, render: &ab::Endpoint) -> bool {
        render.handle::<JackDevice>().is_some()
    }

    /// One process callback runs the capture callback, then the render callback (K22: the live
    /// input ring aligns with no slack).
    fn capture_precedes_render(&self, endpoint: &ab::Endpoint) -> bool {
        endpoint.handle::<JackDevice>().is_some()
    }

    /// `f32`, every capture port of the group, at the server's rate (a requested rate is not
    /// applied: the server dictates it).
    fn capture_format(&self, endpoint: &ab::Endpoint, _rate: Option<u32>) -> Result<ab::Format, String> {
        let device = endpoint.handle::<JackDevice>().ok_or("the endpoint is not a JACK port group")?;
        Ok(ab::Format::new(device.rate, clamp(device.capture.len()), ab::SampleFormat::F32))
    }

    /// The ports' short names.
    fn channel_names(&self, endpoint: &ab::Endpoint, direction: ab::Direction) -> Vec<String> {
        endpoint.handle::<JackDevice>().map(|d| d.channel_names(direction)).unwrap_or_default()
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
    /// (source, destination) pairs to connect at this side's start.
    connections: Vec<(String, String)>,
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

/// The open client and its directions.
pub(crate) struct Session {
    id: u64,
    /// `None` once the server shut the client down (the watcher closed it).
    client: Option<Box<dyn JackClient>>,
    rate: u32,
    buffer: u32,
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
    fn new(client: Box<dyn JackClient>, rate: u32, buffer: u32, id: u64) -> Self {
        Self {
            id,
            client: Some(client),
            rate,
            buffer,
            shared: Arc::new(Shared::new(rate, buffer)),
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
        for i in 0..usize::from(channels) {
            let short = if input { format!("in_{}", i + 1) } else { format!("out_{}", i + 1) };
            let ours = client.register_port(&short, input).map_err(|e| ab::OpenError::Other(format!("JACK: registering {short}: {e}")))?;
            if let Some(theirs) = device_ports.get(i).filter(|_| auto_connect) {
                connections.push(if input { (theirs.clone(), ours) } else { (ours, theirs.clone()) });
            }
        }
        let error = Arc::new(Mutex::new(error));
        self.sides.push(Side { generation, direction, waiting: Some(waiting), connections, error, panic_reported: false });
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

    /// Starts one direction: activates the client at the first start, then connects its ports.
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
            log::info!("JACK: client \"{}\" active at {} Hz, quantum {}", client.name(), self.rate, self.buffer);
            match spawn_watcher(Arc::downgrade(slot), self.id, Arc::clone(api), timing) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(e) => log::warn!("JACK: the watcher thread did not start ({e}); a server shutdown will not be reported"),
            }
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
        let now = (self.shared.sample_rate(), self.shared.buffer_size());
        if now != (self.rate, self.buffer) && now != self.reset_for {
            self.reset_for = now;
            reports.extend(self.errors(ab::StreamError::ResetRequested));
        }
        Tick { reports, dead: None, done: false }
    }

    fn errors(&self, error: ab::StreamError) -> Vec<(SharedError, ab::StreamError)> {
        self.sides.iter().map(|s| (Arc::clone(&s.error), error)).collect()
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

    /// The process thread and its priority are the server's.
    fn thread_priority(&self) -> ab::WorkerPriority {
        ab::WorkerPriority::Unobserved
    }

    /// This direction's process callbacks as wakes, server xruns as overloads, rate and quantum
    /// changes and the server's return as resets, a server shutdown or a panic as a fault.
    fn stats(&self) -> Option<ab::StreamStats> {
        Some(self.shared.stats(self.direction == ab::Direction::Output))
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
