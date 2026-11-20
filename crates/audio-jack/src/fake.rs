//! A scripted JACK server for tests (ADR 0021, "Fake server"): it opens clients, lists physical
//! ports, records registrations, activations, connections and closes, and drives the callbacks the
//! real libjack would call (`process`, buffer size, sample rate, xrun, info shutdown) through the
//! same safe engine and handlers `crate::callback` calls.

use crate::api::{ClientError, JackApi, JackClient, PhysicalPort, PortName};
use crate::engine::{Clock, Cycle, Engine, InputPlanes, OutputPlanes, Shared};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

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
    /// The process engine `activate` was given, dropped at close (as `crate::ffi` frees it).
    pub(crate) engine: Option<EngineSlot>,
}

/// The engine of an active fake client.
pub(crate) struct EngineSlot(Arc<Mutex<Engine>>);

impl std::fmt::Debug for EngineSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Engine")
    }
}

#[derive(Debug)]
struct State {
    library: bool,
    running: bool,
    /// The server refuses clients with this text (a status other than `JackServerFailed`).
    refuse: Option<String>,
    /// The server refuses a quantum change with this text (`jack_set_buffer_size` fails).
    refuse_quantum: Option<String>,
    /// Quantum requests the server was given, in order.
    quantum_requests: Vec<u32>,
    rate: u32,
    buffer: u32,
    ports: Vec<PhysicalPort>,
    clients: Vec<ClientRecord>,
    version: Option<String>,
    /// The frame counter and JACK's clock, µs, at the next cycle's start.
    frames: u32,
    usecs: u64,
    /// Every link of the graph (source, destination), whichever client made it.
    links: Vec<(String, String)>,
    /// Audio and MIDI ports of other clients: (full name, a JACK output, audio).
    others: Vec<(String, bool, bool)>,
    /// Ports whose names are not valid UTF-8: (the fake's key for the port, the lossy form libjack's
    /// bytes would give). The binding reports such a name as `Err(lossy)` (`api::PortName`).
    invalid: Vec<(String, String)>,
    /// Every connect a transit client asked for, in order (made, existing or refused).
    connects: Vec<(String, String)>,
    /// A connect of a pair already linked makes a second link (PipeWire's libjack) instead of
    /// answering `EEXIST` (JACK 2).
    duplicates: bool,
    /// Another client makes this link just before transit's connect of it is served.
    race: Option<(String, String)>,
    /// PipeWire's `jack.self-connect-mode` for links with an end on a transit port: `ignore-*`
    /// answers 0 and makes no link; `fail-*` answers 1.
    self_connect: SelfConnect,
}

/// The fake's `jack.self-connect-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfConnect {
    Allow,
    Ignore,
    Fail,
}

impl State {
    /// (a JACK output, audio) of the port `name`, if it exists.
    fn port(&self, name: &str) -> Option<(bool, bool)> {
        if let Some(p) = self.ports.iter().find(|p| p.name == name) {
            return Some((p.capture, true));
        }
        if let Some((_, output, audio)) = self.others.iter().find(|(n, _, _)| n == name) {
            return Some((*output, *audio));
        }
        self.clients.iter().filter(|c| !c.closed).flat_map(|c| &c.ports).find(|(n, _)| n == name).map(|(_, input)| (!input, true))
    }

    /// Removes every link with an end in `ports`.
    fn unlink(&mut self, ports: &[String]) {
        self.links.retain(|(a, b)| !ports.contains(a) && !ports.contains(b));
    }
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
        let state = State {
            library: true,
            running: true,
            refuse: None,
            refuse_quantum: None,
            quantum_requests: Vec::new(),
            rate,
            buffer,
            ports,
            clients: Vec::new(),
            version: None,
            frames: 0,
            usecs: 1_000_000,
            links: Vec::new(),
            others: Vec::new(),
            invalid: Vec::new(),
            connects: Vec::new(),
            duplicates: false,
            race: None,
            self_connect: SelfConnect::Allow,
        };
        Self { state: Arc::new(Mutex::new(state)) }
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

