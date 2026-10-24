//! A scripted JACK server for tests (ADR 0021, "Fake server"): it opens clients, lists physical
//! ports, records registrations, activations, connections and closes, and drives the callbacks the
//! real libjack would call (`process`, buffer size, sample rate, xrun, info shutdown) through the
//! same safe handlers `crate::callback` calls.

use crate::api::{ClientError, JackApi, JackClient, PhysicalPort};
use crate::engine::{self, OutputPlanes, Shared};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// What the fake server knows of one client.
#[derive(Debug, Default)]
pub(crate) struct ClientRecord {
    pub(crate) name: String,
    /// Full names of the client's ports, inputs and outputs, in registration order.
    pub(crate) ports: Vec<(String, bool)>,
    pub(crate) active: bool,
    pub(crate) activations: u32,
    pub(crate) deactivations: u32,
    pub(crate) closed: bool,
    /// The thread that closed the client, by name.
    pub(crate) closed_on: Option<String>,
    pub(crate) connections: Vec<(String, String)>,
    pub(crate) shared: Option<Arc<Shared>>,
}

#[derive(Debug)]
struct State {
    library: bool,
    running: bool,
    /// The server refuses clients with this text (a status other than `JackServerFailed`).
    refuse: Option<String>,
    rate: u32,
    buffer: u32,
    ports: Vec<PhysicalPort>,
    clients: Vec<ClientRecord>,
}

/// The scripted server. Clones share it.
#[derive(Debug, Clone)]
pub(crate) struct FakeServer {
    state: Arc<Mutex<State>>,
}

impl FakeServer {
    /// A running server at `rate` and quantum `buffer` with `ports` (full names; `capture` per port).
    pub(crate) fn running(rate: u32, buffer: u32, ports: &[(&str, bool)]) -> Self {
        let ports = ports.iter().map(|(name, capture)| PhysicalPort { name: (*name).to_owned(), capture: *capture }).collect();
        Self { state: Arc::new(Mutex::new(State { library: true, running: true, refuse: None, rate, buffer, ports, clients: Vec::new() })) }
    }

    /// JACK 2 with one stereo interface: `system:capture_1..2`, `system:playback_1..2`.
    pub(crate) fn jack2(rate: u32, buffer: u32) -> Self {
        Self::running(
            rate,
            buffer,
            &[("system:capture_1", true), ("system:capture_2", true), ("system:playback_1", false), ("system:playback_2", false)],
        )
    }

    /// No `libjack.so.0` on the loader path.
    pub(crate) fn missing_library() -> Self {
        let server = Self::running(48_000, 256, &[]);
        server.lock().library = false;
        server
    }

    /// The library loads but no server answers.
    pub(crate) fn not_running() -> Self {
        let server = Self::running(48_000, 256, &[]);
        server.lock().running = false;
        server
    }

