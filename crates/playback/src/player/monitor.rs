//! MIDI Monitor taps (#91): two fixed-size SPSC rings, one per direction, that copy every
//! message the ingress worker pops (any open input port, routed or not) and every message the
//! output thread sends, for a window to show. Contract: `docs/contributing/knowledge/midi-monitor.md`.
//!
//! The producing threads (ingress worker, output thread) never allocate, lock, log or block here:
//! an event is a `Copy` struct pushed into a preallocated ring, and a full ring drops the event
//! and bumps a counter. Nothing is produced while the monitor is off. System Real-Time Clock is
//! never an event unless asked for: it feeds a per-port rate estimator instead.
use super::sources::PORTS;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Events each ring holds.
pub const RING: usize = 4096;
/// A clock rate is not reported when the last pulse is older than this.
pub const CLOCK_TIMEOUT_NS: u64 = 1_000_000_000;
/// A port counts as sending Active Sensing when one was seen this recently.
pub const SENSING_TIMEOUT_NS: u64 = 2_000_000_000;
/// "No channel" in [`MonitorEvent::channel`].
pub const NO_CHANNEL: u8 = 0xFF;

/// Which way a message went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dir {
    /// Received on an input port.
    #[default]
    In,
    /// Sent to an output port.
    Out,
}

/// One ring element: 24 bytes, `Copy`, no heap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonitorEvent {
    /// Host time in nanoseconds on the runtime's [`HostOrigin`](crate::device_clock::HostOrigin).
    /// Inputs carry the callback's arrival time, outputs the time of the send.
    pub host_ns: u64,
    /// SysEx byte count (status 0xF0 only); the bytes themselves are not copied.
    pub sysex_len: u32,
    /// Input: the ingress input id; output: the output thread's port index. Names come from
    /// [`Report::ports`].
    pub port: u16,
    pub dir: Dir,
    /// 0..=15, or [`NO_CHANNEL`] for system messages.
    pub channel: u8,
    /// The full status byte (0xF0 for a SysEx).
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}
const _: () = assert!(std::mem::size_of::<MonitorEvent>() == 24);

impl MonitorEvent {
    /// A complete message of 1 to 3 bytes, status first. Empty input gives status 0.
    #[must_use]
    pub fn message(dir: Dir, port: u16, host_ns: u64, bytes: &[u8]) -> Self {
        let status = bytes.first().copied().unwrap_or(0);
        Self {
            host_ns,
            sysex_len: 0,
            port,
            dir,
            channel: if (0x80..0xF0).contains(&status) { status & 0x0F } else { NO_CHANNEL },
            status,
            data1: bytes.get(1).copied().unwrap_or(0),
            data2: bytes.get(2).copied().unwrap_or(0),
        }
    }

    /// A SysEx of `len` bytes (not carried).
    #[must_use]
    pub fn sysex(dir: Dir, port: u16, host_ns: u64, len: u32) -> Self {
        Self { host_ns, sysex_len: len, port, dir, channel: NO_CHANNEL, status: 0xF0, data1: 0, data2: 0 }
    }
}

/// Pulses per quarter note.
const PPQN: f64 = 24.0;
const SMOOTHING: f64 = 0.1;

/// Estimates tempo from MIDI Clock pulse times: an exponential average of the pulse interval.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockRate {
    last_ns: Option<u64>,
    interval_ns: f64,
}

impl ClockRate {
    /// One pulse at `host_ns`. A gap over [`CLOCK_TIMEOUT_NS`] starts the estimate again.
    pub fn pulse(&mut self, host_ns: u64) {
        if let Some(last) = self.last_ns {
            let gap = host_ns.saturating_sub(last);
            if gap == 0 {
                return;
            }
            if gap > CLOCK_TIMEOUT_NS {
                self.interval_ns = 0.0;
            } else if self.interval_ns == 0.0 {
                self.interval_ns = gap as f64;
            } else {
                self.interval_ns += SMOOTHING * (gap as f64 - self.interval_ns);
            }
        }
        self.last_ns = Some(host_ns);
    }

    /// Beats per minute, or `None` before two pulses and once no pulse came for a second.
    #[must_use]
    pub fn bpm(&self, now_ns: u64) -> Option<f64> {
        let last = self.last_ns?;
        (self.interval_ns > 0.0 && now_ns.saturating_sub(last) <= CLOCK_TIMEOUT_NS).then(|| 60e9 / (self.interval_ns * PPQN))
    }