    /// The server refuses every quantum change from now on.
    pub(crate) fn refusing_quantum(self, text: &str) -> Self {
        self.lock().refuse_quantum = Some(text.to_owned());
        self
    }

    /// The quantums clients asked this server for, in order.
    pub(crate) fn quantum_requests(&self) -> Vec<u32> {
        self.lock().quantum_requests.clone()
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

    /// The library names itself `version` (`jack_get_version_string`).
    pub(crate) fn set_version(&self, version: &str) {
        self.lock().version = Some(version.to_owned());
    }

    /// Sets the frame counter of the next cycle (to script a wrap or a jump).
    pub(crate) fn set_frame_counter(&self, frames: u32) {
        self.lock().frames = frames;
    }

    /// The engine of the active client.
    fn active_engine(&self) -> Option<Arc<Mutex<Engine>>> {
        self.clients(|c| c.iter().find(|c| c.active && !c.closed).and_then(|c| c.engine.as_ref().map(|e| Arc::clone(&e.0))))
    }

    /// One process cycle of `frames` on the active client over `outputs` and `inputs` as its port
    /// buffers, with the cycle clock advancing at the nominal rate; `late_us` is how long after the
    /// cycle start the callback reads `jack_get_time`. False when no client is active.
    pub(crate) fn cycle_at(&self, frames: u32, late_us: u64, outputs: &mut [Vec<f32>], inputs: &[Vec<f32>]) -> bool {
        let Some(engine) = self.active_engine() else { return false };
        let clock = {
            let mut state = self.lock();
            let clock = Clock { current_frames: state.frames, current_usecs: state.usecs, now_usecs: state.usecs + late_us };
            state.frames = state.frames.wrapping_add(frames);
            state.usecs += u64::from(frames) * 1_000_000 / u64::from(state.rate.max(1));
            clock
        };
        let cycle = Cycle { frames, arrival: Instant::now(), clock: Some(clock) };
        engine.lock().unwrap_or_else(PoisonError::into_inner).process(cycle, &mut VecPlanes(outputs), &InVecPlanes(inputs));
        true
    }

    /// [`FakeServer::cycle_at`] with the callback reading the clock 100 µs into the cycle.
    pub(crate) fn cycle(&self, frames: u32, outputs: &mut [Vec<f32>], inputs: &[Vec<f32>]) -> bool {
        self.cycle_at(frames, 100, outputs, inputs)
    }

    /// From now on the server refuses clients with `text` (`None`: it takes them again).
    pub(crate) fn set_refusing(&self, text: Option<&str>) {
        self.lock().refuse = text.map(str::to_owned);
    }

    /// A physical port appears, after the ports the server listed before it.
    pub(crate) fn add_physical(&self, name: &str, capture: bool) {
        self.lock().ports.push(PhysicalPort { name: name.to_owned(), capture });
    }

    /// A physical port goes away (a device profile with fewer channels), and its links with it.
    pub(crate) fn remove_physical(&self, name: &str) {
        let mut state = self.lock();
        state.ports.retain(|p| p.name != name);
        state.unlink(&[name.to_owned()]);
    }

    /// From now on a connect of a pair already linked makes a second link, as PipeWire's libjack
    /// does, instead of answering `EEXIST`.
    pub(crate) fn duplicating_links(self) -> Self {
        self.lock().duplicates = true;
        self
    }

    /// Another client makes `source` -> `destination` just before transit's next connect of that
    /// pair is served (a graph edit racing the restore).
    pub(crate) fn link_appears_on_connect(&self, source: &str, destination: &str) {
        self.lock().race = Some((source.to_owned(), destination.to_owned()));
    }

    /// From now on transit's connects are served under this `jack.self-connect-mode`.
    pub(crate) fn set_self_connect(&self, mode: SelfConnect) {
        self.lock().self_connect = mode;
    }

    /// How many times `source` -> `destination` is linked (more than once only on a duplicating
    /// server).
    pub(crate) fn link_count(&self, source: &str, destination: &str) -> usize {
        self.lock().links.iter().filter(|(a, b)| a == source && b == destination).count()
    }

    /// Another client registers an audio port (`output`: a JACK output, data can be read from it).
    pub(crate) fn add_port(&self, name: &str, output: bool) {
        self.lock().others.push((name.to_owned(), output, true));
    }

    /// Another client registers an audio port whose name is not valid UTF-8: `key` names it in the
    /// fake (for `outside_link`), `shown` is the lossy form, which may equal another port's name.
    pub(crate) fn add_port_with_invalid_name(&self, key: &str, shown: &str, output: bool) {
        let mut state = self.lock();
        state.others.push((key.to_owned(), output, true));
        state.invalid.push((key.to_owned(), shown.to_owned()));
    }

    /// Another client registers a MIDI port.
    pub(crate) fn add_midi_port(&self, name: &str, output: bool) {
        self.lock().others.push((name.to_owned(), output, false));
    }

    /// Another client's port goes away, and its links with it.
    pub(crate) fn remove_port(&self, name: &str) {
        let mut state = self.lock();
        state.others.retain(|(n, _, _)| n != name);
        state.unlink(&[name.to_owned()]);
    }

    /// Another client (a patchbay) links `source` to `destination`. Panics on a link the server
    /// would refuse, so a test cannot script an impossible graph.
    pub(crate) fn outside_link(&self, source: &str, destination: &str) {
        let mut state = self.lock();
        let (from, to) = (state.port(source), state.port(destination));
        assert!(matches!((from, to), (Some((true, a)), Some((false, b))) if a == b), "{source} -> {destination} is not a link the server makes");
        let link = (source.to_owned(), destination.to_owned());
        if !state.links.contains(&link) {
            state.links.push(link);
        }
    }

    /// Every link of the graph, in the order it was made.
    pub(crate) fn graph(&self) -> Vec<(String, String)> {
        self.lock().links.clone()
    }

    /// Whether `source` is linked to `destination`.
    pub(crate) fn linked(&self, source: &str, destination: &str) -> bool {
        self.lock().links.iter().any(|(a, b)| a == source && b == destination)
    }

    /// Every connect transit's clients asked for, in order.
    pub(crate) fn connects(&self) -> Vec<(String, String)> {
        self.lock().connects.clone()
    }

    /// The server stops: libjack calls the info shutdown callback of every active client.
    pub(crate) fn shutdown(&self) {
        let shared: Vec<Arc<Shared>> = {
            let mut state = self.lock();
            state.running = false;
            // The graph goes with the server.
            state.links.clear();
            state.clients.iter().filter(|c| c.active && !c.closed).filter_map(|c| c.shared.clone()).collect()
        };
        for s in shared {
            s.on_shutdown();
        }
    }

    /// The server runs again (after [`FakeServer::shutdown`]): new clients open.
    pub(crate) fn restart(&self) {
        self.lock().running = true;
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

/// Plain vectors as the input port buffers of one cycle.
pub(crate) struct InVecPlanes<'a>(pub(crate) &'a [Vec<f32>]);

impl InputPlanes for InVecPlanes<'_> {
    fn count(&self) -> usize {
        self.0.len()
    }

    fn plane(&self, index: usize) -> Option<&[f32]> {
        self.0.get(index).map(Vec::as_slice)
    }
}

impl JackApi for FakeServer {
    fn version(&self) -> Option<String> {
        self.lock().version.clone()
    }

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

