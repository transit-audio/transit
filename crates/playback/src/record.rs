//! Audio input capture for recording.
//!
//! A [`Recorder`] opens the default input device. While armed, the input callback copies each
//! block into a preallocated lock-free ring (it never locks or allocates), and a writer thread
//! streams the ring into one WAV file per take, rewriting the header every second so a crash leaves
//! a playable file.

pub mod start;

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use transit_audio_backend as ab;
use transit_model::TrackId;

/// What [`Recorder::finish`] returns: the takes that hold audio and the first problem met.
pub type Takes = (Vec<(TrackId, PathBuf)>, Option<String>);

/// Frames each block of a [`LiveTake`] sums up.
pub const LIVE_BLOCK: usize = 128;
/// Blocks a live take keeps per channel at most (three hours at 48 kHz); its file records on.
const LIVE_BLOCKS: usize = 1 << 22;

/// A take while it records, summed up for drawing: per channel, the (min, max) of each
/// [`LIVE_BLOCK`] frames so far.
pub type LiveTake = Vec<Vec<(f32, f32)>>;

const RING_SECONDS: usize = 4;
/// How long a recording input may go without a callback before it counts as gone.
const STALL: Duration = Duration::from_secs(2);
const GONE: u8 = 1;
const BROKEN: u8 = 2;
/// The driver asked for a reset (ASIO): the input is reopened with the device.
const RESET: u8 = 3;
/// The driver stopped calling back (the ASIO watchdog): the input is reopened with the device.
const DRIVER_STALLED: u8 = 4;
const ARMED: u64 = 1;
const CAPTURING: u64 = 2;
const CAPTURE_FLAGS: u64 = ARMED | CAPTURING;

/// Why the input can't go on.
#[derive(Debug, Clone, PartialEq)]
pub enum Trouble {
    /// The device went away or stopped sending audio, or its callback hit a bug.
    Device(String),
    /// Writing a take failed (a full disk, say).
    Disk(String),
}

