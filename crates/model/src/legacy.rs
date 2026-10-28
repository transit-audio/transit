//! Retired session features that old files may still carry (ADR 0031).
//!
//! transit does not host video. Builds before v0.4.0 could write a `Video` track kind, `Video`
//! clip content and a `videos` list. The types are gone, so serde would reject such a file;
//! [`drop_video`] removes those parts from the parsed JSON before it is deserialized, and
//! the loader reports one note. No released studio build could create them, so this is a safety
//! net, not a migration.

use serde_json::Value;

/// The note a session that carried video shows once on opening.
pub const VIDEO_REMOVED_NOTE: &str = "video clips removed: transit does not host video";

/// Remove every video part from a parsed session: the `videos` list, each Video track (with its
/// clips and automation: it has no audio path, so nothing else routes through it), and any Video
/// clip on another track. Returns true when anything was dropped.
pub fn drop_video(session: &mut Value) -> bool {
    let Some(root) = session.as_object_mut() else { return false };
    let mut dropped = false;
    if let Some(Value::Array(videos)) = root.get_mut("videos") {
        dropped |= !videos.is_empty();
    }
    root.remove("videos");
    if let Some(Value::Array(tracks)) = root.get_mut("tracks") {
        let before = tracks.len();
        tracks.retain(|t| t.get("kind").and_then(Value::as_str) != Some("Video"));
        dropped |= tracks.len() != before;
        for clips in
            tracks.iter_mut().filter_map(|t| t.get_mut("playlists")).filter_map(Value::as_array_mut).flatten().filter_map(|p| p.get_mut("clips"))
        {
            if let Value::Array(clips) = clips {
                let before = clips.len();
                clips.retain(|c| c.get("content").and_then(Value::as_object).is_none_or(|o| !o.contains_key("Video")));
                dropped |= clips.len() != before;
            }
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn video_parts_are_dropped_and_other_tracks_stay() {
        let mut v = json!({
            "videos": [{"id": 9, "name": "m", "path": "/m.mov"}],
            "tracks": [
                {"kind": "Audio", "playlists": [{"clips": [
                    {"id": 1, "content": {"Audio": {"source": 2, "offset": 0}}},
                    {"id": 3, "content": {"Video": {"source": 9, "offset": 0}}}]}]},
                {"kind": "Video", "playlists": [{"clips": []}]}
            ]
        });
        assert!(drop_video(&mut v));
        assert!(v.get("videos").is_none());
        assert_eq!(v["tracks"].as_array().unwrap().len(), 1);
        assert_eq!(v["tracks"][0]["playlists"][0]["clips"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_session_without_video_is_untouched_apart_from_an_empty_videos_key() {
        let mut v = json!({"videos": [], "tracks": [{"kind": "Midi", "playlists": [{"clips": []}]}]});
        assert!(!drop_video(&mut v));
        assert_eq!(v, json!({"tracks": [{"kind": "Midi", "playlists": [{"clips": []}]}]}));
        assert!(!drop_video(&mut json!(7)));
    }
}
