//! How the first start settles the session's rate and buffer size (ADR 0021, amendment for #760,
//! rules 1 and 2), on the opening thread under the session lock, after `jack_activate` returned
//! and before either direction is started (so the engine skips both user callbacks, the output
//! ports carry silence and captured blocks are not delivered).
//!
//! - **Publication.** The session's [`Shared`] figures are unknown until the client is active.
//!   Right after activation the control thread reads the active client's rate and buffer size
//!   and stores each only while it is still unknown ([`Shared::publish_active`]): a figure a
//!   callback published first (PipeWire's library calls the buffer size callback inside
//!   `jack_activate`) is never overwritten. The observation is then read back from `Shared`.
//! - **The request window.** Under PipeWire a chosen buffer size is a force on transit's own node
//!   that arrives after activation. With a size requested, not declined and not observed yet, the
//!   start waits for it, in steps of the watcher period through [`JackApi::await_graph`] on the
//!   API's clock ([`JackApi::now_us`]), for at most [`REQUEST_WINDOW`]. Whatever runs at its
//!   arrival or at the window's end is the baseline. A server shutdown during the window aborts
//!   the start; a rate change ends it (the rate path below).
//! - **The rate.** The settled rate is compared with the rate the caller prepared for; a
//!   difference latches a rebuild ([`Settled::rebuild`]) instead of starting either direction.
//! - **First-cycle initialization** ([`Init::FirstCycle`], rule 1's fallback, decided in the record
//!   and selected only if the tester's sitting shows the figures read after activation differing
//!   from the first cycle): no getter before a process cycle ran; the cycle's `nframes` is the
//!   buffer, the rate is read after it; no cycle within the window fails the start visibly.

use crate::api::JackApi;
use crate::engine::{Shared, UNKNOWN};
use std::time::Duration;
use transit_audio_backend as ab;

/// The longest a start waits for a requested buffer size to arrive (rule 2), and in first-cycle
/// initialization for the first process cycle.
pub(crate) const REQUEST_WINDOW: Duration = Duration::from_millis(500);

/// Where the session's first figures come from (rule 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Init {
    /// Read from the active client right after `jack_activate` returned.
    #[default]
    AfterActivation,
    /// Taken from the first process cycle (rule 1's fallback; not selected in shipping code until
    /// the tester's sitting answers the record's Q1).
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
    pub(crate) init: Init,
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
struct Clock<'a> {
    api: &'a dyn JackApi,
    start: u64,
    limit: u64,
    step: Duration,
    steps_left: u64,
}

impl Clock<'_> {
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

/// Settles the started session's figures (see the module). `read` reads the active client's
/// (rate, buffer); it is called at most once, and never before activation.
pub(crate) fn settle(
    shared: &Shared,
    api: &dyn JackApi,
    step: Duration,
    window: Duration,
    ask: &Ask,
    read: impl FnOnce() -> (u32, u32),
) -> Result<Settled, ab::OpenError> {
    let micros = |d: Duration| u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
    let steps = micros(window) / micros(step).max(1) + 1;
    let mut clock = Clock { api, start: api.now_us(), limit: micros(window), step, steps_left: steps };
    let stopped = || ab::OpenError::DeviceNotAvailable(crate::SERVER_NOT_RUNNING.into());
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
                    return Err(ab::OpenError::Other(format!("JACK: no process cycle arrived within {} ms of activation", window.as_millis())));
                }
            }
            let buffer = shared.last_frames();
            let (rate, _) = read();
            shared.publish_active(rate, buffer);
            log::info!("JACK: prepared for {} Hz; the first process cycle ran {buffer} frames at {rate} Hz", ask.prepared_rate);
        }
    }
    if shared.shutdown() {
        return Err(stopped());
    }
    if let Some(requested) = ask.requested.filter(|_| ask.pipewire && !ask.declined)
        && shared.buffer_size() != requested
    {
        let mut arrived = false;
        while shared.sample_rate() == ask.prepared_rate && clock.wait() {
            if shared.shutdown() {
                log::warn!("JACK: the server stopped while waiting for the requested buffer size");
                return Err(stopped());
            }
            if shared.buffer_size() == requested {
                arrived = true;
                break;
            }
        }
        if arrived {
            log::info!("JACK: the requested buffer size {requested} arrived {} ms after activation", clock.elapsed_ms());
        } else {
            log::info!(
                "JACK: the requested buffer size {requested} was not running {} ms after activation; buffer {} is the baseline",
                clock.elapsed_ms(),
                shared.buffer_size()
            );
        }
    }
    let (rate, buffer) = (shared.sample_rate(), shared.buffer_size());
    if rate == UNKNOWN || buffer == UNKNOWN {
        return Err(ab::OpenError::Other("JACK: the active client reported no rate or buffer size".into()));
    }
    Ok(Settled { rate, buffer, rebuild: rate != ask.prepared_rate })
}
