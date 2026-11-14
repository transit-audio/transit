//! The silent device: an audio backend with one stereo output and no hardware behind it, for the
//! app's `--null-audio` diagnostic run. The Player opens it through the same backend trait and the
//! same open path as every device ([`crate::player::Player::with_null_device`]), so the plan, the
//! mixer, hosted plug-ins and their editors, the meters and the transport run as they do on a real
//! device, while nothing reaches an audio system.
//!
//! No device API is touched, by construction: this module calls no audio system (no WASAPI, ASIO,
//! CoreAudio, ALSA or JACK function, not even a listing). Its one endpoint is a constant; its stream
//! is a thread of this process that calls the render callback with a fixed block of [`FRAMES`]
//! frames at [`RATE`] Hz, paced at real time by a monotonic clock ([`MonotonicClock`], or an
//! injected [`PaceClock`] in tests), and discards what the callback wrote. The thread gets the
//! device-independent part of a real callback thread's setup: the realtime promotion of
//! `transit-rt` (MMCSS "Pro Audio" on Windows), as the native WASAPI worker does.
//!
//! There are no inputs: a capture open is refused ([`NO_INPUT`]).
//!
//! The thread runs its loop under `catch_unwind`, as the native WASAPI render loop does: a panic in
//! the render callback ends the stream and is reported once through the error callback as
//! [`ab::StreamError::Other`], instead of showing only as a stall.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use transit_audio_backend as ab;

/// The audio system name of the silent device.
pub const SYSTEM: &str = "No hardware";
/// The name of its one output, as the Audio Devices window and the menus show it.
pub const DEVICE: &str = "No hardware (silent)";
/// Its fixed sample rate.
pub const RATE: u32 = 48_000;
/// Its fixed output channels.
pub const CHANNELS: u16 = 2;
/// Its fixed block size in frames.
pub const FRAMES: u32 = 512;
/// Why an input does not open on it.
pub const NO_INPUT: &str = "the silent device (no hardware) has no inputs: recording is unavailable in this run";
/// A wake later than this many blocks re-anchors the schedule (a suspended machine, a debugger
/// break) instead of rendering the missed blocks back to back; the next stamp restarts the clock fit.
const MAX_BEHIND_BLOCKS: u64 = 4;

/// The monotonic time that paces the silent device's blocks.
pub trait PaceClock: Send + Sync {
    /// Time since a fixed point of this clock; never goes back.
    fn now(&self) -> Duration;
    /// Block until [`Self::now`] reaches `deadline` (it may return early; the caller checks again)
    /// or `stop` is set.
    fn wait_until(&self, deadline: Duration, stop: &AtomicBool);
    /// Wake a [`Self::wait_until`] in progress because its `stop` was just set.
    fn interrupt(&self) {}
    /// A stream (re)starts on this clock, before its thread runs.
    fn resume(&self) {}
    /// The pacing thread ended (stopped, or its callback panicked) and waits on this clock no more.
    fn ended(&self) {}
}

/// The real-time clock: `std::time::Instant` from the process origin the stamps use, so the time it
/// reads is the host time of [`ab::Stamp::host_callback_ns`].
pub struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    pub fn new(origin: ab::HostOrigin) -> Self {
        Self { origin: origin.instant() }
    }
}

impl PaceClock for MonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
    /// Sleeps at most one block (about 10.7 ms at the defaults); a stop is seen at the next wake.
    fn wait_until(&self, deadline: Duration, stop: &AtomicBool) {
        let now = self.now();
        if !stop.load(Ordering::Acquire) && now < deadline {
            std::thread::sleep(deadline - now);
        }
    }
}

/// The silent device's backend and factory.
#[derive(Clone)]
pub struct NullBackend {
    format: ab::Format,
    frames: u32,
    /// The pacing clock; `None` is a [`MonotonicClock`] from the open request's origin.
    clock: Option<Arc<dyn PaceClock>>,
}

impl Default for NullBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NullBackend {
    /// [`RATE`] Hz, [`CHANNELS`] channels of `f32`, [`FRAMES`] frames, paced at real time.
    pub fn new() -> Self {
        Self { format: ab::Format::new(RATE, CHANNELS, ab::SampleFormat::F32), frames: FRAMES, clock: None }
    }
    /// Paced by `clock` instead of the real-time clock (tests step it).
    pub fn with_clock(mut self, clock: Arc<dyn PaceClock>) -> Self {
        self.clock = Some(clock);
        self
    }
    fn endpoint(&self) -> ab::Endpoint {
        ab::Endpoint::new("null".into(), DEVICE.into(), SYSTEM.into(), 0, self.format.channels, Vec::new(), Arc::new(()))
    }
    fn latency(&self) -> Duration {
        Duration::from_nanos(block_ns(1, self.frames, self.format.rate))
    }
}

