//! Native MIDI ingestion, independent of the UI pump. The worker owns only raw
//! consumers and a bounded action producer, never devices, plug-ins or editors.
//! Main retains the input handles; closing one invalidates its source first.
use super::midi_map::{self, MapEvent, Snapshot};
use super::monitor::{self, ClockRate, Dir, MonitorEvent, Tap};
use super::{ActionResult, PlayedMidi, PlayedNote, RuntimeControl, sources};
use crate::action_lane::{self, NoteKind, Outcome};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;
use transit_midi_io::{InputBoundary, InputConsumer, InputEvent};
use transit_mix::{EngineEpoch, EventOutcome, Message, NoteId, SlotBinding, Source};

const BATCH: usize = 256;
/// A port with no admitting route that holds more than this many events (all already shown by the
/// MIDI Monitor) is trimmed to half of it.
const MONITOR_TRIM: usize = 512;
const RELEASE_RESERVE: usize = 32;
/// After work, the worker keeps stepping at this interval for `LINGER` rounds so the audio
/// thread's receipts (credits, release bookkeeping) are reclaimed promptly. `thread::sleep` uses a
/// high-resolution timer on Windows; a park timeout would round up to the system tick (#62).
const LINGER_PERIOD: Duration = Duration::from_micros(250);
const LINGER: u32 = 16;
/// Idle safety net only: input data, attachments, route edits and stop all unpark the worker.
/// Port health (unplug, close) is also noticed within this period.
pub(super) const IDLE: Duration = Duration::from_millis(20);

/// Original timestamps and raw sequence survive scheduling. Audio maps H into
/// its consumed-output/converter phase and rechecks the source cutoff there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub engine: EngineEpoch,
    pub target: SlotBinding,
    pub source: Source,
    pub message: Message,
    pub raw: InputEvent,
    /// Explicit monitoring lookahead in output-device frames, never callbacks.
    pub lookahead: u32,
    /// False for a track with an external MIDI destination (#61): the audio thread only
    /// captures the event for a MIDI recording; the instrument does not play it.
    pub deliver: bool,
}
const _: () = assert!(std::mem::size_of::<Packet>() <= 128);

pub type Lane = (action_lane::Control<Packet, ActionResult>, action_lane::Audio<Packet, ActionResult>);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub offered: u64,
    pub admitted: u64,
    pub filtered: u64,
    pub cancelled: u64,
    pub shed: u64,
    pub recovered: u64,
    pub refused: u64,
    /// Audio-admitted packets using approximate callback-relative timing.
    pub approximate: u64,
    /// Input-to-worker latency (backend callback entry to the worker popping the event) of the
    /// latest event, and the largest seen. Add the audio block period for input-to-sound.
    pub latency_last_ns: u64,
    pub latency_max_ns: u64,
    /// Input ports muted by the feedback storm detector (#99).
    pub storms: u64,
}

/// UI tap capacity, preallocated before the ingress thread starts.
const TAP_CAPACITY: usize = 256;

/// A tap entry with the instant the worker popped it: Step entry times holds from this, not from
/// the UI drain (#139). `Instant` is Copy, so stamping allocates nothing.
pub type Stamped = (PlayedMidi, std::time::Instant);
/// MIDI Learn event ring capacity, preallocated before the ingress thread starts.
const MAP_CAPACITY: usize = 256;

#[derive(Default)]
struct Shared {
    /// Overflow suppresses further tap writes until the UI discards the incomplete batch.
    tap_lost: AtomicBool,
    stop: AtomicBool,
    finished: AtomicBool,
    /// MIDI Learn is armed (Main sets it; the worker clears it when it forwards the candidate).
    learn: AtomicBool,
    /// Map events the full ring refused.
    map_dropped: AtomicU64,
    /// Button presses and candidates pushed to the ring, ever.
    map_pushed: AtomicU64,
    offered: AtomicU64,
    admitted: AtomicU64,
    filtered: AtomicU64,
    cancelled: AtomicU64,
    shed: AtomicU64,
    recovered: AtomicU64,
    refused: AtomicU64,
    approximate: AtomicU64,
    /// Callback-to-worker-pop age of the latest and the largest popped event (#62).
    latency_last_ns: AtomicU64,
    latency_max_ns: AtomicU64,
    /// Input ports muted by the feedback storm detector (#99).
    storms: AtomicU64,
    /// Tests: the storm detector is off, so a feedback loop can be shown to form.
    #[cfg(test)]
    storm_off: AtomicBool,
}

/// Main's per-open cancellation identity. Capture/publish all port cutoffs
/// before admitting the corresponding transport Stop. This never drains a ring.
pub struct Port {
    permit: sources::Permit,
    boundary: InputBoundary,
}
impl Port {
    pub(super) fn new(permit: sources::Permit, boundary: InputBoundary) -> Self {
        Self { permit, boundary }
    }
    pub(super) fn permit(&self) -> sources::Permit {
        self.permit.clone()
    }
    pub fn token(&self) -> sources::Token {
        self.permit.token()
    }
    pub fn cancel_before_stop<T: Copy, R: Copy>(&self, owner: &mut RuntimeControl<T, R>) -> Result<(), super::PlayerError> {
        owner.cancel_source_before(self.token(), self.boundary.cutoff())
    }
    pub fn invalidate(&self) {
        self.permit.invalidate();
    }
    pub fn admitted(&self) -> bool {
        self.permit.admits()
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        self.invalidate();
    }
}

