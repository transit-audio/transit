//! Split to Mono Tracks (`clip.split_to_mono`): each channel of the selected stereo or wider audio
//! clips on a new mono track below its track, the originals muted, in one undo step: two
//! microphones recorded onto one stereo track come apart this way. The clips of one track share
//! its new tracks. An in-memory source splits in memory (each channel is written to `Audio Files`
//! on save, as a recording is); a source played from its file (ADR 0035) gets one mono WAV per
//! channel written beside that file, played from disk in turn.

use super::*;
use crate::cmd;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileEncoder, FileFormat};
use transit_model::disk::DiskSource;
use transit_model::{ChannelFormat, ClipContent, ClipId, SourceAudio, SourceId, TrackId, TrackKind};

const ID: &str = "clip.split_to_mono";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "clip.split_to_mono",
        "Split to Mono Tracks",
        ["Clip"],
        None,
        "{clips?} — each channel of the stereo or wider audio clips on a new mono track below its track; the originals are muted",
        has_selection,
        split_to_mono
    )]
}

/// A channel's audio, ready to become a source.
enum Mono {
    Memory(SourceAudio),
    /// The file written for it, as the session names it, and its audio played from that file.
    Disk(String, Arc<SourceAudio>),
}

fn split_to_mono(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session();
    // The clips to split on each track, in track order, with their sources' channel counts.
    let mut by_track: BTreeMap<usize, (TrackId, Vec<(ClipId, SourceId, usize)>)> = BTreeMap::new();
    for id in &ids {
        let (t, c) = s.find_clip(*id).ok_or_else(|| bad(ID, format!("unknown clip {}", id.0)))?;
        let ClipContent::Audio { source, .. } = c.content else { continue };
        let n = s.pool.get(source).map_or(0, |a| a.buffer.num_channels());
        if let (true, Some(i)) = (n > 1, s.track_index(t)) {
            by_track.entry(i).or_insert((t, Vec::new())).1.push((*id, source, n));
        }
    }
    if by_track.is_empty() {
        return Err(bad(ID, "no stereo or wider audio clip is selected"));
    }
    // Muting the originals edits their tracks: refused before any file is written.
    if let Some(tr) = by_track.values().filter_map(|(t, _)| s.track(*t)).find(|t| t.freeze.is_some()) {
        return Err(bad(ID, format!("{} is frozen: unfreeze it to split its clips", tr.name)));
    }
    // Every channel's audio before the session changes, so a refusal leaves it as it was.
    let mut split: BTreeMap<SourceId, Vec<(String, Mono)>> = BTreeMap::new();
    for (_, clips) in by_track.values() {
        for &(_, src, n) in clips {
            if split.contains_key(&src) {
                continue;
            }
            let (Some(meta), Some(audio)) = (s.source(src), s.pool.get(src)) else { continue };
            let names: Vec<String> = (0..n).map(|ch| format!("{}.{}", meta.name, crate::io::split_label(n, ch))).collect();
            let monos = match &audio.disk {
                Some(disk) => write_files(disk, &meta.path, &names)?.into_iter().map(|(path, a)| Mono::Disk(path, a)).collect(),
                None => (0..n).map(|ch| Mono::Memory(mono_audio(audio, ch))).collect::<Vec<_>>(),
            };
            split.insert(src, names.into_iter().zip(monos).collect());
        }
    }
    let s = e.session_mut();
    let mut sources: BTreeMap<(SourceId, usize), SourceId> = BTreeMap::new();
    for (src, monos) in split {
        for (ch, (name, mono)) in monos.into_iter().enumerate() {
            let id = match mono {
                // The extension keeps the label: a source is named after its file's stem.
                Mono::Memory(audio) => crate::io::add_source_audio(s, &format!("{name}.wav"), Arc::new(audio), None, FileFormat::Wav),
                Mono::Disk(path, audio) => crate::io::add_disk_source(s, &format!("{name}.wav"), audio, &path),
            };
            sources.insert((src, ch), id);
        }
    }
    let (mut new_tracks, mut new_clips) = (Vec::new(), Vec::new());
    for (t, clips) in by_track.into_values() {
        let Some(tr) = s.track(t).cloned() else { continue };
        let n = clips.iter().map(|c| c.2).max().unwrap_or(0);
        let mut lanes = Vec::with_capacity(n);
        for ch in 0..n {
            let nt = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("{} {}", tr.name, crate::io::split_label(n, ch))));
            if let (Some(at), Some(from)) = (s.track_index(t), s.track_index(nt)) {
                let moved = s.tracks.remove(from);
                s.tracks.insert((at + 1 + ch).min(s.tracks.len()), moved);
            }
            if let Some(m) = s.track_mut(nt) {
                m.folder = tr.folder;
                m.mixer.volume_db = tr.mixer.volume_db;
                m.mixer.output = tr.mixer.output.clone();
                m.mixer.sends = tr.mixer.sends.clone();
            }
            lanes.push(nt);
            new_tracks.push(nt);
        }
        for (cid, src, channels) in clips {
            let Some(orig) = s.find_clip(cid).map(|(_, c)| c.clone()) else { continue };
            let ClipContent::Audio { offset, .. } = orig.content else { continue };
            for (ch, &lane) in lanes.iter().enumerate().take(channels) {
                let Some(&mono) = sources.get(&(src, ch)) else { continue };
                let mut c = orig.clone();
                c.id = s.new_clip_id();
                c.name = format!("{}.{}", orig.name, crate::io::split_label(channels, ch));
                c.content = ClipContent::Audio { source: mono, offset };
                c.group = None;
                new_clips.push(c.id);
                // Pushed as they are: the original's layering on its track stays as it was.
                if let Some(pl) = s.track_mut(lane).and_then(|l| l.playlist_mut()) {
                    pl.clips.push(c);
                    pl.sort();
                }
            }
            if let Some(c) = s.find_clip_mut(cid) {
                c.muted = true;
            }
        }
    }
    Ok(json!({"tracks": new_tracks, "clips": new_clips}))
}

