//! What the JACK callbacks do, in safe code over [`OutputPlanes`] and [`InputPlanes`], so it runs
//! on plain vectors in tests: the duplex process engine and the notification handlers.
//! `crate::callback` calls it from libjack's threads with the real port buffers. Nothing here
//! allocates, locks, logs or formats after open (clippy denies `format!`, `to_string` and the log
//! macros in this module; a `CountingAlloc` test drives it).
//!
//! Every notification handler only stores atomics (ADR 0021, decision 4): the buffer size, sample
//! rate, xrun and shutdown callbacks are treated as real-time context on every server, and the
//! shutdown handler must behave like an async-signal handler (jack.h, `jack_on_info_shutdown`). The
//! watcher thread of `crate::session` reads them and acts.
//!
//! **One process callback, both directions** ([`Engine::process`]): JACK hands the capture and the
//! playback buffers of one cycle to one call, so the capture callback runs first (its block was
//! just recorded) and the render callback second; a reader in the render callback always finds the
//! same cycle's capture already delivered (`Backend::capture_precedes_render`, K22). JACK ports are
//! non-interleaved `f32`: the engine interleaves capture into scratch and de-interleaves the render
//! scratch into the output ports, in chunks of at most `SCRATCH_FRAMES`, so any quantum is served.
//!
//! **Stamps** (ADR 0021, decision 5): the host time is the arrival read at process entry in the
//! request's `HostOrigin`; the backend callback time is the cycle start in that origin, `arrival -
//! (jack_get_time() - current_usecs)`, a difference of two JACK readings of one callback; the
//! device position is `current_frames`, unwrapped from 32 bits. A step that is not the previous
//! cycle's frames counts a position jump; it, an xrun and a quantum change each make the next stamp
//! [`Quality::Invalid`]. Frame times equal the callback time in slice 2 (port latencies are slice
//! 3); stamps never go backward. Without cycle times the arrival is the backend time (`Arrival`).
#![deny(clippy::disallowed_macros, clippy::disallowed_methods)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use transit_audio_backend::{
    CaptureBlock, CaptureCallback, CaptureSource, Format, HostOrigin, Quality, RenderBlock, RenderCallback, RenderSink, SCRATCH_FRAMES, STATS_BINS,
    Stamp, StreamStats,
};

/// The life of one direction, published by the control side and read by the process callback.
pub(crate) const OPENED: u8 = 0;
pub(crate) const STARTED: u8 = 1;
pub(crate) const STOPPED: u8 = 2;
/// The stream was dropped; its callback stays allocated (never called) until the client closes.
pub(crate) const DETACHED: u8 = 3;

/// What the control side and the process callback share for one direction.
#[derive(Debug)]
pub(crate) struct SideShared {
    state: AtomicU8,
    /// A process callback is inside this direction's part: what [`SideShared::quiesce`] waits for.
    in_cycle: AtomicBool,
    /// The user callback panicked: it is not called again; the watcher reports it once.
    panicked: AtomicBool,
    wakes: AtomicU64,
}

impl SideShared {
    fn new() -> Self {
        Self { state: AtomicU8::new(OPENED), in_cycle: AtomicBool::new(false), panicked: AtomicBool::new(false), wakes: AtomicU64::new(0) }
    }

    /// Marks the direction in flight, then reads whether it runs. Both `SeqCst`, as are the control
    /// side's state change and its read of the mark in [`SideShared::quiesce`], so either the
    /// callback sees the stop or the stop sees the callback.
    fn enter(&self) -> bool {
        self.in_cycle.store(true, Ordering::SeqCst);
        self.state.load(Ordering::SeqCst) == STARTED && !self.panicked.load(Ordering::Relaxed)
    }

    fn leave(&self) {
        self.in_cycle.store(false, Ordering::SeqCst);
    }

    pub(crate) fn state(&self) -> u8 {
        self.state.load(Ordering::SeqCst)
    }

    pub(crate) fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::SeqCst);
    }

    /// Control side, after the state left [`STARTED`]: waits until no process callback is inside
    /// this direction, so none runs after `stop()` returns while the other direction keeps the
    /// client active. False if one did not leave within `limit`.
    pub(crate) fn quiesce(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.in_cycle.load(Ordering::SeqCst) {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::yield_now();
        }
        true
    }

    pub(crate) fn panicked(&self) -> bool {
        self.panicked.load(Ordering::Acquire)
    }

    pub(crate) fn wakes(&self) -> u64 {
        self.wakes.load(Ordering::Relaxed)
    }
}

