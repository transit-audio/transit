//! A scripted JACK server for tests (ADR 0021, "Fake server"): it opens clients, lists physical
//! ports, records registrations, activations, connections and closes, and drives the callbacks the
//! real libjack would call (`process`, buffer size, sample rate, xrun, info shutdown) through the
//! same safe engine and handlers `crate::callback` calls.

use crate::api::{ClientError, JackApi, JackClient, Latency, PhysicalPort, PortName};
use crate::engine::{Clock, Cycle, Engine, InputPlanes, OutputPlanes, Shared};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

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
    /// Reads of the rate or the buffer size while the client was not active.
    pub(crate) inactive_reads: u32,
    /// PipeWire: the quantum this client forces while it is active (`node.force-quantum`).
    pub(crate) force: Option<u32>,
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
    /// JACK 2's double-buffered graph: a link a connect makes shows only after this many graph
    /// cycles (0: at once). Each [`JackApi::await_graph`] is one cycle.
    link_delay: u32,
    /// Links made and not shown yet, with the graph cycles left until they show.
    unseen: Vec<((String, String), u32)>,
    /// PipeWire's JACK library (ADR 0021, amendment for #760: the fake's PipeWire mode); `None`
    /// is JACK 2's behavior (one global quantum, applied at once).
    pipewire: Option<PipeWireMode>,
    /// What the graph does after a number of graph steps ([`JackApi::await_graph`]), in order.
    scheduled: Vec<(u32, Event)>,
    /// The latency each port reports, by (name, playback); a port not listed reports zero ("Before
    /// a port is connected, this returns the default latency: zero", jack.h).
    latencies: Vec<(String, bool, (u32, u32))>,
    /// Ports whose latency the read cannot report (gone between the listing and the read).
    unreported: Vec<String>,
    /// The library lacks `jack_port_get_latency_range`.
    no_latency_call: bool,
    /// Latency reads so far.
    latency_reads: u32,
    /// Link reads of transit's own ports so far ([`JackClient::links`]).
    link_reads: u32,
    /// The server stops as the next read of this kind begins ([`Read`]); the read still answers
    /// (the client handle stays valid until transit closes it). The shutdown callback runs on
    /// another thread and takes no lock of transit's.
    shutdown_during: Option<Read>,
}

/// A client read the fake can stop the server in ([`FakeServer::shutdown_during`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    PhysicalPorts,
    Links,
    Latency,
}

/// How the fake answers as PipeWire's JACK library (amendment for #760, "The fake gains a PipeWire
/// mode"). The running figures are the server's `rate` and `buffer`.
#[derive(Debug, Clone, Default)]
pub(crate) struct PipeWireMode {
    /// What an inactive client without a `node.latency` request reports: the server's defaults
    /// (`clock.rate`, `clock.quantum`), not what the graph runs.
    pub(crate) default: (u32, u32),
    /// The client's own `node.latency` request (`PIPEWIRE_LATENCY`): an inactive client reports it.
    pub(crate) latency: Option<(u32, u32)>,
    /// The figure the buffer size callback reports synchronously inside `jack_activate`; `None`:
    /// the running buffer (or the client's force, when it arrives at once).
    pub(crate) activation_buffer: Option<u32>,
    /// A position that is stale after activation: the getters of an active client report it
    /// until the first process cycle corrects it.
    pub(crate) stale: Option<(u32, u32)>,
    /// Graph steps between a client's activation (or a request while active) and the arrival of
    /// the quantum it forces; 0 arrives at activation.
    pub(crate) force_delay: u32,
    /// The quantum the driver picks once the forcing client deactivates or closes; `None` keeps
    /// the forced one.
    pub(crate) restore: Option<u32>,
}

