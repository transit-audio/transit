//! Format tags of the side files transit writes: clip group files and track presets (ADR 0049).
//!
//! Each reader checks its file's `format` and `version` here before it uses anything else in the
//! file or posts any message. The fork's tags are read through v0.7.x and refused from the v0.8.0
//! tag with [`PRE_0_7_REFUSAL`]; any other tag and any newer version are refused with their own
//! messages.

use serde_json::Value;

/// The refusal for a file saved by a version before transit 0.7.0, once its tag is no longer read.
pub const PRE_0_7_REFUSAL: &str = "This file was saved by a version before transit 0.7.0. Open and save it in transit 0.7.0 first: https://github.com/transit-audio/transit/releases/tag/v0.7.0";

/// The highest format `version` the readers understand.
pub const VERSION: u64 = 1;

/// The tags one reader knows.
pub struct Tags {
    /// What the writer writes.
    pub current: &'static str,
    /// Read as well as `current`; a read tells the caller to write the file again.
    pub accepted_legacy: &'static [&'static str],
    /// Refused with [`PRE_0_7_REFUSAL`].
    pub refused_pre_0_7: &'static [&'static str],
    /// "clip group file" or "track preset", for the plain refusal.
    pub what: &'static str,
}

/// A checked file's tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Current,
    Legacy,
}

/// Why a file is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No string `format`, or no integer `version`: not a transit file of any version. Each reader
    /// keeps its own structural error for this.
    Structural,
    /// A refusal with this message.
    Message(String),
}

impl Tags {
    /// The tag and version of the file `v`.
    ///
    /// # Errors
    /// [`Refusal`] as described on each variant.
    pub fn check(&self, v: &Value) -> Result<Tag, Refusal> {
        let format = v.get("format").and_then(Value::as_str).ok_or(Refusal::Structural)?;
        let tag = if format == self.current {
            Tag::Current
        } else if self.accepted_legacy.contains(&format) {
            Tag::Legacy
        } else if self.refused_pre_0_7.contains(&format) {
            return Err(Refusal::Message(PRE_0_7_REFUSAL.into()));
        } else {
            return Err(Refusal::Message(format!("not a transit {} (format \"{format}\")", self.what)));
        };
        let version = v.get("version").and_then(Value::as_u64).ok_or(Refusal::Structural)?;
        if version > VERSION {
            return Err(Refusal::Message(format!("saved by a newer transit (format version {version}); open it in that version or later")));
        }
        Ok(tag)
    }
}

/// The final path component of `path`, for the notice.
#[must_use]
pub fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().and_then(|x| x.to_str()).unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TAGS: Tags = Tags { current: "new", accepted_legacy: &["old"], refused_pre_0_7: &["older"], what: "thing" };

    #[test]
    fn tags_and_versions() {
        assert_eq!(TAGS.check(&json!({"format": "new", "version": 1})), Ok(Tag::Current));
        assert_eq!(TAGS.check(&json!({"format": "old", "version": 1})), Ok(Tag::Legacy));
        assert_eq!(TAGS.check(&json!({"format": "older", "version": 1})), Err(Refusal::Message(PRE_0_7_REFUSAL.into())));
        assert_eq!(TAGS.check(&json!({"format": "x", "version": 1})), Err(Refusal::Message("not a transit thing (format \"x\")".into())));
        assert_eq!(
            TAGS.check(&json!({"format": "new", "version": 2})),
            Err(Refusal::Message("saved by a newer transit (format version 2); open it in that version or later".into()))
        );
        assert_eq!(TAGS.check(&json!({"version": 1})), Err(Refusal::Structural));
        assert_eq!(TAGS.check(&json!({"format": 7, "version": 1})), Err(Refusal::Structural));
        assert_eq!(TAGS.check(&json!({"format": "new"})), Err(Refusal::Structural));
        assert_eq!(TAGS.check(&json!({"format": "new", "version": "1"})), Err(Refusal::Structural));
        assert_eq!(file_name("a/b/Bass.json"), "Bass.json");
    }
}
