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
//! by linear interpolation, with the ratio trimmed by a PI controller (at most [`MAX_PPM`]) to hold
//! the ring at a target fill, and reports the integral term (the endpoint's drift) as the drift in the diagnostics. The input converter then
//! takes the widened input to the session rate as it does any input. The callback never waits on
//! the loopback client: an empty ring reads as zeros (a loopback stream sends nothing while nothing
//! plays on some drivers), a full one drops and counts. The capture has no timeline relation to
//! the session: its latency (the loopback stream's plus the ring's target) is reported, not
//! compensated.
//!
//! Linux delivers monitor sources through the JACK and ALSA paths already; [`monitor_label`]
//! names them in Audio Devices. macOS needs a virtual device (#24).

use crate::devices::AudioConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use transit_audio_backend as ab;

/// The name of the input path (the track input picker's entry and the route's name).
pub const PATH_NAME: &str = transit_model::SYSTEM_AUDIO_PATH;
/// The ring's target fill, in milliseconds of the endpoint's rate: about three loopback periods.
pub const TARGET_MS: u32 = 30;
/// The ring's capacity, in milliseconds.
const RING_MS: u32 = 500;
/// The largest trim of the rate ratio, in parts per million (two crystals differ by well under 100).
pub const MAX_PPM: f64 = 1_000.0;
/// The proportional gain of the rate controller, in ppm of trim per unit of relative fill error
/// (a ring 10% over its target adds 400 ppm). Together with the integral gain it is derived from
/// the plant: the fill error changes at `rate / target` (about 33 per second at 48 kHz and 30 ms)
/// times the ratio error, so the loop is `s^2 + 33 Kp s + 33 Ki = 0`. `Kp` = 4000 ppm gives
/// `2 wn` = 0.133 per second and `Ki` = 150 ppm per second per unit error gives `wn^2` = 0.005,
/// so `wn` = 0.071 per second and the damping is 0.94: a step of 900 ppm in the endpoint's rate
/// moves the fill by 16% of the target at most and settles in about a minute.
const KP_PPM: f64 = 4_000.0;
/// The integral gain, in ppm per second per unit of relative fill error. The integral term holds
/// the steady-state ratio, so the fill settles at its target at any drift inside `MAX_PPM`.
const KI_PPM_PER_S: f64 = 150.0;
/// A ring this many targets full is resynchronized (the excess discarded), not trimmed down.
const RESYNC_TARGETS: usize = 4;
/// The time constant of the fill average, in seconds.
const FILL_TAU_SECONDS: f64 = 0.5;
/// Frames the tap widens at a time (its scratch is this many frames).
const PIECE: usize = 256;

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
    /// The rate trim, in tenths of a ppm: positive when the endpoint's clock runs fast.
    drift_deci_ppm: AtomicI32,
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

/// The reading end of a loopback capture, handed to the input stream's callback through a [`Tap`].
pub struct Source {
    queue: rtrb::Consumer<f32>,
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
    #[cfg(all(windows, feature = "native-wasapi"))]
    {
        Some(Box::new(transit_audio_wasapi::WasapiBackend::default()))
    }
    #[cfg(not(all(windows, feature = "native-wasapi")))]
    {
        None
    }
}

/// Whether this build can capture System Audio (Windows with the native WASAPI backend).
pub fn offered() -> bool {
    cfg!(all(windows, feature = "native-wasapi"))
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
    let feed = Feed { producer, status: Arc::clone(&status) };
    let errors = Arc::clone(&status);
    let on_error: ab::ErrorCallback = Box::new(move |_| errors.stopped.store(true, Ordering::Relaxed));
    let (stream, achieved) = backend.open_loopback(endpoint, feed.callback(), on_error).map_err(|e| e.to_string())?;
    stream.start().map_err(|e| e.to_string())?;
    let name = endpoint.map_or_else(|| "the default output".to_owned(), str::to_owned);
    log::info!("System Audio: capturing {name} at {:?}", achieved.format);
    let rate = achieved.format.rate.max(1);
    let running = Running { stream, status: Arc::clone(&status), endpoint: name, rate, latency: achieved.latency };
    Ok((running, Source { queue, status, rate }))
}