    fn activate(&mut self, shared: Arc<Shared>, engine: Engine) -> Result<(), String> {
        self.record(|c, running| {
            if !*running {
                return Err("the server is gone".into());
            }
            if c.shared.is_some() {
                return Err("the callbacks are installed once".into());
            }
            c.shared = Some(shared);
            c.engine = Some(EngineSlot(Arc::new(Mutex::new(engine))));
            c.active = true;
            c.activations += 1;
            Ok(())
        })
    }

    /// As jack_connect: an active client, an output port to an input port of one type; a link that
    /// exists is fine (`EEXIST`, mapped to success as `crate::ffi` does) and is not made twice,
    /// unless the server duplicates links (PipeWire's libjack).
    fn connect(&mut self, source: &str, destination: &str) -> Result<(), String> {
        let mut state = self.server.lock();
        let link = (source.to_owned(), destination.to_owned());
        state.connects.push(link.clone());
        if state.race.as_ref() == Some(&link) {
            state.race = None;
            if !state.links.contains(&link) {
                state.links.push(link.clone());
            }
        }
        if !state.clients[self.index].active {
            return Err("connect needs an active client".into());
        }
        match (state.port(source), state.port(destination)) {
            (Some((true, a)), Some((false, b))) if a == b => {}
            (None, _) | (_, None) => return Err("jack_connect returned -1 (no such port)".into()),
            _ => return Err("jack_connect returned -1 (wrong direction or type)".into()),
        }
        // Every connect a transit client makes has one end on its own port.
        match state.self_connect {
            SelfConnect::Allow => {}
            SelfConnect::Ignore => return Ok(()),
            SelfConnect::Fail => return Err("jack_connect returned 1".into()),
        }
        if state.duplicates || !state.links.contains(&link) {
            state.links.push(link.clone());
            state.clients[self.index].connections.push(link);
        }
        Ok(())
    }

