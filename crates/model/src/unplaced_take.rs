//! Recorded files that have no clip. Legacy text entries remain readable without guessing paths.

use crate::SourceId;
use serde::{Deserialize, Serialize};

/// A recorded file and its placement failure, or an older session's text entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UnplacedTake {
    /// Structured entries keep file identity separate from display text.
    File {
        /// Stored like a source path, relative to the session folder when possible.
        path: String,
        /// Short explanation displayed beside the path.
        reason: String,
        /// The source this entry belongs to, when the audio could be loaded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SourceId>,
        /// Try placing a formerly refused file when opening it in a newer build.
        #[serde(default)]
        retry_on_open: bool,
    },
    /// Older files stored `<path> (<reason>)`; only the compatibility reader parses this.
    Legacy(String),
}

impl UnplacedTake {
    /// An entry whose file identity does not depend on its explanation.
    pub fn new(path: String, reason: String, source: Option<SourceId>, retry_on_open: bool) -> Self {
        Self::File { path, reason, source, retry_on_open }
    }

    /// Whether opening may place this take automatically.
    pub fn retry_on_open(&self) -> bool {
        match self {
            Self::File { retry_on_open, .. } => *retry_on_open,
            Self::Legacy(text) => !text.ends_with(" (recorded before bar 1)"),
        }
    }

    /// Follow a rewritten source. Legacy conversion uses the known source path as the boundary,
    /// so parentheses in a filename or explanation cannot change which file is matched.
    /// Returns whether this entry belongs to that source.
    pub fn follow_source(&mut self, id: SourceId, old: &str, new: &str) -> bool {
        match self {
            Self::File { path, source, .. } if *source == Some(id) || (source.is_none() && path == old) => {
                *source = Some(id);
                *path = new.to_string();
                true
            }
            Self::Legacy(text) => {
                if let Some(reason) = legacy_reason(text, old) {
                    *self = Self::new(new.to_string(), reason.to_string(), Some(id), self.retry_on_open());
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}

/// The legacy reason must be one complete parenthesized group. Otherwise `Vox.wav` could
/// claim the entry for `Vox.wav (copy).wav`. Malformed text stays readable, unchanged.
fn legacy_reason<'a>(text: &'a str, path: &str) -> Option<&'a str> {
    let reason = text.strip_prefix(path)?.strip_prefix(" (")?.strip_suffix(')')?;
    let depth = reason.chars().try_fold(0usize, |depth, c| match c {
        '(' => depth.checked_add(1),
        ')' => depth.checked_sub(1),
        _ => Some(depth),
    })?;
    (depth == 0).then_some(reason)
}

impl From<String> for UnplacedTake {
    fn from(text: String) -> Self {
        Self::Legacy(text)
    }
}

impl From<&str> for UnplacedTake {
    fn from(text: &str) -> Self {
        Self::Legacy(text.to_string())
    }
}

impl std::fmt::Display for UnplacedTake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File { path, reason, .. } => write!(f, "{path} ({reason})"),
            Self::Legacy(text) => f.write_str(text),
        }
    }
}
