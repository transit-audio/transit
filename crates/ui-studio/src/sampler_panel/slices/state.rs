//! What the Slices view reads: `mix.sampler_slice_state` as the engine reports it, read again only
//! when the document changes (the engine's revision). Positions are the source's own frames at its
//! native rate; pads and keys are the engine's effective values. The view never derives them.

use crate::StudioApp;
use serde::Deserialize;
use serde_json::json;
use transit_model::{SourceId, TrackId};

/// Which marker set shows (`mode`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Auto,
    Manual,
}

impl Mode {
    /// Its segment in `Auto · Manual`.
    pub fn index(self) -> usize {
        usize::from(self == Self::Manual)
    }
}

/// One slice of a set: its start frame and end (the next start, or the loop end), its pad
/// (1-based) and that pad's effective note when a pad plays it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Slice {
    pub id: Option<u32>,
    pub frame: u64,
    pub end: u64,
    pub label: Option<String>,
    pub pad: Option<u8>,
    pub key: Option<u8>,
}

/// The patch's slice loop.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Loop {
    pub source: Option<SourceId>,
    pub start: u64,
    pub end: u64,
    /// The source's native rate.
    pub rate: u32,
    pub mode: Mode,
    pub snap: bool,
    pub auto: Vec<Slice>,
    pub manual: Vec<Slice>,
}

impl Loop {
    /// The set that shows.
    pub fn showing(&self) -> &[Slice] {
        match self.mode {
            Mode::Auto => &self.auto,
            Mode::Manual => &self.manual,
        }
    }

    /// Seconds from S (the loop start) to `frame`, at the source's rate.
    pub fn seconds(&self, frame: f64) -> f64 {
        (frame - self.start as f64) / f64::from(self.rate.max(1))
    }

    /// The loop's length in frames.
    pub fn frames(&self) -> f64 {
        self.end.saturating_sub(self.start) as f64
    }
}

/// The last read, and the revision it was read at.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub data: Option<Loop>,
    /// A read the engine refused, in its words.
    pub error: Option<String>,
    revision: Option<u64>,
    /// Reads so far (each is one query; the tests count them).
    pub reads: u64,
}

impl Snapshot {
    /// Reads the state again when the document changed since the last read.
    pub fn refresh(&mut self, app: &mut StudioApp, track: TrackId) {
        let revision = app.engine.revision;
        if self.revision == Some(revision) {
            return;
        }
        self.revision = Some(revision);
        self.reads += 1;
        match app.engine.execute("mix.sampler_slice_state", &json!({"track": track.0})) {
            Ok(v) => match serde_json::from_value::<Option<Loop>>(v["loop"].clone()) {
                Ok(data) => {
                    self.data = data;
                    self.error = None;
                }
                Err(e) => {
                    self.data = None;
                    self.error = Some(e.to_string());
                }
            },
            Err(e) => {
                self.data = None;
                self.error = Some(e.to_string());
            }
        }
    }
}
