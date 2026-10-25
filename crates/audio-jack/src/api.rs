//! The safe surface over libjack (the `ComApi` pattern of the native WASAPI backend): what the
//! backend needs of a JACK server and of one client of it. `crate::ffi` implements it over the real
//! library, `crate::fake` over a scripted server, so everything above runs in tests anywhere.

use crate::engine::{Engine, Shared};
use std::sync::Arc;

/// One physical audio port of the server, by its full name ("system:capture_1").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhysicalPort {
    pub(crate) name: String,
    /// A capture port (audio from the hardware: a JACK output port that transit's inputs read);
    /// else a playback port (audio to the hardware: a JACK input port that transit's outputs feed).
    pub(crate) capture: bool,
}

impl PhysicalPort {
    /// The client part of the full name (the device a port belongs to): before the first `:`.
    pub(crate) fn group(&self) -> &str {
        self.name.split_once(':').map_or(self.name.as_str(), |(group, _)| group)
    }
}

/// Why a client did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientError {
    /// `libjack.so.0` (or a function `jack-sys` binds) did not load; the loader's text.
    LibraryMissing(String),
    /// No server answered, and none is started (`JackNoStartServer`; status `JackServerFailed`).
    ServerNotRunning,
    /// The server refused the client for another reason.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::LibraryMissing(_) => f.write_str(crate::LIBRARY_MISSING),
            ClientError::ServerNotRunning => f.write_str(crate::SERVER_NOT_RUNNING),
            ClientError::Failed(text) => write!(f, "JACK: {text}"),
        }
    }
}

/// A JACK library: opens clients. Never starts a server.
pub(crate) trait JackApi: Send + Sync {
    /// `jack_client_open` with `JackNoStartServer`; the server may change `name` to make it unique.
    fn open_client(&self, name: &str) -> Result<Box<dyn JackClient>, ClientError>;
    /// What the library says it is (`jack_get_version_string`, optional in jack.h): "1.9.20" for
    /// JACK 2, a text naming PipeWire for PipeWire's libjack. `None` where it is missing.
    fn version(&self) -> Option<String>;
}

/// One open client. Control-thread calls only, one thread at a time (the backend keeps it behind a
/// `Mutex`); none is made from a JACK callback.
pub(crate) trait JackClient: Send {
    /// The client's name as the server made it (`jack_get_client_name`).
    fn name(&self) -> String;
    /// The server's rate (`jack_get_sample_rate`).
    fn sample_rate(&self) -> u32;
    /// The server's current quantum (`jack_get_buffer_size`).
    fn buffer_size(&self) -> u32;
    /// The server's physical audio ports, capture ports first, each in the server's order.
    fn physical_ports(&self) -> Vec<PhysicalPort>;
    /// Registers this client's audio port `short` (`in_1`, `out_1`...): an input of ours (fed from
    /// a capture port) when `input`, else an output. Returns its full name.
    fn register_port(&mut self, short: &str, input: bool) -> Result<String, String>;
    /// Installs the callbacks over `shared` with `engine` as the process state (both directions'
    /// user callbacks), and activates the client. Once per client. The engine is dropped after the
    /// client closed.
    fn activate(&mut self, shared: Arc<Shared>, engine: Engine) -> Result<(), String>;
    /// Connects `source` (an output port) to `destination` (an input port); one that exists is fine.
    fn connect(&mut self, source: &str, destination: &str) -> Result<(), String>;
    /// The full names of the ports `port` is connected to (`jack_port_get_all_connections`).
    #[cfg(test)]
    fn connections(&self, port: &str) -> Vec<String>;
    /// Asks the server for a new quantum (`jack_set_buffer_size`): every client of the server gets
    /// it. Tests only (the app never changes the server's quantum, ADR 0021 decision 4).
    #[cfg(test)]
    #[cfg_attr(not(all(target_os = "linux", feature = "libjack")), allow(dead_code))]
    fn set_buffer_size(&mut self, frames: u32) -> Result<(), String>;
    /// Removes the client from the process graph and disconnects its ports (`jack_deactivate`).
    fn deactivate(&mut self) -> Result<(), String>;
    /// Closes the client; the callback state is freed after the server has let go of it. Never
    /// called from a JACK callback.
    fn close(self: Box<Self>) -> Result<(), String>;
}