/// What the callbacks and the control side share for one client. Relaxed counters, except the
/// shutdown flag (release by the handler, acquire by the watcher) and the side states.
#[derive(Debug)]
pub(crate) struct Shared {
    /// Process callbacks served.
    cycles: AtomicU64,
    /// The `nframes` of the last process callback.
    last_frames: AtomicU32,
    /// Xruns the server reported.
    xruns: AtomicU64,
    /// The server shut the client down (`jack_on_info_shutdown`).
    shutdown: AtomicBool,
    /// The server's quantum: `jack_get_buffer_size` at open, then each buffer size callback.
    buffer_size: AtomicU32,
    /// Buffer size callbacks that changed the quantum.
    buffer_changes: AtomicU64,
    /// The server's rate: `jack_get_sample_rate` at open, then each sample rate callback.
    sample_rate: AtomicU32,
    /// Reset requests the watcher delivered (a rate or quantum change, the server back after a stop).
    resets: AtomicU64,
    /// Faults the watcher delivered (the server stopped, a user callback panicked).
    faults: AtomicU64,
    /// Process callbacks that found the engine busy (not expected: JACK serializes them).
    busy_skips: AtomicU64,
    /// Cycles whose `current_frames` did not advance by the previous cycle's frames.
    position_jumps: AtomicU64,
    /// Time from process entry to the end of the work, in µs: histogram and maximum.
    service_us: [AtomicU64; STATS_BINS],
    max_service_us: AtomicU64,
    /// How late each cycle started against the previous one plus its period, in µs.
    late_us: [AtomicU64; STATS_BINS],
    max_late_us: AtomicU64,
    pub(crate) render: SideShared,
    pub(crate) capture: SideShared,
}

impl Shared {
    pub(crate) fn new(sample_rate: u32, buffer_size: u32) -> Self {
        Self {
            cycles: AtomicU64::new(0),
            last_frames: AtomicU32::new(0),
            xruns: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            buffer_size: AtomicU32::new(buffer_size),
            buffer_changes: AtomicU64::new(0),
            sample_rate: AtomicU32::new(sample_rate),
            resets: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            busy_skips: AtomicU64::new(0),
            position_jumps: AtomicU64::new(0),
            service_us: std::array::from_fn(|_| AtomicU64::new(0)),
            max_service_us: AtomicU64::new(0),
            late_us: std::array::from_fn(|_| AtomicU64::new(0)),
            max_late_us: AtomicU64::new(0),
            render: SideShared::new(),
            capture: SideShared::new(),
        }
    }