    #[cfg(test)]
    fn connections(&self, port: &str) -> Vec<String> {
        partners(&self.server.lock().links, port)
    }

    fn links(&self, input: bool, index: usize) -> Vec<PortName> {
        let state = self.server.lock();
        let ours = state.clients[self.index].ports.iter().filter(|(_, i)| *i == input).nth(index).map(|(name, _)| name.clone());
        let partners = ours.map(|port| partners(&state.links, &port)).unwrap_or_default();
        let named = |name: String| match state.invalid.iter().find(|(key, _)| *key == name) {
            Some((_, shown)) => Err(shown.clone()),
            None => Ok(name),
        };
        partners.into_iter().map(named).collect()
    }

    /// As the binding: only a port whose name is exactly `name` (so never one whose name is not
    /// valid UTF-8).
    fn has_port(&self, name: &str, output: bool) -> bool {
        let state = self.server.lock();
        !state.invalid.iter().any(|(key, _)| key == name) && state.port(name) == Some((output, true))
    }

    fn set_buffer_size(&mut self, frames: u32) -> Result<(), String> {
        self.server.lock().quantum_requests.push(frames);
        if let Some(text) = self.server.lock().refuse_quantum.clone() {
            return Err(text);
        }
        self.server.set_buffer_size(frames);
        Ok(())
    }

    /// jack.h: "disconnect all ports belonging to it, since inactive clients have no port
    /// connections."
    fn deactivate(&mut self) -> Result<(), String> {
        let mut state = self.server.lock();
        let ports: Vec<String> = state.clients[self.index].ports.iter().map(|(p, _)| p.clone()).collect();
        state.unlink(&ports);
        let c = &mut state.clients[self.index];
        c.active = false;
        c.deactivations += 1;
        c.connections.clear();
        Ok(())
    }

    fn close(self: Box<Self>) -> Result<(), String> {
        let mut state = self.server.lock();
        let ports: Vec<String> = state.clients[self.index].ports.iter().map(|(p, _)| p.clone()).collect();
        state.unlink(&ports);
        let c = &mut state.clients[self.index];
        c.active = false;
        c.closed = true;
        c.engine = None;
        c.closed_on = std::thread::current().name().map(str::to_owned);
        Ok(())
    }
}

/// The ports linked to `port` in `links`.
fn partners(links: &[(String, String)], port: &str) -> Vec<String> {
    links
        .iter()
        .filter_map(|(a, b)| {
            if a == port {
                Some(b.clone())
            } else if b == port {
                Some(a.clone())
            } else {
                None
            }
        })
        .collect()
}
