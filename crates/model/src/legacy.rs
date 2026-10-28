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

/// Is this clip JSON a Video clip (content `{"Video": ...}`)?
fn is_video_clip(c: &Value) -> bool {
    c.get("content").and_then(Value::as_object).is_some_and(|o| o.contains_key("Video"))
}

/// Remove every video part from a parsed session and return whether anything was dropped:
/// the `videos` list and every Video clip, on every playlist of every track.
///
/// A Video track was a live mix strip, and audio clips could be pasted onto one, so it is not
/// deleted while it holds a clip: it becomes an Audio track (the kind that already played those
/// clips). Only a Video track left with no clips is removed.
pub fn drop_video(session: &mut Value) -> bool {
    let Some(root) = session.as_object_mut() else { return false };
    let mut dropped = false;
    if let Some(Value::Array(videos)) = root.get_mut("videos") {
        dropped |= !videos.is_empty();
    }
    root.remove("videos");
    if let Some(Value::Array(tracks)) = root.get_mut("tracks") {
        for track in tracks.iter_mut() {
            let mut left = 0;
            let playlists = track.get_mut("playlists").and_then(Value::as_array_mut).into_iter().flatten();
            for clips in playlists.filter_map(|p| p.get_mut("clips")).filter_map(Value::as_array_mut) {
                let before = clips.len();
                clips.retain(|c| !is_video_clip(c));
                dropped |= clips.len() != before;
                left += clips.len();
            }
            if track.get("kind").and_then(Value::as_str) == Some("Video") {
                dropped = true;
                if left > 0 {
                    track["kind"] = Value::from("Audio");
                }
            }
        }
        // A Video track still named Video here holds no clip.
        tracks.retain(|t| t.get("kind").and_then(Value::as_str) != Some("Video"));
    }
    dropped
}

/// The same for a clip group file (`lanes`: clips per lane): Video clips are dropped.
pub fn drop_video_from_lanes(group: &mut Value) -> bool {
    let mut dropped = false;
    let lanes = group.get_mut("lanes").and_then(Value::as_array_mut).into_iter().flatten();
    for lane in lanes.filter_map(Value::as_array_mut) {
        let before = lane.len();
        lane.retain(|c| !is_video_clip(c));
        dropped |= lane.len() != before;
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

    #[test]
    fn a_video_track_that_holds_audio_becomes_an_audio_track() {
        let audio = json!({"id": 4, "content": {"Audio": {"source": 2, "offset": 0}}});
        let video = json!({"id": 5, "content": {"Video": {"source": 9, "offset": 0}}});
        let mut v = json!({
            "tracks": [
                {"kind": "Video", "playlists": [{"clips": [video.clone(), audio.clone()]}, {"clips": [audio.clone()]}]},
                {"kind": "Video", "playlists": [{"clips": [video.clone()]}, {"clips": []}]},
            ]
        });
        assert!(drop_video(&mut v));
        assert_eq!(v["tracks"].as_array().unwrap().len(), 1, "the track left empty is deleted");
        assert_eq!(v["tracks"][0]["kind"], "Audio");
        assert_eq!(v["tracks"][0]["playlists"][0]["clips"], json!([audio.clone()]));
        assert_eq!(v["tracks"][0]["playlists"][1]["clips"], json!([audio]));
    }

    #[test]
    fn a_clip_group_loses_its_video_clips() {
        let audio = json!({"id": 4, "content": {"Audio": {"source": 2, "offset": 0}}});
        let mut g = json!({"lanes": [[audio.clone(), {"id": 5, "content": {"Video": {"source": 9, "offset": 0}}}], []]});
        assert!(drop_video_from_lanes(&mut g));
        assert_eq!(g["lanes"], json!([[audio], []]));
        assert!(!drop_video_from_lanes(&mut g));
    }
}