    /// The info shutdown callback: one atomic store, nothing else (async-signal safe).
    pub(crate) fn on_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// The xrun callback.
    pub(crate) fn on_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// The buffer size callback: the quantum the next process callbacks get.
    pub(crate) fn on_buffer_size(&self, frames: u32) {
        if self.buffer_size.swap(frames, Ordering::Relaxed) != frames {
            self.buffer_changes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The sample rate callback.
    pub(crate) fn on_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    /// A process callback that could not take the engine.
    #[cfg_attr(not(all(target_os = "linux", feature = "libjack")), allow(dead_code))]
    pub(crate) fn busy_skip(&self) {
        self.busy_skips.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn cycles(&self) -> u64 {
        self.cycles.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn last_frames(&self) -> u32 {
        self.last_frames.load(Ordering::Relaxed)
    }

    pub(crate) fn xruns(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        self.buffer_size.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn buffer_changes(&self) -> u64 {
        self.buffer_changes.load(Ordering::Relaxed)
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// The watcher delivered a reset request.
    pub(crate) fn count_reset(&self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }

    /// The watcher delivered a fault.
    pub(crate) fn count_fault(&self) {
        self.faults.fetch_add(1, Ordering::Relaxed);
    }

    /// The shared state of one direction (`output`: render).
    pub(crate) fn side(&self, output: bool) -> &SideShared {
        if output { &self.render } else { &self.capture }
    }

    /// The counters of one direction, as [`StreamStats`].
    pub(crate) fn stats(&self, output: bool) -> StreamStats {
        let read = |bins: &[AtomicU64; STATS_BINS]| -> [u64; STATS_BINS] { std::array::from_fn(|i| bins[i].load(Ordering::Relaxed)) };
        StreamStats {
            wakes: self.side(output).wakes(),
            overloads: self.xruns(),
            resets: self.resets.load(Ordering::Relaxed),
            faults: self.faults.load(Ordering::Relaxed),
            busy_skips: self.busy_skips.load(Ordering::Relaxed),
            position_jumps: self.position_jumps.load(Ordering::Relaxed),
            service_us: read(&self.service_us),
            max_service_us: self.max_service_us.load(Ordering::Relaxed),
            wake_late_us: read(&self.late_us),
            max_wake_late_us: self.max_late_us.load(Ordering::Relaxed),
            ..Default::default()
        }
    }

    fn record(bins: &[AtomicU64; STATS_BINS], max: &AtomicU64, us: u64) {
        bins[StreamStats::bin_of_us(us)].fetch_add(1, Ordering::Relaxed);
        max.fetch_max(us, Ordering::Relaxed);
    }
}

/// The output port buffers of one process callback, in port order (`out_1` first).
pub(crate) trait OutputPlanes {
    fn count(&self) -> usize;
    /// The buffer of output port `index` for this cycle, `None` when the server gave none.
    fn plane(&mut self, index: usize) -> Option<&mut [f32]>;
}

/// The input port buffers of one process callback, in port order (`in_1` first).
pub(crate) trait InputPlanes {
    fn count(&self) -> usize;
    /// The buffer of input port `index` for this cycle, `None` when the server gave none.
    fn plane(&self, index: usize) -> Option<&[f32]>;
}

/// Writes silence to the first `frames` frames of every output port.
pub(crate) fn silence<P: OutputPlanes + ?Sized>(outputs: &mut P, frames: usize) {
    for index in 0..outputs.count() {
        if let Some(plane) = outputs.plane(index) {
            let n = frames.min(plane.len());
            plane[..n].fill(0.0);
        }
    }
}

/// What JACK said about the cycle's time (`jack_get_cycle_times` and `jack_get_time`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Clock {
    /// The frame counter at the start of the cycle.
    pub(crate) current_frames: u32,
    /// JACK's time of the cycle start, µs.
    pub(crate) current_usecs: u64,
    /// `jack_get_time()` read at process entry, µs.
    pub(crate) now_usecs: u64,
}

/// One process callback's inputs other than the buffers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cycle {
    pub(crate) frames: u32,
    /// `Instant::now()` at process entry.
    pub(crate) arrival: Instant,
    /// `None` when `jack_get_cycle_times` failed (or is missing from the library).
    pub(crate) clock: Option<Clock>,
}

/// The previous stamp's times (callback, frame, host), so the next never goes backward (the rule
/// of the native ASIO and WASAPI backends).
#[derive(Debug, Clone, Copy, Default)]
struct Held(Option<(i64, i64, u64)>);

impl Held {
    fn hold(&mut self, callback: i64, frame: i64, host: u64) -> (i64, i64, u64) {
        let held = match self.0 {
            Some((c, f, h)) => (callback.max(c), frame.max(f), host.max(h.saturating_add(1))),
            None => (callback, frame, host),
        };
        self.0 = Some(held);
        held
    }
}

/// One direction inside the engine: its callback and what only the process thread touches.
pub(crate) struct EngineSide<C> {
    callback: C,
    format: Format,
    origin: HostOrigin,
    /// `SCRATCH_FRAMES` interleaved frames, allocated at open.
    scratch: Vec<f32>,
    held: Held,
    /// The next stamp is [`Quality::Invalid`].
    invalidate: bool,
}

impl<C> EngineSide<C> {
    /// A direction of `format` in `origin`, with its scratch allocated here (on the control side).
    pub(crate) fn new(callback: C, format: Format, origin: HostOrigin) -> Self {
        let channels = usize::from(format.channels).max(1);
        Self { callback, format, origin, scratch: vec![0.0; SCRATCH_FRAMES * channels], held: Held::default(), invalidate: false }
    }

    fn stamp(&mut self, cycle: &Cycle) -> Stamp {
        let since_origin = cycle.arrival.checked_duration_since(self.origin.instant());
        let Some(host) = since_origin.and_then(|d| u64::try_from(d.as_nanos()).ok()) else { return Stamp::INVALID };
        let Ok(arrival) = i64::try_from(host) else { return Stamp::INVALID };
        let (callback, quality) = match cycle.clock {
            Some(clock) => {
                // The cycle start in the host origin: only the difference of two JACK readings of
                // this callback is used, whatever clock `jack_get_time` reads.
                let since_us = clock.now_usecs.saturating_sub(clock.current_usecs);
                let since_ns = i64::try_from(since_us.saturating_mul(1_000)).unwrap_or(i64::MAX);
                (arrival.saturating_sub(since_ns), Quality::BackendEstimate)
            }
            None => (arrival, Quality::Arrival),
        };
        // Slice 2: the frame times are the callback time (port latencies are slice 3).
        let (callback, frame, host) = self.held.hold(callback, callback, host);
        let quality = if std::mem::take(&mut self.invalidate) { Quality::Invalid } else { quality };
        Stamp { host_callback_ns: host, backend_callback_ns: callback, backend_frame_ns: frame, quality, offset_uncertainty_ns: None }
    }
}

/// The render scratch as the block's sink: each chunk is filled, then de-interleaved into the
/// output ports.
struct PlanarSink<'a, P: OutputPlanes + ?Sized> {
    scratch: &'a mut [f32],
    planes: &'a mut P,
    frames: usize,
    channels: usize,
}

impl<P: OutputPlanes + ?Sized> RenderSink for PlanarSink<'_, P> {
    fn render(&mut self, fill: &mut dyn FnMut(&mut [f32])) {
        let channels = self.channels.max(1);
        let per_chunk = (self.scratch.len() / channels).max(1);
        let mut offset = 0;
        while offset < self.frames {
            let n = per_chunk.min(self.frames - offset);
            let chunk = &mut self.scratch[..n * channels];
            chunk.fill(0.0);
            fill(chunk);
            for port in 0..self.planes.count().min(channels) {
                let Some(plane) = self.planes.plane(port) else { continue };
                let end = (offset + n).min(plane.len());
                if let Some(part) = plane.get_mut(offset..end) {
                    for (i, sample) in part.iter_mut().enumerate() {
                        *sample = chunk[i * channels + port];
                    }
                }
            }
            offset += n;
        }
    }
}

/// The input ports as the block's source: each chunk is interleaved into scratch.
struct PlanarSource<'a, P: InputPlanes + ?Sized> {
    scratch: &'a mut [f32],
    planes: &'a P,
    frames: usize,
    channels: usize,
}

impl<P: InputPlanes + ?Sized> CaptureSource for PlanarSource<'_, P> {
    fn chunks(&mut self, each: &mut dyn FnMut(usize, &[f32])) {
        let channels = self.channels.max(1);
        let per_chunk = (self.scratch.len() / channels).max(1);
        let mut offset = 0;
        while offset < self.frames {
            let n = per_chunk.min(self.frames - offset);
            let chunk = &mut self.scratch[..n * channels];
            chunk.fill(0.0);
            for port in 0..self.planes.count().min(channels) {
                let Some(plane) = self.planes.plane(port) else { continue };
                let end = (offset + n).min(plane.len());
                if let Some(part) = plane.get(offset..end) {
                    for (i, sample) in part.iter().enumerate() {
                        chunk[i * channels + port] = *sample;
                    }
                }
            }
            each(offset, chunk);
            offset += n;
        }
    }
}

