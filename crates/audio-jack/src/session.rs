//! The backend over a [`JackApi`]: enumeration through a probe client, and one client "transit" per
//! open session (ADR 0021, decision 4).
//!
//! - The first `open_capture` or `open_render` opens the client and registers one port per channel
//!   (`in_1..` or `out_1..`). The other direction joins the same client while neither has started
//!   (the Player opens capture first, as on ASIO). An open after a start is refused in slice 1, as
//!   is a second open of one direction.
//! - The first `start` installs the callbacks and activates the client, then connects that
//!   direction's ports to its device's ports (`out_i` to the i-th playback port, the i-th capture
//!   port to `in_i`); the other direction's `start` connects its own. Connections a user makes in a
//!   patchbay later are left alone. A stop is for good: when no direction runs, the client is
//!   deactivated. Dropping the last stream deactivates and closes it.
//! - The watcher thread (`transit-jack-watch`, every 20 ms while the client is active) reads the
//!   flags the callbacks set ([`Shared`]) and calls the streams' error callbacks: a server
//!   shutdown is [`ab::StreamError::DeviceNotAvailable`] once, and the dead client is closed there
//!   (jack.h: `jack_client_close` "cannot be called directly in the context of the thread that calls
//!   the shutdown callback"); a sample rate other than the open one is
//!   [`ab::StreamError::ResetRequested`] once per new rate. Xruns and quantum changes are counted,
//!   not errors. Error callbacks run with no lock held, so one may stop its stream.

use crate::api::{JackApi, JackClient};
use crate::devices::{self, JackDevice, Server, ServerState};
use crate::engine::Shared;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;
use transit_audio_backend as ab;

/// How often the watcher reads the callbacks' flags.
pub(crate) const WATCH_PERIOD: Duration = Duration::from_millis(20);
/// The watcher thread's name.
pub(crate) const WATCHER_NAME: &str = "transit-jack-watch";

type Slot = Arc<Mutex<Option<Session>>>;
type SharedError = Arc<Mutex<ab::ErrorCallback>>;

fn lock(slot: &Mutex<Option<Session>>) -> MutexGuard<'_, Option<Session>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The native JACK audio system. `Send` and `Sync`; clones share the session.
#[derive(Clone)]
pub struct JackBackend {
    api: Arc<dyn JackApi>,
    slot: Slot,
    next_generation: Arc<AtomicU64>,
    watch_period: Duration,
}