impl ab::BackendFactory for NullBackend {
    fn backend(&self, _system: Option<&str>) -> Option<Box<dyn ab::Backend>> {
        Some(Box::new(self.clone()))
    }
}

impl ab::Backend for NullBackend {
    fn system(&self) -> &str {
        SYSTEM
    }
    fn endpoints(&self, direction: ab::Direction) -> Result<Vec<ab::Endpoint>, ab::OpenError> {
        Ok(if direction == ab::Direction::Output { vec![self.endpoint()] } else { Vec::new() })
    }
    /// The one format, whatever was wished: the silent device has no other.
    fn plan_render(&self, _wish: &ab::OutputWish) -> Result<ab::Plan, String> {
        let request = ab::OpenRequest {
            format: self.format,
            period: ab::Period::Frames(self.frames),
            mode: ab::Mode::Shared,
            origin: ab::HostOrigin::default(),
            calibration: None,
        };
        Ok(ab::Plan { endpoint: self.endpoint(), attempts: vec![request] })
    }
    fn plan_capture(&self, _wish: &ab::InputWish) -> Result<ab::Plan, String> {
        Err(NO_INPUT.into())
    }
    fn capture_with_render(&self, _render: &ab::Endpoint) -> bool {
        false
    }
    fn capture_format(&self, _endpoint: &ab::Endpoint, _rate: Option<u32>) -> Result<ab::Format, String> {
        Err(NO_INPUT.into())
    }
    fn open_render(
        &self,
        _endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        render: ab::RenderCallback,
        error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        // The silent device itself never faults: its error callback is called only for a panic in
        // the render callback.
        let clock = self.clock.clone().unwrap_or_else(|| Arc::new(MonotonicClock::new(request.origin)));
        let stream = NullStream {
            callbacks: Arc::new(Mutex::new(Some((render, error)))),
            thread: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
            priority: Arc::new(AtomicU8::new(PRIO_PENDING)),
            clock,
            format: self.format,
            frames: self.frames,
            latency: self.latency(),
        };
        let achieved = ab::Achieved {
            latency: Some(self.latency()),
            buffer_frames: Some(self.frames),
            ..ab::Achieved::requested(self.format, ab::Period::Frames(self.frames))
        };
        Ok((Box::new(stream), achieved))
    }
    fn open_capture(
        &self,
        _endpoint: &ab::Endpoint,
        _request: &ab::OpenRequest,
        _capture: ab::CaptureCallback,
        _error: ab::ErrorCallback,
    ) -> Result<(Box<dyn ab::Stream>, ab::Achieved), ab::OpenError> {
        Err(ab::OpenError::Other(NO_INPUT.into()))
    }
}

const PRIO_PENDING: u8 = 0;
const PRIO_PRO_AUDIO: u8 = 1;
const PRIO_RAISED: u8 = 2;
const PRIO_FAILED: u8 = 3;
const PRIO_UNOBSERVED: u8 = 4;
const PRIO_RELEASED: u8 = 5;

/// The render and error callbacks, lent to the pacing thread while it runs.
type Callbacks = (ab::RenderCallback, ab::ErrorCallback);

/// The silent device's output stream: the pacing thread runs between `start` and `stop`. The
/// callbacks are lent to the thread while it runs and given back when it ends, so a stopped stream
/// can start again.
struct NullStream {
    callbacks: Arc<Mutex<Option<Callbacks>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    stop: Arc<AtomicBool>,
    priority: Arc<AtomicU8>,
    clock: Arc<dyn PaceClock>,
    format: ab::Format,
    frames: u32,
    latency: Duration,
}

