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

/// The stored detector settings (`detector`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Detector {
    /// `auto`, `cues`, `transients` or `grid`.
    pub source: String,
    pub sensitivity: f32,
}

/// The grid's tempo and where it comes from (`grid_tempo`: `bars`, `grid_bpm`, `acid` or `session`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct GridTempo {
    pub bpm: f64,
    pub origin: String,
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
    /// The root in use (slice i plays `root + i`).
    #[serde(default)]
    pub root: Option<u8>,
    #[serde(default)]
    pub detector: Detector,
    /// `1/4`, `1/8`, `1/16` or `1/32`.
    #[serde(default)]
    pub division: String,
    #[serde(default)]
    pub bars: Option<u8>,
    #[serde(default)]
    pub grid_bpm: Option<f64>,
    /// `null` when it needs the session tempo and no placement was given.
    #[serde(default)]
    pub grid_tempo: Option<GridTempo>,
    #[serde(default)]
    pub beat_length: Option<f64>,
    #[serde(default)]
    pub default_beat_length: Option<f64>,
    /// `ascending`, `stale` or `none`.
    #[serde(default)]
    pub mapping: String,
}

impl Loop {
    /// No loop: what the panels are drawn from (faded, inert) on the empty bench.
    pub fn empty() -> Loop {
        Loop {
            source: None,
            start: 0,
            end: 1,
            rate: 1,
            mode: Mode::Auto,
            snap: true,
            auto: Vec::new(),
            manual: Vec::new(),
            root: None,
            detector: Detector::default(),
            division: String::new(),
            bars: None,
            grid_bpm: None,
            grid_tempo: None,
            beat_length: None,
            default_beat_length: None,
            mapping: String::new(),
        }
    }

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

    /// The showing set's marker frames (S first), as the model's marker set: the input of the
    /// model's own checks (the ghost's `too close`, the drag's clamp), never a second rule.
    pub fn markers(&self) -> transit_model::sampler_patch::Markers {
        use transit_model::sampler_patch::{Marker, Markers};
        let markers = self
            .showing()
            .iter()
            .enumerate()
            .map(|(i, s)| Marker { id: s.id.unwrap_or(i as u32 + 1), frame: s.frame, ..Marker::default() })
            .collect();
        Markers { markers, ..Markers::default() }
    }

    /// The beat length the Fit timing uses by default: the stored one, else the engine's default.
    pub fn beats(&self) -> Option<f64> {
        self.beat_length.or(self.default_beat_length)
    }
}

/// The last read, and the revision and placement it was read at.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub data: Option<Loop>,
    /// A read the engine refused, in its words.
    pub error: Option<String>,
    revision: Option<(u64, i64)>,
    /// Reads so far (each is one query; the tests count them).
    pub reads: u64,
}

impl Snapshot {
    /// Reads the state again when the document or the placement `at` (a tick, where the grid tempo
    /// and the default beat length read the session tempo) changed since the last read.
    pub fn refresh(&mut self, app: &mut StudioApp, track: TrackId, at: i64) {
        let revision = app.engine.revision;
        if self.revision == Some((revision, at)) {
            return;
        }
        self.revision = Some((revision, at));
        self.reads += 1;
        match app.engine.execute("mix.sampler_slice_state", &json!({"track": track.0, "at": at})) {
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