impl JackBackend {
    pub(crate) fn with_api(api: Arc<dyn JackApi>) -> Self {
        Self { api, slot: Arc::default(), next_generation: Arc::new(AtomicU64::new(1)), watch_period: WATCH_PERIOD }
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
        direction: ab::Direction,
        error: ab::ErrorCallback,
    ) -> Result<(JackStream, ab::Achieved), ab::OpenError> {
        let device = endpoint.handle::<JackDevice>().ok_or_else(|| ab::OpenError::Other("the endpoint is not a JACK port group".into()))?;
        let channels = request.format.channels;
        if channels == 0 {
            return Err(ab::OpenError::Other("JACK: a stream needs at least one channel".into()));
        }
        let mut guard = lock(&self.slot);
        if guard.is_none() {
            let client = self.api.open_client(crate::CLIENT_NAME).map_err(|e| ab::OpenError::DeviceNotAvailable(e.to_string()))?;
            let (rate, buffer) = (client.sample_rate(), client.buffer_size());
            *guard = Some(Session::new(client, rate, buffer, self.next_generation.fetch_add(1, Ordering::Relaxed)));
        }
        let Some(session) = guard.as_mut() else { return Err(ab::OpenError::Other("JACK: no session".into())) };
        let result = session.attach(device, direction, channels, error, self.next_generation.fetch_add(1, Ordering::Relaxed));
        let opened = match result {
            Ok(generation) => generation,
            Err(e) => {
                // A client opened for this request alone is closed again.
                if session.sides_empty() {
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
            format: ab::Format::new(session.rate, channels, ab::SampleFormat::F32),
            period: ab::Period::Frames(session.buffer),
            latency: None,
            device_position: None,
            min_period: None,
            buffer_frames: Some(session.buffer),
        };
        let stream = JackStream {
            slot: Arc::clone(&self.slot),
            session: session.id,
            generation: opened,
            direction,
            shared: Arc::clone(&session.shared),
            watch_period: self.watch_period,
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

    #[cfg(test)]
    pub(crate) fn with_watch_period(mut self, period: Duration) -> Self {
        self.watch_period = period;
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
        _render: ab::RenderCallback,
        error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        // Slice 1 writes silence and runs no render callback; the duplex engine is slice 2.
        let (stream, achieved) = self.open_side(endpoint, request, ab::Direction::Output, error)?;
        Ok((Box::new(stream), achieved))
    }

    /// Registers `in_1..in_n` on the session's client, before either direction starts.
    fn open_capture(
        &self,
        endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        _capture: ab::CaptureCallback,
        error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        let (stream, achieved) = self.open_side(endpoint, request, ab::Direction::Input, error)?;
        Ok((Box::new(stream), achieved))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideState {
    Opened,
    Started,
    Stopped,
}

/// One direction of the session.
struct Side {
    generation: u64,
    direction: ab::Direction,
    state: SideState,
    /// (source, destination) pairs to connect at this side's start.
    connections: Vec<(String, String)>,
    error: SharedError,
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
    /// The last rate a reset was requested for.
    reset_rate: u32,
    /// The quantum changes already logged.
    quantum_changes_seen: u64,
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
            reset_rate: rate,
            quantum_changes_seen: 0,
        }
    }

    fn sides_empty(&self) -> bool {
        self.sides.is_empty()
    }

    fn side_mut(&mut self, generation: u64) -> Option<&mut Side> {
        self.sides.iter_mut().find(|s| s.generation == generation)
    }

    /// Registers `channels` ports of `direction` and plans their connections to `device`.
    fn attach(
        &mut self,
        device: &JackDevice,
        direction: ab::Direction,
        channels: u16,
        error: ab::ErrorCallback,
        generation: u64,
    ) -> Result<u64, ab::OpenError> {
        if self.active || self.sides.iter().any(|s| s.state != SideState::Opened) {
            return Err(ab::OpenError::Other("JACK: open both directions before starting either; reopen the device to add a direction".into()));
        }
        if self.sides.iter().any(|s| s.direction == direction) {
            return Err(ab::OpenError::InUse(format!("JACK: transit's {direction:?} ports are already open")));
        }
        let client = self.client.as_mut().ok_or_else(|| ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into()))?;
        let input = direction == ab::Direction::Input;
        let device_ports = device.ports(direction);
        let mut connections = Vec::with_capacity(usize::from(channels));
        for i in 0..usize::from(channels) {
            let short = if input { format!("in_{}", i + 1) } else { format!("out_{}", i + 1) };
            let ours = client.register_port(&short, input).map_err(|e| ab::OpenError::Other(format!("JACK: registering {short}: {e}")))?;
            if let Some(theirs) = device_ports.get(i) {
                connections.push(if input { (theirs.clone(), ours) } else { (ours, theirs.clone()) });
            }
        }
        let error = Arc::new(Mutex::new(error));
        self.sides.push(Side { generation, direction, state: SideState::Opened, connections, error });
        Ok(generation)
    }

    /// Starts one direction: activates the client at the first start, then connects its ports.
    fn start(&mut self, generation: u64, slot: &Slot, watch_period: Duration) -> Result<(), ab::OpenError> {
        let state = self.side_mut(generation).map(|s| s.state).ok_or_else(|| ab::OpenError::Other("JACK: the stream was closed".into()))?;
        match state {
            SideState::Started => return Ok(()),
            SideState::Stopped => return Err(ab::OpenError::Other("JACK: a stopped stream is reopened, not restarted".into())),
            SideState::Opened => {}
        }
        let Some(client) = self.client.as_mut() else { return Err(ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())) };
        if !self.active {
            client.activate(Arc::clone(&self.shared)).map_err(|e| ab::OpenError::Other(format!("JACK: activating the client: {e}")))?;
            self.active = true;
            log::info!("JACK: client \"{}\" active at {} Hz, quantum {}", client.name(), self.rate, self.buffer);
            match spawn_watcher(Arc::downgrade(slot), self.id, watch_period) {
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
        if let Some(side) = self.side_mut(generation) {
            side.state = SideState::Started;
        }
        Ok(())
    }

    /// Stops one direction for good; when none runs, deactivates the client. Returns the watcher
    /// to finish with the lock released.
    fn stop(&mut self, generation: u64) -> Option<Watcher> {
        if let Some(side) = self.side_mut(generation) {
            side.state = SideState::Stopped;
        }
        if self.sides.iter().any(|s| s.state == SideState::Started) || !self.active {
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

    /// Ends the session: stops the watcher, deactivates and closes the client. Called with the
    /// session out of its slot (no lock held).
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
            for side in &mut self.sides {
                if side.state == SideState::Started {
                    side.state = SideState::Stopped;
                }
            }
            return Tick { report: self.errors(), error: Some(ab::StreamError::DeviceNotAvailable), dead: self.client.take(), done: true };
        }
        let rate = self.shared.sample_rate();
        if rate != self.rate && rate != self.reset_rate {
            self.reset_rate = rate;
            return Tick { report: self.errors(), error: Some(ab::StreamError::ResetRequested), dead: None, done: false };
        }
        Tick { report: Vec::new(), error: None, dead: None, done: false }
    }

    fn errors(&self) -> Vec<SharedError> {
        self.sides.iter().map(|s| Arc::clone(&s.error)).collect()
    }
}

/// What one watcher tick found.
struct Tick {
    report: Vec<SharedError>,
    error: Option<ab::StreamError>,
    dead: Option<Box<dyn JackClient>>,
    /// The client is gone: the watcher ends.
    done: bool,
}

fn spawn_watcher(slot: Weak<Mutex<Option<Session>>>, id: u64, period: Duration) -> std::io::Result<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new().name(WATCHER_NAME.into()).spawn(move || {
        while !flag.load(Ordering::Acquire) {
            std::thread::park_timeout(period);
            if flag.load(Ordering::Acquire) || !watch_once(&slot, id) {
                break;
            }
        }
    })?;
    Ok(Watcher { stop, thread })
}

/// One tick of the watcher of session `id`; false when it should end (the session is gone or its
/// client shut down).
fn watch_once(slot: &Weak<Mutex<Option<Session>>>, id: u64) -> bool {
    let Some(slot) = slot.upgrade() else { return false };
    let mut guard = lock(&slot);
    let Some(session) = guard.as_mut().filter(|s| s.id == id) else { return false };
    let shared = Arc::clone(&session.shared);
    let tick = session.watch();
    let changes = shared.buffer_changes();
    let quantum = (changes != session.quantum_changes_seen).then(|| {
        session.quantum_changes_seen = changes;
        shared.buffer_size()
    });
    if tick.done {
        // This thread ends after the tick: dropping its own handle detaches it.
        session.watcher.take();
    }
    drop(guard);
    // The dead client is closed here, on the watcher, never in the shutdown handler.
    if let Some(dead) = tick.dead
        && let Err(e) = dead.close()
    {
        log::warn!("JACK: closing the client after the server stopped: {e}");
    }
    if let Some(frames) = quantum {
        // Served by chunking from slice 2; the shown period follows it in slice 3 (ADR 0021).
        log::info!("JACK: the server's quantum is now {frames} frames");
    }
    if let Some(error) = tick.error {
        match error {
            ab::StreamError::DeviceNotAvailable => {
                shared.count_fault();
                log::warn!("JACK: the server stopped");
            }
            ab::StreamError::ResetRequested => {
                shared.count_reset();
                log::info!("JACK: the server's sample rate changed to {} Hz", shared.sample_rate());
            }
            _ => {}
        }
        for callback in tick.report {
            let mut callback = callback.lock().unwrap_or_else(PoisonError::into_inner);
            (*callback)(error);
        }
    }
    !tick.done
}

/// One direction of the session's client. Dropping it detaches the direction; the last one
/// deactivates and closes the client, and joins the watcher: drop it on a control thread.
pub(crate) struct JackStream {
    slot: Slot,
    session: u64,
    generation: u64,
    direction: ab::Direction,
    shared: Arc<Shared>,
    watch_period: Duration,
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
        session.start(self.generation, &self.slot, self.watch_period)
    }

    fn stop(&self) -> Result<(), ab::OpenError> {
        let watcher = {
            let mut guard = lock(&self.slot);
            guard.as_mut().filter(|s| s.id == self.session).and_then(|s| s.stop(self.generation))
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

    /// Process callbacks as wakes, server xruns as overloads, sample rate changes as resets, and a
    /// server shutdown as a fault.
    fn stats(&self) -> Option<ab::StreamStats> {
        Some(ab::StreamStats {
            wakes: self.shared.cycles(),
            overloads: self.shared.xruns(),
            resets: self.shared.resets(),
            faults: self.shared.faults(),
            ..Default::default()
        })
    }
}

impl Drop for JackStream {
    fn drop(&mut self) {
        let closing = {
            let mut guard = lock(&self.slot);
            let Some(session) = guard.as_mut().filter(|s| s.id == self.session) else { return };
            session.sides.retain(|s| s.generation != self.generation);
            if session.sides.is_empty() { guard.take() } else { None }
        };
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