/// Something the fake's graph does after some graph steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// The quantum changes (another client's force, or a client's own request arriving).
    Quantum(u32),
    /// The graph's rate changes.
    Rate(u32),
    /// The server stops.
    Shutdown,
    /// One process cycle of this many frames on the active client, with no port buffers.
    Cycle(u32),
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
        self.unseen.retain(|((a, b), _)| !ports.contains(a) && !ports.contains(b));
    }

    /// Whether the graph holds `link`, shown or not yet.
    fn holds(&self, link: &(String, String)) -> bool {
        self.links.contains(link) || self.unseen.iter().any(|(l, _)| l == link)
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
            link_delay: 0,
            unseen: Vec::new(),
            pipewire: None,
            scheduled: Vec::new(),
            latencies: Vec::new(),
            unreported: Vec::new(),
            no_latency_call: false,
            latency_reads: 0,
            link_reads: 0,
            shutdown_during: None,
        };
        Self { state: Arc::new(Mutex::new(state)) }
    }

    /// PipeWire's JACK library over one stereo device node, the graph running at `rate` and
    /// quantum `buffer`, the server's defaults (what an inactive client reports) at `default`.
    pub(crate) fn pipewire(rate: u32, buffer: u32, default: (u32, u32)) -> Self {
        let server = Self::running(
            rate,
            buffer,
            &[("Scarlett:capture_AUX0", true), ("Scarlett:capture_AUX1", true), ("Scarlett:playback_AUX0", false), ("Scarlett:playback_AUX1", false)],
        );
        server.set_version("1.0.5 (PipeWire 1.0.5)");
        server.lock().pipewire = Some(PipeWireMode { default, ..PipeWireMode::default() });
        server
    }

    /// Changes the PipeWire mode's script.
    pub(crate) fn script_pipewire(&self, f: impl FnOnce(&mut PipeWireMode)) {
        if let Some(mode) = self.lock().pipewire.as_mut() {
            f(mode);
        }
    }

    /// `event` happens after `steps` more graph steps.
    pub(crate) fn schedule(&self, steps: u32, event: Event) {
        self.lock().scheduled.push((steps, event));
    }

    /// One graph step on the stepped clock, as a wait of `period` would be.
    pub(crate) fn step(&self, period: Duration) {
        JackApi::await_graph(self, period);
    }

    /// The port `name` reports the latency range `range` for `playback` (else capture).
    pub(crate) fn set_latency(&self, name: &str, playback: bool, range: (u32, u32)) {
        self.lock().latencies.push((name.to_owned(), playback, range));
    }

    /// The latency read of `name` cannot report it.
    pub(crate) fn unreported_latency(&self, name: &str) {
        self.lock().unreported.push(name.to_owned());
    }

    /// The library lacks `jack_port_get_latency_range`.
    pub(crate) fn without_latency_call(&self) {
        self.lock().no_latency_call = true;
    }

    /// Latency reads so far.
    pub(crate) fn latency_reads(&self) -> u32 {
        self.lock().latency_reads
    }

    /// Link reads of transit's own ports so far.
    pub(crate) fn link_reads(&self) -> u32 {
        self.lock().link_reads
    }

    /// One graph cycle: links made and not shown yet come one cycle closer to showing.
    fn graph_cycle(state: &mut State) {
        for (_, left) in &mut state.unseen {
            *left = left.saturating_sub(1);
        }
        let (shown, unseen): (Vec<_>, Vec<_>) = std::mem::take(&mut state.unseen).into_iter().partition(|(_, left)| *left == 0);
        state.unseen = unseen;
        state.links.extend(shown.into_iter().map(|(link, _)| link));
    }

    /// The server stops as the next read of kind `read` begins; the read still answers.
    pub(crate) fn shutdown_during(&self, read: Read) {
        self.lock().shutdown_during = Some(read);
    }

    /// Stops the server when `read` is the scripted one (with no lock of the fake's held).
    fn maybe_shutdown(&self, read: Read) {
        let due = {
            let mut state = self.lock();
            let due = state.shutdown_during == Some(read);
            if due {
                state.shutdown_during = None;
            }
            due
        };
        if due {
            self.shutdown();
        }
    }

    /// A link goes away (another program removed it).
    pub(crate) fn unlink(&self, source: &str, destination: &str) {
        self.lock().links.retain(|(a, b)| !(a == source && b == destination));
    }

    /// How many times a client read the rate or the buffer size while it was not active.
    pub(crate) fn inactive_reads(&self, client: usize) -> u32 {
        self.clients(|c| c.get(client).map_or(0, |c| c.inactive_reads))
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
        // PipeWire: no process callback runs before the notify thread delivered the graph's
        // figures (pj:2081-2084), so a stale position is corrected, with its callbacks, first.
        let corrected = {
            let mut state = self.lock();
            let stale = state.pipewire.as_mut().and_then(|mode| mode.stale.take());
            stale.map(|_| (state.rate, state.buffer))
        };
        if let (Some((rate, buffer)), Some(shared)) = (corrected, self.active_shared()) {
            shared.on_sample_rate(rate);
            shared.on_buffer_size(buffer);
        }
        let clock = {
            let mut state = self.lock();
            Self::graph_cycle(&mut state);
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

    /// From now on a link a connect makes shows in the graph only after `cycles` graph cycles (JACK
    /// 2 switches to the graph state a connect wrote at the start of a later cycle, postponed while
    /// the previous graph has not finished).
    pub(crate) fn delaying_links(&self, cycles: u32) {
        self.lock().link_delay = cycles;
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

    /// Another client's audio port changes its name (a stream that came back with another serial):
    /// it keeps its place in the server's order, and its links go.
    pub(crate) fn rename_port(&self, name: &str, to: &str) {
        let mut state = self.lock();
        state.unlink(&[name.to_owned()]);
        if let Some(port) = state.others.iter_mut().find(|(n, _, _)| n == name) {
            port.0 = to.to_owned();
        }
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

    /// The quantum changes and the active client's buffer size callback reports it (JACK 2: any
    /// client's `jack_set_buffer_size`; PipeWire: a force, the restore after a forcing client left,
    /// a node moved between drivers or a driver resume, since every JACK client locks the quantum by
    /// default, ADR 0021, amendment for #760, rule 7).
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

    /// One graph cycle of `period`, on the stepped clock: links not shown yet come one cycle
    /// closer to showing.
    fn await_graph(&self, period: Duration) {
        let due = {
            let mut state = self.lock();
            state.usecs += u64::try_from(period.as_micros()).unwrap_or(u64::MAX);
            Self::graph_cycle(&mut state);
            for (left, _) in &mut state.scheduled {
                *left = left.saturating_sub(1);
            }
            let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut state.scheduled).into_iter().partition(|(left, _)| *left == 0);
            state.scheduled = later;
            due
        };
        for (_, event) in due {
            match event {
                Event::Quantum(frames) => self.set_buffer_size(frames),
                Event::Rate(rate) => self.set_sample_rate(rate),
                Event::Shutdown => self.shutdown(),
                Event::Cycle(frames) => {
                    self.cycle(frames, &mut [], &[]);
                }
            }
        }
    }

    /// The fake's stepped clock (µs), advanced by cycles and graph waits only.
    fn now_us(&self) -> u64 {
        self.lock().usecs
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

    /// One of the (rate, buffer) figures this client reads now.
    fn read(&self, pick: impl Fn((u32, u32)) -> u32) -> u32 {
        let mut state = self.server.lock();
        let running = (state.rate, state.buffer);
        let client = &mut state.clients[self.index];
        let active = client.active;
        if !active {
            client.inactive_reads += 1;
        }
        let figures = match &state.pipewire {
            Some(mode) if !active => mode.latency.unwrap_or(mode.default),
            Some(PipeWireMode { stale: Some(stale), .. }) => *stale,
            _ => running,
        };
        pick(figures)
    }

    /// PipeWire: the force this client holds arrives at the graph, now or after the mode's delay.
    fn force_arrives(&self, frames: u32) {
        let delay = self.server.lock().pipewire.as_ref().map_or(0, |m| m.force_delay);
        if delay == 0 {
            self.server.set_buffer_size(frames);
        } else {
            self.server.schedule(delay, Event::Quantum(frames));
        }
    }

    /// PipeWire: the client stops forcing (it deactivated or closed): the driver picks its own.
    fn force_ends(&self) {
        let mut state = self.server.lock();
        let had = state.clients[self.index].force.take().is_some();
        if let Some(restore) = state.pipewire.as_ref().and_then(|m| m.restore).filter(|_| had) {
            state.buffer = restore;
        }
    }
}

impl JackClient for FakeClient {
    fn name(&self) -> String {
        self.record(|c, _| c.name.clone())
    }

    /// JACK 2: the server's rate. PipeWire: an inactive client's request or the default; an active
    /// client's stale position until the first cycle; else the graph's rate.
    fn sample_rate(&self) -> u32 {
        self.read(|figures| figures.0)
    }

    fn buffer_size(&self) -> u32 {
        self.read(|figures| figures.1)
    }

    fn physical_ports(&self) -> Vec<PhysicalPort> {
        self.server.maybe_shutdown(Read::PhysicalPorts);
        let state = self.server.lock();
        state.ports.iter().filter(|p| p.capture).chain(state.ports.iter().filter(|p| !p.capture)).cloned().collect()
    }

    /// As the binding: physical capture ports, then other clients' audio outputs, then the output
    /// ports of clients that are open (transit's own among them); a name that is not valid UTF-8
    /// is left out.
    fn output_ports(&self) -> Vec<String> {
        let state = self.server.lock();
        let physical = state.ports.iter().filter(|p| p.capture).map(|p| p.name.clone());
        let others = state.others.iter().filter(|(name, output, audio)| *output && *audio && !state.invalid.iter().any(|(key, _)| key == name));
        let clients =
            state.clients.iter().filter(|c| !c.closed).flat_map(|c| c.ports.iter().filter(|(_, input)| !input).map(|(name, _)| name.clone()));
        physical.chain(others.map(|(name, _, _)| name.clone())).chain(clients).collect()
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
        self.record(|c, running| -> Result<(), String> {
            if !*running {
                return Err("the server is gone".into());
            }
            if c.shared.is_some() {
                return Err("the callbacks are installed once".into());
            }
            c.shared = Some(Arc::clone(&shared));
            c.engine = Some(EngineSlot(Arc::new(Mutex::new(engine))));
            c.active = true;
            c.activations += 1;
            Ok(())
        })?;
        // PipeWire: activation clears the cached buffer size and calls the buffer size callback
        // synchronously with the position the client then holds (pj:4920-4921, pj:4955-4958); a
        // force the client holds arrives at the graph now or later.
        let pipewire = self.server.lock().pipewire.clone();
        let force = self.record(|c, _| c.force);
        if let Some(mode) = pipewire {
            let delay = mode.force_delay;
            if let Some(frames) = force.filter(|_| delay == 0) {
                self.server.lock().buffer = frames;
            }
            let buffer = self.server.lock().buffer;
            shared.on_buffer_size(mode.activation_buffer.unwrap_or(buffer));
            if let Some(frames) = force.filter(|_| delay > 0) {
                self.server.schedule(delay, Event::Quantum(frames));
            }
        }
        Ok(())
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
            if !state.holds(&link) {
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
        if state.duplicates || !state.holds(&link) {
            if state.link_delay > 0 {
                let delay = state.link_delay;
                state.unseen.push((link.clone(), delay));
            } else {
                state.links.push(link.clone());
            }
            state.clients[self.index].connections.push(link);
        }
        Ok(())
    }

    #[cfg(test)]
    fn connections(&self, port: &str) -> Vec<String> {
        partners(&self.server.lock().links, port)
    }

    fn links(&self, input: bool, index: usize) -> Vec<PortName> {
        self.server.maybe_shutdown(Read::Links);
        let mut state = self.server.lock();
        state.link_reads += 1;
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

    /// JACK 2: the server's one quantum changes at once, for every client. PipeWire: the client's
    /// own node forces it while the client is active (pj:5413-5437, keys.h:173-174).
    fn latency_range(&self, name: &str, playback: bool) -> Latency {
        self.server.maybe_shutdown(Read::Latency);
        let mut state = self.server.lock();
        if state.no_latency_call {
            return Latency::NoFunction;
        }
        state.latency_reads += 1;
        if state.unreported.iter().any(|n| n == name) || state.port(name).is_none() {
            return Latency::NoPort;
        }
        let (min, max) = state.latencies.iter().find(|(n, p, _)| n == name && *p == playback).map_or((0, 0), |(_, _, range)| *range);
        Latency::Range(min, max)
    }

    fn set_buffer_size(&mut self, frames: u32) -> Result<(), String> {
        self.server.lock().quantum_requests.push(frames);
        if let Some(text) = self.server.lock().refuse_quantum.clone() {
            return Err(text);
        }
        if self.server.lock().pipewire.is_none() {
            self.server.set_buffer_size(frames);
            return Ok(());
        }
        let active = self.record(|c, _| {
            c.force = Some(frames);
            c.active
        });
        if active {
            self.force_arrives(frames);
        }
        Ok(())
    }

    /// jack.h: "disconnect all ports belonging to it, since inactive clients have no port
    /// connections."
    fn deactivate(&mut self) -> Result<(), String> {
        self.force_ends();
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
        self.force_ends();
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