impl ab::Stream for NullStream {
    fn start(&self) -> Result<(), ab::OpenError> {
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        if thread.is_some() {
            return Ok(());
        }
        let callbacks = self.callbacks.lock().unwrap_or_else(PoisonError::into_inner).take();
        let (render, error) = callbacks.ok_or_else(|| ab::OpenError::Other("the silent device's render callback is gone".into()))?;
        self.stop.store(false, Ordering::Release);
        self.clock.resume();
        let pacing = Pacing {
            render,
            error,
            slot: Arc::clone(&self.callbacks),
            stop: Arc::clone(&self.stop),
            priority: Arc::clone(&self.priority),
            clock: Arc::clone(&self.clock),
            format: self.format,
            frames: self.frames,
        };
        let spawned = std::thread::Builder::new().name("transit-null-audio".into()).spawn(move || pacing.run());
        *thread = Some(spawned.map_err(|e| ab::OpenError::Other(format!("the silent device's thread did not start: {e}")))?);
        Ok(())
    }
    /// Joins the pacing thread: a control-thread call, never from the render callback.
    fn stop(&self) -> Result<(), ab::OpenError> {
        self.stop.store(true, Ordering::Release);
        self.clock.interrupt();
        if let Some(thread) = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = thread.join();
        }
        Ok(())
    }
    fn latency(&self) -> Option<Duration> {
        Some(self.latency)
    }
    fn thread_priority(&self) -> ab::WorkerPriority {
        match self.priority.load(Ordering::Acquire) {
            PRIO_PRO_AUDIO => ab::WorkerPriority::MmcssProAudio,
            PRIO_RAISED => ab::WorkerPriority::RaisedPriority,
            PRIO_FAILED => ab::WorkerPriority::PromotionFailed,
            PRIO_UNOBSERVED => ab::WorkerPriority::Unobserved,
            PRIO_RELEASED => ab::WorkerPriority::Released,
            _ => ab::WorkerPriority::Pending,
        }
    }
}

impl Drop for NullStream {
    fn drop(&mut self) {
        let _ = ab::Stream::stop(self);
    }
}

/// What the pacing thread owns while it runs.
struct Pacing {
    render: ab::RenderCallback,
    error: ab::ErrorCallback,
    slot: Arc<Mutex<Option<Callbacks>>>,
    stop: Arc<AtomicBool>,
    priority: Arc<AtomicU8>,
    clock: Arc<dyn PaceClock>,
    format: ab::Format,
    frames: u32,
}

impl Pacing {
    fn run(self) {
        let Pacing { mut render, mut error, slot, stop, priority, clock, format, frames } = self;
        // The device-independent setup of a real callback thread, at thread start (never per block).
        let promotion = transit_rt::promote_audio_thread();
        priority.store(
            match promotion {
                transit_rt::Promotion::Mmcss { .. } => PRIO_PRO_AUDIO,
                transit_rt::Promotion::Priority(_) => PRIO_RAISED,
                transit_rt::Promotion::Denied(_) => PRIO_FAILED,
                _ => PRIO_UNOBSERVED,
            },
            Ordering::Release,
        );
        let channels = usize::from(format.channels.max(1));
        // Allocated once, before the first block; the loop allocates nothing.
        let mut data = vec![0.0f32; frames as usize * channels];
        let paced = catch_unwind(AssertUnwindSafe(|| {
            let mut pacer = Pacer::new(clock.now(), frames, format.rate);
            while !stop.load(Ordering::Acquire) {
                clock.wait_until(pacer.deadline(), &stop);
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let Some(tick) = pacer.wake(clock.now()) else { continue };
                let mut sink = Discard { data: &mut data, channels };
                let mut block = ab::RenderBlock::new(frames as usize, tick.stamp, format, &mut sink);
                block.device_position = Some(tick.position);
                render(&mut block);
            }
        }));
        if let Err(payload) = paced {
            // Reported once; the error callback may panic too, and there is nobody left to tell.
            let _ = catch_unwind(AssertUnwindSafe(|| error(ab::StreamError::Other)));
            // A payload whose own drop panics must not skip the teardown below either.
            let _ = catch_unwind(AssertUnwindSafe(move || drop(payload)));
        }
        transit_rt::demote(promotion);
        if promotion.is_promoted() {
            priority.store(PRIO_RELEASED, Ordering::Release);
        }
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((render, error));
        clock.ended();
    }
}

/// The block's samples go nowhere: the callback writes them into preallocated scratch, chunked as
/// an adapter's scratch is.
struct Discard<'a> {
    data: &'a mut [f32],
    channels: usize,
}

impl ab::RenderSink for Discard<'_> {
    fn render(&mut self, fill: &mut dyn FnMut(&mut [f32])) {
        for chunk in self.data.chunks_mut(ab::SCRATCH_FRAMES * self.channels) {
            fill(chunk);
        }
    }
}

