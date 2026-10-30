//! Disk-backed sources in the session (ADR 0035, issue #187): which files are streamed, opening
//! them through the checked file access, and re-placing the takes older builds left unplaced.

use super::*;
use transit_model::disk::{DiskSource, register, stream_threshold_frames};

/// How much of an audio file's start holds its header (chunks before the data are small).
pub(super) const HEADER_BYTES: u64 = 64 * 1024;

#[cfg(test)]
thread_local! {
    /// A file length the size checks use instead of the real one, so a test needs no giant file.
    pub(super) static FAKE_LEN: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// The length of a file `real` bytes long, as the size checks see it.
pub(super) fn file_len(real: u64) -> u64 {
    #[cfg(test)]
    if let Some(n) = FAKE_LEN.with(std::cell::Cell::get) {
        return n;
    }
    real
}

/// What the header of the audio file at `path` says, read without its audio, and the file's
/// length. `None` for a file that cannot be read or whose header does not size it.
pub(super) fn probe(path: &Path) -> Option<(transit_audio_io::AudioInfo, u64)> {
    use std::io::Read;
    crate::file_access::check_read(path).ok()?;
    let len = file_len(std::fs::metadata(path).ok().filter(|m| m.is_file())?.len());
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(HEADER_BYTES).read_to_end(&mut head).ok()?;
    Some((transit_audio_io::probe_sized(&head, len).ok()?, len))
}

/// Whether a file of `info` is too long to hold in memory at all (the decoder's limit).
pub(super) fn over_decode_limit(info: &transit_audio_io::AudioInfo) -> bool {
    info.frames.saturating_mul(u64::from(info.channels)) > transit_audio_io::MAX_DECODE_SAMPLES
}

/// Whether a take of `info` in `s` is placed disk-backed: longer than the setting (ten minutes by
/// default, [`transit_model::disk::STREAM_MINUTES_KEY`]) or than the decoder takes.
pub(super) fn streams(s: &Session, info: &transit_audio_io::AudioInfo) -> bool {
    over_decode_limit(info) || info.frames > stream_threshold_frames(s, info.sample_rate)
}

/// The audio file at `path` (`len` bytes) as a disk-backed source at `rate`, streamed from now
/// on. Only native PCM at the session's rate streams in slice 1 of ADR 0035; anything else is an
/// error naming the file.
pub(crate) fn open_disk_audio(path: &Path, len: u64, rate: u32) -> std::result::Result<Arc<SourceAudio>, String> {
    crate::file_access::check_read(path)?;
    let disk = DiskSource::open(path, len)?;
    if disk.rate() != rate {
        return Err(format!(
            "{}: a file played from disk must be at the session's rate ({} Hz, the file is {} Hz)",
            path.display(),
            rate,
            disk.rate()
        ));
    }
    register(&disk);
    Ok(Arc::new(SourceAudio::disk(disk)))
}

/// Register a disk-backed source's audio as a new source playing the file at `path`, as saved
/// (never rewritten; ADR 0035).
pub fn add_disk_source(s: &mut Session, name: &str, audio: Arc<SourceAudio>, path: &str) -> SourceId {
    let id = SourceId(s.alloc());
    let stem = Path::new(name).file_stem().and_then(|x| x.to_str()).unwrap_or(name).to_string();
    s.sources.push(Source {
        id,
        name: stem,
        path: path.to_string(),
        channels: u16::try_from(audio.buffer.num_channels()).unwrap_or(u16::MAX),
        frames: audio.frames() as u64,
        sample_rate: audio.buffer.sample_rate,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: true,
    });
    s.pool.insert(id, audio);
    id
}

/// The file an unplaced-take entry of an older build names (`<path> (<what it holds>)`, #192),
/// when it is still there.
fn unplaced_file(entry: &str) -> Option<PathBuf> {
    entry.match_indices(" (").map(|(i, _)| PathBuf::from(&entry[..i])).find(|p| crate::file_access::check_read(p).is_ok() && p.is_file())
}

/// On open: place each take an older build left unplaced (#192) whose file is still there, as one
/// disk-backed clip at the session's start, on the track whose name the file is named after
/// (`<track>_<NN>.wav`) or else a new track. Returns what it says; the entries placed leave
/// the list.
pub(crate) fn place_unplaced(s: &mut Session) -> Vec<String> {
    let mut said = Vec::new();
    let entries = std::mem::take(&mut s.unplaced_takes);
    for entry in entries {
        let placed = unplaced_file(&entry).and_then(|path| {
            let (info, len) = probe(&path)?;
            match open_disk_audio(&path, len, s.sample_rate.hz()) {
                Ok(audio) => Some((path, info, audio)),
                Err(e) => {
                    said.push(format!("the unplaced take {entry} is still not placed: {e}"));
                    None
                }
            }
        });
        let Some((path, info, audio)) = placed else {
            s.unplaced_takes.push(entry);
            continue;
        };
        let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or("Take").to_string();
        let track_name = stem.rsplit_once('_').map_or(stem.as_str(), |(t, _)| t);
        let format = if info.channels >= 2 { ChannelFormat::Stereo } else { ChannelFormat::Mono };
        let tid = match s.tracks.iter().find(|t| t.kind == TrackKind::Audio && sanitize_name(&t.name) == track_name) {
            Some(t) => t.id,
            None => s.add_track(TrackKind::Audio, format, Some(track_name)),
        };
        let src = add_disk_source(s, &stem, audio, &path.to_string_lossy());
        let len = i64::try_from(info.frames).unwrap_or(i64::MAX);
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, tid, Clip::audio(cid, &stem, src, 0, 0, len));
        let track = s.track(tid).map_or_else(String::new, |t| t.name.clone());
        said.push(format!(
            "the take {} is placed now, played from disk, at the start of track {track}: move it where it was recorded",
            path.display()
        ));
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{open_session, save_session};

    /// Removes the directory on drop, so a failed assertion leaks nothing (E51: pid + counter).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("transit-disk-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            FAKE_LEN.with(|l| l.set(None));
        }
    }

    /// A header for `frames` mono float frames at 48 kHz, its length faked to hold them.
    fn header_only(dir: &TempDir, name: &str, frames: u64) -> PathBuf {
        let path = dir.0.join(name);
        std::fs::write(&path, transit_audio_io::wav_float_header(1, 48_000, frames).unwrap()).unwrap();
        FAKE_LEN.with(|l| l.set(Some(std::fs::metadata(&path).unwrap().len() + frames * 4)));
        path
    }

    #[test]
    fn a_take_an_older_build_left_unplaced_is_placed_on_open_when_its_file_is_there_issue_187() {
        let dir = TempDir::new("unplaced");
        let frames = transit_audio_io::MAX_DECODE_SAMPLES + 1_000;
        let path = header_only(&dir, "Loopback_01.wav", frames);
        let gone = dir.0.join("Gone_01.wav");
        let mut e = Engine::default();
        let vox = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Loopback"));
        e.session_mut().unplaced_takes =
            vec![format!("{} (6.21 h of mono at 48 kHz)", path.display()), format!("{} (6.21 h of mono at 48 kHz)", gone.display())];
        let session = dir.0.join("Old.transit").to_string_lossy().into_owned();
        save_session(&mut e, &session).unwrap();

        let mut r = Engine::default();
        open_session(&mut r, &session).unwrap();
        let s = r.session();
        assert_eq!(s.unplaced_takes, [format!("{} (6.21 h of mono at 48 kHz)", gone.display())], "the missing file stays listed");
        let src = s.sources.last().unwrap();
        assert!(src.disk && src.frames == frames && src.path == path.to_string_lossy(), "{src:?}");
        let clips = s.track(vox).unwrap().clips();
        assert_eq!(clips.len(), 1, "on the track the file is named after");
        assert_eq!((clips[0].start, clips[0].length, clips[0].source()), (0, frames as i64, Some(src.id)));
        assert!(r.messages.iter().any(|m| m.contains("is placed now, played from disk")), "{:?}", r.messages);
        assert!(r.messages.iter().any(|m| m.contains("never placed") && m.contains("Gone_01.wav")), "{:?}", r.messages);
        assert!(!r.messages.iter().any(|m| m.contains("never placed") && m.contains("Loopback_01.wav")), "{:?}", r.messages);
    }

    #[test]
    fn a_saved_disk_backed_source_reopens_disk_backed_and_its_file_is_never_rewritten() {
        let dir = TempDir::new("reopen");
        // Short, but marked disk-backed: the mark alone opens it from disk.
        let path = dir.0.join("Audio Files").join("Take_01.wav");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = transit_audio_io::wav_float_header(1, 48_000, 480).unwrap();
        bytes.extend((0..480).flat_map(|i| (i as f32 / 480.0).to_le_bytes()));
        std::fs::write(&path, &bytes).unwrap();
        let mut e = Engine::default();
        let audio = open_disk_audio(&path, bytes.len() as u64, 48_000).unwrap();
        let src = add_disk_source(e.session_mut(), "Take_01", audio, &path.to_string_lossy());
        let session = dir.0.join("S.transit").to_string_lossy().into_owned();
        save_session(&mut e, &session).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(e.session().source(src).unwrap().path, "Audio Files/Take_01.wav");
        let mut r = Engine::default();
        open_session(&mut r, &session).unwrap();
        let back = r.session().sources.last().unwrap();
        assert!(back.disk);
        assert!(r.session().pool.get(back.id).is_some_and(|a| a.is_disk() && a.frames() == 480));
        // Saved elsewhere, it points at the file where it is.
        let other = TempDir::new("elsewhere");
        let moved = other.0.join("Moved.transit").to_string_lossy().into_owned();
        crate::io::save_session(&mut r, &moved).unwrap();
        let p = r.session().sources.last().unwrap().path.clone();
        assert!(Path::new(&p).is_absolute() && same_file(Path::new(&p), &path), "{p}");
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
    }

    #[test]
    fn a_compressed_or_other_rate_file_is_not_streamed() {
        let dir = TempDir::new("rate");
        let path = dir.0.join("A.wav");
        let mut bytes = transit_audio_io::wav_float_header(1, 44_100, 10).unwrap();
        bytes.extend([0u8; 40]);
        std::fs::write(&path, &bytes).unwrap();
        let err = open_disk_audio(&path, bytes.len() as u64, 48_000).unwrap_err();
        assert!(err.contains("session's rate"), "{err}");
        let flac = dir.0.join("B.flac");
        std::fs::write(&flac, b"fLaC\0\0\0\0").unwrap();
        assert!(open_disk_audio(&flac, 8, 48_000).is_err());
    }
}
