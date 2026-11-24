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
use std::path::PathBuf;
use std::sync::Arc;
use transit_audio_io::{AudioBuffer, FileFormat};
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

/// The most channels a clip splits into, one track each, so a hostile file's channel count never
/// becomes tens of thousands of tracks: a MADI-sized recording (64) fits, a wider one is refused.
const MAX_CHANNELS: usize = 64;

/// Channel `ch` of `n` in a split's names: `L` and `R` for stereo, else its number from 1 (a
/// file's channels carry no layout here; an eight-track field recording is not 7.1).
fn split_label(n: usize, ch: usize) -> String {
    match (n, ch) {
        (2, 0) => "L".into(),
        (2, 1) => "R".into(),
        _ => (ch + 1).to_string(),
    }
}

/// A channel's audio, ready to become a source.
enum Mono {
    Memory(SourceAudio),
    /// The file written for it, and its audio played from that file.
    Disk(PathBuf, Arc<SourceAudio>),
}

/// One source's split: its id, its BWF time stamp, and each channel's name and audio.
type Split = (SourceId, u64, Vec<(String, Mono)>);

fn split_to_mono(e: &mut Engine, p: &Value) -> Result<Value> {
    let mut ids = clip_ids_param(e, p);
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(*id));
    let s = e.session();
    // The clips to split on each track, in track order, with their sources' channel counts.
    let mut by_track: BTreeMap<usize, (TrackId, Vec<(ClipId, SourceId, usize)>)> = BTreeMap::new();
    for id in &ids {
        let (t, c) = s.find_clip(*id).ok_or_else(|| bad(ID, format!("unknown clip {}", id.0)))?;
        let ClipContent::Audio { source, .. } = c.content else { continue };
        let n = s.pool.get(source).map_or(0, |a| a.buffer.num_channels());
        if n > MAX_CHANNELS {
            return Err(bad(ID, format!("{} has {n} channels: a clip splits into at most {MAX_CHANNELS} tracks", c.name)));
        }
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
    // Every channel's audio before the session changes, so a refusal leaves it as it was and no
    // file written for it behind.
    let mut written = Vec::new();
    let split = match prepare(s, &by_track, &mut written) {
        Ok(split) => split,
        Err(err) => {
            crate::io::split_files::remove(&written);
            return Err(err);
        }
    };
    let s = e.session_mut();
    let mut sources: BTreeMap<(SourceId, usize), SourceId> = BTreeMap::new();
    for (src, stamp, monos) in split {
        for (ch, (name, mono)) in monos.into_iter().enumerate() {
            let id = match mono {
                // The extension keeps the label: a source is named after its file's stem.
                Mono::Memory(audio) => crate::io::add_source_audio(s, &format!("{name}.wav"), Arc::new(audio), None, FileFormat::Wav),
                // The path written, absolute as a recording's is: a save brings it into the session folder.
                Mono::Disk(path, audio) => crate::io::add_disk_source(s, &format!("{name}.wav"), audio, &path.to_string_lossy()),
            };
            if let Some(sx) = s.sources.last_mut() {
                sx.time_reference = stamp;
            }
            sources.insert((src, ch), id);
        }
    }
    let (mut new_tracks, mut new_clips) = (Vec::new(), Vec::new());
    for (t, clips) in by_track.into_values() {
        let Some(tr) = s.track(t).cloned() else { continue };
        let n = clips.iter().map(|c| c.2).max().unwrap_or(0);
        let mut lanes = Vec::with_capacity(n);
        for ch in 0..n {
            let nt = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("{} {}", tr.name, split_label(n, ch))));
            // Taken out before `t` is found: a new track goes in before a Master, which may sit above `t`.
            if let Some(from) = s.track_index(nt) {
                let moved = s.tracks.remove(from);
                let at = s.track_index(t).map_or(s.tracks.len(), |at| at + 1 + ch);
                s.tracks.insert(at.min(s.tracks.len()), moved);
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
                c.name = format!("{}.{}", orig.name, split_label(channels, ch));
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

/// Each source's channels, in memory or as one mono WAV per channel written beside its file (a
/// number added while a name is taken). The files are opened to play from disk only once every
/// one is complete; each path created is in `written`, for the caller to remove on a failure.
fn prepare(
    s: &transit_model::Session,
    by_track: &BTreeMap<usize, (TrackId, Vec<(ClipId, SourceId, usize)>)>,
    written: &mut Vec<PathBuf>,
) -> Result<Vec<Split>> {
    let mut out: Vec<(SourceId, u64, Vec<(String, Option<SourceAudio>)>, Option<Arc<DiskSource>>)> = Vec::new();
    for (_, clips) in by_track.values() {
        for &(_, src, n) in clips {
            if out.iter().any(|o| o.0 == src) {
                continue;
            }
            let (Some(meta), Some(audio)) = (s.source(src), s.pool.get(src)) else { continue };
            let names: Vec<String> = (0..n).map(|ch| format!("{}.{}", meta.name, split_label(n, ch))).collect();
            let channels = match &audio.disk {
                Some(disk) => {
                    crate::io::split_files::write(disk, &names, written)?;
                    names.into_iter().map(|name| (name, None)).collect()
                }
                None => names.into_iter().enumerate().map(|(ch, name)| (name, Some(mono_audio(audio, ch)))).collect(),
            };
            out.push((src, meta.time_reference, channels, audio.disk.clone()));
        }
    }
    let mut files = written.iter();
    out.into_iter()
        .map(|(src, stamp, channels, disk)| {
            let monos = channels
                .into_iter()
                .map(|(name, mem)| match (mem, &disk) {
                    (Some(audio), _) => Ok((name, Mono::Memory(audio))),
                    (None, Some(disk)) => {
                        let p = files.next().ok_or_else(|| EngineError::Io(format!("{name}: its file was not written")))?;
                        Ok((name, Mono::Disk(p.clone(), crate::io::split_files::open(p, disk.rate())?)))
                    }
                    (None, None) => Err(EngineError::Io(format!("{name}: no audio"))),
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((src, stamp, monos))
        })
        .collect()
}