/// Channel `ch` of in-memory `audio` as mono audio, its peaks kept (rebuilt when there are none).
fn mono_audio(audio: &SourceAudio, ch: usize) -> SourceAudio {
    let buffer = AudioBuffer { sample_rate: audio.buffer.sample_rate, channels: audio.buffer.channels.get(ch).cloned().into_iter().collect() };
    match audio.peaks.get(ch) {
        Some(p) => SourceAudio { buffer, peaks: vec![p.clone()], disk: None },
        None => SourceAudio::new(buffer),
    }
}

/// One mono WAV per channel of `disk`'s file, written beside it in one pass as `<name>.wav` (a
/// number added while that name is taken), each opened to play from disk. Each comes back with
/// its path as the session names it, beside `named` (the source's own path). Nothing is left
/// behind on a failure.
fn write_files(disk: &DiskSource, named: &str, names: &[String]) -> Result<Vec<(String, Arc<SourceAudio>)>> {
    let dir = disk.path().parent().map(Path::to_path_buf).unwrap_or_default();
    let mut paths: Vec<PathBuf> = Vec::new();
    let r = write_channels(disk, &dir, names, &mut paths);
    let opened = r.and_then(|()| {
        paths
            .iter()
            .map(|p| {
                let len = std::fs::metadata(p).map_err(|err| EngineError::Io(format!("{}: {err}", p.display())))?.len();
                let audio = crate::io::disk::open_disk_audio(p, len, disk.rate()).map_err(EngineError::Io)?;
                let file = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                Ok((Path::new(named).with_file_name(file).to_string_lossy().into_owned(), audio))
            })
            .collect::<Result<Vec<_>>>()
    });
    if opened.is_err() {
        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
    }
    opened
}

/// Writes the files [`write_files`] describes, pushing each path to `paths` once it is created.
fn write_channels(disk: &DiskSource, dir: &Path, names: &[String], paths: &mut Vec<PathBuf>) -> Result<()> {
    let io = |p: &Path, err: &dyn std::fmt::Display| EngineError::Io(format!("{}: {err}", p.display()));
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None };
    let mut encoders = Vec::with_capacity(names.len());
    for name in names {
        let stem = crate::io::sanitize_name(name);
        let (path, file) = (1..1000)
            .map(|i| dir.join(if i == 1 { format!("{stem}.wav") } else { format!("{stem}-{i}.wav") }))
            .find_map(|p| {
                crate::file_access::check_write(&p).ok()?;
                std::fs::OpenOptions::new().write(true).create_new(true).open(&p).ok().map(|f| (p, f))
            })
            .ok_or_else(|| EngineError::Io(format!("{}: no file name free for {stem}.wav", dir.display())))?;
        paths.push(path.clone());
        let enc = FileEncoder::new(std::io::BufWriter::new(file), &opts, 1, disk.rate(), None).map_err(|err| io(&path, &err))?;
        enc.check_room(disk.frames()).map_err(|err| io(&path, &err))?;
        encoders.push((path, enc));
    }
    const BLOCK: usize = 64 * 1024;
    let frames = usize::try_from(disk.frames()).unwrap_or(usize::MAX);
    let mut block = vec![vec![0.0f32; BLOCK]; encoders.len()];
    let unread = || EngineError::Io(format!("{}: the file could not be read", disk.path().display()));
    disk.with_sync(|read| {
        let mut at = 0;
        while at < frames {
            let len = BLOCK.min(frames - at);
            // Frame by frame across the channels, so the reader passes over the file once.
            for i in 0..len {
                for (ch, out) in block.iter_mut().enumerate() {
                    out[i] = read(ch, at + i).ok_or_else(unread)?;
                }
            }
            for ((path, enc), out) in encoders.iter_mut().zip(&block) {
                enc.write(std::slice::from_ref(out), len).map_err(|err| io(path, &err))?;
            }
            at += len;
        }
        Ok::<(), EngineError>(())
    })?;
    for (path, enc) in encoders {
        let mut w = enc.finish().map_err(|err| io(&path, &err))?;
        w.flush().map_err(|err| io(&path, &err))?;
        w.into_inner().map_err(|err| io(&path, &err.error()))?.sync_all().map_err(|err| io(&path, &err))?;
    }
    Ok(())
}
