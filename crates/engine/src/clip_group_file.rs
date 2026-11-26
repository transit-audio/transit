//! Clip group files (`.tclips`; `.scgrp` before transit 0.7.0, ADR 0049): clips exported
//! from a session (`file.export_clip_groups`) and imported into another (`file.import_clip_groups`).
//! The reader goes by the file's `format` tag, never its extension.
//!
//! The file is JSON: clips per lane with starts relative to the group, plus the audio they play
//! as 32-bit float WAVs in a `<name> Audio/` folder next to it.

use crate::file_tags::{Refusal, Tag, Tags};
use crate::{Engine, EngineError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use transit_audio_io::{EncodeOptions, FileFormat};
use transit_model::{Clip, ClipContent, ClipId, SourceId, TrackId};
use transit_time::Samples;

/// Added when the path given has no extension (ADR 0049).
pub const EXTENSION: &str = "tclips";
const FORMAT: &str = "transit.clipgroup";
/// The fork's tag (ADR 0049): read through v0.7.x; from the v0.8.0 tag it moves to `refused_pre_0_7`.
const LEGACY_FORMAT: &str = "soundcraft.clipgroup";
const TAGS: Tags = Tags { version: VERSION, current: FORMAT, accepted_legacy: &[LEGACY_FORMAT], refused_pre_0_7: &[], what: "clip group file" };
/// The format version a group file holding a re-anchored clip is written at (ADR 0056 decision 4.4):
/// a build without the clip's `anchor` would play it from the wrong place, so it refuses the file by
/// ADR 0049's newer-version rule. A file without one is written at version 1, as before.
const VERSION: u64 = 2;
/// Refuse files that would place absurd numbers of clips, tracks or audio files.
const MAX_CLIPS: usize = 10_000;

#[derive(Debug, Serialize, Deserialize)]
struct GroupFile {
    format: String,
    version: u32,
    sample_rate: u32,
    length: Samples,
    /// Clips per lane (one lane per source track), starts relative to the group start.
    lanes: Vec<Vec<Clip>>,
    /// Source id (as written) → audio file path relative to the group file.
    audio: BTreeMap<u64, String>,
}

/// Write the clips `ids` (grouped by track) to `path`. Returns the number of clips written.
pub fn export(e: &Engine, ids: &[ClipId], path: &str) -> Result<usize> {
    let p = with_extension(path);
    // First, so a refused group file leaves nothing behind and is refused whatever else is wrong.
    crate::file_access::check_write(&p).map_err(EngineError::Io)?;
    let s = e.session();
    let mut lanes: Vec<(usize, Vec<Clip>)> = Vec::new();
    for id in ids {
        let Some((t, c)) = s.find_clip(*id) else { continue };
        let ti = s.tracks.iter().position(|x| x.id == t).unwrap_or(0);
        match lanes.iter_mut().find(|(i, _)| *i == ti) {
            Some((_, v)) => v.push(c.clone()),
            None => lanes.push((ti, vec![c.clone()])),
        }
    }
    let clips: Vec<&Clip> = lanes.iter().flat_map(|(_, v)| v.iter()).collect();
    let (Some(start), Some(end)) = (clips.iter().map(|c| c.start).min(), clips.iter().map(|c| c.start.saturating_add(c.length)).max()) else {
        return Err(EngineError::BadParams("file.export_clip_groups".into(), "no clips to export".into()));
    };
    let n = clips.len();
    lanes.sort_by_key(|(i, _)| *i);
    let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("Clip Group").to_string();
    let audio_dir_name = format!("{} Audio", crate::io::sanitize_name(&stem));
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut audio = BTreeMap::new();
    let mut out_lanes = Vec::new();
    for (_, v) in lanes {
        let mut lane = Vec::new();
        for mut c in v {
            // An export of clips is a copy: it never carries what a clip's left edge hides (ADR
            // 0052 decision 17.10, slice 6a), as `edit.copy_clips` does not.
            crate::edit::drop_left_hidden(&mut c);
            c.start -= start;
            if let ClipContent::Audio { source, .. } = c.content
                && let std::collections::btree_map::Entry::Vacant(slot) = audio.entry(source.0)
                && let (Some(buf), Some(src)) = (s.pool.get(source), s.sources.iter().find(|x| x.id == source))
            {
                crate::io::make_dir(dir.join(&audio_dir_name))?;
                let name = format!("{audio_dir_name}/{}_{}", crate::io::sanitize_name(&src.name), source.0);
                let rel = match &buf.disk {
                    // A disk-backed source's file is its audio (ADR 0035): copied as it is.
                    Some(d) => {
                        let file = d.path();
                        let ext = file.extension().and_then(|x| x.to_str()).unwrap_or("wav");
                        let rel = format!("{name}.{ext}");
                        crate::io::copy_path(&file, dir.join(&rel))?;
                        rel
                    }
                    None => {
                        let rel = format!("{name}.wav");
                        let opts = EncodeOptions {
                            format: FileFormat::Wav,
                            bit_depth: transit_audio_io::BitDepth::Float32,
                            dither: false,
                            bwf: None,
                            bitrate_kbps: None,
                            title: None,
                        };
                        let bytes = transit_audio_io::encode(&buf.buffer, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
                        crate::io::write_path(dir.join(&rel), bytes)?;
                        rel
                    }
                };
                slot.insert(rel);
            }
            lane.push(c);
        }
        out_lanes.push(lane);
    }
    let version = if out_lanes.iter().flatten().any(|c| c.anchor() != 0) { VERSION } else { 1 };
    let file =
        GroupFile { format: FORMAT.into(), version: version as u32, sample_rate: s.sample_rate.hz(), length: end - start, lanes: out_lanes, audio };
    let text = serde_json::to_string_pretty(&file).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_path(&p, text)?;
    Ok(n)
}

/// What an import placed, and the notice for a file written before transit 0.7.0 (ADR 0049).
#[derive(Debug, Default)]
pub struct Imported {
    pub clips: Vec<ClipId>,
    /// Posted through [`Engine::message`] too; `None` for a file in transit's format.
    pub note: Option<String>,
}

/// The notice for reading a clip group file saved before transit 0.7.0 (ADR 0049).
#[must_use]
pub fn legacy_note(path: &str) -> String {
    format!(
        "{}: saved before transit 0.7.0; import it and export the clips again to keep it opening in 0.8.0 and later",
        crate::file_tags::file_name(path)
    )
}

/// Import a clip group file at `at`, one lane per track starting at `first` (new audio tracks
/// are made when the session runs out).
pub fn import(e: &mut Engine, path: &str, first: Option<TrackId>, at: Samples) -> Result<Imported> {
    let id = "file.import_clip_groups";
    let text = crate::io::read_text(path)?;
    let not_a_group = |err: serde_json::Error| EngineError::BadParams(id.into(), format!("not a clip group file: {err}"));
    let mut value: serde_json::Value = serde_json::from_str(&text).map_err(not_a_group)?;
    // The tag and version first (ADR 0049), so a refused file posts nothing.
    let tag = match TAGS.check(&value) {
        Ok(tag) => tag,
        Err(Refusal::Message(m)) => return Err(EngineError::BadParams(id.into(), m)),
        Err(Refusal::Structural) => {
            let err = serde_json::from_value::<GroupFile>(value).err().map_or_else(|| "no format tag".to_string(), |err| err.to_string());
            return Err(EngineError::BadParams(id.into(), format!("not a clip group file: {err}")));
        }
    };
    // Video is retired (ADR 0031): its clips are dropped, as when a session opens.
    if transit_model::legacy::drop_video_from_lanes(&mut value) {
        e.message(transit_model::legacy::VIDEO_REMOVED_NOTE.to_string());
    }
    let file: GroupFile = serde_json::from_value(value).map_err(not_a_group)?;
    if file.lanes.iter().map(|l| l.len().max(1)).sum::<usize>() > MAX_CLIPS || file.audio.len() > MAX_CLIPS {
        return Err(EngineError::BadParams(id.into(), "clip group file has too many clips".into()));
    }
    let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
    // Decode audio first so a bad file changes nothing, and each file once however many ids name it.
    let (mut decoded, mut index, mut file_of) = (Vec::new(), BTreeMap::new(), BTreeMap::new());
    let rate = e.session().sample_rate.hz();
    for (old, rel) in &file.audio {
        let full = dir.join(rel);
        if !full.starts_with(&dir) || rel.contains("..") {
            return Err(EngineError::BadParams(id.into(), format!("audio path escapes the group folder: {rel}")));
        }
        let key = crate::io::file_key(&full);
        let n = match index.get(&key) {
            Some(n) => *n,
            None => {
                let bytes = crate::io::read_file(&full)?;
                let (_, buf) = transit_audio_io::decode(&bytes, Some("wav")).map_err(|err| EngineError::Io(format!("{rel}: {err}")))?;
                let buf = crate::io::at_rate(buf, rate, crate::io::MAX_IMPORT_SAMPLES)
                    .ok_or_else(|| EngineError::Io(format!("{rel}: too large to import")))?;
                decoded.push((rel.clone(), buf));
                index.insert(key, decoded.len() - 1);
                decoded.len() - 1
            }
        };
        file_of.insert(*old, n);
    }
    let src_rate = f64::from(file.sample_rate.max(1));
    let ratio = f64::from(e.session().sample_rate.hz()) / src_rate;
    let scale = |v: Samples| -> Samples { (v as f64 * ratio).round() as Samples };
    // Every clip's rescaled mapping inside the numeric bounds before anything is added (ADR 0056
    // decision 4.4): one clip outside them refuses the file.
    for c in file.lanes.iter().flatten() {
        let mut probe = c.clone();
        rescale(&mut probe, scale);
        if let Some(why) = probe.domain_problem() {
            return Err(EngineError::BadParams(id.into(), format!("the clip group file cannot be placed at this sample rate: {why}")));
        }
    }
    let s = e.session_mut();
    let sources: Vec<SourceId> = decoded
        .into_iter()
        .map(|(rel, buf)| {
            let name = Path::new(&rel).file_name().and_then(|x| x.to_str()).unwrap_or("clip group").to_string();
            crate::io::add_source(s, &name, buf, None, FileFormat::Wav)
        })
        .collect();
    let map: BTreeMap<u64, SourceId> = file_of.into_iter().filter_map(|(old, n)| Some((old, *sources.get(n)?))).collect();
    let start_idx = first.and_then(|t| s.tracks.iter().position(|x| x.id == t));
    let mut placed = Vec::new();
    let mut stretch_notes = Vec::new();
    let group_id = s.alloc();
    for (li, lane) in file.lanes.into_iter().enumerate() {
        let audio_lane = lane.iter().any(|c| matches!(c.content, ClipContent::Audio { .. }));
        let target = start_idx.and_then(|i| s.tracks.get(i + li)).map(|t| t.id);
        let target = match target {
            Some(t) => t,
            None => {
                let channels = lane
                    .iter()
                    .find_map(|c| match c.content {
                        ClipContent::Audio { source, .. } => map.get(&source.0).and_then(|n| s.pool.get(*n)).map(|a| a.buffer.num_channels()),
                        _ => None,
                    })
                    .unwrap_or(2);
                let kind = if audio_lane { transit_model::TrackKind::Audio } else { transit_model::TrackKind::Midi };
                let fmt = if channels == 1 { transit_model::ChannelFormat::Mono } else { transit_model::ChannelFormat::Stereo };
                s.add_track(kind, fmt, Some("Clip Group"))
            }
        };
        for mut c in lane {
            if let ClipContent::Audio { source, .. } = &mut c.content {
                let Some(n) = map.get(&source.0) else { continue };
                *source = *n;
            } else if !matches!(c.content, ClipContent::Midi { .. }) {
                continue;
            }
            rescale(&mut c, scale);
            c.id = s.new_clip_id();
            c.start = at.saturating_add(c.start);
            c.group = Some(group_id);
            // A stretch outside what a clip can play is clamped and said (ADR 0056 decision 5.2).
            stretch_notes.extend(c.sanitize_noting());
            placed.push(c.id);
            crate::edit::place_clip(s, target, c);
        }
    }
    for n in stretch_notes {
        e.message(n);
    }
    // Last, so a file refused for its contents posts nothing.
    let note = (tag == Tag::Legacy).then(|| legacy_note(path));
    if let Some(n) = &note {
        e.message(n.clone());
    }
    Ok(Imported { clips: placed, note })
}

/// A group file clip's positions at this session's rate: start, length, and an audio clip's offset
/// and anchor, each rounded to the nearest sample (the clip-time and source-frame scales change
/// together, so the clip's rate is unchanged; ADR 0056 decision 4.4). The loop, fades, sync point
/// and envelope are not rescaled, as before.
fn rescale(c: &mut Clip, scale: impl Fn(Samples) -> Samples) {
    if let ClipContent::Audio { offset, anchor, .. } = &mut c.content {
        *offset = scale(*offset);
        *anchor = scale(*anchor);
    }
    c.start = scale(c.start.max(0));
    c.length = scale(c.length);
}

fn with_extension(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_none() { p.with_extension(EXTENSION) } else { p }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn export_then_import_round_trips_audio_clips() {
        let dir = transit_test_support::TestDir::new("grp");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("drums.scgrp");
        let path = path.to_string_lossy().to_string();
        let mut e = crate::demo::demo_engine();
        let kick = e.session().tracks.iter().find(|t| t.name == "Kick").map(|t| t.id).unwrap();
        let ids: Vec<_> = e.session().track(kick).unwrap().clips().iter().take(2).map(|c| c.id).collect();
        let r = e.execute("file.export_clip_groups", &json!({"path": path, "clips": ids.iter().map(|c| c.0).collect::<Vec<_>>()})).unwrap();
        assert_eq!(r["clips"], 2);
        let before = e.session().tracks.len();
        let at = 30_000_000;
        let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": at, "track": "Bass"})).unwrap();
        let placed: Vec<_> = r["clips"].as_array().unwrap().iter().filter_map(serde_json::Value::as_u64).collect();
        assert_eq!(placed.len(), 2);
        let bass = e.session().tracks.iter().find(|t| t.name == "Bass").map(|t| t.id).unwrap();
        for id in &placed {
            let (t, c) = e.session().find_clip(transit_model::ClipId(*id)).unwrap();
            assert_eq!(t, bass);
            assert!(c.start >= at && c.group.is_some());
            assert!(matches!(c.content, transit_model::ClipContent::Audio { source, .. } if e.session().pool.contains(source)));
        }
        e.undo();
        assert_eq!(e.session().tracks.len(), before);
        assert!(e.session().find_clip(transit_model::ClipId(placed[0])).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_above_full_scale_survives_a_group_file_bit_for_bit() {
        use transit_model::{ChannelFormat, Clip, TrackKind};
        let dir = transit_test_support::TestDir::new("grp-overs");
        let path = dir.join("hot.tclips").to_string_lossy().into_owned();
        let hot = vec![1.5f32, -2.0, 0.25, 1.0e3, f32::from_bits(1), -0.0];
        let mut e = crate::Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![hot.clone()] };
        let src = crate::io::add_source(s, "Hot", audio, None, transit_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::audio(id, "Hot", src, 0, 0, 6));
        assert_eq!(e.execute("file.export_clip_groups", &json!({"path": path, "clips": [id.0]})).unwrap()["clips"], 1);
        let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": 48_000})).unwrap();
        let placed = transit_model::ClipId(r["clips"][0].as_u64().unwrap());
        let Some((_, c)) = e.session().find_clip(placed) else { panic!("no imported clip") };
        let transit_model::ClipContent::Audio { source, .. } = &c.content else { panic!("not audio") };
        let source = *source;
        assert_ne!(source, src, "the import reads the file, not the session's own source");
        let got = &e.session().pool.get(source).unwrap().buffer.channels[0];
        assert_eq!(got.iter().map(|s| s.to_bits()).collect::<Vec<_>>(), hot.iter().map(|s| s.to_bits()).collect::<Vec<_>>(), "{got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_exported_midi_clip_carries_nothing_its_left_edge_hides() {
        use transit_midi::{Note, Sequence};
        use transit_model::{ChannelFormat, Clip, ClipContent, TrackKind};
        let dir = transit_test_support::TestDir::new("grp-hidden");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("trimmed.tclips").to_string_lossy().into_owned();
        let mut e = crate::Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        let n = |pitch, start| Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length: 480 };
        // A start trim left a note before the clip's edge (ADR 0052 decision 17).
        let seq = Sequence { notes: vec![n(72, -960), n(60, 0)], ctrls: Vec::new() };
        crate::edit::place_clip(s, t, Clip::midi(id, "Trimmed", 48_000, 96_000, seq));
        assert_eq!(e.execute("file.export_clip_groups", &json!({"path": path, "clips": [id.0]})).unwrap()["clips"], 1);
        let text = std::fs::read_to_string(&path).unwrap();
        let file: super::GroupFile = serde_json::from_str(&text).unwrap();
        let ClipContent::Midi { sequence } = &file.lanes[0][0].content else { panic!("not MIDI") };
        assert_eq!(sequence.notes.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(60, 0)]);
        let ClipContent::Midi { sequence } = &e.session().find_clip(id).unwrap().1.content else { panic!("not MIDI") };
        assert_eq!(sequence.notes.len(), 2, "the session's clip keeps the hidden note");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn positions_in_a_group_file_are_kept_within_reach() {
        let dir = transit_test_support::TestDir::new("grp-far");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("far.scgrp");
        let note = json!({"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 0, "start": i64::MAX, "length": i64::MAX});
        let clip =
            json!({"id": 1, "name": "far", "start": i64::MAX, "length": i64::MAX, "content": {"Midi": {"sequence": {"notes": [note], "ctrls": []}}}});
        let file = json!({"format": "soundcraft.clipgroup", "version": 1, "sample_rate": 48000, "length": 1, "lanes": [[clip]], "audio": {}});
        std::fs::write(&p, file.to_string()).unwrap();
        let mut e = crate::demo::demo_engine();
        let r = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).unwrap();
        let id = transit_model::ClipId(r["clips"][0].as_u64().unwrap());
        let c = e.session().find_clip(id).unwrap().1;
        assert!(c.start <= crate::cmd::MAX_POSITION && c.length <= crate::cmd::MAX_POSITION);
        e.execute("edit.select_all", &json!({})).unwrap();
        for cmd in ["edit.duplicate", "event.extract_chords"] {
            assert!(!matches!(e.execute(cmd, &json!({})), Err(crate::EngineError::Internal(..))), "{cmd}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_named_by_many_ids_becomes_one_source() {
        let dir = transit_test_support::TestDir::new("grp-shared");
        let _ = std::fs::create_dir_all(&dir);
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] };
        std::fs::write(dir.join("k.wav"), transit_audio_io::encode(&audio, &transit_audio_io::EncodeOptions::default()).unwrap()).unwrap();
        let clip = |id: u64| json!({"id": id, "name": "k", "start": id * 1000, "length": 480, "content": {"Audio": {"source": id, "offset": 0}}});
        let audio_paths = json!({"1": "k.wav", "2": "./k.wav", "3": "k.wav"});
        let file = json!({"format": "soundcraft.clipgroup", "version": 1, "sample_rate": 48000, "length": 4000, "lanes": [[clip(1), clip(2), clip(3)]], "audio": audio_paths});
        let p = dir.join("k.scgrp");
        std::fs::write(&p, file.to_string()).unwrap();
        let mut e = crate::demo::demo_engine();
        let before = e.session().sources.len();
        let r = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).unwrap();
        assert_eq!((r["clips"].as_array().unwrap().len(), e.session().sources.len()), (3, before + 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_foreign_files_and_escaping_paths() {
        let dir = transit_test_support::TestDir::new("grp-bad");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("x.scgrp");
        std::fs::write(
            &p,
            r#"{"format":"soundcraft.clipgroup","version":1,"sample_rate":48000,"length":1,"lanes":[],"audio":{"1":"../../etc/passwd"}}"#,
        )
        .unwrap();
        let mut e = crate::demo::demo_engine();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        std::fs::write(&p, "{}").unwrap();
        assert!(e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy()})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR 0056 decision 4.4, export: a group file holding a re-anchored clip (a split piece of a
    /// stretched clip) is written at format version 2, which a build reading only version 1 refuses
    /// instead of playing the piece from the wrong place; this build imports it with its anchor. A
    /// file without one is written at version 1, as before.
    #[test]
    fn a_group_file_with_an_anchored_clip_is_written_at_the_next_version() {
        use transit_model::{ChannelFormat, Clip, TrackKind};
        let dir = transit_test_support::TestDir::new("grp-anchor");
        let _ = std::fs::create_dir_all(&dir);
        let mut e = crate::Engine::default();
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![(0..4_000).map(|i| i as f32).collect()] };
        let src = crate::io::add_source(s, "Ramp", audio, None, transit_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "Ramp", src, 100, 0, 400);
        c.stretch = 2.0;
        crate::edit::place_clip(s, t, c);
        s.edit.selected_tracks = vec![t];
        let plain = dir.join("plain.tclips").to_string_lossy().into_owned();
        e.execute("file.export_clip_groups", &json!({"path": plain, "clips": [id.0]})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&plain).unwrap()).unwrap();
        assert_eq!(v["version"], 1, "an unsplit stretched clip has no anchor");
        e.execute("edit.separate", &json!({"tracks": [t.0], "at": 100})).unwrap();
        let piece = e.session().track(t).unwrap().clips()[1].clone();
        assert_eq!(piece.anchor(), -100);
        let path = dir.join("piece.tclips").to_string_lossy().into_owned();
        e.execute("file.export_clip_groups", &json!({"path": path, "clips": [piece.id.0]})).unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["version"], 2);
        let older = super::Tags { version: crate::file_tags::VERSION, ..super::TAGS };
        assert!(matches!(older.check(&v), Err(super::Refusal::Message(m)) if m.contains("newer transit")), "a build reading version 1 refuses it");
        let r = e.execute("file.import_clip_groups", &json!({"path": path, "at": 10_000})).unwrap();
        let placed = e.session().find_clip(transit_model::ClipId(r["clips"][0].as_u64().unwrap())).unwrap().1.clone();
        assert_eq!((placed.source_offset(), placed.anchor(), placed.stretch, placed.length), (100, -100, 2.0, 300));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ADR 0056 decision 4.4, import: a 44.1 kHz group file into a 48 kHz session rescales the
    /// anchor with the offset (each rounded to the nearest sample, so the rate is unchanged); a clip
    /// whose rescaled anchor leaves +-2^46 refuses the whole file and adds nothing.
    #[test]
    fn a_group_file_import_rescales_the_anchor_with_the_offset() {
        let dir = transit_test_support::TestDir::new("grp-rescale");
        let _ = std::fs::create_dir_all(&dir);
        let audio = transit_audio_io::AudioBuffer { sample_rate: 44_100, channels: vec![vec![0.25; 4_410]] };
        std::fs::write(dir.join("k.wav"), transit_audio_io::encode(&audio, &transit_audio_io::EncodeOptions::default()).unwrap()).unwrap();
        let clip = |anchor: i64| {
            json!({"id": 1, "name": "k", "start": 441, "length": 882, "stretch": 2.0,
                "content": {"Audio": {"source": 1, "offset": 441, "anchor": anchor}}})
        };
        let file = |anchor: i64| json!({"format": "transit.clipgroup", "version": 2, "sample_rate": 44100, "length": 1323, "lanes": [[clip(anchor)]], "audio": {"1": "k.wav"}});
        let p = dir.join("k.tclips");
        std::fs::write(&p, file(-441).to_string()).unwrap();
        let mut e = crate::demo::demo_engine();
        let r = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy(), "at": 0})).unwrap();
        let c = e.session().find_clip(transit_model::ClipId(r["clips"][0].as_u64().unwrap())).unwrap().1.clone();
        assert_eq!((c.source_offset(), c.anchor(), c.start, c.length), (480, -480, 480, 960));
        let far = (transit_model::MAX_ANCHOR as f64 * 44_100.0 / 48_000.0) as i64 + 1_000;
        std::fs::write(&p, file(-far).to_string()).unwrap();
        let (sources, tracks) = (e.session().sources.len(), e.session().tracks.len());
        let err = e.execute("file.import_clip_groups", &json!({"path": p.to_string_lossy(), "at": 0})).unwrap_err();
        assert!(err.to_string().contains("its anchor"), "{err}");
        assert_eq!((e.session().sources.len(), e.session().tracks.len()), (sources, tracks), "nothing added");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
