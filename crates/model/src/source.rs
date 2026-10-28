//! Audio sources (files in the session's Audio Files folder) and their decoded data.

use crate::SourceId;
use std::collections::BTreeMap;
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, FileFormat, peaks::Peaks};

/// Metadata for an audio file used by the session (serialised).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub name: String,
    /// Path relative to the session folder (or absolute for referenced media).
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
    pub format: FileFormat,
    /// Original BWF time stamp (samples), if any.
    #[serde(default)]
    pub time_reference: u64,
    /// True when the file has not been written to disk yet (recorded/rendered this session).
    #[serde(default)]
    pub unsaved: bool,
}

/// Decoded audio plus waveform overviews (not serialised).
#[derive(Debug)]
pub struct SourceAudio {
    pub buffer: AudioBuffer,
    pub peaks: Vec<Peaks>,
}

impl SourceAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let peaks = buffer.channels.iter().map(|c| Peaks::build(c)).collect();
        SourceAudio { buffer, peaks }
    }
    pub fn frames(&self) -> usize {
        self.buffer.frames()
    }
    pub fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.buffer.channels.get(ch).map(Vec::as_slice)
    }
}

/// Shared decoded audio, keyed by source id.
#[derive(Debug, Clone, Default)]
pub struct SourcePool {
    map: BTreeMap<SourceId, Arc<SourceAudio>>,
}

impl SourcePool {
    pub fn insert(&mut self, id: SourceId, audio: Arc<SourceAudio>) {
        self.map.insert(id, audio);
    }
    pub fn get(&self, id: SourceId) -> Option<&Arc<SourceAudio>> {
        self.map.get(&id)
    }
    pub fn remove(&mut self, id: SourceId) -> Option<Arc<SourceAudio>> {
        self.map.remove(&id)
    }
    pub fn contains(&self, id: SourceId) -> bool {
        self.map.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.map.keys().copied()
    }
}

impl PartialEq for SourcePool {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}