/// A route is bound to one instrument incarnation. A replacement needs a new
/// binding; stale queued packets are refused by the mixer, never retargeted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub engine: EngineEpoch,
    pub target: SlotBinding,
    pub channels: u16,
    pub lookahead: u32,
    /// Play the event on the target instrument (false: capture only, see [`Packet::deliver`]).
    pub deliver: bool,
    /// Forward the event to the track's external MIDI destination through the output thread's
    /// echo ring, as soon as it is popped here (#61).
    pub echo: bool,
    /// The echo goes to the input port's own device (the track allows it, #99): what comes back
    /// from there is correlated by the storm detector.
    pub self_echo: bool,
}

struct Input {
    id: u64,
    consumer: InputConsumer,
    routes: Vec<(sources::Permit, Route)>,
    /// MIDI Monitor bookkeeping (#91), including discarding closed-window SysEx captures.
    mon: Mon,
    /// Feedback storm detector and the port's mute flag (#99).
    guard: storm::Guard,
    /// Whether a route admitted at the last pass: the edge to none is a revocation (#134).
    live: bool,
}

/// Per-input state of the MIDI Monitor tap. Only the ingress worker touches it.
#[derive(Default)]
struct Mon {
    /// The next input sequence number not yet tapped, so a message seen while its port had no
    /// admitting route and again after it was popped appears once.
    tap_next: u64,
    clock: ClockRate,
    /// The rate slot this input publishes into.
    slot: Option<(Arc<monitor::Shared>, usize)>,
    /// The last filtered Active Sensing stamp published (the ring's `sensing_host_ns`; 0: none).
    sensing: u64,
    sysex_dropped: u64,
    sysex_truncated: u64,
    /// Whether the callback of this input is told to capture SysEx.
    capturing: bool,
    /// The next input sequence number the MIDI Learn matcher has not seen.
    map_next: u64,
    /// Notes whose note-on a mapping consumed (channel x key), so exactly their note-off is
    /// consumed too, and a note held while a mapping is added still gets its note-off through.
    held: Box<[u128; 16]>,
}
impl Drop for Mon {
    fn drop(&mut self) {
        if let Some((shared, slot)) = self.slot.take() {
            shared.release(slot);
        }
    }
}
impl Mon {
    /// Switch the input's SysEx capture. Turning it on starts from clean baselines: stale captures
    /// are discarded (at most one ring) and the drop and truncation counts seen so far are absorbed.
    fn set_capture(&mut self, on: bool, consumer: &mut InputConsumer) {
        self.capturing = on;
        consumer.set_sysex_capture(on);
        if on {
            for _ in 0..64 {
                if consumer.pop_sysex().is_none() {
                    break;
                }
            }
            let counters = consumer.counters();
            self.sysex_dropped = counters.sysex_capture_dropped;
            self.sysex_truncated = counters.sysex_truncated;
        }
    }

    /// One message of the input's ring. Real-Time Clock feeds the rate estimator and is an event
    /// only on request; Active Sensing is only noted.
    fn tap(&mut self, tap: &mut Tap, id: u64, raw: &InputEvent) {
        if raw.seq < self.tap_next {
            return;
        }
        self.tap_next = raw.seq.saturating_add(1);
        if !matches!(raw.bytes[0], 0xF8 | 0xFE) {
            tap.shared().note_activity(id as u16, raw.bytes);
        }
        if !tap.enabled() {
            return;
        }
        match raw.bytes[0] {
            0xF8 => {
                self.clock.pulse(raw.host_arrival_ns);
                if let Some((shared, slot)) = &self.slot {
                    shared.publish_rate(*slot, self.clock.raw());
                }
                if !tap.show_clock() {
                    return;
                }
            }
            0xFE => {
                if let Some((shared, slot)) = &self.slot {
                    shared.publish_sensing(*slot, raw.host_arrival_ns);
                }
                return;
            }
            _ => {}
        }
        tap.push(MonitorEvent::message(Dir::In, id as u16, raw.host_arrival_ns, raw.message()));
    }

    /// Filtered sensing and bounded diagnostic SysEx, independent of playable MIDI routes.
    fn watch(&mut self, tap: &mut Tap, id: u64, consumer: &mut InputConsumer) {
        let counters = consumer.counters();
        // The FE's own arrival stamp (the ring records it at push), never this worker's clock: a
        // sensing readout depends on when the controller sent it, not on when the worker looked.
        if counters.sensing_host_ns != self.sensing {
            self.sensing = counters.sensing_host_ns;
            if let Some((shared, slot)) = &self.slot {
                shared.publish_sensing(*slot, self.sensing);
            }
        }
        if tap.enabled() {
            tap.note_sysex_counts(
                counters.sysex_capture_dropped.saturating_sub(self.sysex_dropped),
                counters.sysex_truncated.saturating_sub(self.sysex_truncated),
            );
        }
        self.sysex_dropped = counters.sysex_capture_dropped;
        self.sysex_truncated = counters.sysex_truncated;
        // At most one ring capacity per step, even if the callback keeps producing.
        for _ in 0..64 {
            let Some((host, total, bytes)) = consumer.pop_sysex() else { break };
            if tap.enabled() {
                tap.push_sysex(id as u16, host, total, &bytes[..(total as usize).min(64)]);
            }
        }
    }
}
impl Input {
    fn healthy(&self) -> bool {
        !self.consumer.is_closed()
            && !self.consumer.is_disconnected()
            && !self.consumer.is_exhausted()
            && self.consumer.counters().dropped_releases == 0
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        for (permit, _) in &self.routes {
            permit.invalidate();
        }
    }
}