/// The process state of one client: both directions' callbacks and the cycle history. Built on the
/// control side before activation; afterwards only the process callback touches it, and it is
/// dropped after the client closed.
pub(crate) struct Engine {
    shared: Arc<Shared>,
    pub(crate) render: Option<EngineSide<RenderCallback>>,
    pub(crate) capture: Option<EngineSide<CaptureCallback>>,
    /// The unwrapped frame counter and the raw value it came from.
    position: Option<(u64, u32)>,
    last_frames: u32,
    last_arrival: Option<Instant>,
    xruns_seen: u64,
    rate: u32,
}

impl Engine {
    pub(crate) fn new(shared: Arc<Shared>) -> Self {
        let rate = shared.sample_rate().max(1);
        let xruns_seen = shared.xruns();
        Self { shared, render: None, capture: None, position: None, last_frames: 0, last_arrival: None, xruns_seen, rate }
    }

    /// Marks the next stamp of each direction invalid.
    fn invalidate_next(&mut self) {
        if let Some(side) = self.render.as_mut() {
            side.invalidate = true;
        }
        if let Some(side) = self.capture.as_mut() {
            side.invalidate = true;
        }
    }

    /// One process callback: counters and the position, then the capture callback (its block was
    /// just recorded), then the render callback (or silence).
    pub(crate) fn process<O: OutputPlanes + ?Sized, I: InputPlanes + ?Sized>(&mut self, cycle: Cycle, outputs: &mut O, inputs: &I) {
        let shared = Arc::clone(&self.shared);
        shared.cycles.fetch_add(1, Ordering::Relaxed);
        shared.last_frames.store(cycle.frames, Ordering::Relaxed);
        let frames = cycle.frames as usize;
        if let Some(previous) = self.last_arrival {
            let due = previous + Duration::from_nanos(u64::from(self.last_frames) * 1_000_000_000 / u64::from(self.rate));
            let late = cycle.arrival.saturating_duration_since(due);
            Shared::record(&shared.late_us, &shared.max_late_us, u64::try_from(late.as_micros()).unwrap_or(u64::MAX));
        }
        self.last_arrival = Some(cycle.arrival);
        // The device position, unwrapped from JACK's 32-bit frame counter.
        let mut jumped = false;
        let position = cycle.clock.map(|clock| match self.position {
            Some((unwrapped, raw)) => {
                let step = clock.current_frames.wrapping_sub(raw);
                jumped = step != self.last_frames;
                unwrapped.wrapping_add(u64::from(step))
            }
            None => u64::from(clock.current_frames),
        });
        if jumped {
            shared.position_jumps.fetch_add(1, Ordering::Relaxed);
        }
        let xruns = shared.xruns();
        let resized = self.last_frames != 0 && cycle.frames != self.last_frames;
        if jumped || xruns != self.xruns_seen || resized {
            self.invalidate_next();
        }
        self.xruns_seen = xruns;
        self.position = position.zip(cycle.clock.map(|c| c.current_frames));
        self.last_frames = cycle.frames;

        if let Some(side) = self.capture.as_mut() {
            if shared.capture.enter() {
                shared.capture.wakes.fetch_add(1, Ordering::Relaxed);
                let stamp = side.stamp(&cycle);
                let channels = usize::from(side.format.channels);
                let mut source = PlanarSource { scratch: &mut side.scratch, planes: inputs, frames, channels };
                let mut block = CaptureBlock::new(frames, stamp, side.format, &mut source);
                block.device_position = position;
                let callback = &mut side.callback;
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| callback(&mut block))) {
                    panicked(&shared.capture, payload);
                }
            }
            shared.capture.leave();
        }
        silence(outputs, frames);
        if let Some(side) = self.render.as_mut() {
            if shared.render.enter() {
                shared.render.wakes.fetch_add(1, Ordering::Relaxed);
                let stamp = side.stamp(&cycle);
                let channels = usize::from(side.format.channels);
                let mut sink = PlanarSink { scratch: &mut side.scratch, planes: &mut *outputs, frames, channels };
                let mut block = RenderBlock::new(frames, stamp, side.format, &mut sink);
                block.device_position = position;
                let callback = &mut side.callback;
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| callback(&mut block))) {
                    panicked(&shared.render, payload);
                    silence(outputs, frames);
                }
            }
            shared.render.leave();
        }
        let service = Instant::now().saturating_duration_since(cycle.arrival);
        Shared::record(&shared.service_us, &shared.max_service_us, u64::try_from(service.as_micros()).unwrap_or(u64::MAX));
    }
}

/// A panicking user callback: never called again; the watcher reports it (`StreamError::Other`).
fn panicked(side: &SideShared, payload: Box<dyn std::any::Any + Send>) {
    side.panicked.store(true, Ordering::Release);
    // A payload whose own drop panics must not unwind into libjack either.
    let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
}
