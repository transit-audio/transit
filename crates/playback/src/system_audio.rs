//! The System Audio input (#268): what the computer plays, recorded and monitored as a stereo
//! input path beside the interface's inputs.
//!
//! On Windows the native WASAPI backend opens a loopback capture of a render endpoint (the system
//! default output unless one is picked) on its own thread ([`transit_audio_backend::Backend::open_loopback`]),
//! whatever backend serves the session's device. Its callback folds each frame to stereo and
//! queues it in a lock-free ring ([`Feed`]). The input stream's callback ([`crate::record`]) reads
//! the ring through a [`Tap`] and appends the pair after the device's own channels, so the monitor
//! ring, the take writer and the I/O map see one wider input: "System Audio" is the last two
//! channels of the live input ([`transit_model::IoMap::system_audio`]).
//!
//! **Order.** The input reserves the pair when it is built ([`reserve`]: zeros until a capture
//! arrives), and the loopback opens only once the session's output runs, then is handed to the
//! tap through a one-slot queue ([`SystemAudio::attach`]). A loopback client opened first would
//! hold the render endpoint shared and stop an exclusive output from opening on it; opened after,
//! it is the loopback Windows refuses, which Audio Devices reports while the session runs.
//!
//! **Clocking.** The loopback stream is clocked by its render endpoint, not by the session's
//! device: it is a second clock. transit has no second-device input rule yet (an input on another
//! device is converted at the nominal rate ratio and re-aligned, ADR 0017's open item), so this is
//! the simplest safe one: the [`Tap`] resamples from the endpoint's rate to the input device's rate
//! with the band-limited reader (ADR 0056 decision 8, #603), at a ratio trimmed by the measured
//! drift of the two clocks (frame counts against QPC time, [`rate`]) plus a slow correction that
//! holds the ring at its target (one input callback plus one loopback packet plus 30 ms;
//! [`Resampler`] names each term). The measured drift is what the diagnostics report. The input converter then takes the widened input
//! to the session rate as it does any input. The callback never waits on the loopback client: an
//! empty ring reads as zeros (a loopback stream sends nothing while nothing plays on some
//! drivers), a full one drops and counts. The capture has no timeline relation to the session: its
//! latency (the loopback stream's plus the ring's target) is reported, not compensated.
//!
//! Linux delivers monitor sources through the JACK and ALSA paths already; [`monitor_label`]
//! names them in Audio Devices. macOS needs a virtual device (#24).

use crate::band_limited;
use crate::devices::AudioConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use transit_audio_backend as ab;

/// The name of the input path (the track input picker's entry and the route's name).
pub const PATH_NAME: &str = transit_model::SYSTEM_AUDIO_PATH;
/// The ring's margin, in milliseconds of the endpoint's rate: the target fill is this plus one
/// input callback plus one loopback packet (`Resampler::target`).
pub const TARGET_MS: u32 = 30;
/// The loopback packet assumed until the first one arrives: the shared-mode engine period.
const PACKET_MS: u32 = 10;
/// The ring's capacity, in milliseconds.
const RING_MS: u32 = 500;
/// The largest trim of the rate ratio, in parts per million: 0.2 %, 3.5 cents (two crystals differ
/// by well under 100 ppm; the tests cover drifts up to 1000 ppm, and the trim has room beyond that
/// for the correction terms).
pub const MAX_TRIM_PPM: f64 = 2_000.0;
/// The proportional gain of the fill correction, in ppm of trim per unit of fill error (the error
/// is over the 30 ms margin, so a fill 10 % of the margin over its target adds 20 ppm). The rate
/// estimate carries the drift; this term only moves the fill back to its target, and at this gain
/// the packet sawtooth in the fill (a full packet is a third of the margin) moves the trim by
/// about 35 ppm at most. The loop it closes is `s^2 + a Kp s + a Ki = 0` with `a` = rate / margin
/// = 33.3 per second: `a Kp` = 0.0067 per second (a time constant of about 150 s).
const KP_PPM: f64 = 200.0;
/// The integral gain of the fill correction, in ppm per second per unit of fill error: with
/// [`KP_PPM`] it gives `wn` = 0.0033 per second and a damping of 1.
const KI_PPM_PER_S: f64 = 0.33;
/// The integral term is held within this many ppm: it only absorbs the estimate's bias.
const INTEGRAL_PPM: f64 = 50.0;
/// A ring this many targets full is resynchronized (the excess discarded), not trimmed down.
const RESYNC_TARGETS: usize = 4;
/// The time constant of the fill average, in seconds.
const FILL_TAU_SECONDS: f64 = 0.5;
/// Frames the tap widens at a time (its scratch is this many frames).
const PIECE: usize = 256;
/// The packet observations queued between the loopback thread and the reader (about 2.5 s of
/// 10 ms packets; a full queue drops observations, never samples).
const STAMPS: usize = 256;

