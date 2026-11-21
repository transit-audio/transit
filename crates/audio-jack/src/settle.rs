//! How the first start settles the session's rate and buffer size (ADR 0021, amendment for #760,
//! rules 1 and 2), on the opening thread under the session lock, after `jack_activate` returned
//! and before either direction is started (so the engine skips both user callbacks, the output
//! ports carry silence and captured blocks are not delivered).
//!
//! - **Publication** ([`observe`]). The session's [`Shared`] figures are unknown until the client
//!   is active. Right after activation the control thread reads the active client's rate and
//!   buffer size and stores each only while it is still unknown ([`Shared::publish_active`]): a
//!   figure a callback published first (PipeWire's library calls the buffer size callback inside
//!   `jack_activate`) is never overwritten. The observation is then read back from `Shared`.
//! - **The settling window** ([`wait`], PipeWire only). Under PipeWire the figure read right after
//!   activation need not be what the graph runs: in the 2026-11-21 report every fresh client read
//!   2048 there and ran 1024 once its ports were linked. So the session links its ports first (the
//!   caller's part) and then waits, in steps of the watcher period through
//!   [`JackApi::await_graph`] on the API's clock ([`JackApi::now_us`]), for at most
//!   [`REQUEST_WINDOW`], until the graph runs the size it expects: a chosen buffer size that was
//!   not declined (a force on transit's own node that arrives after activation), else the size the
//!   graph ran when transit last observed it (the period hint, kept like rule 1's rate hint), else,
//!   with neither, the size of the first process cycle that began after the links were made.
//!   Whatever runs at readiness or at the window's end is the baseline. A server shutdown during
//!   the window aborts the start; a rate change ends it (the rate path below). Under JACK 2 there
//!   is no window: its read after activation is the server's one quantum.
//! - **The rate.** The settled rate is compared with the rate the caller prepared for; a
//!   difference latches a rebuild ([`Settled::rebuild`]) instead of starting either direction.
//! - **First-cycle initialization** ([`Init::FirstCycle`], rule 1's fallback, decided in the
//!   record): no getter before a process cycle ran; the cycle's `nframes` is the buffer, the rate
//!   is read after it; no cycle within the window fails the start visibly. Not selected in
//!   shipping code: the window above waits for a cycle after the links instead, which a node that
//!   the graph runs only once it is linked can give, and which never fails the start.

use crate::api::JackApi;
use crate::engine::{Shared, UNKNOWN};
use std::time::Duration;
use transit_audio_backend as ab;

/// The longest a start waits in the settling window (rule 2), and in first-cycle initialization
/// for the first process cycle.
pub(crate) const REQUEST_WINDOW: Duration = Duration::from_millis(500);

/// Where the session's first figures come from (rule 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Init {
    /// Read from the active client right after `jack_activate` returned.
    #[default]
    AfterActivation,
    /// Taken from the first process cycle (rule 1's fallback; not selected in shipping code).
    #[cfg_attr(not(test), allow(dead_code))]
    FirstCycle,
}

/// What the start asked for and was prepared for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ask {
    /// The library is PipeWire's (its version string names it).
    pub(crate) pipewire: bool,
    /// The buffer size the open requested, if any.
    pub(crate) requested: Option<u32>,
    /// The request returned an error (it will not arrive).
    pub(crate) declined: bool,
    /// The rate the Player and its recorder were built for.
    pub(crate) prepared_rate: u32,
    /// Under PipeWire, the buffer size the graph ran when transit last observed it (the period
    /// hint); `None` without one.
    pub(crate) hint: Option<u32>,
    pub(crate) init: Init,
}

impl Ask {
    /// The buffer size the settling window waits for: a request that was not declined, else the
    /// period hint; none under JACK 2.
    fn target(&self) -> Option<u32> {
        self.requested.filter(|_| !self.declined).or(self.hint).filter(|_| self.pipewire)
    }
}

/// The settled figures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) rate: u32,
    pub(crate) buffer: u32,
    /// The graph runs at a rate other than the prepared one: rebuild before any audio.
    pub(crate) rebuild: bool,
}

/// The waits of one start, on the API's clock: at most `window` in all, in steps of `step`.
pub(crate) struct Clock<'a> {
    api: &'a dyn JackApi,
    start: u64,
    limit: u64,
    step: Duration,
    steps_left: u64,
}