/// Interleaved stereo samples in a ring of [`RING_MS`] at `rate`.
fn ring_samples(rate: u32) -> usize {
    (rate as usize * RING_MS as usize / 1000).max(PIECE) * 2
}

/// The target fill in frames at `rate`.
pub fn target_frames(rate: u32) -> usize {
    (rate as usize * TARGET_MS as usize / 1000).max(1)
}

/// The input path's latency: the loopback stream's own (where it reports one) plus the ring's
/// target at the endpoint's rate.
pub fn path_latency(stream: Option<Duration>, rate: u32) -> Duration {
    let ring = Duration::from_nanos(target_frames(rate) as u64 * 1_000_000_000 / u64::from(rate.max(1)));
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
                    latency_ms: Some(path_latency(r.latency, r.rate).as_secs_f64() * 1000.0),
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
/// queue, count. No allocation, no lock.
struct Feed {
    producer: rtrb::Producer<f32>,
    status: Arc<Status>,
}

impl Feed {
    fn callback(mut self) -> ab::CaptureCallback {
        Box::new(move |block: &mut ab::CaptureBlock<'_>| {
            let channels = usize::from(block.channels).max(1);
            let (producer, status) = (&mut self.producer, &*self.status);
            block.chunks(&mut |_, samples| push_frames(producer, status, samples, channels));
        })
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

/// The endpoint-rate to device-rate converter of the reader: linear interpolation between two held
/// frames, with the ratio trimmed to hold the ring at its target fill. Audio-thread code.
pub(crate) struct Resampler {
    /// Endpoint frames per device frame at the nominal rates.
    nominal: f64,
    /// The integral term: the ratio the endpoint's drift needs, relative (`MAX_PPM` at most
    /// either way). It is the drift reported once settled.
    integral: f64,
    phase: f64,
    a: [f32; 2],
    b: [f32; 2],
    /// Reading: the ring reached its target and both frames are held. Until then (and after the
    /// ring runs dry) the reader outputs zeros and waits for the target again.
    primed: bool,
    /// The reader has primed once: zeros from a dry ring count from then on.
    started: bool,
    target: usize,
    fill: f64,
    /// The weight of one device frame in the fill average.
    per_frame: f64,
    /// Seconds per device frame.
    seconds: f64,
}

impl Resampler {
    pub(crate) fn new(source_rate: u32, device_rate: u32) -> Self {
        let (src, dst) = (f64::from(source_rate.max(1)), f64::from(device_rate.max(1)));
        Self {
            nominal: src / dst,
            integral: 0.0,
            phase: 0.0,
            a: [0.0; 2],
            b: [0.0; 2],
            primed: false,
            started: false,
            target: target_frames(source_rate),
            fill: 0.0,
            per_frame: 1.0 / (FILL_TAU_SECONDS * dst),
            seconds: 1.0 / dst,
        }
    }

    fn pop(queue: &mut rtrb::Consumer<f32>) -> Option<[f32; 2]> {
        if queue.slots() < 2 {
            return None;
        }
        Some([queue.pop().unwrap_or(0.0), queue.pop().unwrap_or(0.0)])
    }

    /// Discard queued frames down to `keep`. One commit.
    fn discard_to(queue: &mut rtrb::Consumer<f32>, keep: usize) -> usize {
        let excess = (queue.slots() / 2).saturating_sub(keep);
        if excess > 0
            && let Ok(chunk) = queue.read_chunk(excess * 2)
        {
            chunk.commit_all();
        }
        excess
    }

    /// Fill `out` (interleaved stereo at the device rate) from `queue`. Never waits: what the ring
    /// does not hold is zeros.
    pub(crate) fn pull(&mut self, queue: &mut rtrb::Consumer<f32>, out: &mut [f32], status: &Status) {
        out.iter_mut().for_each(|x| *x = 0.0);
        let frames = out.len() / 2;
        let queued = queue.slots() / 2;
        if !self.primed {
            if queued < self.target {
                if self.started {
                    status.underrun.fetch_add(frames as u64, Ordering::Relaxed);
                }
                return;
            }
            status.discarded.fetch_add(Self::discard_to(queue, self.target) as u64, Ordering::Relaxed);
            let (Some(a), Some(b)) = (Self::pop(queue), Self::pop(queue)) else { return };
            (self.a, self.b, self.phase, self.fill, self.primed, self.started) = (a, b, 0.0, self.target as f64, true, true);
        } else if queued > self.target * RESYNC_TARGETS {
            status.discarded.fetch_add(Self::discard_to(queue, self.target) as u64, Ordering::Relaxed);
        }
        // The fill average over about FILL_TAU_SECONDS, and the trim it asks for.
        let weight = (frames as f64 * self.per_frame).min(1.0);
        self.fill += weight * ((queue.slots() / 2) as f64 - self.fill);
        let error = (self.fill - self.target as f64) / self.target as f64;
        // PI controller. Anti-windup: the integral term itself is held inside the clamp, so it
        // can never wind past what the output can use (the output is clamped the same way). It
        // keeps integrating while the sum saturates, so a drift at the clamp is still reported.
        let max = MAX_PPM * 1e-6;
        let proportional = error * KP_PPM * 1e-6;
        self.integral = (self.integral + error * KI_PPM_PER_S * 1e-6 * frames as f64 * self.seconds).clamp(-max, max);
        // The sum may reach twice the clamp: at a drift of exactly MAX_PPM the integral term is
        // spent, and the proportional term needs room beyond it to pull the fill back to target.
        let trim = (self.integral + proportional).clamp(-2.0 * max, 2.0 * max);
        status.drift_deci_ppm.store((self.integral * 1e7).round() as i32, Ordering::Relaxed);
        let step = self.nominal * (1.0 + trim);
        for (n, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let p = self.phase as f32;
            frame[0] = self.a[0] + (self.b[0] - self.a[0]) * p;
            frame[1] = self.a[1] + (self.b[1] - self.a[1]) * p;
            self.phase += step;
            while self.phase >= 1.0 {
                self.a = self.b;
                match Self::pop(queue) {
                    Some(next) => self.b = next,
                    None => {
                        // Dry: the rest of the block is zeros, and the reader waits for the target.
                        self.primed = false;
                        let rest = frames - n - 1;
                        out[(n + 1) * 2..].iter_mut().for_each(|x| *x = 0.0);
                        status.underrun.fetch_add(rest as u64, Ordering::Relaxed);
                        return;
                    }
                }
                self.phase -= 1.0;
            }
        }
    }
}

/// The reading end in the input stream's callback: appends the System Audio pair to each frame of
/// the device's input, zeros until the capture arrives through its one-slot queue. Its scratch is
/// allocated in [`reserve`], on the control thread; [`Tap::widen`] allocates nothing (taking the
/// capture moves it out of the queue's preallocated slot), takes no lock and never waits.
pub struct Tap {
    incoming: rtrb::Consumer<Source>,
    /// The capture's ring and counters once it arrived; kept for the tap's life, so nothing is
    /// freed on the audio thread.
    source: Option<(rtrb::Consumer<f32>, Arc<Status>)>,
    resampler: Resampler,
    device_rate: u32,
    device_channels: usize,
    stereo: Box<[f32]>,
    wide: Box<[f32]>,
}

impl Tap {
    /// The input's width with the pair appended.
    pub fn channels(&self) -> usize {
        self.device_channels + 2
    }

    /// Pass the device's interleaved `samples` on to `each` with the pair appended to every frame,
    /// in pieces, each with its frame offset into `samples`. A trailing partial frame is dropped.
    pub fn widen(&mut self, samples: &[f32], each: &mut dyn FnMut(usize, &[f32])) {
        if self.source.is_none()
            && let Ok(source) = self.incoming.pop()
        {
            self.resampler = Resampler::new(source.rate, self.device_rate);
            source.status.reading.store(true, Ordering::Relaxed);
            self.source = Some((source.queue, source.status));
        }
        let (ch, wide_ch) = (self.device_channels, self.device_channels + 2);
        let frames = samples.len() / ch;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(PIECE);
            let stereo = &mut self.stereo[..n * 2];
            match self.source.as_mut() {
                Some((queue, status)) => self.resampler.pull(queue, stereo, status),
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

#[cfg(test)]
#[path = "system_audio_tests.rs"]
mod tests;