/// The shared counters of one System Audio stream: written by the loopback thread and the input
/// callback (atomics only), read by the control thread for Audio Devices.
#[derive(Default)]
pub struct Status {
    /// Frames the feed dropped because the ring was full, once the reader runs.
    overflow: AtomicU64,
    /// Frames the reader gave as zeros because the ring was dry after it had read audio once (the
    /// endpoint fell silent, which a loopback stream may show as no packets, or it stalled).
    underrun: AtomicU64,
    /// Frames the reader discarded to resynchronize a ring far over its target.
    discarded: AtomicU64,
    /// The measured drift of the endpoint's clock against the device's, in tenths of a ppm:
    /// positive when the endpoint's clock runs fast.
    drift_deci_ppm: AtomicI32,
    /// The ring's target fill in endpoint frames, once the reader knows the input's callback (0
    /// before): the latency report follows it.
    target_frames: AtomicU32,
    /// The peak of each side since the last read, as `f32` bits (non-negative, so the bits order).
    peak: [AtomicU32; 2],
    /// The reader has started (the input stream runs): overflow counts from then on.
    reading: AtomicBool,
    /// The loopback stream reported a fault (its endpoint went away): it delivers nothing more.
    stopped: AtomicBool,
}

impl Status {
    fn note_peak(&self, side: usize, x: f32) {
        if let Some(p) = self.peak.get(side) {
            p.fetch_max(x.abs().to_bits(), Ordering::Relaxed);
        }
    }
}

/// What Audio Devices shows of System Audio.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Report {
    /// "off", "running", "refused" or "stopped".
    pub state: &'static str,
    /// The render endpoint captured (running), or why it is not (refused, stopped).
    pub text: String,
    /// The endpoint's mix rate and the input device's rate it is converted to.
    pub rate: u32,
    pub device_rate: u32,
    /// The input path's latency: the loopback stream's plus the ring's target.
    pub latency_ms: Option<f64>,
    pub drift_ppm: f64,
    pub underrun_frames: u64,
    pub overflow_frames: u64,
    pub discarded_frames: u64,
    /// The peak of each side since the previous report (the level meter).
    pub peak: [f32; 2],
}

/// System Audio as the recorder holds it, on the control side.
#[derive(Default)]
pub enum SystemAudio {
    /// Not asked for (or not offered on this platform).
    #[default]
    Off,
    /// The input reserved the pair; the loopback opens once the output runs ([`Self::attach`]).
    Pending(Box<rtrb::Producer<Source>>),
    Running(Box<Running>),
    /// The loopback did not open (why); the session's input opened without it.
    Refused(Box<str>),
}

/// A running loopback capture: its stream (dropped with the recorder) and counters.
pub struct Running {
    stream: Box<dyn ab::Stream>,
    status: Arc<Status>,
    pub endpoint: String,
    pub rate: u32,
    pub latency: Option<Duration>,
}

impl Running {
    /// Whether the loopback thread runs with a raised priority, as its stream reports it.
    pub fn thread_priority(&self) -> ab::WorkerPriority {
        self.stream.thread_priority()
    }
}

/// The reading end of a loopback capture, handed to the input stream's callback through a [`Tap`]:
/// the samples, and the packets' clock observations for the rate estimate.
pub struct Source {
    queue: rtrb::Consumer<f32>,
    stamps: rtrb::Consumer<rate::Obs>,
    status: Arc<Status>,
    rate: u32,
}

/// What [`open`] returns: the control-side state and, when it runs, the reading end.
pub struct Opened {
    pub state: SystemAudio,
    pub source: Option<Source>,
}

impl Opened {
    pub fn off() -> Self {
        Self { state: SystemAudio::Off, source: None }
    }
}