enum Update {
    Input(Box<Input>),
    Echo(super::midi_out::Producer),
    Monitor(Tap),
    Routes {
        id: u64,
        routes: Vec<(sources::Permit, Route)>,
        applied: Arc<AtomicBool>,
    },
    /// The MIDI Learn mappings in force (ADR 0028).
    Maps(Arc<Snapshot>),
}

/// Control-side handle. Drop closes admission immediately and wakes the worker;
/// it does not join or move an input-device destructor onto the audio thread.
pub struct Worker {
    attach: SyncSender<Update>,
    permits: Vec<sources::Permit>,
    next_input: u64,
    shared: Arc<Shared>,
    thread: std::thread::JoinHandle<()>,
    tap: RefCell<rtrb::Consumer<Stamped>>,
    /// MIDI Learn events for Main (ADR 0028).
    maps: RefCell<rtrb::Consumer<MapEvent>>,
}
impl Worker {
    pub fn start(lane: action_lane::Control<Packet, ActionResult>) -> std::io::Result<Self> {
        Self::start_with_idle(lane, IDLE)
    }

    /// [`Worker::start`] with the idle park timeout chosen by the caller. Tests set it far above
    /// any assertion bound so only a wake on data, never the timer, can make the worker pop (E49, #110).
    pub(super) fn start_with_idle(lane: action_lane::Control<Packet, ActionResult>, idle: Duration) -> std::io::Result<Self> {
        let (attach, receive) = mpsc::sync_channel(sources::PORTS);
        let shared = Arc::new(Shared::default());
        let (tap, tap_receive) = rtrb::RingBuffer::new(TAP_CAPACITY);
        let (map_out, map_receive) = rtrb::RingBuffer::new(MAP_CAPACITY);
        let mut pump = Pump {
            lane,
            receive,
            inputs: Vec::with_capacity(sources::PORTS),
            cursor: 0,
            shared: Arc::clone(&shared),
            echo: None,
            tap,
            monitor: None,
            maps: None,
            map_out,
            map_pass_on: false,
            #[cfg(test)]
            between_check_and_push: None,
        };
        let thread = std::thread::Builder::new().name("transit-midi-ingress".into()).spawn(move || {
            let mut linger = 0u32;
            while !pump.shared.stop.load(Ordering::Acquire) {
                if pump.step() {
                    linger = LINGER;
                }
                if linger > 0 {
                    linger -= 1;
                    std::thread::sleep(LINGER_PERIOD);
                } else {
                    // Woken by the input callback (`InputConsumer::wake_on`) or an attach.
                    std::thread::park_timeout(idle);
                }
            }
        })?;
        Ok(Self {
            attach,
            permits: Vec::with_capacity(sources::PORTS),
            next_input: 0,
            shared,
            thread,
            tap: RefCell::new(tap_receive),
            maps: RefCell::new(map_receive),
        })
    }