/// Nanoseconds of `blocks` blocks of `frames` frames at `rate`, exact to the nanosecond below.
fn block_ns(blocks: u64, frames: u32, rate: u32) -> u64 {
    let ns = u128::from(blocks) * u128::from(frames) * 1_000_000_000 / u128::from(rate.max(1));
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// One block the pacer let through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tick {
    /// The device frame of the block's first frame.
    pub position: u64,
    pub stamp: ab::Stamp,
}

/// The schedule of the silent device's blocks: block `k` since the anchor is due at the anchor plus
/// `k` block lengths, computed from `k` (no drift accumulates). A wake renders one block; a late
/// wake finds the next deadline already past and renders again at once, so the blocks keep real
/// time on average. A wake more than [`MAX_BEHIND_BLOCKS`] late re-anchors at that wake instead, and
/// its stamp is [`ab::Quality::Invalid`] so the clock fit restarts there.
pub(crate) struct Pacer {
    anchor: Duration,
    blocks: u64,
    position: u64,
    frames: u32,
    rate: u32,
    /// The time of the first callback: the stamps' backend times are offsets from it.
    first: Option<Duration>,
}

impl Pacer {
    pub(crate) fn new(now: Duration, frames: u32, rate: u32) -> Self {
        Self { anchor: now, blocks: 0, position: 0, frames, rate, first: None }
    }

    /// When the next block is due.
    pub(crate) fn deadline(&self) -> Duration {
        self.anchor + Duration::from_nanos(block_ns(self.blocks, self.frames, self.rate))
    }

    /// The block to render at `now`, or `None` when it is not due yet.
    pub(crate) fn wake(&mut self, now: Duration) -> Option<Tick> {
        let mut due = self.deadline();
        if now < due {
            return None;
        }
        let behind = now - due;
        let mut quality = ab::Quality::BackendEstimate;
        if behind > Duration::from_nanos(block_ns(MAX_BEHIND_BLOCKS, self.frames, self.rate)) {
            (self.anchor, self.blocks, due, quality) = (now, 0, now, ab::Quality::Invalid);
        }
        let first = *self.first.get_or_insert(now);
        let offset = |t: Duration| i64::try_from(t.as_nanos()).unwrap_or(i64::MAX) - i64::try_from(first.as_nanos()).unwrap_or(i64::MAX);
        let stamp = ab::Stamp {
            host_callback_ns: u64::try_from(now.as_nanos()).unwrap_or(u64::MAX),
            backend_callback_ns: offset(now),
            backend_frame_ns: offset(due),
            quality,
            offset_uncertainty_ns: None,
        };
        let tick = Tick { position: self.position, stamp };
        self.blocks += 1;
        self.position += u64::from(self.frames);
        Some(tick)
    }
}

/// A clock the test steps, with a handshake: [`StepClock::advance`] returns once the pacing thread
/// has rendered every block due by the new time and waits again (or the stream stopped). No wall
/// clock decides anything.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub struct StepClock {
    state: Mutex<StepState>,
    changed: std::sync::Condvar,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
struct StepState {
    now: Duration,
    /// The deadline the pacing thread waits for, while it waits.
    waiting: Option<Duration>,
    /// A stop interrupted the wait, or the thread ended: nothing waits on this clock now.
    interrupted: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
impl StepClock {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Wait until the pacing thread waits for a deadline later than now (or has stopped).
    pub fn settle(&self) {
        // timing: stall bound only, so a broken handshake fails instead of hanging the run.
        let bound = Duration::from_secs(60);
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, timeout) = self
            .changed
            .wait_timeout_while(state, bound, |s| !s.interrupted && !s.waiting.is_some_and(|deadline| deadline > s.now))
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
        assert!(!timeout.timed_out(), "the silent device's pacing thread did not come back to its clock");
    }

    /// Move the time on by `by`, then [`Self::settle`].
    pub fn advance(&self, by: Duration) {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.now += by;
            self.changed.notify_all();
        }
        self.settle();
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl PaceClock for StepClock {
    fn now(&self) -> Duration {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).now
    }
    fn wait_until(&self, deadline: Duration, stop: &AtomicBool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.waiting = Some(deadline);
        self.changed.notify_all();
        while state.now < deadline && !stop.load(Ordering::Acquire) {
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        state.waiting = None;
    }
    fn interrupt(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.interrupted = true;
        self.changed.notify_all();
    }
    fn resume(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).interrupted = false;
    }
    /// The thread is gone: a [`Self::settle`] in progress returns instead of waiting for a wait.
    fn ended(&self) {
        self.interrupt();
    }
}

#[cfg(test)]
#[path = "null_tests.rs"]
mod tests;
