//! The safe surface over libjack (the `ComApi` pattern of the native WASAPI backend): what the
//! backend needs of a JACK server and of one client of it. `crate::ffi` implements it over the real
//! library, `crate::fake` over a scripted server, so everything above runs in tests anywhere.

use crate::engine::{Engine, Shared};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

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

/// A port name as libjack gave it. JACK names are byte strings, not always UTF-8: `Ok` holds a
/// name whose bytes are valid UTF-8, so the `String` is the exact identity; `Err` holds a lossy
/// form of any other name, for messages only. It is never stored, compared or connected (#431).
pub(crate) type PortName = Result<String, String>;

/// Why a client did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientError {
    /// No libjack loaded with every function the table needs; the loader's text.
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
    /// Which library this API talks through, with every library it tried to get there (#230):
    /// the last selection, `None` before the first, or where nothing is chosen (the fake server).
    fn library(&self) -> Option<crate::library::Selection> {
        None
    }
    /// Waits about `period` on the calling control thread while the server runs its graph: never
    /// under the session's lock, never from a JACK callback. A link a connect made can show in
    /// [`JackClient::links`] only at a later graph cycle: JACK 2 writes it into the graph's next
    /// state and the server switches to that state at the start of a cycle whose previous graph
    /// finished (#431, the restore's confirmation).
    fn await_graph(&self, period: Duration) {
        std::thread::sleep(period);
    }
    /// A monotonic clock in microseconds, for the cap on [`JackApi::await_graph`] waits.
    fn now_us(&self) -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        u64::try_from(ORIGIN.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(u64::MAX)
    }
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
    /// The full names of the ports linked to this client's `index`-th registered input port (when
    /// `input`) or output port, whichever client made the links (`jack_port_get_all_connections`
    /// on the port this client owns). Empty when there is no such port (#431, rule 6).
    fn links(&self, input: bool, index: usize) -> Vec<PortName>;
    /// Whether an audio port named exactly `name` exists: one that data can be read from when
    /// `output` (a JACK output port), else one that can receive data (`jack_get_ports`). Only a
    /// port whose name is exactly `name`, byte for byte, counts.
    fn has_port(&self, name: &str, output: bool) -> bool;
    /// Asks the server for a new quantum (`jack_set_buffer_size`): every client of the server gets
    /// it. Called from the open path (ADR 0021, 2026-10-25 amendment) on the thread that opens the
    /// client, never from a JACK callback.
    fn set_buffer_size(&mut self, frames: u32) -> Result<(), String>;
    /// Removes the client from the process graph and disconnects its ports (`jack_deactivate`).
    fn deactivate(&mut self) -> Result<(), String>;
    /// Closes the client; the callback state is freed after the server has let go of it. Never
    /// called from a JACK callback.
    fn close(self: Box<Self>) -> Result<(), String>;
}

/// Tests: the order in which a client closed and its callback state was freed.
#[cfg(all(test, target_os = "linux", feature = "libjack"))]
pub(crate) type CloseLog = Arc<std::sync::Mutex<Vec<&'static str>>>;