    /// The estimate without the freshness check, with the last pulse time.
    #[must_use]
    pub fn raw(&self) -> Option<(f64, u64)> {
        let last = self.last_ns?;
        (self.interval_ns > 0.0).then(|| (60e9 / (self.interval_ns * PPQN), last))
    }
}

#[derive(Default)]
struct PortStat {
    /// The input id holding this slot; 0 when free.
    id: AtomicU64,
    bpm_milli: AtomicU64,
    last_pulse_ns: AtomicU64,
    /// Last Active Sensing seen, plus one (0: never).
    sensing_ns: AtomicU64,
}

/// State shared by the producers and the control side.
pub struct Shared {
    enabled: AtomicBool,
    show_clock: AtomicBool,
    dropped: [AtomicU64; 2],
    /// Input activity for the header LED: a count of channel and system messages other than
    /// Clock and Active Sensing, and the last one's port and bytes. Updated whether or not the
    /// monitor is on.
    activity: AtomicU64,
    activity_msg: AtomicU64,
    stats: [PortStat; PORTS],
}

impl Shared {
    fn index(dir: Dir) -> usize {
        match dir {
            Dir::In => 0,
            Dir::Out => 1,
        }
    }

    /// One input message that counts as activity (never Clock or Active Sensing).
    pub(crate) fn note_activity(&self, port: u16, bytes: [u8; 3]) {
        let packed = u64::from(port) << 24 | u64::from(bytes[0]) << 16 | u64::from(bytes[1]) << 8 | u64::from(bytes[2]);
        self.activity_msg.store(packed, Ordering::Relaxed);
        self.activity.fetch_add(1, Ordering::Release);
    }

    /// Reserves a rate slot for input `id` (never 0).
    pub(crate) fn claim(&self, id: u64) -> Option<usize> {
        self.stats.iter().position(|s| s.id.compare_exchange(0, id, Ordering::AcqRel, Ordering::Relaxed).is_ok())
    }

    pub(crate) fn release(&self, slot: usize) {
        if let Some(s) = self.stats.get(slot) {
            s.bpm_milli.store(0, Ordering::Relaxed);
            s.last_pulse_ns.store(0, Ordering::Relaxed);
            s.sensing_ns.store(0, Ordering::Relaxed);
            s.id.store(0, Ordering::Release);
        }
    }

    pub(crate) fn publish_rate(&self, slot: usize, rate: Option<(f64, u64)>) {
        if let (Some(s), Some((bpm, last))) = (self.stats.get(slot), rate) {
            s.bpm_milli.store((bpm * 1000.0).round() as u64, Ordering::Relaxed);
            s.last_pulse_ns.store(last, Ordering::Release);
        }
    }

    pub(crate) fn publish_sensing(&self, slot: usize, host_ns: u64) {
        if let Some(s) = self.stats.get(slot) {
            s.sensing_ns.store(host_ns.saturating_add(1), Ordering::Release);
        }
    }

    /// (clock bpm, sensing present) of input `id` at `now_ns`.
    fn port_rate(&self, id: u64, now_ns: u64) -> (Option<f64>, bool) {
        let Some(s) = self.stats.iter().find(|s| s.id.load(Ordering::Acquire) == id) else { return (None, false) };
        let last = s.last_pulse_ns.load(Ordering::Acquire);
        let milli = s.bpm_milli.load(Ordering::Relaxed);
        let bpm = (milli > 0 && now_ns.saturating_sub(last) <= CLOCK_TIMEOUT_NS).then(|| milli as f64 / 1000.0);
        let sensing = s.sensing_ns.load(Ordering::Acquire);
        (bpm, sensing > 0 && now_ns.saturating_sub(sensing - 1) <= SENSING_TIMEOUT_NS)
    }
}

/// The producing end of one ring. Owned by the ingress worker or the output thread.
pub struct Tap {
    tx: rtrb::Producer<MonitorEvent>,
    shared: Arc<Shared>,
    dir: Dir,
}