/// The backend that captures System Audio on this platform: native WASAPI on Windows (in a build
/// with it), whatever backend serves the session; none elsewhere.
pub fn loopback_backend() -> Option<Box<dyn ab::Backend>> {
    #[cfg(windows)]
    {
        Some(Box::new(transit_audio_wasapi::WasapiBackend::default()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Whether this build can capture System Audio (Windows with the native WASAPI backend).
pub fn offered() -> bool {
    cfg!(windows)
}

/// Whether `cfg` asks for System Audio and this build can capture it: the input then reserves the pair.
pub fn requested(cfg: &AudioConfig) -> bool {
    cfg.system_audio && offered()
}

/// The render endpoints System Audio can capture, by name, the default first (a control-thread
/// call that enumerates devices).
pub fn outputs() -> Vec<String> {
    loopback_backend().and_then(|b| b.loopback_endpoints().ok()).map_or_else(Vec::new, |list| list.into_iter().map(|e| e.name).collect())
}

/// Open System Audio as `cfg` asks (off unless `cfg.system_audio`), through the platform's
/// loopback backend.
pub fn open(cfg: &AudioConfig) -> Opened {
    if !cfg.system_audio {
        return Opened::off();
    }
    match loopback_backend() {
        Some(backend) => open_on(&*backend, cfg.system_audio_output.as_deref()),
        None => Opened { state: SystemAudio::Refused("System Audio needs the native WASAPI backend (Windows)".into()), source: None },
    }
}

/// Open and start a loopback capture of `endpoint` (else the default output) on `backend`. A
/// failure is the [`SystemAudio::Refused`] state, never an error of the session's input.
pub fn open_on(backend: &dyn ab::Backend, endpoint: Option<&str>) -> Opened {
    match try_open(backend, endpoint) {
        Ok((running, source)) => Opened { state: SystemAudio::Running(Box::new(running)), source: Some(source) },
        Err(why) => {
            log::warn!("System Audio did not open: {why}");
            Opened { state: SystemAudio::Refused(why.into()), source: None }
        }
    }
}

fn try_open(backend: &dyn ab::Backend, endpoint: Option<&str>) -> Result<(Running, Source), String> {
    let status = Arc::new(Status::default());
    // The ring is sized before the open knows the rate: for the largest common mix rate.
    let (producer, queue) = rtrb::RingBuffer::new(ring_samples(192_000));
    let (stamp_producer, stamps) = rtrb::RingBuffer::new(STAMPS);
    let feed = Feed::new(producer, stamp_producer, Arc::clone(&status));
    let errors = Arc::clone(&status);
    let on_error: ab::ErrorCallback = Box::new(move |_| errors.stopped.store(true, Ordering::Relaxed));
    let (stream, achieved) = backend.open_loopback(endpoint, feed.callback(), on_error).map_err(|e| e.to_string())?;
    stream.start().map_err(|e| e.to_string())?;
    let name = endpoint.map_or_else(|| "the default output".to_owned(), str::to_owned);
    log::info!("System Audio: capturing {name} at {:?}", achieved.format);
    let rate = achieved.format.rate.max(1);
    let running = Running { stream, status: Arc::clone(&status), endpoint: name, rate, latency: achieved.latency };
    Ok((running, Source { queue, stamps, status, rate }))
}

/// Interleaved stereo samples in a ring of [`RING_MS`] at `rate`.
fn ring_samples(rate: u32) -> usize {
    (rate as usize * RING_MS as usize / 1000).max(PIECE) * 2
}

/// The ring's margin ([`TARGET_MS`]) in frames at `rate`.
pub fn target_frames(rate: u32) -> usize {
    (rate as usize * TARGET_MS as usize / 1000).max(1)
}

/// The loopback packet assumed before one arrives ([`PACKET_MS`]), in frames at `rate`.
fn packet_frames(rate: u32) -> usize {
    (rate as usize * PACKET_MS as usize / 1000).max(1)
}

/// The input path's latency: the loopback stream's own (where it reports one) plus the ring's
/// target at the endpoint's rate. `target` is the reader's target in frames; 0 (the reader has not
/// seen a callback yet) counts the margin and one packet.
pub fn path_latency(stream: Option<Duration>, rate: u32, target: usize) -> Duration {
    let target = if target == 0 { target_frames(rate) + packet_frames(rate) } else { target };
    let ring = Duration::from_nanos(target as u64 * 1_000_000_000 / u64::from(rate.max(1)));
    stream.unwrap_or_default() + ring
}

impl SystemAudio {
    /// What Audio Devices shows, with the input device's rate; reading it resets the peaks.
    pub fn report(&self, device_rate: u32) -> Report {
        match self {
            SystemAudio::Off => Report { state: "off", ..Report::default() },
            SystemAudio::Pending(_) => Report { state: "pending", text: "opens once the output runs".into(), ..Report::default() },
            SystemAudio::Refused(why) => Report { state: "refused", text: why.to_string(), ..Report::default() },
            SystemAudio::Running(r) => {
                let s = &r.status;
                let stopped = s.stopped.load(Ordering::Relaxed);
                let peak = [0, 1].map(|i| f32::from_bits(s.peak[i].swap(0, Ordering::Relaxed)));
                Report {
                    state: if stopped { "stopped" } else { "running" },
                    text: if stopped { format!("{} stopped sending (the output went away)", r.endpoint) } else { r.endpoint.clone() },
                    rate: r.rate,
                    device_rate,
                    latency_ms: Some(path_latency(r.latency, r.rate, s.target_frames.load(Ordering::Relaxed) as usize).as_secs_f64() * 1000.0),
                    drift_ppm: f64::from(s.drift_deci_ppm.load(Ordering::Relaxed)) / 10.0,
                    underrun_frames: s.underrun.load(Ordering::Relaxed),
                    overflow_frames: s.overflow.load(Ordering::Relaxed),
                    discarded_frames: s.discarded.load(Ordering::Relaxed),
                    peak,
                }
            }
        }
    }

    pub fn running(&self) -> bool {
        matches!(self, SystemAudio::Running(_))
    }

    /// Control thread, once, after the session's output started: hand the opened loopback to the
    /// input's tap. A refusal (or a second call) leaves the reserved pair silent and says why.
    pub fn attach(&mut self, opened: Opened) {
        let SystemAudio::Pending(mut slot) = std::mem::take(self) else {
            *self = SystemAudio::Refused("System Audio was not reserved on this input".into());
            return;
        };
        *self = match (opened.state, opened.source) {
            (SystemAudio::Running(running), Some(source)) => match slot.push(source) {
                Ok(()) => SystemAudio::Running(running),
                Err(_) => SystemAudio::Refused("the input did not take the System Audio capture".into()),
            },
            (SystemAudio::Refused(why), _) => SystemAudio::Refused(why),
            _ => SystemAudio::Refused("System Audio did not open".into()),
        };
    }
}

/// Reserve the System Audio pair on an input of `device_channels` at `device_rate`: the pending
/// state (its end of the handoff) and the tap for the input's callback.
pub fn reserve(device_channels: usize, device_rate: u32) -> (SystemAudio, Tap) {
    let (slot, incoming) = rtrb::RingBuffer::new(1);
    let device_channels = device_channels.max(1);
    let tap = Tap {
        incoming,
        source: None,
        resampler: Resampler::new(device_rate, device_rate),
        device_rate,
        device_channels,
        origin: std::time::Instant::now(),
        stereo: vec![0.0; PIECE * 2].into_boxed_slice(),
        wide: vec![0.0; PIECE * (device_channels + 2)].into_boxed_slice(),
    };
    (SystemAudio::Pending(Box::new(slot)), tap)
}

/// The pair a frame of the endpoint's mix contributes: the front pair, a mono frame on both sides,
/// silence for none. Other channels of a surround mix are left out.
pub fn fold_to_stereo(frame: &[f32]) -> (f32, f32) {
    match frame {
        [] => (0.0, 0.0),
        [x] => (*x, *x),
        [l, r, ..] => (*l, *r),
    }
}

/// The writing end, in the loopback stream's callback (its own thread, not the session's): fold,
/// queue, count, and note each packet's clock observation for the rate estimate. No allocation, no
/// lock.
struct Feed {
    producer: rtrb::Producer<f32>,
    stamps: rtrb::Producer<rate::Obs>,
    status: Arc<Status>,
    /// The endpoint's frame index of the next packet's first frame.
    next_frame: u64,
    /// The origin of the arrival times, for a packet whose stamp is invalid.
    origin: std::time::Instant,
}

impl Feed {
    fn new(producer: rtrb::Producer<f32>, stamps: rtrb::Producer<rate::Obs>, status: Arc<Status>) -> Self {
        Self { producer, stamps, status, next_frame: 0, origin: std::time::Instant::now() }
    }

    fn callback(mut self) -> ab::CaptureCallback {
        Box::new(move |block: &mut ab::CaptureBlock<'_>| self.packet(block))
    }

    /// One loopback packet: its observation first (the frame index of its first frame and the
    /// QPC time WASAPI gives for it, else the arrival now; a full queue drops it, and the next one
    /// carries the index on), then its samples.
    fn packet(&mut self, block: &mut ab::CaptureBlock<'_>) {
        let frames = u32::try_from(block.frames).unwrap_or(u32::MAX);
        if let Some((kind, domain, time_ns)) = packet_time(block.stamp, self.origin) {
            let _ = self.stamps.push(rate::Obs { first_frame: self.next_frame, frames, time_ns, kind, domain });
        }
        self.next_frame += u64::from(frames);
        let channels = usize::from(block.channels).max(1);
        let (producer, status) = (&mut self.producer, &*self.status);
        block.chunks(&mut |_, samples| push_frames(producer, status, samples, channels));
    }
}

fn push_frames(producer: &mut rtrb::Producer<f32>, status: &Status, samples: &[f32], channels: usize) {
    let mut lost = 0u64;
    for frame in samples.chunks_exact(channels) {
        let (l, r) = fold_to_stereo(frame);
        let (l, r) = (if l.is_finite() { l } else { 0.0 }, if r.is_finite() { r } else { 0.0 });
        status.note_peak(0, l);
        status.note_peak(1, r);
        if producer.slots() >= 2 {
            let _ = producer.push(l);
            let _ = producer.push(r);
        } else {
            lost += 1;
        }
    }
    if lost > 0 && status.reading.load(Ordering::Relaxed) {
        status.overflow.fetch_add(lost, Ordering::Relaxed);
    }
}

/// The endpoint-rate to device-rate converter of the reader: the band-limited reader (ADR 0056
/// decision 8, #603) at a step of the nominal ratio trimmed by the measured drift plus a slow
/// correction that holds the ring at its target. Audio-thread code: [`Self::begin`] once per input
/// callback (the clocks, the target, priming and the trim), [`Self::pull`] for each piece of it.
///
/// **Positions.** The step changes with the trim at every callback, so the position is formed as
/// `x0 + n·q` from an anchor, never by adding `q` once per output frame: `x0` is an integer frame
/// (from the ring's front) plus a fraction, `n` counts the output frames since the anchor. At every
/// [`Self::begin`] the anchor moves to the next output frame's position, computed with the old
/// anchor and step (`x0 := x0 + n·q_old`, its integer part and fraction split exactly, `n := 0`), and
/// then the new step applies: the next output frame is read exactly where the old step would have
/// read it, so a trim change bends the position's slope and never moves the position (no click).
/// Committing played frames from the ring's front moves the anchor's integer part by the same whole
/// frames. At equal nominal rates with a zero trim the step is exactly 1 and every position a whole
/// frame, so the frames pass through bit for bit.
///
/// **Look-ahead.** The reader needs `halo(q)` frames past its position, which lie inside the target
/// fill (the 30 ms margin alone is 1,440 frames at 48 kHz): the fill the servo holds is counted from
/// the read position, so the path's latency is the target, as before, and the reader's look-ahead
/// only narrows the margin before the ring runs dry by those frames. The frames behind the position
/// that the taps reach stay in the ring until they are no longer needed.
///
/// The trim has three terms:
/// - **The rate estimate** ([`rate::RateEstimate`], fed forward): the drift of the endpoint's
///   clock against the device's from frame counts and timestamps. It carries the whole
///   steady-state correction and is the drift Audio Devices reports. It does not depend on how
///   packets and callbacks line up.
/// - **The proportional term** ([`KP_PPM`]) on the fill error (the averaged fill at callback start
///   less the target, over the 30 ms margin): it pulls the fill back to its target after the start,
///   after a step in the clock (while the estimate's window catches up) and after a re-prime. Its
///   gain is low on purpose: the fill seen at callback instants carries the packet sawtooth
///   (aliased to a slow beat when the callback and the packet line up), and a low gain keeps that
///   out of the trim.
/// - **The integral term** ([`KI_PPM_PER_S`], held within [`INTEGRAL_PPM`]): what the estimate
///   leaves over time (its bias), so the fill settles on its target and not beside it.
pub(crate) struct Resampler {
    /// Endpoint frames per device frame at the nominal rates.
    nominal: f64,
    device_rate: f64,
    /// The anchor: the whole frame from the ring's front and the fraction in `[0, 1)`.
    anchor: i64,
    fraction: f64,
    /// Output frames read since the anchor.
    n: u64,
    /// Frames kept in the ring behind the read position for the reader's taps: `halo(q)` at the
    /// largest step the trim allows.
    behind: usize,
    /// Frames committed from the ring's front since the last retune (with the anchor, the position
    /// from the capture's first kept frame; the tests read it).
    consumed: u64,
    table: &'static band_limited::SincTable,
    /// Reading: the ring reached its target. Until then (and after the ring runs dry) the reader
    /// outputs zeros and waits for the target again at the next callback.
    primed: bool,
    /// The reader has primed once: zeros from a dry ring count from then on.
    started: bool,
    /// The margin of [`TARGET_MS`] in endpoint frames, and the largest loopback packet and input
    /// callback seen, in endpoint frames.
    base: usize,
    packet: usize,
    callback: usize,
    /// The target fill in endpoint frames, their sum: one input callback plus one loopback packet
    /// plus the 30 ms margin, so a callback of any size finds what it reads already queued.
    target: usize,
    /// The fill at callback start, averaged over about [`FILL_TAU_SECONDS`].
    fill: f64,
    integral: f64,
    /// The applied trim, relative (the three terms' sum, within [`MAX_TRIM_PPM`]).
    pub(crate) trim: f64,
    step: f64,
    /// Device frames read so far (the device clock's frame index).
    device_frames: u64,
    rate: rate::RateEstimate,
}

/// The ring's stereo frames as the reader sees them: the front frame at index 0, zeros outside.
struct Pairs<'a> {
    first: &'a [f32],
    second: &'a [f32],
    frames: usize,
}

impl band_limited::Frames for Pairs<'_> {
    fn channels(&self) -> usize {
        2
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        let Ok(k) = usize::try_from(frame) else { return 0.0 };
        if k >= self.frames || channel >= 2 {
            return 0.0;
        }
        let i = 2 * k + channel;
        match self.first.get(i) {
            Some(x) => *x,
            None => self.second.get(i - self.first.len()).copied().unwrap_or(0.0),
        }
    }
}

impl Resampler {
    /// Allocates nothing itself; takes the reader's shared table (built on its first use, on the
    /// control thread that reserves the pair).
    pub(crate) fn new(source_rate: u32, device_rate: u32) -> Self {
        let mut r = Self {
            nominal: 1.0,
            device_rate: 1.0,
            anchor: 0,
            fraction: 0.0,
            n: 0,
            behind: 0,
            consumed: 0,
            table: band_limited::table(),
            primed: false,
            started: false,
            base: 1,
            packet: 1,
            callback: 0,
            target: 1,
            fill: 0.0,
            integral: 0.0,
            trim: 0.0,
            step: 1.0,
            device_frames: 0,
            rate: rate::RateEstimate::new(1.0, 1.0),
        };
        r.retune(source_rate, device_rate);
        r
    }

    /// Start again for a capture at `source_rate`, in place (the tap does this on the audio
    /// thread when the capture arrives: no allocation, no large temporary).
    pub(crate) fn retune(&mut self, source_rate: u32, device_rate: u32) {
        let (src, dst) = (f64::from(source_rate.max(1)), f64::from(device_rate.max(1)));
        (self.nominal, self.device_rate) = (src / dst, dst);
        (self.anchor, self.fraction, self.n, self.consumed, self.primed, self.started) = (0, 0.0, 0, 0, false, false);
        self.behind = band_limited::halo(self.nominal * (1.0 + MAX_TRIM_PPM * 1e-6));
        (self.base, self.packet, self.callback) = (target_frames(source_rate), packet_frames(source_rate), 0);
        self.target = self.base + self.packet;
        (self.fill, self.integral, self.trim, self.step, self.device_frames) = (0.0, 0.0, 0.0, self.nominal, 0);
        self.rate.restart(src, dst);
    }

    /// The next output frame's position from the ring's front: `x0 + n·q`.
    fn position(&self) -> f64 {
        self.anchor as f64 + (self.fraction + self.n as f64 * self.step)
    }

    /// Move the anchor to the next output frame's position at the current step (exactly the
    /// position the current anchor gives it), with `n` back to 0.
    fn reanchor(&mut self) {
        let v = self.fraction + self.n as f64 * self.step;
        let whole = v.floor();
        // `v - floor(v)` is exact; the anchor's integer part takes the whole frames.
        (self.anchor, self.fraction, self.n) = (self.anchor + whole as i64, v - whole, 0);
    }

    /// The step changes to `step` at the next output frame, with the position carried on (see the
    /// type's notes).
    fn set_step(&mut self, step: f64) {
        self.reanchor();
        self.step = step;
    }

    /// Commit `frames` frames from the ring's front (played or discarded); the anchor follows.
    fn commit(&mut self, queue: &mut rtrb::Consumer<f32>, frames: usize) {
        if frames > 0
            && let Ok(chunk) = queue.read_chunk(frames * 2)
        {
            chunk.commit_all();
        }
        self.anchor -= frames as i64;
        self.consumed += frames as u64;
    }

    /// Queued frames behind the read position (played, kept for the taps).
    fn played(&self, queued: usize) -> usize {
        usize::try_from(self.position().floor() as i64).unwrap_or(0).min(queued)
    }

    /// Restart reading at the frame `keep` frames before the newest: discard the rest from the
    /// front and put the anchor on the new front. Returns the frames discarded unplayed.
    fn restart(&mut self, queue: &mut rtrb::Consumer<f32>, keep: usize) -> usize {
        let queued = queue.slots() / 2;
        let excess = queued.saturating_sub(keep);
        let played = self.played(queued).min(excess);
        self.commit(queue, excess);
        (self.anchor, self.fraction, self.n) = (0, 0.0, 0);
        excess - played
    }

    /// Once per input callback of `frames` device frames whose first frame is at `now` (its kind
    /// and time; `None` when no time could be read): take the loopback's packet observations and
    /// the callback's into the rate estimate, size the target, prime or resynchronize, and set the
    /// trim for the callback's pieces. Never waits.
    pub(crate) fn begin(&mut self, source: &mut Source, frames: usize, now: Option<(rate::Kind, rate::Domain, i64)>) {
        let max_packet = self.base * 100 / TARGET_MS as usize;
        while let Ok(obs) = source.stamps.pop() {
            self.packet = self.packet.max((obs.frames as usize).min(max_packet));
            self.rate.endpoint(obs);
        }
        if let Some((kind, domain, time)) = now {
            self.rate.device(self.device_frames, time, kind, domain);
        }
        self.device_frames += frames as u64;
        self.callback = self.callback.max((frames as f64 * self.nominal).ceil() as usize);
        self.target = self.base + self.callback + self.packet;
        let status = &*source.status;
        status.target_frames.store(u32::try_from(self.target).unwrap_or(u32::MAX), Ordering::Relaxed);
        let estimate = self.rate.drift();
        let queue = &mut source.queue;
        let queued = queue.slots() / 2;
        if !self.primed {
            if queued < self.target {
                return;
            }
            status.discarded.fetch_add(self.restart(queue, self.target) as u64, Ordering::Relaxed);
            (self.fill, self.primed, self.started) = (self.target as f64, true, true);
        } else if queued > (self.target * RESYNC_TARGETS).min(queue.buffer().capacity() / 2 * 3 / 4) {
            status.discarded.fetch_add(self.restart(queue, self.target) as u64, Ordering::Relaxed);
        }
        // The fill is counted from the read position: the frames behind it, kept for the taps, are
        // played.
        let ahead = (queue.slots() / 2) as f64 - self.position();
        let weight = (frames as f64 / (FILL_TAU_SECONDS * self.device_rate)).min(1.0);
        self.fill += weight * (ahead - self.fill);
        let error = (self.fill - self.target as f64) / self.base as f64;
        let held = INTEGRAL_PPM * 1e-6;
        self.integral = (self.integral + error * KI_PPM_PER_S * 1e-6 * frames as f64 / self.device_rate).clamp(-held, held);
        let max = MAX_TRIM_PPM * 1e-6;
        let fed = estimate.unwrap_or(0.0).clamp(-max, max);
        self.trim = (fed + error * KP_PPM * 1e-6 + self.integral).clamp(-max, max);
        status.drift_deci_ppm.store(reported_deci_ppm(estimate, self.integral), Ordering::Relaxed);
        self.set_step(self.nominal * (1.0 + self.trim));
    }

    /// Fill `out` (interleaved stereo at the device rate) from the ring at the step
    /// [`Self::begin`] set. Never waits: what the ring does not hold is zeros. A frame whose taps
    /// are not all queued yet makes the ring dry: it and the rest of the piece are zeros, counted,
    /// and the reader waits for the target again.
    pub(crate) fn pull(&mut self, queue: &mut rtrb::Consumer<f32>, out: &mut [f32], status: &Status) {
        out.iter_mut().for_each(|x| *x = 0.0);
        let frames = out.len() / 2;
        if !self.primed {
            if self.started {
                status.underrun.fetch_add(frames as u64, Ordering::Relaxed);
            }
            return;
        }
        let queued = queue.slots() / 2;
        {
            let Ok(chunk) = queue.read_chunk(queued * 2) else { return };
            let (first, second) = chunk.as_slices();
            let source = Pairs { first, second, frames: queued };
            for (k, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let x = self.position();
                if band_limited::span(x, self.step).end > queued as i64 {
                    self.primed = false;
                    status.underrun.fetch_add((frames - k) as u64, Ordering::Relaxed);
                    break;
                }
                band_limited::frame(self.table, &source, x, self.step, frame);
                self.n += 1;
            }
        }
        // Frames more than `behind` before the next position are never read again.
        let next = self.position().floor() as i64 - self.behind as i64;
        let done = usize::try_from(next).unwrap_or(0).min(queued);
        self.commit(queue, done);
    }

    /// The next output frame's position from the first frame the reader kept since the capture
    /// arrived (the frames before priming were discarded): the tests follow it across callbacks.
    #[cfg(test)]
    pub(crate) fn absolute_position(&self) -> f64 {
        self.consumed as f64 + self.position()
    }
}

/// The drift Audio Devices shows, in tenths of a ppm: the rate estimate clamped to
/// [`MAX_TRIM_PPM`] (the figure fed forward into the trim), or the fill's integral term before the
/// estimate is valid.
fn reported_deci_ppm(estimate: Option<f64>, integral: f64) -> i32 {
    let max = MAX_TRIM_PPM * 1e-6;
    let reported = estimate.map_or(integral, |d| d.clamp(-max, max)) * 1e7;
    reported.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// The reading end in the input stream's callback: appends the System Audio pair to each frame of
/// the device's input, zeros until the capture arrives through its one-slot queue. Its scratch is
/// allocated in [`reserve`], on the control thread; [`Tap::begin`] and [`Tap::widen`] allocate
/// nothing (taking the capture moves it out of the queue's preallocated slot), take no lock and
/// never wait.
pub struct Tap {
    incoming: rtrb::Consumer<Source>,
    /// The capture once it arrived; kept for the tap's life, so nothing is freed on the audio
    /// thread.
    source: Option<Source>,
    resampler: Resampler,
    device_rate: u32,
    device_channels: usize,
    /// The origin of the arrival times, for a callback whose stamp is invalid.
    origin: std::time::Instant,
    stereo: Box<[f32]>,
    wide: Box<[f32]>,
}

impl Tap {
    /// The input's width with the pair appended.
    pub fn channels(&self) -> usize {
        self.device_channels + 2
    }

    /// Once per input callback, before [`Self::widen`]: the callback's frame count and stamp. Its
    /// host callback time (the QPC when the block arrived on any backend, else the reader's own
    /// arrival clock) feeds the rate estimate; then the target, priming and the trim are set.
    pub fn begin(&mut self, frames: usize, stamp: ab::Stamp) {
        if self.source.is_none()
            && let Ok(source) = self.incoming.pop()
        {
            self.resampler.retune(source.rate, self.device_rate);
            source.status.reading.store(true, Ordering::Relaxed);
            self.source = Some(source);
        }
        let Some(source) = self.source.as_mut() else { return };
        let now = callback_time(stamp, self.origin);
        self.resampler.begin(source, frames, now);
    }

    /// Pass the device's interleaved `samples` on to `each` with the pair appended to every frame,
    /// in pieces, each with its frame offset into `samples`. A trailing partial frame is dropped.
    pub fn widen(&mut self, samples: &[f32], each: &mut dyn FnMut(usize, &[f32])) {
        let (ch, wide_ch) = (self.device_channels, self.device_channels + 2);
        let frames = samples.len() / ch;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(PIECE);
            let stereo = &mut self.stereo[..n * 2];
            match self.source.as_mut() {
                Some(source) => self.resampler.pull(&mut source.queue, stereo, &source.status),
                None => stereo.iter_mut().for_each(|x| *x = 0.0),
            }
            let wide = &mut self.wide[..n * wide_ch];
            for ((out, input), pair) in wide.chunks_exact_mut(wide_ch).zip(samples[done * ch..].chunks_exact(ch)).zip(stereo.as_chunks::<2>().0) {
                out[..ch].copy_from_slice(input);
                out[ch..].copy_from_slice(pair);
            }
            each(done, wide);
            done += n;
        }
    }
}

/// The time of a loopback packet's first frame for the rate estimate: the stamp's frame time where
/// it is valid (on native WASAPI the QPC `GetBuffer` returns for it), else the arrival now on
/// `origin` (one `Instant::now`, QPC on Windows: no allocation). Both are QPC time.
fn packet_time(stamp: ab::Stamp, origin: std::time::Instant) -> Option<(rate::Kind, rate::Domain, i64)> {
    if stamp.quality != ab::Quality::Invalid {
        return Some((rate::Kind::Position, rate::Domain::Qpc, stamp.backend_frame_ns));
    }
    arrival(origin)
}

/// The time of an input callback for the rate estimate: the stamp's host callback time on every
/// backend, which is the QPC read when the block arrived. Not `backend_frame_ns`: on native ASIO
/// that is the driver's sample position at the nominal rate, so its slope is always nominal and
/// says nothing about the interface's clock. Else the arrival now on `origin` (QPC on Windows).
fn callback_time(stamp: ab::Stamp, origin: std::time::Instant) -> Option<(rate::Kind, rate::Domain, i64)> {
    if stamp.quality != ab::Quality::Invalid
        && let Ok(host) = i64::try_from(stamp.host_callback_ns)
    {
        return Some((rate::Kind::Host, rate::Domain::Qpc, host));
    }
    arrival(origin)
}

fn arrival(origin: std::time::Instant) -> Option<(rate::Kind, rate::Domain, i64)> {
    i64::try_from(origin.elapsed().as_nanos()).ok().map(|t| (rate::Kind::Arrival, rate::Domain::Qpc, t))
}

/// The name Audio Devices gives a Linux input that is the monitor source of an output (what that
/// output plays): "System Audio (monitor of <output>)". It recognizes PulseAudio's and PipeWire's
/// description ("Monitor of Built-in Audio"), the source name (`alsa_output.<card>.monitor`) and a
/// PipeWire JACK port group whose capture ports are all `monitor_*` (a sink's monitor ports).
/// `None` for anything else.
pub fn monitor_label(device: &str, channels: &[String]) -> Option<String> {
    let name = device.trim();
    let output = if let Some(rest) = name.strip_prefix("Monitor of ") {
        rest.trim().to_owned()
    } else if let Some(rest) = name.strip_suffix(".monitor") {
        rest.strip_prefix("alsa_output.").unwrap_or(rest).to_owned()
    } else if !channels.is_empty() && channels.iter().all(|c| c.starts_with("monitor_")) {
        name.to_owned()
    } else {
        return None;
    };
    (!output.is_empty()).then(|| format!("System Audio (monitor of {output})"))
}

#[path = "system_audio_rate.rs"]
pub(crate) mod rate;

#[cfg(test)]
#[path = "system_audio_tests.rs"]
mod tests;
