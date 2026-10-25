//! The native JACK backend (ADR 0021), slices 1 and 2: the client, enumeration and the duplex
//! process engine. One code path for JACK 2 and for PipeWire through its JACK library
//! (`pipewire-jack`, or `pw-jack transit`). Linux with the `libjack` feature only: elsewhere, and
//! without the feature, the crate is empty, except that its tests build everywhere (the safe core
//! runs over a fake server).
//!
//! - **Bindings.** The raw `jack-sys` declarations with run-time loading (dlopen of `libjack.so.0`,
//!   nothing links against it), behind this crate's `libjack` feature. All calls are in `ffi`
//!   and the `extern "C"` trampolines libjack calls are in `callback`; those two modules are the
//!   only ones that may contain `unsafe`. The rest is safe code over the `JackApi` and
//!   `JackClient` traits, so enumeration, the client lifecycle and the process engine run against
//!   the fake server (`fake`, tests only) anywhere.
//! - **Device model.** The server is the system ("JACK"), a port group is the device (the client
//!   part of the physical ports' names: `system` under JACK 2, one entry per device node under
//!   PipeWire), its ports are the channels ([`JackDevice`]).
//! - **Enumeration** opens a short-lived probe client with `JackNoStartServer` and never starts a
//!   server ([`JackBackend::probe`]). Three states: the library did not load, no server answered,
//!   or a server runs with its port groups ([`ServerState`]).
//! - **Lifecycle.** The first open creates the client "transit" and registers one port per channel
//!   (`in_1..`, `out_1..`); the other direction joins it before either starts (the ASIO open-order
//!   rule). The first `start` builds the engine, installs the callbacks, activates the client and
//!   connects the ports to the device's (unless `TRANSIT_JACK_AUTOCONNECT=0`); dropping the last
//!   stream deactivates and closes it ([`session`]). The server dictates the rate and the quantum:
//!   `Achieved` reports them, whatever was requested.
//! - **Process** (`engine`): one callback per cycle runs the capture callback, then the render
//!   callback (`capture_precedes_render`), with stamps from `jack_get_cycle_times` and the frame
//!   counter as the device position. It and every other callback store atomics only; a watcher
//!   thread (`transit-jack-watch`, 20 ms) turns the flags into the stream errors the Player already
//!   handles: server shutdown is `StreamError::DeviceNotAvailable` (once; the dead client is closed
//!   on the watcher, never in the handler), then `ResetRequested` when the server answers again; a
//!   sample-rate or quantum change is `ResetRequested`; an xrun is counted in
//!   `StreamStats::overloads`.
#![cfg(any(all(target_os = "linux", feature = "libjack"), test))]
#![deny(unsafe_code)]
// The callback modules (`engine`, `callback`) deny these lints; control-thread code may allocate,
// format and log.
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

mod api;
#[cfg(all(target_os = "linux", feature = "libjack"))]
#[allow(unsafe_code)]
mod callback;
mod devices;
mod engine;
#[cfg(test)]
mod fake;
#[cfg(all(target_os = "linux", feature = "libjack"))]
#[allow(unsafe_code)]
mod ffi;
#[cfg(all(test, target_os = "linux", feature = "libjack"))]
mod server_tests;
mod session;
#[cfg(test)]
mod tests;

pub use devices::{JackDevice, Server, ServerState};
pub use session::{AUTO_CONNECT_ENV, JackBackend, auto_connect_from, server_name};

/// The system name every endpoint of this backend carries (the one saved configurations use).
pub const SYSTEM: &str = "JACK";
/// What the system says when `libjack.so.0` does not load.
pub const LIBRARY_MISSING: &str = "JACK library not found: install pipewire-jack (or JACK), or start transit with pw-jack";
/// What the system says when no JACK or PipeWire server answers (none is ever started).
pub const SERVER_NOT_RUNNING: &str = "JACK/PipeWire server not running";
/// The name transit's client asks for; the server makes it unique.
pub const CLIENT_NAME: &str = "transit";
/// The name of the short-lived client that lists the ports.
pub const PROBE_NAME: &str = "transit-probe";

/// Creates the native JACK backend for the system named "JACK": every call hands out the same
/// process-wide backend ([`JackBackend::shared`]), so the render and capture opens of one Player
/// share one client.
#[cfg(all(target_os = "linux", feature = "libjack"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct JackFactory;

#[cfg(all(target_os = "linux", feature = "libjack"))]
impl transit_audio_backend::BackendFactory for JackFactory {
    fn backend(&self, system: Option<&str>) -> Option<Box<dyn transit_audio_backend::Backend>> {
        if system.is_some_and(|s| !s.eq_ignore_ascii_case(SYSTEM)) {
            return None;
        }
        Some(Box::new(JackBackend::shared()))
    }
}

#[cfg(all(target_os = "linux", feature = "libjack"))]
impl JackBackend {
    /// The process's one backend over the real libjack (loaded at its first use, not here). Its
    /// ports connect to the device's unless `TRANSIT_JACK_AUTOCONNECT` is `0`, `no`, `off` or
    /// `false` (read once).
    pub fn shared() -> JackBackend {
        static SHARED: std::sync::OnceLock<JackBackend> = std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| {
                let backend = JackBackend::with_api(std::sync::Arc::new(ffi::LibJack));
                let value = std::env::var(AUTO_CONNECT_ENV).ok();
                if session::auto_connect_from(value.as_deref()) { backend } else { backend.without_auto_connect() }
            })
            .clone()
    }
}