impl Tap {
    /// Whether the monitor is open. Producers skip all tap work when it is not.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    /// Whether Clock pulses are shown as events.
    #[must_use]
    pub fn show_clock(&self) -> bool {
        self.shared.show_clock.load(Ordering::Relaxed)
    }

    /// Wait-free, no allocation. A full ring drops the event and counts it.
    pub fn push(&mut self, event: MonitorEvent) {
        if self.tx.push(event).is_err() {
            self.shared.dropped[Shared::index(self.dir)].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The shared state (rate slots).
    #[must_use]
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }
}

/// Whether a port is usable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PortState {
    #[default]
    Open,
    /// The open failed (held by another program, or the backend refused).
    Failed,
    /// It was open and vanished.
    Missing,
}

/// The last input message that counted as activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Messages so far; only ever grows.
    pub count: u64,
    /// The input id of the port it came from.
    pub port: u16,
    pub bytes: [u8; 3],
}

/// A port as the window lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct PortReport {
    pub state: PortState,
    /// Tracks that listen to this input (raw track ids).
    pub tracks: Vec<u64>,
    pub dir: Dir,
    pub id: u16,
    pub name: String,
    /// Smoothed Clock tempo; `None` without pulses in the last second.
    pub clock_bpm: Option<f64>,
    /// Active Sensing seen lately.
    pub sensing: bool,
}

/// What the window reads besides the events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub dropped_in: u64,
    pub dropped_out: u64,
    pub ports: Vec<PortReport>,
}

/// The control side: the two consumers and the switches.
pub struct Hub {
    shared: Arc<Shared>,
    rx: RefCell<(rtrb::Consumer<MonitorEvent>, rtrb::Consumer<MonitorEvent>)>,
}

impl Hub {
    /// The hub, the input tap and the output tap. Everything is allocated here.
    #[must_use]
    pub fn new() -> (Self, Tap, Tap) {
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(false),
            show_clock: AtomicBool::new(false),
            dropped: Default::default(),
            activity: AtomicU64::new(0),
            activity_msg: AtomicU64::new(0),
            stats: std::array::from_fn(|_| PortStat::default()),
        });
        let (tx_in, rx_in) = rtrb::RingBuffer::new(RING);
        let (tx_out, rx_out) = rtrb::RingBuffer::new(RING);
        (
            Self { shared: Arc::clone(&shared), rx: RefCell::new((rx_in, rx_out)) },
            Tap { tx: tx_in, shared: Arc::clone(&shared), dir: Dir::In },
            Tap { tx: tx_out, shared, dir: Dir::Out },
        )
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.shared.enabled.store(enabled, Ordering::Relaxed);
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub fn set_show_clock(&self, show: bool) {
        self.shared.show_clock.store(show, Ordering::Relaxed);
    }

    /// Appends the pending events of both rings to `out`, merged by time, at most `max` per ring.
    pub fn drain(&self, out: &mut Vec<MonitorEvent>, max: usize) {
        let Ok(mut rx) = self.rx.try_borrow_mut() else { return };
        let start = out.len();
        for _ in 0..max {
            let Ok(event) = rx.0.pop() else { break };
            out.push(event);
        }
        for _ in 0..max {
            let Ok(event) = rx.1.pop() else { break };
            out.push(event);
        }
        if let Some(tail) = out.get_mut(start..) {
            tail.sort_by_key(|e| e.host_ns);
        }
    }

    /// Events dropped so far on the input and output rings.
    #[must_use]
    pub fn dropped(&self) -> (u64, u64) {
        (self.shared.dropped[0].load(Ordering::Relaxed), self.shared.dropped[1].load(Ordering::Relaxed))
    }

    /// The input activity counter and the last message behind it.
    #[must_use]
    pub fn activity(&self) -> Activity {
        let count = self.shared.activity.load(Ordering::Acquire);
        let m = self.shared.activity_msg.load(Ordering::Relaxed);
        Activity { count, port: (m >> 24) as u16, bytes: [(m >> 16) as u8, (m >> 8) as u8, m as u8] }
    }

    /// Clock tempo and Active Sensing presence of input `id` at `now_ns`.
    #[must_use]
    pub fn input_rate(&self, id: u64, now_ns: u64) -> (Option<f64>, bool) {
        self.shared.port_rate(id, now_ns)
    }
}

#[cfg(test)]
mod tests;