/// Single-producer single-consumer FIFO of samples (input callback → writer thread). The slots
/// are atomics, so it needs no lock and no `unsafe`.
struct CaptureRing {
    slots: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl CaptureRing {
    fn new(capacity: usize) -> CaptureRing {
        let n = capacity.max(2).next_power_of_two();
        CaptureRing { slots: (0..n).map(|_| AtomicU32::new(0)).collect(), mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Queue up to `n` samples (`sample(i)` is the i-th); returns how many fit.
    fn push(&self, n: usize, sample: impl Fn(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let n = n.min(self.slots.len().saturating_sub(head.wrapping_sub(self.tail.load(Ordering::Acquire))));
        for i in 0..n {
            if let Some(slot) = self.slots.get(head.wrapping_add(i) & self.mask) {
                slot.store(sample(i).to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    fn pop_all(&self, out: &mut Vec<f32>) {
        let (tail, head) = (self.tail.load(Ordering::Relaxed), self.head.load(Ordering::Acquire));
        out.extend(
            (0..head.wrapping_sub(tail))
                .map(|i| self.slots.get(tail.wrapping_add(i) & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)))),
        );
        self.tail.store(head, Ordering::Release);
    }
}

/// An input sample, or silence for NaN and infinities, which would poison the take and the mix.
fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

struct Shared {
    /// Generation in the high bits; admission and in-flight producer in the low bits.
    capture_state: AtomicU64,
    ring: CaptureRing,
    first_frame: AtomicU64,
    /// Producer-owned while admitted; reset only when quiescent, drained by the writer at stop.
    pending_silence: AtomicUsize,
    /// Samples lost while the ring was full.
    dropped: AtomicUsize,
    /// Input callbacks so far, to tell a device that stopped sending.
    beats: AtomicU64,
    /// Set from the input stream when it can't go on: `GONE` or `BROKEN`.
    fault: AtomicU8,
    /// The first failure writing a take, until it's reported.
    failed: Mutex<Option<String>>,
}

impl Shared {
    fn new(ring: CaptureRing) -> Shared {
        let (beats, fault, failed) = (AtomicU64::new(0), AtomicU8::new(0), Mutex::new(None));
        Shared {
            capture_state: AtomicU64::new(0),
            ring,
            first_frame: AtomicU64::new(u64::MAX),
            pending_silence: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            beats,
            fault,
            failed,
        }
    }

    /// Control thread, after the preceding writer has joined (or failed startup quiesced).
    fn start_capture(&self) {
        let generation = (self.capture_state.load(Ordering::Relaxed) & !CAPTURE_FLAGS).wrapping_add(CAPTURE_FLAGS + 1);
        self.pending_silence.store(0, Ordering::Relaxed);
        self.capture_state.store(generation | ARMED, Ordering::Release);
    }

    fn stop_capture(&self) {
        self.capture_state.fetch_and(!ARMED, Ordering::AcqRel);
    }

    fn armed(&self) -> bool {
        self.capture_state.load(Ordering::Relaxed) & ARMED != 0
    }

    /// Single bounded admission attempt. Stop and admission are ordered by the same atomic;
    /// generation prevents an attempt delayed across stop/rearm from joining the next take.
    fn admit_capture(&self, observed: u64) -> Option<CaptureLease<'_>> {
        if observed & CAPTURE_FLAGS != ARMED {
            return None;
        }
        self.capture_state.compare_exchange(observed, observed | CAPTURING, Ordering::AcqRel, Ordering::Relaxed).ok()?;
        Some(CaptureLease(self))
    }

    fn begin_capture(&self) -> Option<CaptureLease<'_>> {
        self.admit_capture(self.capture_state.load(Ordering::Acquire))
    }

    fn quiescent(&self) -> bool {
        self.capture_state.load(Ordering::Acquire) & CAPTURE_FLAGS == 0
    }

    /// Control thread only, after closing admission. Also needed if no writer was spawned or
    /// its thread panicked: joining that thread alone would not prove producer quiescence.
    /// Total wait time depends on producer progress; the polling interval is not a timeout.
    fn wait_for_capture(&self) {
        while !self.quiescent() {
            #[cfg(test)]
            tests::on_capture_wait();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Writer only. Observe quiescence before the final pop, so it covers every admitted push.
    /// Once true no producer can enter until the control thread has joined this writer.
    fn drain_capture(&self, block: &mut Vec<f32>) -> bool {
        let finished = self.quiescent();
        self.ring.pop_all(block);
        finished
    }

    /// Run the input callback's `work`. After a panic in it, the input is broken and does
    /// nothing more.
    fn guarded(&self, work: impl FnOnce()) {
        self.beats.fetch_add(1, Ordering::Relaxed);
        if self.fault.load(Ordering::Relaxed) != BROKEN && std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            self.fault.store(BROKEN, Ordering::Relaxed);
        }
    }

    /// The input callback's work under a CaptureLease. Samples that don't fit are owed and queued
    /// as silence before the next block, so the take keeps its timing.
    fn capture_at(&self, first_frame: u64, data: &[f32]) {
        let _ = self.first_frame.compare_exchange(u64::MAX, first_frame, Ordering::Release, Ordering::Relaxed);
        self.capture(data);
    }

    fn capture(&self, data: &[f32]) {
        let owed = self.pending_silence.load(Ordering::Relaxed);
        let owed = owed - self.ring.push(owed, |_| 0.0);
        let kept = if owed == 0 { self.ring.push(data.len(), |i| data.get(i).copied().map_or(0.0, finite)) } else { 0 };
        self.pending_silence.store(owed.saturating_add(data.len() - kept), Ordering::Relaxed);
        self.dropped.fetch_add(data.len() - kept, Ordering::Relaxed);
    }
}

/// Release on normal return and unwind. No callback must arrive after Stop to acknowledge it.
struct CaptureLease<'a>(&'a Shared);

impl Drop for CaptureLease<'_> {
    fn drop(&mut self) {
        self.0.capture_state.fetch_and(!CAPTURING, Ordering::Release);
    }
}

pub struct Recorder {
    shared: Arc<Shared>,
    /// Live input for monitoring (always fed while the input stream runs).
    pub monitor: Arc<InputRing>,
    _stream: Option<Box<dyn transit_audio_backend::Stream>>,
    clock: Option<crate::device_clock::MaintainedClock>,
    record_start: Option<start::Reader>,
    pub device_name: String,
    pub sample_rate: u32,
    /// The live input's width: the device's channels, then the System Audio pair when it runs.
    pub channels: usize,
    /// System Audio (#268), the loopback capture appended to this input.
    pub system_audio: crate::system_audio::SystemAudio,
    /// What the input stream opened with, as the backend reported it: the buffer in frames, the
    /// latency (ASIO: `getLatencies`) and the device sample format (None where not reported).
    pub buffer_frames: Option<u32>,
    pub latency: Option<Duration>,
    pub sample_format: Option<ab::SampleFormat>,
    /// The take writer, and what each track's take holds so far.
    writer: Option<(std::thread::JoinHandle<Takes>, Vec<(TrackId, Arc<Mutex<LiveTake>>)>)>,
    /// The callback count last seen, and since when.
    seen: (u64, Instant),
}

/// The input callbacks: device samples arrive as `f32` chunks from the adapter's preallocated
/// scratch (so the callback never allocates), then feed the monitor ring and, while armed, the
/// capture.
fn capture_callbacks(
    shared: &Arc<Shared>,
    monitor: &Arc<InputRing>,
    mut clock: crate::device_clock::CallbackClock,
    progress: Option<start::InputProgress>,
    loop_probe: Option<Arc<crate::loop_probe::LoopProbe>>,
    mut tap: Option<crate::system_audio::Tap>,
) -> (ab::CaptureCallback, ab::ErrorCallback) {
    let (sh, errors) = (Arc::clone(shared), Arc::clone(shared));
    let mon = Arc::clone(monitor);
    let capture = move |block: &mut ab::CaptureBlock<'_>| {
        crate::mark_audio_thread();
        let first_frame = clock.observer.next_frame();
        let (position, channels) = (block.device_position, usize::from(block.channels));
        clock.observer.record(block.frames, block.stamp);
        if let Some(progress) = &progress {
            progress.publish(clock.observer.next_frame(), block.frames as u64);
        }
        // System Audio (#268): the callback's clock observation, target and trim, once per callback.
        if let Some(tap) = tap.as_mut() {
            tap.begin(block.frames, block.stamp);
        }
        sh.guarded(|| {
            // One admission covers the entire device callback, including its chunks.
            let capture = sh.begin_capture();
            block.chunks(&mut |offset, samples| {
                // The loopback calibration (native ASIO) records the return by sample position.
                if let Some(probe) = &loop_probe {
                    probe.record(samples, channels, position.map(|p| p + offset as u64));
                }
                let feed = |at: usize, frames: &[f32]| {
                    mon.push(frames);
                    if capture.is_some()
                        && let Some(frame) = first_frame.checked_add(at as u64)
                    {
                        sh.capture_at(frame, frames);
                    }
                };
                // System Audio (#268): the pair appended to every frame, read from its ring.
                match tap.as_mut() {
                    Some(tap) => tap.widen(samples, &mut |piece, wide| feed(offset + piece, wide)),
                    None => feed(offset, samples),
                }
            });
        });
    };
    let error = move |e: ab::StreamError| {
        // Possibly the audio thread (see `crate::mark_audio_thread`).
        crate::mark_audio_thread();
        let fault = match e {
            ab::StreamError::DeviceNotAvailable => GONE,
            ab::StreamError::ResetRequested => RESET,
            ab::StreamError::Stalled => DRIVER_STALLED,
            ab::StreamError::Other => return,
        };
        // After a reset request or a stall the driver may go on calling back (at another rate, or
        // after a gap) until the app stops it at its next frame: the take ends here, at once, so
        // no such block joins it. One atomic; `finish` still joins the writer as usual.
        if matches!(e, ab::StreamError::ResetRequested | ab::StreamError::Stalled) {
            errors.stop_capture();
        }
        let _ = errors.fault.compare_exchange(0, fault, Ordering::Relaxed, Ordering::Relaxed);
    };
    (Box::new(capture), Box::new(error))
}

impl Recorder {
    /// Observed scheduling state of this input worker generation. A configured feature is
    /// not evidence of successful registration; unsupported backends report Unobserved.
    pub fn thread_priority(&self) -> crate::scheduling::WorkerPriority {
        self._stream.as_ref().map_or(crate::scheduling::WorkerPriority::NoWorker, |stream| stream.thread_priority())
    }

    /// The device loop's counters of the input stream, when the backend keeps them (native WASAPI).
    pub fn backend_stats(&self) -> Option<crate::telemetry::StreamStats> {
        self._stream.as_ref().and_then(|stream| stream.stats())
    }

    /// Open the default input device. Errors when there is none (the caller shows a message).
    pub fn open() -> Result<Recorder, String> {
        Recorder::open_with(&crate::AudioConfig::default())
    }

    /// Hand a new loopback calibration to the running input stream (the app's Measure); false
    /// where the backend applies it only at the next open.
    pub(crate) fn set_calibration(&self, calibration: Option<ab::Calibration>) -> bool {
        self._stream.as_ref().is_some_and(|stream| stream.set_calibration(calibration))
    }

    /// Open the input device `audio` chooses (else the default) at its chosen rate and buffer
    /// size when the device offers them, else at the device's defaults. Integer devices (ASIO
    /// drivers usually are) are converted to f32. On Windows through the native backend of the
    /// effective backend, never cpal (ADR 0046); elsewhere through cpal.
    pub fn open_with(audio: &crate::AudioConfig) -> Result<Recorder, String> {
        let audio = crate::backend::native::open_config(audio);
        Recorder::open_on(&audio, cfg!(windows), || crate::backend::native::input_factory(&audio), &crate::backend::cpal::CpalFactory)
    }

    /// [`Self::open_with`] with the platform (`windows`), the native factory lookup and the cpal
    /// adapter as parameters (fakes in tests): on Windows the factory `native` gives opens it, or
    /// the open fails when there is none, and `cpal` is never asked. Elsewhere `cpal` opens it and
    /// `native` is never called: each platform asks exactly one boundary.
    pub(crate) fn open_on(
        audio: &crate::AudioConfig,
        windows: bool,
        native: impl FnOnce() -> Option<Box<dyn ab::BackendFactory>>,
        cpal: &dyn ab::BackendFactory,
    ) -> Result<Recorder, String> {
        if !windows {
            return Recorder::open_through(audio, cpal);
        }
        let native = native().ok_or_else(|| crate::backend::native::unserved(audio))?;
        Recorder::open_through(audio, &*native)
    }

    pub(crate) fn open_through(audio: &crate::AudioConfig, factory: &dyn ab::BackendFactory) -> Result<Recorder, String> {
        let backend = factory.backend(audio.system.as_deref()).ok_or("no audio system")?;
        let plan = backend.plan_capture(&ab::InputWish { device: audio.input.clone(), rate: audio.sample_rate, period: audio.buffer_size })?;
        // The chosen buffer size first; if the device refuses it, its default.
        let mut failure = "no usable input stream".to_string();
        let mut opened = None;
        for (index, request) in plan.attempts.iter().enumerate() {
            match Recorder::build(&*backend, &plan.endpoint, request, None, None, crate::system_audio::requested(audio)) {
                Ok(recorder) => {
                    opened = Some(recorder);
                    break;
                }
                Err(error) => {
                    if index + 1 < plan.attempts.len() {
                        log::warn!("cannot open the input with buffer {:?}: {error}; using the device default", request.period);
                    }
                    failure = error.to_string();
                }
            }
        }
        let mut recorder = opened.ok_or(failure)?;
        recorder.start()?;
        // The session's output already runs: the loopback may open now (see `crate::system_audio`).
        if crate::system_audio::requested(audio) {
            recorder.attach_system_audio(crate::system_audio::open(audio));
        }
        Ok(recorder)
    }

    /// Build the input stream on `endpoint` (the output's own ASIO device) at `rate` and exactly
    /// `period`, without starting it: [`Player`](crate::Player) builds it before the output so
    /// the driver creates both directions' buffers once, then starts both ([`Recorder::start`]).
    /// `mode` is the output's: both streams open in the same one.
    /// The buffer must be the output's: cpal sizes the input's scratch buffer at build time,
    /// and the output's build recreates the buffers at its own size. `loop_probe` is the Player's
    /// loopback calibration probe, which the capture callback feeds.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_on(
        backend: &dyn ab::Backend,
        endpoint: &ab::Endpoint,
        rate: u32,
        period: ab::Period,
        mode: ab::Mode,
        progress: start::InputProgress,
        calibration: Option<ab::Calibration>,
        loop_probe: Option<Arc<crate::loop_probe::LoopProbe>>,
        system_audio: bool,
    ) -> Result<Recorder, ab::OpenError> {
        let format = backend.capture_format(endpoint, Some(rate)).map_err(ab::OpenError::Other)?;
        if format.rate != rate {
            return Err(ab::OpenError::Other(format!("the input does not run at {rate} Hz")));
        }
        let request = ab::OpenRequest { format, period, mode, origin: ab::HostOrigin::default(), calibration };
        Recorder::build(backend, endpoint, &request, Some(progress), loop_probe, system_audio)
    }

    /// The recorder around a new, not yet started input stream on `endpoint`.
    pub(crate) fn build(
        backend: &dyn ab::Backend,
        endpoint: &ab::Endpoint,
        request: &ab::OpenRequest,
        progress: Option<start::InputProgress>,
        loop_probe: Option<Arc<crate::loop_probe::LoopProbe>>,
        system_audio: bool,
    ) -> Result<Recorder, ab::OpenError> {
        let format = request.format;
        let device_channels = usize::from(format.channels).max(1);
        // System Audio rides on this input as two more channels (see `crate::system_audio`): the
        // pair is reserved now and filled once the loopback is attached.
        let (system, tap) = if system_audio {
            let (state, tap) = crate::system_audio::reserve(device_channels, format.rate);
            (state, Some(tap))
        } else {
            (crate::system_audio::SystemAudio::Off, None)
        };
        let channels = tap.as_ref().map_or(device_channels, crate::system_audio::Tap::channels);
        let ring = CaptureRing::new((format.rate as usize).saturating_mul(channels * RING_SECONDS).min(1 << 25));
        let shared = Arc::new(Shared::new(ring));
        let monitor = InputRing::with_pairing(channels, format.rate, backend.capture_precedes_render(endpoint));
        if tap.is_some() {
            monitor.set_system_channel(device_channels);
        }
        let (callback_clock, clock) = crate::device_clock::stream_clock(crate::device_clock::Direction::Input, format.rate)
            .map_err(|error| ab::OpenError::Other(error.into()))?;
        let (capture, on_error) = capture_callbacks(&shared, &monitor, callback_clock, progress, loop_probe, tap);
        let (stream, achieved) = backend.open_capture(endpoint, request, capture, on_error)?;
        let clock = crate::device_clock::MaintainedClock::start(clock)
            .map_err(|error| {
                log::warn!("input clock maintenance unavailable: {error}");
            })
            .ok();
        let seen = (0, Instant::now());
        Ok(Recorder {
            monitor,
            shared,
            _stream: Some(stream),
            clock,
            record_start: None,
            device_name: endpoint.name.clone(),
            sample_rate: format.rate,
            channels,
            system_audio: system,
            buffer_frames: achieved.period.frames(),
            latency: achieved.latency,
            sample_format: Some(achieved.format.sample),
            writer: None,
            seen,
        })
    }

    /// The first live input channel of the System Audio pair, when this input carries one (it is
    /// silent until the loopback runs).
    pub fn system_channel(&self) -> Option<usize> {
        self.monitor.system_channel()
    }

    /// The device's own input channels (the live input without the System Audio pair).
    pub fn device_channels(&self) -> usize {
        self.system_channel().unwrap_or(self.channels)
    }

    /// Hand the loopback capture to this input's reserved pair (control thread, once, after the
    /// session's output started).
    pub fn attach_system_audio(&mut self, opened: crate::system_audio::Opened) {
        self.system_audio.attach(opened);
    }

    /// What Audio Devices shows of System Audio (reading it resets the level meter's peaks).
    pub fn system_audio_report(&self) -> crate::system_audio::Report {
        self.system_audio.report(self.sample_rate)
    }

    /// Start the input stream.
    pub(crate) fn start(&self) -> Result<(), String> {
        if let Some(stream) = &self._stream {
            stream.start().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// A recorder without a device, for tests.
    #[cfg(test)]
    pub(crate) fn detached(name: &str) -> Recorder {
        Recorder {
            shared: Arc::new(Shared::new(CaptureRing::new(8))),
            monitor: InputRing::new(1, 48_000),
            _stream: None,
            clock: None,
            record_start: None,
            device_name: name.into(),
            sample_rate: 48_000,
            channels: 1,
            system_audio: crate::system_audio::SystemAudio::Off,
            buffer_frames: None,
            latency: None,
            sample_format: None,
            writer: None,
            seen: (0, Instant::now()),
        }
    }

    /// Create each take's file and start recording into them: a track's take records the given
    /// input channels into a new file.
    pub fn arm(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>) -> Result<(), String> {
        self.arm_with(takes, None)
    }

    /// Prepare files and bounded input history. Only the applied audio transaction
    /// supplies the timestamp at which samples start belonging to the take.
    pub fn prepare(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>, start: start::Reader) -> Result<(), String> {
        if self.has_pending_take() {
            return Err("already recording".into());
        }
        {
            let clock = self.clock.as_mut().ok_or("input clock is unavailable for recording")?;
            // The key, not a validated map: the audio thread falls back to the degraded policy
            // when the clock has not fitted, and says why when there is no fallback.
            let input = clock.key();
            start.mark_audio();
            if input.direction != crate::device_clock::Direction::Input {
                return Err("recording requires this input device's clock".into());
            }
            let input_start = clock.prepare_recording(start.clone())?;
            let gate = WriterStart {
                reader: start,
                history: start::History::new(self.channels, (self.sample_rate as usize).saturating_mul(RING_SECONDS)),
                input_start,
                placed: false,
                next: None,
            };
            self.arm_with(takes, Some(gate))
        }
    }

    /// The latch of the take prepared for this pass, for a MIDI pass that must start with it.
    pub fn start_reader(&self) -> Option<start::Reader> {
        self.record_start.clone()
    }

    /// The applied origin survives Stop until the caller has collected the take.
    pub(crate) fn prepared_start(&self) -> Option<start::Ticket> {
        self.record_start.as_ref().map(start::Reader::ticket)
    }

    /// Was the applied start placed without a validated device clock (see
    /// [`start::Reader::fallback`])? Such a take is approximate by up to one audio block.
    pub fn start_approximate(&self) -> bool {
        self.record_start.as_ref().and_then(start::Reader::fallback).is_some_and(|f| f.approximate)
    }

    /// The applied origin survives Stop until the caller has collected the take.
    pub fn applied_start(&self) -> Option<start::Point> {
        match self.record_start.as_ref()?.status() {
            start::Status::Started(point) => Some(point),
            _ => None,
        }
    }

    fn arm_with(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>, gate: Option<WriterStart>) -> Result<(), String> {
        if self.writer.is_some() {
            return Err("already recording".into());
        }
        let takes =
            takes.into_iter().map(|(id, inputs, path)| TakeWriter::create(id, inputs, path, self.sample_rate)).collect::<Result<Vec<_>, _>>()?;
        let live = takes.iter().map(|t| (t.id, Arc::clone(&t.live))).collect();
        let ring = &self.shared.ring;
        ring.tail.store(ring.head.load(Ordering::Acquire), Ordering::Release);
        *self.shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.shared.first_frame.store(u64::MAX, Ordering::Relaxed);
        self.record_start = gate.as_ref().map(|gate| gate.reader.clone());
        self.shared.start_capture();
        let (shared, channels) = (Arc::clone(&self.shared), self.channels);
        match std::thread::Builder::new().name("take writer".into()).spawn(move || write_takes_started(&shared, takes, channels, gate)) {
            Ok(w) => self.writer = Some((w, live)),
            Err(e) => {
                self.shared.stop_capture();
                // No writer exists to join. Quiesce any admitted callback before a later arm
                // resets the ring/debt. This waits on the control thread, never the callback.
                self.shared.wait_for_capture();
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// What the take recording onto `track` holds so far.
    pub fn live_take(&self, track: TrackId) -> Option<MutexGuard<'_, LiveTake>> {
        let (_, take) = self.writer.as_ref()?.1.iter().find(|(id, _)| *id == track)?;
        Some(take.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Why the input can't go on, if it can't: the device went away or, while recording, stopped
    /// sending audio; its callback hit a bug; or writing a take failed (reported once).
    pub fn trouble(&mut self) -> Option<Trouble> {
        if let Some(e) = self.shared.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Some(Trouble::Disk(e));
        }
        let name = &self.device_name;
        match self.shared.fault.load(Ordering::Relaxed) {
            GONE => return Some(Trouble::Device(format!("“{name}” was disconnected"))),
            BROKEN => return Some(Trouble::Device("the audio input hit an internal error (see the log)".into())),
            RESET => return Some(Trouble::Device(crate::RESET_REQUESTED_TEXT.into())),
            DRIVER_STALLED => return Some(Trouble::Device(crate::DRIVER_STALLED_TEXT.into())),
            _ => {}
        }
        let beats = self.shared.beats.load(Ordering::Relaxed);
        if beats != self.seen.0 || !self.shared.armed() {
            self.seen = (beats, Instant::now());
        }
        (self.seen.1.elapsed() >= STALL).then(|| Trouble::Device(format!("“{name}” stopped sending audio")))
    }

    /// Input the current take has lost to a slow disk so far, in milliseconds.
    pub fn lost_ms(&self) -> u64 {
        let frames = self.shared.dropped.load(Ordering::Relaxed) / self.channels.max(1);
        (frames as u64).saturating_mul(1000) / u64::from(self.sample_rate.max(1))
    }

    /// Independently maintained input clock snapshot; separate from output at every rate.
    pub fn clock_report(&mut self) -> Option<crate::device_clock::ClockReport> {
        self.clock.as_mut().map(crate::device_clock::MaintainedClock::report)
    }

    /// A prepared or active take must be finalized and collected before handing
    /// this input back to its device owner.
    pub fn has_pending_take(&self) -> bool {
        self.writer.is_some()
    }

    pub fn finish(&mut self) -> Takes {
        if let Some(start) = &self.record_start {
            start.cancel();
        }
        self.shared.stop_capture();
        self.shared.wait_for_capture();
        let (takes, problem) = self.writer.take().map_or_else(Takes::default, |(w, _)| {
            w.join().unwrap_or_else(|_| (Vec::new(), Some("the recording writer stopped unexpectedly; check the take files".into())))
        });
        let lost = self.shared.dropped.swap(0, Ordering::Relaxed) / self.channels.max(1);
        let slow =
            (lost > 0).then(|| format!("{} ms of input lost: the disk was too slow", lost.saturating_mul(1000) / self.sample_rate.max(1) as usize));
        (takes, problem.or(slow))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
fn write_takes(shared: &Shared, takes: Vec<TakeWriter>, channels: usize) -> Takes {
    write_takes_started(shared, takes, channels, None)
}

struct WriterStart {
    reader: start::Reader,
    history: start::History,
    input_start: start::mapping::InputStart,
    placed: bool,
    next: Option<u64>,
}

impl WriterStart {
    fn finished(&self) -> Result<bool, &'static str> {
        if self.placed || self.reader.status() == start::Status::Cancelled {
            return Ok(true);
        }
        // Mapping can fail after the last write. Propagate that failure before
        // finalizing; a terminal mapping state is not evidence of a saved take.
        self.input_start.frame()?;
        Ok(false)
    }

    fn write(&mut self, shared: &Shared, takes: &mut [TakeWriter], block: &[f32], channels: usize) -> Result<(), String> {
        if !self.placed {
            match self.reader.status() {
                start::Status::Pending => {}
                start::Status::Started(_) => {
                    if let Some(frame) = self.input_start.frame()? {
                        self.history.set_start(frame);
                        self.placed = true;
                    }
                }
                start::Status::Cancelled => {
                    shared.stop_capture();
                    return Ok(());
                }
            }
        }
        let first = *self.next.get_or_insert_with(|| shared.first_frame.load(Ordering::Acquire));
        if first == u64::MAX {
            if block.is_empty() {
                self.next = None;
                return Ok(());
            }
            return Err("recording input arrived without a device position".into());
        }
        self.history.push(first, block, |data| takes.iter_mut().for_each(|take| take.write(data, channels)))?;
        self.next = Some(first.checked_add((block.len() / channels.max(1)) as u64).ok_or("recording input position exhausted")?);
        Ok(())
    }
}

fn write_block(shared: &Shared, gate: &mut Option<WriterStart>, takes: &mut [TakeWriter], block: &[f32], channels: usize) {
    if let Some(gate) = gate {
        if let Err(error) = gate.write(shared, takes, block, channels) {
            shared.stop_capture();
            takes.iter_mut().for_each(|take| take.fail(&error));
        }
    } else {
        takes.iter_mut().for_each(|take| take.write(block, channels));
    }
}

fn write_takes_started(shared: &Shared, mut takes: Vec<TakeWriter>, channels: usize, mut gate: Option<WriterStart>) -> Takes {
    let (mut block, mut header_written, mut reported) = (Vec::new(), Instant::now(), false);
    loop {
        let stopping = shared.drain_capture(&mut block);
        // The ring holds interleaved samples and a pop can end mid-frame: write whole frames and
        // keep the remainder for the next pop, so channels never shift.
        let whole = block.len() - block.len() % channels.max(1);
        write_block(shared, &mut gate, &mut takes, &block[..whole], channels);
        block.drain(..whole);
        if !reported && let Some(e) = takes.iter().find_map(|t| t.error.clone()) {
            *shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
            reported = true;
        }
        if stopping {
            // No producer remains. Preserve the final overflow duration even if the device
            // never calls us again. Bound writer scratch instead of allocating the entire debt.
            let mut owed = shared.pending_silence.swap(0, Ordering::Relaxed);
            let chunk = 8192 * channels.max(1);
            while owed > 0 && takes.iter().any(|t| t.error.is_none()) {
                let n = owed.min(chunk);
                block.resize(block.len() + n, 0.0);
                let whole = block.len() - block.len() % channels.max(1);
                write_block(shared, &mut gate, &mut takes, &block[..whole], channels);
                block.drain(..whole);
                owed -= n;
            }
            // An applied start may still be in the independent clock worker.
            // Preserve the final input history until that worker acknowledges;
            // no further audio callback is required. Worker teardown fails the
            // pending mapping explicitly instead of leaving this join stranded.
            #[cfg(test)]
            tests::on_finalize_gate();
            let finished = match gate.as_ref().map_or(Ok(true), WriterStart::finished) {
                Ok(finished) => finished,
                Err(error) => {
                    takes.iter_mut().for_each(|take| take.fail(error));
                    true
                }
            };
            if finished || takes.iter().all(|take| take.error.is_some()) {
                break;
            }
            #[cfg(test)]
            tests::on_placement_wait();
        }
        if header_written.elapsed() >= Duration::from_secs(1) {
            header_written = Instant::now();
            takes.iter_mut().for_each(TakeWriter::write_header);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut done, mut problem) = (Vec::new(), None);
    for mut t in takes {
        t.write_header();
        if t.frames == 0 {
            let _ = std::fs::remove_file(&t.path);
        } else {
            done.push((t.id, t.path));
        }
        problem = problem.or(t.error);
    }
    (done, problem)
}

/// One take's file. After an error it stops writing; its header keeps covering what was written.
struct TakeWriter {
    id: TrackId,
    inputs: Vec<usize>,
    path: PathBuf,
    file: File,
    rate: u32,
    frames: u64,
    error: Option<String>,
    live: Arc<Mutex<LiveTake>>,
}

impl TakeWriter {
    fn create(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32) -> Result<TakeWriter, String> {
        let file = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| File::create_new(&path));
        let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
        let live = Arc::new(Mutex::new(vec![Vec::new(); inputs.len()]));
        let mut take = TakeWriter { id, inputs, path, file, rate, frames: 0, error: None, live };
        take.write_header();
        take.error.take().map_or(Ok(take), Err)
    }

    /// Append this take's channels of `block` (interleaved, `channels` wide).
    fn write(&mut self, block: &[f32], channels: usize) {
        let channels = channels.max(1);
        if self.error.is_some() {
            return;
        }
        let bytes: Vec<u8> = block
            .chunks_exact(channels)
            .flat_map(|f| self.inputs.iter().flat_map(move |&i| f.get(i).copied().unwrap_or(0.0).to_le_bytes()))
            .collect();
        match self.file.write_all(&bytes) {
            Ok(()) => {
                self.summarize(block, channels);
                self.frames += (block.len() / channels) as u64;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Add this take's channels of `block` to its live take.
    fn summarize(&self, block: &[f32], channels: usize) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        for (f, n) in block.chunks_exact(channels).zip(self.frames..) {
            let at = (n / LIVE_BLOCK as u64) as usize;
            for (blocks, &i) in live.iter_mut().zip(&self.inputs) {
                let v = f.get(i).copied().unwrap_or(0.0);
                match blocks.get_mut(at) {
                    Some(b) => *b = (b.0.min(v), b.1.max(v)),
                    None if at < LIVE_BLOCKS => blocks.push((v, v)),
                    None => {}
                }
            }
        }
    }

    fn write_header(&mut self) {
        let header = match transit_audio_io::wav_float_header(self.inputs.len(), self.rate, self.frames) {
            Ok(h) => h,
            Err(e) => return self.fail(e),
        };
        let f = &mut self.file;
        if let Err(e) = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header)).and_then(|()| f.seek(SeekFrom::End(0))) {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.error.get_or_insert_with(|| format!("{}: {e}", self.path.display()));
    }
}

#[cfg(test)]
mod tests;

mod monitor;
pub use monitor::{ALIGNED_BACKLOG_BLOCKS_PAIRED, ALIGNED_BACKLOG_BLOCKS_UNPAIRED, InputRing, MonitorTelemetry};