    /// The server answers but refuses clients (`text` is what the backend reports).
    pub(crate) fn refusing(text: &str) -> Self {
        let server = Self::running(48_000, 256, &[]);
        server.lock().refuse = Some(text.to_owned());
        server
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` over the client records.
    pub(crate) fn clients<R>(&self, f: impl FnOnce(&[ClientRecord]) -> R) -> R {
        f(&self.lock().clients)
    }

    /// Clients opened and not closed.
    pub(crate) fn open_clients(&self) -> usize {
        self.clients(|c| c.iter().filter(|c| !c.closed).count())
    }

    /// The callback state of the active client (the one `activate` was given).
    pub(crate) fn active_shared(&self) -> Option<Arc<Shared>> {
        self.clients(|c| c.iter().find(|c| c.active && !c.closed).and_then(|c| c.shared.clone()))
    }

    /// One process cycle of `frames` on every active client, over `outputs` as its port buffers.
    pub(crate) fn cycle(&self, frames: u32, outputs: &mut [Vec<f32>]) {
        if let Some(shared) = self.active_shared() {
            engine::process(&shared, frames, &mut VecPlanes(outputs));
        }
    }

    /// The server stops: libjack calls the info shutdown callback of every active client.
    pub(crate) fn shutdown(&self) {
        let shared: Vec<Arc<Shared>> = {
            let mut state = self.lock();
            state.running = false;
            state.clients.iter().filter(|c| c.active && !c.closed).filter_map(|c| c.shared.clone()).collect()
        };
        for s in shared {
            s.on_shutdown();
        }
    }

    pub(crate) fn xrun(&self) {
        if let Some(shared) = self.active_shared() {
            shared.on_xrun();
        }
    }

    /// The quantum changes (PipeWire, when another client asks for a smaller one).
    pub(crate) fn set_buffer_size(&self, frames: u32) {
        self.lock().buffer = frames;
        if let Some(shared) = self.active_shared() {
            shared.on_buffer_size(frames);
        }
    }

    pub(crate) fn set_sample_rate(&self, rate: u32) {
        self.lock().rate = rate;
        if let Some(shared) = self.active_shared() {
            shared.on_sample_rate(rate);
        }
    }
}

/// Plain vectors as the output port buffers of one cycle.
pub(crate) struct VecPlanes<'a>(pub(crate) &'a mut [Vec<f32>]);

impl OutputPlanes for VecPlanes<'_> {
    fn count(&self) -> usize {
        self.0.len()
    }

    fn plane(&mut self, index: usize) -> Option<&mut [f32]> {
        self.0.get_mut(index).map(Vec::as_mut_slice)
    }
}

impl JackApi for FakeServer {
    fn open_client(&self, name: &str) -> Result<Box<dyn JackClient>, ClientError> {
        let mut state = self.lock();
        if !state.library {
            return Err(ClientError::LibraryMissing("libjack.so.0: cannot open shared object file: No such file or directory".into()));
        }
        if !state.running {
            return Err(ClientError::ServerNotRunning);
        }
        if let Some(text) = &state.refuse {
            return Err(ClientError::Failed(text.clone()));
        }
        // The server makes the name unique, as JACK does without JackUseExactName.
        let taken = state.clients.iter().filter(|c| !c.closed && (c.name == name || c.name.starts_with(&format!("{name}-")))).count();
        let unique = if taken == 0 { name.to_owned() } else { format!("{name}-{:02}", taken + 1) };
        state.clients.push(ClientRecord { name: unique, ..Default::default() });
        Ok(Box::new(FakeClient { server: self.clone(), index: state.clients.len() - 1 }))
    }
}

struct FakeClient {
    server: FakeServer,
    index: usize,
}

impl FakeClient {
    fn record<R>(&self, f: impl FnOnce(&mut ClientRecord, &mut bool) -> R) -> R {
        let mut state = self.server.lock();
        let State { clients, running, .. } = &mut *state;
        f(&mut clients[self.index], running)
    }
}

impl JackClient for FakeClient {
    fn name(&self) -> String {
        self.record(|c, _| c.name.clone())
    }

    fn sample_rate(&self) -> u32 {
        self.server.lock().rate
    }

    fn buffer_size(&self) -> u32 {
        self.server.lock().buffer
    }

    fn physical_ports(&self) -> Vec<PhysicalPort> {
        let state = self.server.lock();
        state.ports.iter().filter(|p| p.capture).chain(state.ports.iter().filter(|p| !p.capture)).cloned().collect()
    }

    fn register_port(&mut self, short: &str, input: bool) -> Result<String, String> {
        self.record(|c, _| {
            let full = format!("{}:{short}", c.name);
            if c.ports.iter().any(|(p, _)| *p == full) {
                return Err(format!("{full} is already registered"));
            }
            c.ports.push((full.clone(), input));
            Ok(full)
        })
    }

    fn activate(&mut self, shared: Arc<Shared>) -> Result<(), String> {
        self.record(|c, running| {
            if !*running {
                return Err("the server is gone".into());
            }
            if c.shared.is_some() {
                return Err("the callbacks are installed once".into());
            }
            c.shared = Some(shared);
            c.active = true;
            c.activations += 1;
            Ok(())
        })
    }

    fn connect(&mut self, source: &str, destination: &str) -> Result<(), String> {
        self.record(|c, _| {
            if !c.active {
                return Err("connect needs an active client".into());
            }
            c.connections.push((source.to_owned(), destination.to_owned()));
            Ok(())
        })
    }

    fn deactivate(&mut self) -> Result<(), String> {
        self.record(|c, _| {
            c.active = false;
            c.deactivations += 1;
            c.connections.clear();
            Ok(())
        })
    }

    fn close(self: Box<Self>) -> Result<(), String> {
        self.record(|c, _| {
            c.active = false;
            c.closed = true;
            c.closed_on = std::thread::current().name().map(str::to_owned);
            Ok(())
        })
    }
}