impl<'a> Clock<'a> {
    /// The window of one start, opening now.
    pub(crate) fn new(api: &'a dyn JackApi, step: Duration, window: Duration) -> Self {
        let micros = |d: Duration| u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
        let steps = micros(window) / micros(step).max(1) + 1;
        Clock { api, start: api.now_us(), limit: micros(window), step, steps_left: steps }
    }

    fn elapsed_ms(&self) -> u64 {
        self.api.now_us().saturating_sub(self.start) / 1000
    }

    /// One step, unless the window is over (by the clock, or by the count of steps a window
    /// holds, so a clock that does not move cannot hold the start).
    fn wait(&mut self) -> bool {
        if self.steps_left == 0 || self.api.now_us().saturating_sub(self.start) >= self.limit {
            return false;
        }
        self.steps_left -= 1;
        self.api.await_graph(self.step);
        true
    }
}

fn stopped() -> ab::OpenError {
    ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into())
}

/// Publishes the started session's first figures (see the module). `read` reads the active
/// client's (rate, buffer); it is called once, and never before activation.
pub(crate) fn observe(shared: &Shared, clock: &mut Clock<'_>, ask: &Ask, read: impl FnOnce() -> (u32, u32)) -> Result<(), ab::OpenError> {
    match ask.init {
        Init::AfterActivation => {
            let (rate, buffer) = read();
            shared.publish_active(rate, buffer);
            log::info!(
                "JACK: prepared for {} Hz; right after activation the client reads {rate} Hz, buffer {buffer}; the session observes {} Hz, buffer {}",
                ask.prepared_rate,
                shared.sample_rate(),
                shared.buffer_size()
            );
        }
        Init::FirstCycle => {
            while shared.cycles() == 0 {
                if shared.shutdown() {
                    return Err(stopped());
                }
                if !clock.wait() {
                    let window = clock.limit / 1000;
                    return Err(ab::OpenError::Other(format!("JACK: no process cycle arrived within {window} ms of activation")));
                }
            }
            let buffer = shared.last_frames();
            let (rate, _) = read();
            shared.publish_active(rate, buffer);
            log::info!("JACK: prepared for {} Hz; the first process cycle ran {buffer} frames at {rate} Hz", ask.prepared_rate);
        }
    }
    if shared.shutdown() { Err(stopped()) } else { Ok(()) }
}

/// Whether the graph runs what the settling window waits for: the target size, else (with
/// `linked`, the count of process cycles right after the session linked its ports) a cycle that
/// began after the links.
fn ready(shared: &Shared, target: Option<u32>, linked: Option<u64>) -> bool {
    match (target, linked) {
        (Some(frames), _) => shared.buffer_size() == frames,
        (None, Some(before)) => shared.cycles() > before,
        (None, None) => true,
    }
}

/// The settling window (see the module), then the settled figures. `linked` is the count of
/// process cycles right after the session linked its ports (`None`: it linked none); under JACK 2
/// nothing is waited for.
pub(crate) fn wait(shared: &Shared, clock: &mut Clock<'_>, ask: &Ask, linked: Option<u64>) -> Result<Settled, ab::OpenError> {
    let target = ask.target();
    let linked = linked.filter(|_| ask.pipewire);
    if shared.sample_rate() == ask.prepared_rate && !ready(shared, target, linked) {
        let mut arrived = false;
        while shared.sample_rate() == ask.prepared_rate && clock.wait() {
            if shared.shutdown() {
                log::warn!("JACK: the server stopped in the settling window");
                return Err(stopped());
            }
            if ready(shared, target, linked) {
                arrived = true;
                break;
            }
        }
        let (ms, buffer) = (clock.elapsed_ms(), shared.buffer_size());
        match (target, arrived) {
            (Some(frames), true) => log::info!("JACK: the graph runs the expected buffer size {frames}, {ms} ms after activation"),
            (Some(frames), false) => {
                log::info!("JACK: the expected buffer size {frames} was not running {ms} ms after activation; buffer {buffer} is the baseline");
            }
            (None, true) => log::info!("JACK: the first process cycle after the links ran {} frames, {ms} ms after activation", shared.last_frames()),
            (None, false) => log::info!("JACK: no process cycle ran after the links within {ms} ms of activation; buffer {buffer} is the baseline"),
        }
    }
    let (rate, buffer) = (shared.sample_rate(), shared.buffer_size());
    if rate == UNKNOWN || buffer == UNKNOWN {
        return Err(ab::OpenError::Other("JACK: the active client reported no rate or buffer size".into()));
    }
    Ok(Settled { rate, buffer, rebuild: rate != ask.prepared_rate })
}
