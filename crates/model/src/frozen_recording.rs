//! Texture's frozen media references. Large data is runtime-only and shared across snapshots.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrozenRecording {
    /// Relative to the session file, normally inside Audio Files.
    pub path: String,
    /// Versioned continuation file (grain positions, effects and converter histories).
    pub continuation: String,
    pub quality: u32,
    pub rate: u32,
    pub channels: u32,
    /// Frames at the codec rate, including the quality's longer low-resolution layouts.
    pub span: u32,
    /// The buffer setting in 16-bit stereo frames at 48 kHz.
    pub buffer_frames: u32,
    /// Captured setup header, independent of the optional continuation file.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<u8>,
    /// A media problem survives saving until the recording is available again.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub report: String,
    #[serde(skip)]
    pub restore_failed: bool,
    #[serde(skip)]
    pub data: Option<Arc<[u8]>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_inserts_and_empty_recordings_default_without_audio() {
        let insert: crate::Insert = serde_json::from_str(r#"{"plugin":"texture"}"#).unwrap();
        assert!(insert.frozen_recording.is_none());
        let state: FrozenRecording = serde_json::from_str("{}").unwrap();
        assert_eq!(state, FrozenRecording::default());
        assert!(!serde_json::to_string(&state).unwrap().contains("data"));
    }
}