    /// Open the input with this runtime's HostOrigin before attaching. The caller
    /// retains its InputHandle and this Port until it explicitly disconnects.
    pub fn attach(&mut self, consumer: InputConsumer, permit: sources::Permit, route: Route) -> Result<Port, &'static str> {
        self.permits.retain(sources::Permit::admits);
        if route.channels == 0 || route.lookahead > 8192 {
            permit.invalidate();
            return Err("invalid MIDI route or lookahead");
        }
        if self.shared.finished.load(Ordering::Acquire) || self.permits.len() >= sources::PORTS || !permit.admits() {
            permit.invalidate();
            return Err("MIDI worker unavailable or source revoked");
        }
        if self.permits.iter().any(|old| old.token() == permit.token()) {
            return Err("MIDI source already attached");
        }
        consumer.wake_on(self.thread.thread().clone());
        let port = Port { boundary: consumer.boundary(), permit: permit.clone() };
        let id = self.next_input.checked_add(1).ok_or("MIDI input identities exhausted")?;
        self.next_input = id;
        self.attach
            .try_send(Update::Input(Box::new(Input {
                id,
                consumer,
                routes: vec![(permit.clone(), route)],
                mon: Mon::default(),
                guard: storm::Guard::default(),
                live: false,
            })))
            .map_err(|_| "MIDI worker attachment queue unavailable")?;
        self.permits.push(permit);
        self.thread.thread().unpark();
        Ok(port)
    }

    /// One consumer per physical open; routing edits never reopen the device. The worker sets
    /// `muted` when the port's feedback storm detector trips (#99); while it is set the port
    /// admits no route, and Main clears it when the user re-enables the port.
    pub(super) fn attach_input(&mut self, consumer: InputConsumer, muted: Arc<AtomicBool>) -> Result<u64, &'static str> {
        if self.shared.finished.load(Ordering::Acquire) {
            return Err("MIDI worker unavailable");
        }
        let id = self.next_input.checked_add(1).ok_or("MIDI input identities exhausted")?;
        self.next_input = id;
        consumer.wake_on(self.thread.thread().clone());
        self.attach
            .try_send(Update::Input(Box::new(Input {
                id,
                consumer,
                routes: Vec::new(),
                mon: Mon::default(),
                guard: storm::Guard::new(muted),
                live: false,
            })))
            .map_err(|_| "MIDI attachment queue unavailable")?;
        self.thread.thread().unpark();
        Ok(id)
    }

    pub(super) fn set_routes(&mut self, id: u64, routes: Vec<(sources::Permit, Route)>) -> Result<Arc<AtomicBool>, &'static str> {
        self.permits.retain(sources::Permit::admits);
        if self.shared.finished.load(Ordering::Acquire)
            || routes.len() > sources::PORTS
            || routes.iter().any(|(permit, route)| !permit.admits() || route.channels == 0 || route.lookahead > 8192)
        {
            return Err("MIDI route unavailable or invalid");
        }
        for (permit, _) in &routes {
            if !self.permits.iter().any(|old| old.token() == permit.token()) {
                if self.permits.len() == sources::PORTS {
                    return Err("MIDI source capacity exhausted");
                }
                self.permits.push(permit.clone());
            }
        }
        let applied = Arc::new(AtomicBool::new(false));
        self.attach.try_send(Update::Routes { id, routes, applied: Arc::clone(&applied) }).map_err(|_| "MIDI route queue unavailable")?;
        self.thread.thread().unpark();
        Ok(applied)
    }

    /// Hands the worker the output thread's echo producer (#61).
    pub(super) fn set_echo(&mut self, echo: super::midi_out::Producer) -> Result<(), &'static str> {
        self.attach.try_send(Update::Echo(echo)).map_err(|_| "MIDI echo queue unavailable")?;
        self.thread.thread().unpark();
        Ok(())
    }

    /// Hands the worker the MIDI Monitor's input tap (#91).
    pub(super) fn set_monitor(&mut self, tap: Tap) -> Result<(), &'static str> {
        self.attach.try_send(Update::Monitor(tap)).map_err(|_| "MIDI monitor queue unavailable")?;
        self.thread.thread().unpark();
        Ok(())
    }

    /// Swaps in the MIDI Learn mappings (ADR 0028). The worker owns a reference until the next swap.
    pub(super) fn set_maps(&mut self, maps: Arc<Snapshot>) -> Result<(), &'static str> {
        self.attach.try_send(Update::Maps(maps)).map_err(|_| "MIDI mapping queue unavailable")?;
        self.thread.thread().unpark();
        Ok(())
    }

    /// Arms or disarms MIDI Learn: while armed, the worker forwards the next CC or note-on once.
    pub(super) fn arm_learn(&self, armed: bool) {
        self.shared.learn.store(armed, Ordering::Release);
        if armed {
            self.thread.thread().unpark();
        }
    }

    /// The MIDI Learn events published since the last call, oldest first.
    pub(super) fn take_map_events(&self) -> Vec<MapEvent> {
        let Ok(mut ring) = self.maps.try_borrow_mut() else { return Vec::new() };
        let mut events = Vec::new();
        for _ in 0..MAP_CAPACITY {
            let Ok(event) = ring.pop() else { break };
            events.push(event);
        }
        events
    }

    pub(super) fn learn_armed(&self) -> bool {
        self.shared.learn.load(Ordering::Acquire)
    }

    /// Press and candidate events lost to a full ring (sweeps are coalesced and never lost).
    pub(super) fn map_pushed(&self) -> u64 {
        self.shared.map_pushed.load(Ordering::Relaxed)
    }

    pub(super) fn map_dropped(&self) -> u64 {
        self.shared.map_dropped.load(Ordering::Relaxed)
    }

    /// The notes played since the last call, oldest first.
    pub fn take_notes(&self) -> Vec<PlayedNote> {
        self.take_midi()
            .into_iter()
            .filter_map(|m| match m {
                PlayedMidi::Note(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// Ordered notes and controls; only the UI allocates or borrows the consumer.
    pub fn take_midi(&self) -> Vec<PlayedMidi> {
        self.take_midi_stamped().into_iter().map(|(message, _)| message).collect()
    }

    /// Like [`Worker::take_midi`], each with the instant the worker saw it (a `Reset` is stamped now).
    pub fn take_midi_stamped(&self) -> Vec<Stamped> {
        let Ok(mut tap) = self.tap.try_borrow_mut() else { return Vec::new() };
        let mut messages = Vec::new();
        // Bound UI work as well: input may continue arriving while this frame drains.
        for _ in 0..TAP_CAPACITY {
            let Ok(message) = tap.pop() else { break };
            messages.push(message);
        }
        if self.shared.tap_lost.load(Ordering::Acquire) {
            while tap.pop().is_ok() {}
            messages.clear();
            messages.push((PlayedMidi::Reset, std::time::Instant::now()));
            self.shared.tap_lost.store(false, Ordering::Release);
        }
        messages
    }

    /// Independent diagnostic counters, not a coherent cancellation frontier.
    pub fn counters(&self) -> Counters {
        let s = &self.shared;
        Counters {
            offered: s.offered.load(Ordering::Relaxed),
            admitted: s.admitted.load(Ordering::Relaxed),
            filtered: s.filtered.load(Ordering::Relaxed),
            cancelled: s.cancelled.load(Ordering::Relaxed),
            shed: s.shed.load(Ordering::Relaxed),
            recovered: s.recovered.load(Ordering::Relaxed),
            refused: s.refused.load(Ordering::Relaxed),
            approximate: s.approximate.load(Ordering::Relaxed),
            latency_last_ns: s.latency_last_ns.load(Ordering::Relaxed),
            latency_max_ns: s.latency_max_ns.load(Ordering::Relaxed),
            storms: s.storms.load(Ordering::Relaxed),
        }
    }

    /// Tests: turns the feedback storm detector off, so a loop can be shown to form.
    #[cfg(test)]
    pub(super) fn disable_storm_detector(&self) {
        self.shared.storm_off.store(true, Ordering::Release);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        for permit in &self.permits {
            permit.invalidate();
        }
        self.shared.stop.store(true, Ordering::Release);
        self.thread.thread().unpark();
    }
}

struct Pump {
    lane: action_lane::Control<Packet, ActionResult>,
    receive: Receiver<Update>,
    inputs: Vec<Input>,
    cursor: usize,
    shared: Arc<Shared>,
    echo: Option<super::midi_out::Producer>,
    tap: rtrb::Producer<Stamped>,
    /// The MIDI Monitor's input ring (#91).
    monitor: Option<Tap>,
    /// The MIDI Learn mappings (ADR 0028) and the ring their events go out on.
    maps: Option<Arc<Snapshot>>,
    map_out: rtrb::Producer<MapEvent>,
    /// Whether the matcher ran on the last step, so a start skips the backlog.
    map_pass_on: bool,
    /// Tests: runs after the route check and before the echo push, where a preemption can land.
    #[cfg(test)]
    between_check_and_push: Option<Box<dyn FnMut() + Send>>,
}
impl Pump {
    fn tap(&mut self, message: Message) {
        let message = match message {
            Message::NoteOn { key, velocity, .. } => PlayedMidi::Note(PlayedNote { pitch: key, velocity, on: true }),
            Message::NoteOff { key, .. } => PlayedMidi::Note(PlayedNote { pitch: key, velocity: 0, on: false }),
            Message::Control { cc: 120 | 123, .. } => PlayedMidi::Reset,
            Message::Control { cc, value } => PlayedMidi::Control { cc, value },
            _ => return,
        };
        if !self.shared.tap_lost.load(Ordering::Acquire) && self.tap.push((message, std::time::Instant::now())).is_err() {
            self.shared.tap_lost.store(true, Ordering::Release);
        }
    }
}
impl Pump {
    /// One pass; true when it popped at least one event.
    fn step(&mut self) -> bool {
        let mut worked = false;
        // Drop only raw ring consumers here, never the native device handles.
        self.inputs.retain(|input| {
            let keep = input.healthy();
            if !keep {
                self.shared.recovered.fetch_add(1, Ordering::Relaxed);
                // A port that went away took its routes with it (#134): a held pedal is gone.
                if input.live || input.routes.iter().any(|(permit, _)| permit.admits()) {
                    self.shared.tap_lost.store(true, Ordering::Release);
                }
            }
            keep
        });
        // Inputs the worker held before this step's attachments (they are appended after these).
        let known = self.inputs.len();
        for _ in 0..sources::PORTS {
            match self.receive.try_recv() {
                Ok(Update::Input(input)) => {
                    if self.inputs.len() < sources::PORTS && input.healthy() {
                        let mut input = input;
                        if let Some(monitor) = &self.monitor {
                            input.mon.slot = monitor.shared().claim(input.id).map(|slot| (Arc::clone(monitor.shared()), slot));
                            let counters = input.consumer.counters();
                            input.mon.sysex_dropped = counters.sysex_capture_dropped;
                            input.mon.sysex_truncated = counters.sysex_truncated;
                        }
                        self.inputs.push(*input);
                    }
                }
                Ok(Update::Echo(echo)) => self.echo = Some(echo),
                Ok(Update::Monitor(tap)) => self.monitor = Some(tap),
                Ok(Update::Maps(maps)) => self.maps = Some(maps),
                Ok(Update::Routes { id, routes, applied }) => {
                    if let Some(input) = self.inputs.iter_mut().find(|input| input.id == id) {
                        for (old, old_route) in &input.routes {
                            match routes.iter().find(|(new, _)| old.token() == new.token()) {
                                None => {
                                    // Revoked (removal, disarm, monitor off): whatever it held is
                                    // gone, so step entry resets (#134).
                                    // Main invalidates the old permit before it sends the update (a new
                                    // token, then this), so `admits()` is already false here: the
                                    // edge is whether the port was live at the last pass.
                                    if input.live || old.admits() {
                                        self.shared.tap_lost.store(true, Ordering::Release);
                                    }
                                    old.invalidate();
                                }
                                // A channel filter change can strand a held pedal the same way.
                                Some((_, new_route)) if new_route.channels != old_route.channels => {
                                    self.shared.tap_lost.store(true, Ordering::Release);
                                }
                                Some(_) => {}
                            }
                        }
                        // A muted port (#99) admits nothing until Main clears the flag; routes
                        // sent after that start a fresh count.
                        if input.guard.muted() {
                            for (permit, _) in &routes {
                                if input.live || permit.admits() {
                                    self.shared.tap_lost.store(true, Ordering::Release);
                                }
                                permit.invalidate();
                            }
                        } else if input.guard.storm.tripped() {
                            input.guard.reset();
                        }
                        input.routes = routes;
                        applied.store(true, Ordering::Release);
                    } else {
                        for (permit, _) in routes {
                            permit.invalidate();
                        }
                    }
                }
                Err(_) => break,
            }
        }
        // Every other way a route stops admitting (a storm mute, Main closing a port or
        // invalidating its permits, a disarm) is seen here as the edge from admitting to not, so
        // one check covers them all (#134). Controls the route held are never released by it.
        for input in &mut self.inputs {
            let now = input.routes.iter().any(|(permit, _)| permit.admits());
            if input.live && !now {
                self.shared.tap_lost.store(true, Ordering::Release);
            }
            input.live = now;
        }
        for _ in 0..action_lane::NOTE_CAPACITY {
            let Some(receipt) = self.lane.reclaim() else { break };
            let accepted = receipt.outcome == Outcome::Applied
                && matches!(
                    receipt.handler_result,
                    Some(ActionResult::MidiBuffered)
                        | Some(ActionResult::MidiBufferedApproximate)
                        | Some(ActionResult::Event(
                            EventOutcome::Queued
                                | EventOutcome::QueuedLate { .. }
                                | EventOutcome::ReleaseAdvanced { .. }
                                | EventOutcome::Coalesced
                                | EventOutcome::Unmatched
                        ))
                );
            if accepted && receipt.handler_result == Some(ActionResult::MidiBufferedApproximate) {
                self.shared.approximate.fetch_add(1, Ordering::Relaxed);
            }
            if !accepted {
                self.shared.refused.fetch_add(1, Ordering::Relaxed);
                if receipt.payload.raw.is_release() {
                    for (permit, _) in self.inputs.iter().flat_map(|input| &input.routes) {
                        let token = permit.token();
                        if token.port() == receipt.payload.source.port
                            && token.generation() == receipt.payload.source.generation
                            && permit.admits_sequence(receipt.payload.raw.seq)
                            && !matches!(receipt.outcome, Outcome::Stopped | Outcome::NotesReleased)
                        {
                            permit.invalidate();
                            // Main rebuilds the route and may cancel what follows (#74): step
                            // entry gets a Reset rather than waiting on a pedal-up.
                            self.shared.tap_lost.store(true, Ordering::Release);
                        }
                    }
                }
            }
        }
        // Zero cost while the monitor is off: the only per-step work is one flag compare per input,
        // and the callback's SysEx capture is switched off with it.
        let on = self.monitor.as_ref().is_some_and(Tap::enabled);
        for input in &mut self.inputs {
            if input.mon.capturing != on {
                input.mon.set_capture(on, &mut input.consumer);
            }
        }
        if let Some(tap) = self.monitor.as_mut().filter(|tap| tap.enabled()) {
            for input in &mut self.inputs {
                input.mon.watch(tap, input.id, &mut input.consumer);
            }
        }
        // MIDI Learn (ADR 0028): zero cost without mappings or an armed learn (two loads and a
        // flag). Otherwise every open port is read without popping, so a controller no track
        // listens to still maps, and each message is seen once (`map_next`). A port with an
        // admitting route is matched where it is popped instead, so nothing arriving between this
        // pass and the pop is missed and a consumed message never reaches the route.
        // The first step after the matcher turns on skips what was already pending on the ports
        // the worker held while it was off; a port attached in this same step opened after Main
        // turned the matcher on (an armed learn opens every port), so its backlog is all news and
        // its first message may be the learn's candidate (#160).
        let learning = self.shared.learn.load(Ordering::Acquire);
        let pass_on = learning || self.maps.as_deref().is_some_and(|maps| !maps.is_empty());
        if pass_on {
            let first = !self.map_pass_on;
            for (index, input) in self.inputs.iter_mut().enumerate() {
                if input.routes.iter().any(|(permit, _)| permit.admits()) {
                    continue;
                }
                if first && index < known {
                    let Input { consumer, mon, .. } = input;
                    consumer.for_each_pending(|raw| mon.map_next = mon.map_next.max(raw.seq.saturating_add(1)));
                }
                map_pass(input, self.maps.as_deref(), &mut self.map_out, &self.shared);
            }
        }
        self.map_pass_on = pass_on;
        // A fixed amount of work even under continuous MIDI traffic. Round-robin
        // across ports prevents one input backlog from starving another release.
        for _ in 0..BATCH {
            if self.inputs.is_empty() {
                break;
            }
            self.cursor %= self.inputs.len();
            let Some(input) = self.inputs.get_mut(self.cursor) else { break };
            self.cursor += 1;
            if !input.healthy() {
                for (permit, _) in &input.routes {
                    permit.invalidate();
                }
                continue;
            }
            // An input can attach before its initial routing update arrives.
            // Preserve that backlog; each new route's raw cutoff rejects older
            // offers once the worker applies it. Admission is not application.
            if !input.routes.iter().any(|(permit, _)| permit.admits()) {
                // The MIDI Monitor still sees what this port receives, without taking it from the
                // ring (a route attached later keeps its backlog); a ring more than half full of
                // events already shown is trimmed so the port keeps accepting.
                let mut watched = pass_on;
                if let Some(tap) = self.monitor.as_mut().filter(|tap| tap.enabled()) {
                    let Input { id, consumer, mon, .. } = input;
                    consumer.for_each_pending(|raw| mon.tap(tap, *id, raw));
                    watched = true;
                }
                // MIDI Learn has seen everything pending too (above), so the trim loses nothing.
                if watched && input.consumer.len() > MONITOR_TRIM {
                    let n = input.consumer.len() - MONITOR_TRIM / 2;
                    input.consumer.discard(n);
                }
                continue;
            }
            let Some(raw) = input.consumer.pop() else { continue };
            worked = true;
            if let Some(tap) = &mut self.monitor {
                input.mon.tap(tap, input.id, &raw);
            }
            #[cfg(test)]
            let detect = !self.shared.storm_off.load(Ordering::Relaxed);
            #[cfg(not(test))]
            let detect = true;
            // Only a port with an echo route can loop, so only those are watched.
            let echoing = input.routes.iter().any(|(permit, route)| route.echo && permit.admits());
            if detect && echoing && input.guard.note(raw.host_arrival_ns, raw.bytes, raw.len) {
                // A feedback storm (#99): the port stops admitting now. Main sees its routes
                // revoked, which releases what they hold (the instrument's notes, and the echo's
                // on the output port), and keeps them off until the user re-enables the port.
                input.guard.muted.store(true, Ordering::Release);
                for (permit, _) in &input.routes {
                    permit.invalidate();
                }
                self.shared.storms.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let age = input.consumer.age_ns(&raw);
            self.shared.latency_last_ns.store(age, Ordering::Relaxed);
            self.shared.latency_max_ns.fetch_max(age, Ordering::Relaxed);
            self.shared.offered.fetch_add(1, Ordering::Relaxed);
            let Some((channel, message)) = decode(raw) else {
                self.shared.filtered.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            // MIDI Learn: match here, so a message is matched exactly once and a mapped one
            // (without pass-through) never reaches the routes.
            let mut consumed = false;
            if pass_on {
                if raw.seq >= input.mon.map_next {
                    input.mon.map_next = raw.seq.saturating_add(1);
                    consumed = match_one(input.id, channel, message, self.maps.as_deref(), &mut self.map_out, &self.shared);
                } else if let Some(maps) = self.maps.as_deref() {
                    consumed = midi_map::consumes(maps, input.id, channel, message);
                }
            }
            if consume_notes(&mut input.mon, channel, message, consumed) {
                self.shared.filtered.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let mut tapped = false;
            let mut reset_step = false;
            for (permit, route) in &input.routes {
                // The echo route generation is read before the route check: a revocation after
                // it leaves this event older than the release, so the output thread drops it.
                let generation = self.echo.as_ref().map_or(0, super::midi_out::Producer::echo_generation);
                if !permit.admits_sequence(raw.seq) {
                    self.shared.cancelled.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if route.channels & (1 << channel) == 0 {
                    self.shared.filtered.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                // Step entry (#74) is control-rate and must see every message a route admits, even
                // one the lane then sheds: a pedal-up shed after its pedal-down got through would
                // leave the step editor holding the pedal.
                tapped = true;
                #[cfg(test)]
                if let Some(hook) = &mut self.between_check_and_push {
                    hook();
                }
                // Channel-mode messages (CC 120 to 127: All Sound Off, Reset All Controllers, Local
                // Control, All Notes Off, Omni and Mono/Poly) are never echoed (#99): sent back to a
                // device that merges its In to its Out, they returned and went out again forever.
                if route.echo
                    && !channel_mode(message)
                    && let Some(echo) = &mut self.echo
                {
                    let pushed = echo.push(super::midi_out::OutEvent {
                        kind: super::midi_out::Kind::Message,
                        track: route.target.0.0,
                        at_ns: raw.host_arrival_ns,
                        bytes: raw.bytes,
                        len: raw.len,
                        delay_ns: generation,
                    });
                    if pushed && route.self_echo {
                        input.guard.echoed(raw.host_arrival_ns, raw.bytes, raw.len);
                    }
                }
                let release = raw.is_release();
                let credits = self.lane.note_credits();
                if credits == 0 || (!release && credits <= RELEASE_RESERVE) {
                    self.shared.shed.fetch_add(1, Ordering::Relaxed);
                    if release {
                        permit.invalidate();
                        reset_step = true;
                    }
                    continue;
                }
                let token = permit.token();
                let packet = Packet {
                    engine: route.engine,
                    target: route.target,
                    source: Source { port: token.port(), channel, generation: token.generation() },
                    message,
                    raw,
                    lookahead: route.lookahead,
                    deliver: route.deliver,
                };
                if self.lane.send_note(packet, if release { NoteKind::Off } else { NoteKind::On }).is_ok() {
                    self.shared.admitted.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.shared.shed.fetch_add(1, Ordering::Relaxed);
                    permit.invalidate();
                    reset_step |= release;
                }
            }
            if tapped {
                self.tap(message);
            }
            // A shed release revokes the route: Main rebuilds it and cancels what the old one
            // still held, so a pedal-up behind it may never reach the tap (#74). Step entry gets
            // a Reset instead (the tap's loss path), after the message above.
            if reset_step {
                self.shared.tap_lost.store(true, Ordering::Release);
            }
        }
        worked
    }
}
impl Drop for Pump {
    fn drop(&mut self) {
        self.shared.finished.store(true, Ordering::Release);
        // Close admission for every input before reclaiming any worker storage.
        for (permit, _) in self.inputs.iter().flat_map(|input| &input.routes) {
            permit.invalidate();
        }
        for _ in 0..sources::PORTS {
            match self.receive.try_recv() {
                Ok(Update::Input(input)) => drop(input),
                Ok(Update::Echo(echo)) => drop(echo),
                Ok(Update::Monitor(tap)) => drop(tap),
                Ok(Update::Maps(maps)) => drop(maps),
                Ok(Update::Routes { routes, .. }) => {
                    for (permit, _) in routes {
                        permit.invalidate();
                    }
                }
                Err(_) => break,
            }
        }
    }
}

/// CC 120 to 127, the channel-mode messages.
fn channel_mode(message: Message) -> bool {
    matches!(message, Message::Control { cc: 120..=127, .. })
}

/// One input's pending messages for the MIDI Learn matcher: the armed learn takes the next
/// candidate (once), everything else is matched against the snapshot.
fn map_pass(input: &mut Input, maps: Option<&Snapshot>, out: &mut rtrb::Producer<MapEvent>, shared: &Shared) {
    let Input { id, consumer, mon, .. } = input;
    let id = *id;
    consumer.for_each_pending(|raw| {
        if raw.seq < mon.map_next {
            return;
        }
        mon.map_next = raw.seq.saturating_add(1);
        let Some((channel, message)) = decode(*raw) else { return };
        match_one(id, channel, message, maps, out, shared);
    });
}

/// One message for the MIDI Learn matcher: the armed learn takes it as the candidate, otherwise it
/// is matched against the snapshot. Returns whether a mapping consumes it (a candidate is not).
fn match_one(id: u64, channel: u8, message: Message, maps: Option<&Snapshot>, out: &mut rtrb::Producer<MapEvent>, shared: &Shared) -> bool {
    let mut push = |event: MapEvent| {
        let sent = out.push(event).is_ok();
        if sent {
            shared.map_pushed.fetch_add(1, Ordering::Relaxed);
        } else {
            shared.map_dropped.fetch_add(1, Ordering::Relaxed);
        }
        sent
    };
    if shared.learn.load(Ordering::Acquire)
        && let Some(candidate) = midi_map::candidate(id, channel, message)
        && shared.learn.swap(false, Ordering::AcqRel)
    {
        push(candidate);
        return false;
    }
    maps.is_some_and(|maps| midi_map::handle(maps, id, channel, message, &mut push))
}

/// Pairs note-offs with consumed note-ons: a note-on is consumed when `mapped`, and its note-off is
/// consumed exactly then, whatever the mappings are by then. Returns whether to consume `message`.
fn consume_notes(mon: &mut Mon, channel: u8, message: Message, mapped: bool) -> bool {
    let (key, on) = match message {
        Message::NoteOn { key, .. } => (key, true),
        Message::NoteOff { key, .. } => (key, false),
        _ => return mapped,
    };
    let word = usize::from(channel & 15);
    let bit = 1u128 << (key & 127);
    let was = mon.held[word] & bit != 0;
    if on && mapped {
        mon.held[word] |= bit;
    } else {
        mon.held[word] &= !bit;
    }
    if on { mapped } else { was }
}

fn decode(raw: InputEvent) -> Option<(u8, Message)> {
    let status = raw.bytes[0];
    if !(0x80..0xf0).contains(&status) || raw.len != if status & 0xe0 == 0xc0 { 2 } else { 3 } {
        return None;
    }
    if raw.message().iter().skip(1).any(|byte| *byte > 127) {
        return None;
    }
    let (a, b) = (raw.bytes[1], raw.bytes[2]);
    let message = match status & 0xf0 {
        0x80 => Message::NoteOff { key: a, velocity: b, id: NoteId::NONE },
        0x90 if b == 0 => Message::NoteOff { key: a, velocity: 0, id: NoteId::NONE },
        0x90 => Message::NoteOn { key: a, velocity: b, id: NoteId::NONE },
        0xa0 => Message::PolyPressure { key: a, value: b, id: NoteId::NONE },
        0xb0 => Message::Control { cc: a, value: b },
        0xc0 => Message::Program { program: a },
        0xd0 => Message::ChannelPressure { value: a },
        0xe0 => Message::PitchBend { value: u16::from(a) | (u16::from(b) << 7) },
        _ => return None,
    };
    Some((status & 0x0f, message))
}

mod storm;
pub use storm::{RATE as STORM_RATE, REPEAT as STORM_REPEAT, WINDOW_NS as STORM_WINDOW_NS, WINDOWS as STORM_WINDOWS};

#[cfg(test)]
mod map_tests;
#[cfg(test)]
mod tests;
