//! ADR 0049: clip group files and track presets saved before transit 0.7.0 carry the fork's tags.
//! v0.7.x reads them, says so once and writes transit's tags; from v0.8.0 they are refused with
//! one fixed message. Other tags and newer versions are refused with their own messages.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use transit_engine::Engine;
use transit_engine::file_tags::PRE_0_7_REFUSAL;

const OLD_GROUP: &str = "soundcraft.clipgroup";
const NEW_GROUP: &str = "transit.clipgroup";
const OLD_PRESET: &str = "soundcraft-track-preset";
const NEW_PRESET: &str = "transit-track-preset";
const AUDIO: &str = "clipgroup_pre_0_7 Audio";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("transit-adr49-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `(major, minor)` of the workspace version, any pre-release suffix ignored (`0.8.0-rc.1` is 0.8).
fn major_minor(version: &str) -> (u64, u64) {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = core.split('.').map(|x| x.parse::<u64>().expect("a numeric version"));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// Whether this build is past the v0.8.0 step of ADR 0049 decision 3.
fn past_0_8() -> bool {
    major_minor(env!("CARGO_PKG_VERSION")) >= (0, 8)
}

const STEP: &str = "ADR 0049 decision 3: at the v0.8.0 tag, move the legacy constant (LEGACY_FORMAT in \
    crates/engine/src/clip_group_file.rs, LEGACY_PRESET_FORMAT in crates/engine/src/cmd/track_more.rs) from \
    `accepted_legacy` to `refused_pre_0_7`, and delete that reader's notice (`legacy_note` / `legacy_preset_note`) \
    and its `note` field";

/// A copy of the legacy clip group fixture in `dir` with `format` and `version` replaced.
fn group_copy(dir: &Path, format: &str, version: u64) -> PathBuf {
    let text = std::fs::read_to_string(fixtures().join("clipgroup_pre_0_7.scgrp")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["format"] = json!(format);
    v["version"] = json!(version);
    std::fs::create_dir_all(dir.join(AUDIO)).unwrap();
    std::fs::copy(fixtures().join(AUDIO).join("k_1.wav"), dir.join(AUDIO).join("k_1.wav")).unwrap();
    let p = dir.join("copy.scgrp");
    std::fs::write(&p, v.to_string()).unwrap();
    p
}

/// A copy of the legacy preset fixture in `dir` with `format` and `version` replaced.
fn preset_copy(dir: &Path, format: &str, version: u64) -> PathBuf {
    let text = std::fs::read_to_string(fixtures().join("track_preset_pre_0_7.json")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["format"] = json!(format);
    v["version"] = json!(version);
    let p = dir.join("copy.json");
    std::fs::write(&p, v.to_string()).unwrap();
    p
}

fn err_text(r: transit_engine::Result<Value>) -> String {
    r.expect_err("refused").to_string()
}

/// What an import placed, comparable across engines: (start, length, offset) per clip.
fn placed(e: &Engine, r: &Value) -> Vec<(i64, i64, i64)> {
    r["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            let c = e.session().find_clip(transit_model::ClipId(id.as_u64().unwrap())).unwrap().1;
            let offset = match c.content {
                transit_model::ClipContent::Audio { offset, .. } => offset,
                _ => -1,
            };
            (c.start, c.length, offset)
        })
        .collect()
}

#[test]
fn the_version_parse_ignores_pre_releases() {
    assert_eq!(major_minor("0.7.0"), (0, 7));
    assert_eq!(major_minor("0.8.0-rc.1"), (0, 8));
    assert_eq!(major_minor("1.0.0+build"), (1, 0));
    // Control: the comparison the deadline tests use.
    assert!(major_minor("0.8.0-rc.1") >= (0, 8) && major_minor("0.7.9") < (0, 8));
}

#[test]
fn the_pre_0_7_refusal_names_0_7_0_and_its_release_link() {
    assert!(PRE_0_7_REFUSAL.starts_with("This file was saved by a version before transit 0.7.0."));
    assert!(PRE_0_7_REFUSAL.ends_with("https://github.com/transit-audio/transit/releases/tag/v0.7.0"));
}

/// The deadline test for clip groups (ADR 0049 decision 3): below 0.8 the legacy fixture reads with
/// the note and converts on export; from 0.8 it is refused with the fixed message.
#[test]
fn a_pre_0_7_clip_group_reads_in_0_7_and_is_refused_from_0_8() {
    let fixture = fixtures().join("clipgroup_pre_0_7.scgrp");
    let before = std::fs::read(&fixture).unwrap();
    let mut e = transit_engine::demo::demo_engine();
    let messages = e.messages.len();
    let r = e.execute("file.import_clip_groups", &json!({"path": s(&fixture), "at": 0}));
    if past_0_8() {
        let text = err_text(r);
        assert!(text.contains(PRE_0_7_REFUSAL), "{STEP}; got: {text}");
        assert_eq!(e.messages.len(), messages, "a refused file posts nothing");
        return;
    }
    let r = r.unwrap_or_else(|err| panic!("{STEP} (not yet: this is {}): {err}", env!("CARGO_PKG_VERSION")));
    let note = "clipgroup_pre_0_7.scgrp: saved before transit 0.7.0; import it and export the clips again to keep it opening in 0.8.0 and later";
    assert_eq!(r["note"], note, "{STEP} (not yet)");
    assert_eq!(e.messages[messages..], [note.to_string()], "one line, once");
    assert_eq!(std::fs::read(&fixture).unwrap(), before, "the read leaves the file as it was");
    let old_placed = placed(&e, &r);
    assert_eq!(old_placed.len(), 2);

    // Control: the same content with transit's tag places the same clips and has no note.
    let dir = temp("group-new");
    let mut fresh = transit_engine::demo::demo_engine();
    let fresh_messages = fresh.messages.len();
    let r2 = fresh.execute("file.import_clip_groups", &json!({"path": s(&group_copy(&dir, NEW_GROUP, 1)), "at": 0})).unwrap();
    assert_eq!(placed(&fresh, &r2), old_placed);
    assert!(r2.get("note").is_none());
    assert_eq!(fresh.messages.len(), fresh_messages);

    // Exporting again writes transit's tag and extension.
    let ids: Vec<u64> = r["clips"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    e.execute("file.export_clip_groups", &json!({"path": s(&dir.join("again")), "clips": ids})).unwrap();
    let written = std::fs::read_to_string(dir.join("again.transitclips")).unwrap();
    let v: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(v["format"], NEW_GROUP);
    assert!(!written.contains(OLD_GROUP));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The deadline test for track presets (ADR 0049 decision 3).
#[test]
fn a_pre_0_7_track_preset_reads_in_0_7_and_is_refused_from_0_8() {
    let fixture = fixtures().join("track_preset_pre_0_7.json");
    let before = std::fs::read(&fixture).unwrap();
    let mut e = transit_engine::demo::demo_engine();
    let messages = e.messages.len();
    let r = e.execute("track.load_preset", &json!({"path": s(&fixture), "tracks": ["Lead"]}));
    if past_0_8() {
        let text = err_text(r);
        assert!(text.contains(PRE_0_7_REFUSAL), "{STEP}; got: {text}");
        assert_eq!(e.messages.len(), messages, "a refused file posts nothing");
        return;
    }
    let r = r.unwrap_or_else(|err| panic!("{STEP} (not yet: this is {}): {err}", env!("CARGO_PKG_VERSION")));
    let note = "track_preset_pre_0_7.json: saved before transit 0.7.0; load it and save it again to keep it opening in 0.8.0 and later";
    assert_eq!(r["note"], note, "{STEP} (not yet)");
    assert_eq!(e.messages[messages..], [note.to_string()], "one line, once");
    assert_eq!(std::fs::read(&fixture).unwrap(), before, "the read leaves the file as it was");
    let lead = e.session().track_by_name("Lead").unwrap();
    assert_eq!(lead.mixer.inserts[0].as_ref().map(|i| i.plugin.as_str()), Some("compressor"));
    assert_eq!(lead.instrument.as_ref().map(|i| i.plugin.as_str()), Some("macro_oscillator"));
    let old = (serde_json::to_value(&lead.mixer).unwrap(), serde_json::to_value(&lead.instrument).unwrap());

    // Control: the same content with transit's tag sets the same mixer and instrument, with no note.
    let dir = temp("preset-new");
    let mut fresh = transit_engine::demo::demo_engine();
    let fresh_messages = fresh.messages.len();
    let r2 = fresh.execute("track.load_preset", &json!({"path": s(&preset_copy(&dir, NEW_PRESET, 1)), "tracks": ["Lead"]})).unwrap();
    assert!(r2.get("note").is_none());
    assert_eq!(fresh.messages.len(), fresh_messages);
    let lead2 = fresh.session().track_by_name("Lead").unwrap();
    assert_eq!((serde_json::to_value(&lead2.mixer).unwrap(), serde_json::to_value(&lead2.instrument).unwrap()), old);

    // Saving again writes transit's tag, as .transitpreset whatever the path's extension.
    e.execute("edit.select", &json!({"tracks": ["Lead"]})).unwrap();
    let saved = e.execute("track.save_preset", &json!({"path": s(&dir.join("again.json"))})).unwrap();
    let path = dir.join("again.transitpreset");
    assert_eq!(saved["path"], s(&path).as_str());
    let written = std::fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(v["format"], NEW_PRESET);
    assert!(!written.contains(OLD_PRESET));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_tag_is_refused_by_name_and_posts_nothing() {
    let dir = temp("unknown");
    // A video clip too: its dropped-video note must not be posted for a refused file.
    let text = std::fs::read_to_string(group_copy(&dir, "something.else", 1)).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["lanes"][0].as_array_mut().unwrap().push(json!({"id": 9, "name": "v", "start": 0, "length": 10, "content": {"Video": {}}}));
    let group = dir.join("unknown.scgrp");
    std::fs::write(&group, v.to_string()).unwrap();
    let mut e = transit_engine::demo::demo_engine();
    let messages = e.messages.len();
    let text = err_text(e.execute("file.import_clip_groups", &json!({"path": s(&group)})));
    assert!(text.contains("not a transit clip group file (format \"something.else\")"), "{text}");
    assert!(!text.contains(PRE_0_7_REFUSAL), "{text}");
    assert_eq!(e.messages.len(), messages, "no note, not even the dropped-video one");
    // Control: the same file with transit's tag reads and posts the dropped-video note.
    v["format"] = json!(NEW_GROUP);
    std::fs::write(&group, v.to_string()).unwrap();
    e.execute("file.import_clip_groups", &json!({"path": s(&group)})).unwrap();
    assert_eq!(e.messages.last().map(String::as_str), Some(transit_model::legacy::VIDEO_REMOVED_NOTE));

    let preset = preset_copy(&dir, "something.else", 1);
    let text = err_text(e.execute("track.load_preset", &json!({"path": s(&preset), "tracks": ["Lead"]})));
    assert!(text.contains(&format!("{}: not a transit track preset (format \"something.else\")", s(&preset))), "{text}");
    assert!(!text.contains(PRE_0_7_REFUSAL), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_newer_format_version_is_refused() {
    let dir = temp("newer");
    let mut e = transit_engine::demo::demo_engine();
    let newer = "saved by a newer transit (format version 2); open it in that version or later";
    let text = err_text(e.execute("file.import_clip_groups", &json!({"path": s(&group_copy(&dir, NEW_GROUP, 2))})));
    assert!(text.contains(newer), "{text}");
    let preset = preset_copy(&dir, NEW_PRESET, 2);
    let text = err_text(e.execute("track.load_preset", &json!({"path": s(&preset), "tracks": ["Lead"]})));
    assert!(text.contains(&format!("{}: {newer}", s(&preset))), "{text}");
    // Control: version 1 reads.
    e.execute("file.import_clip_groups", &json!({"path": s(&group_copy(&dir, NEW_GROUP, 1))})).unwrap();
    e.execute("track.load_preset", &json!({"path": s(&preset_copy(&dir, NEW_PRESET, 1)), "tracks": ["Lead"]})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_without_a_format_keeps_the_structural_error() {
    let dir = temp("structural");
    let mut e = transit_engine::demo::demo_engine();
    let p = dir.join("x.json");
    std::fs::write(&p, r#"{"version": 1}"#).unwrap();
    let text = err_text(e.execute("file.import_clip_groups", &json!({"path": s(&p)})));
    assert!(text.contains("not a clip group file: missing field `format`"), "{text}");
    let text = err_text(e.execute("track.load_preset", &json!({"path": s(&p), "tracks": ["Lead"]})));
    assert!(text.contains(&format!("{}: not a transit track preset", s(&p))), "{text}");
    assert!(!text.contains("(format"), "{text}");
    assert!(!text.contains(PRE_0_7_REFUSAL), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
