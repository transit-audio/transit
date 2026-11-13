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

/// Copy the file at `from` to `to` (a disk-backed source's audio, which is never decoded), both
/// through the checked file access.
pub fn copy_path(from: &Path, to: impl AsRef<Path>) -> Result<()> {
    let to = to.as_ref();
    crate::file_access::check_read(from).map_err(EngineError::Io)?;
    crate::file_access::check_write(to).map_err(EngineError::Io)?;
    // Copying a file onto itself would truncate it before it is read.
    if std::fs::canonicalize(to).is_ok_and(|t| std::fs::canonicalize(from).is_ok_and(|f| f == t)) {
        return Err(EngineError::Io(format!("{} is the file being copied; it was not copied onto itself", to.display())));
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|err| EngineError::Io(format!("{} to {}: {err}", from.display(), to.display())))
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

#[cfg(test)]
thread_local! {
    /// When set, every path an unplaced-take entry leads to is recorded here and reported as not
    /// there, so a test sees what would reach the file system without anything being touched.
    pub(super) static UNPLACED_PROBES: std::cell::RefCell<Option<Vec<PathBuf>>> = const { std::cell::RefCell::new(None) };
}

/// Whether the take file at `path` is there: the one place an unplaced-take entry reaches the
/// file system.
fn take_file_there(path: &Path) -> bool {
    #[cfg(test)]
    if let Some(there) = UNPLACED_PROBES.with(|l| {
        l.borrow_mut().as_mut().map(|seen| {
            seen.push(path.to_path_buf());
            false
        })
    }) {
        return there;
    }
    crate::file_access::check_read(path).is_ok() && path.is_file()
}

/// The file an unplaced-take entry of an older build names (`<path> (<what it holds>)`, #192),
/// when it is still there, or why it is not looked for. The path is a stored path like every other
/// the session file `links` names: it goes through [`stored_path`] before the file system sees it,
/// so an entry naming another computer, a device or a URL is refused unopened, and a relative one
/// is taken from the session's folder, never the working directory.
fn unplaced_file(entry: &str, links: &Path) -> std::result::Result<Option<PathBuf>, String> {
    let (mut looked, mut refused) = (false, None);
    for (i, _) in entry.match_indices(" (") {
        match stored_path(&entry[..i], links) {
            Ok(path) if take_file_there(&path) => return Ok(Some(path)),
            Ok(_) => looked = true,
            Err(why) => {
                refused.get_or_insert(why);
            }
        }
    }
    // A file looked for and not found is the plain "never placed" case; only an entry with no
    // path that may be looked for says why.
    match refused {
        Some(why) if !looked => Err(why),
        _ => Ok(None),
    }
}

/// On open: place each take an older build left unplaced (#192) whose file is still there, as one
/// disk-backed clip at the session's start on a new track named after the file (`<track>_<NN>.wav`
/// gives `<track>`), so nothing already in the session is touched. `links` is the session file the
/// entries' paths are stored in. Returns what it says and whether anything was placed; the entries
/// placed leave the list, the rest stay listed.
pub(crate) fn place_unplaced(s: &mut Session, links: &Path) -> (Vec<String>, bool) {
    let mut placed_any = false;
    let mut said = Vec::new();
    let entries = std::mem::take(&mut s.unplaced_takes);
    for entry in entries {
        let file = unplaced_file(&entry, links).unwrap_or_else(|why| {
            said.push(format!("the unplaced take {entry} is not looked for: {why}"));
            None
        });
        let placed = file.and_then(|path| {
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
        let format = ChannelFormat::for_channels(usize::from(info.channels));
        // Never the take's own track: placing there would clear what is on it now.
        let tid = s.add_track(TrackKind::Audio, format, Some(track_name));
        let src = add_disk_source(s, &stem, audio, &path.to_string_lossy());
        let len = i64::try_from(info.frames).unwrap_or(i64::MAX);
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, tid, Clip::audio(cid, &stem, src, 0, 0, len));
        let track = s.track(tid).map_or_else(String::new, |t| t.name.clone());
        said.push(format!(
            "the take {} is placed now, played from disk, at the start of a new track {track}: move it where it was recorded",
            path.display()
        ));
        placed_any = true;
    }
    (said, placed_any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{open_session, save_session};

    /// Removes the directory on drop, so a failed assertion leaks nothing, and clears the faked file length.
    struct TempDir(transit_test_support::TestDir);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            TempDir(transit_test_support::TestDir::created(tag))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
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
        // A clip already on the track the take was recorded on.
        let kept = crate::io::add_source(
            e.session_mut(),
            "Kept",
            AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4_800]] },
            None,
            FileFormat::Wav,
        );
        let kept_clip = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), vox, Clip::audio(kept_clip, "Kept", kept, 0, 1_000, 4_800));
        e.session_mut().unplaced_takes =
            vec![format!("{} (6.21 h of mono at 48 kHz)", path.display()), format!("{} (6.21 h of mono at 48 kHz)", gone.display())];
        let session = dir.0.join("Old.transit").to_string_lossy().into_owned();
        save_session(&mut e, &session).unwrap();

        let mut r = Engine::default();
        open_session(&mut r, &session).unwrap();
        let s = r.session();
        assert_eq!(s.unplaced_takes, [format!("{} (6.21 h of mono at 48 kHz)", gone.display())], "the missing file stays listed");
        let src = s.sources.iter().find(|x| x.disk).unwrap();
        assert!(src.frames == frames && src.path == path.to_string_lossy(), "{src:?}");
        let old = s.track(vox).unwrap().clips();
        assert_eq!(old.len(), 1, "the recorded-on track keeps what it had");
        assert_eq!((old[0].id, old[0].start, old[0].length), (kept_clip, 1_000, 4_800));
        let new = s.tracks.last().unwrap();
        assert!(new.id != vox && new.name.starts_with("Loopback"), "{}", new.name);
        let clips = new.clips();
        assert_eq!(clips.len(), 1, "on a new track named after the file");
        assert_eq!((clips[0].start, clips[0].length, clips[0].source()), (0, frames as i64, Some(src.id)));
        assert!(r.messages.iter().any(|m| m.contains("is placed now, played from disk") && m.contains("a new track")), "{:?}", r.messages);
        assert!(r.is_dirty(), "the placement is a change the next close asks to save");
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
        // Saved elsewhere, the new folder gets its own copy of the file (#573); the original stays.
        let other = TempDir::new("elsewhere");
        let moved = other.0.join("Moved.transit").to_string_lossy().into_owned();
        crate::io::save_session(&mut r, &moved).unwrap();
        assert_eq!(r.session().sources.last().unwrap().path, "Audio Files/Take_01.wav");
        assert_eq!(std::fs::read(other.0.join("Audio Files").join("Take_01.wav")).unwrap(), bytes);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
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

    /// A short tone in a real WAV, and two engines playing it as one clip of 24 000 frames: one
    /// from memory, one forced disk-backed. (dir, path, memory engine, disk engine, track, clip)
    fn pair(tag: &str) -> (TempDir, PathBuf, Engine, Engine, TrackId, transit_model::ClipId) {
        let dir = TempDir::new(tag);
        let path = dir.0.join("Tone_01.wav");
        let data: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut bytes = transit_audio_io::wav_float_header(1, 48_000, data.len() as u64).unwrap();
        bytes.extend(data.iter().flat_map(|v| v.to_le_bytes()));
        std::fs::write(&path, &bytes).unwrap();
        let make = |disk: bool| {
            let mut e = Engine::default();
            let s = e.session_mut();
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
            let src = if disk {
                let audio = open_disk_audio(&path, bytes.len() as u64, 48_000).unwrap();
                add_disk_source(s, "Tone_01", audio, &path.to_string_lossy())
            } else {
                let (_, buf) = transit_audio_io::decode(&bytes, Some("wav")).unwrap();
                crate::io::add_source(s, "Tone_01", buf, None, FileFormat::Wav)
            };
            let id = s.new_clip_id();
            crate::edit::place_clip(s, t, Clip::audio(id, "Tone", src, 0, 0, 24_000));
            (e, t, id)
        };
        let (m, t, id) = make(false);
        let (d, _, _) = make(true);
        (dir, path, m, d, t, id)
    }

    /// The audio of the clip `id` now plays (its source is in memory: a render, never empty).
    fn rendered(e: &Engine, id: transit_model::ClipId) -> Vec<Vec<f32>> {
        let src = e.session().find_clip(id).unwrap().1.source().unwrap();
        let audio = e.session().pool.get(src).unwrap();
        assert!(!audio.is_disk(), "a render is a new source in memory");
        audio.buffer.channels.clone()
    }

    fn same_render(m: &Engine, d: &Engine, id: transit_model::ClipId, what: &str) {
        let (a, b) = (rendered(m, id), rendered(d, id));
        assert!(a[0].iter().any(|v| v.abs() > 0.1), "{what}: the reference is not silent");
        assert_eq!(a, b, "{what}: read from the file, the render equals the in-memory one");
    }

    #[test]
    fn elastic_warp_of_a_disk_backed_clip_renders_its_audio_not_silence() {
        let (_dir, _, mut m, mut d, t, id) = pair("elastic");
        for e in [&mut m, &mut d] {
            e.execute("track.elastic", &serde_json::json!({"track": t.0, "algorithm": "polyphonic"})).unwrap();
            assert_eq!(e.execute("clip.elastic_properties", &serde_json::json!({"clips": [id.0], "ratio": 1.5})).unwrap()["rendered"], 1);
        }
        same_render(&m, &d, id, "elastic warp");
    }

    #[test]
    fn remove_pitch_shift_of_a_disk_backed_clip_renders_its_audio_not_silence() {
        let (_dir, _, mut m, mut d, _, id) = pair("pitch");
        for e in [&mut m, &mut d] {
            e.session_mut().find_clip_mut(id).unwrap().stretch = 1.25;
            assert_eq!(e.execute("clip.remove_pitch_shift", &serde_json::json!({"clips": [id.0]})).unwrap()["clips"], 1);
        }
        same_render(&m, &d, id, "remove pitch shift");
    }

    #[test]
    fn tce_to_timeline_of_a_disk_backed_clip_renders_its_audio_not_silence() {
        let (_dir, _, mut m, mut d, _, id) = pair("tce");
        for e in [&mut m, &mut d] {
            e.execute("edit.tce_to_timeline", &serde_json::json!({"clips": [id.0], "start": 1_000, "end": 31_000})).unwrap();
        }
        same_render(&m, &d, id, "TCE to timeline");
    }

    #[test]
    fn a_clip_group_export_of_a_disk_backed_clip_carries_its_file() {
        let (dir, path, _, mut d, _, id) = pair("group");
        let group = dir.0.join("g.scgrp").to_string_lossy().into_owned();
        d.execute("file.export_clip_groups", &serde_json::json!({"path": group, "clips": [id.0]})).unwrap();
        let copied = dir.0.join("g Audio").join(format!("Tone_01_{}.wav", d.session().sources[0].id.0));
        assert_eq!(std::fs::read(&copied).unwrap(), std::fs::read(&path).unwrap(), "the take's file, not an empty WAV");
    }

    #[test]
    fn a_file_is_never_copied_onto_itself() {
        let dir = TempDir::new("self-copy");
        let path = dir.0.join("Take_01.wav");
        std::fs::write(&path, b"RIFF....").unwrap();
        let err = copy_path(&path, dir.0.join(".").join("Take_01.wav")).unwrap_err();
        assert!(err.to_string().contains("onto itself"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"RIFF....", "untouched");
        copy_path(&path, dir.0.join("Copy.wav")).unwrap();
        assert_eq!(std::fs::read(dir.0.join("Copy.wav")).unwrap(), b"RIFF....");
    }

    #[test]
    fn a_re_placed_take_of_more_than_two_channels_gets_a_track_of_its_width() {
        let dir = TempDir::new("wide");
        let path = dir.0.join("Room_01.wav");
        let mut bytes = transit_audio_io::wav_float_header(6, 48_000, 100).unwrap();
        bytes.extend([0u8; 6 * 4 * 100]);
        std::fs::write(&path, &bytes).unwrap();
        let mut s = Session { unplaced_takes: vec![format!("{} (0.00 h of 6-channel at 48 kHz)", path.display())], ..Session::default() };
        let (_, placed) = place_unplaced(&mut s, &dir.0.join("S.transit"));
        assert!(placed);
        let t = s.tracks.last().unwrap();
        assert_eq!((t.format, t.channels()), (ChannelFormat::for_channels(6), 6), "{}", t.name);
    }

    /// Records the paths unplaced-take entries lead to instead of touching them, until dropped.
    struct Probes;
    impl Probes {
        fn on() -> Probes {
            UNPLACED_PROBES.with(|l| *l.borrow_mut() = Some(Vec::new()));
            Probes
        }
        fn take(&self) -> Vec<PathBuf> {
            UNPLACED_PROBES.with(|l| l.borrow_mut().as_mut().map(std::mem::take).unwrap_or_default())
        }
    }
    impl Drop for Probes {
        fn drop(&mut self) {
            UNPLACED_PROBES.with(|l| *l.borrow_mut() = None);
        }
    }

    const HOLDS: &str = " (8.00 h of mono at 48 kHz)";

    /// Entries a hostile session could hold: other computers, devices, URLs, a drive the session is
    /// not on, and climbs above the root, in Windows and Unix spellings and mixed separators.
    fn hostile_entries(session: &Path) -> Vec<String> {
        let drive = session.to_str().and_then(|s| s.chars().next()).filter(|_| cfg!(windows)).unwrap_or('C').to_ascii_uppercase();
        let other = if drive == 'Q' { 'R' } else { 'Q' };
        let mut out: Vec<String> = [
            r"\\host\share\Take_01.wav",
            "//host/share/Take_01.wav",
            r"\/host/share\Take_01.wav",
            r"/\host\share/Take_01.wav",
            r"\\host\share\..\..\Take_01.wav",
            r"\\?\UNC\host\share\Take_01.wav",
            "//?/UNC/host/share/Take_01.wav",
            r"\\.\UNC\host\share\Take_01.wav",
            r"\\.\pipe\Take_01",
            r"\\.\C:\Take_01.wav",
            r"\\?\GLOBALROOT\Device\Mup\host\share\Take_01.wav",
            r"\??\C:\Take_01.wav",
            "file://host/share/Take_01.wav",
            "file:////host/share/Take_01.wav",
            "FILE://host/Take_01.wav",
            "file:%5C%5Chost%5Cshare%5CTake_01.wav",
            "smb://host/share/Take_01.wav",
            "https://host/Take_01.wav",
            r"http:\\host\Take_01.wav",
            "webdav://host/Take_01.wav",
            "sftp://host/Take_01.wav",
            "ms-settings:Take_01.wav",
            "search-ms:query=Take_01",
            "NUL",
            "CON.wav",
            r"Audio Files\COM1.wav",
            "Audio Files/LPT1",
            "/../Take_01.wav",
            "Take_01.wav\u{7}",
        ]
        .map(String::from)
        .into();
        out.push(format!("{other}:Take_01.wav"));
        out.push(format!("{}Take_01.wav", "../".repeat(200)));
        out.push(format!(r"Audio Files\{}Take_01.wav", r"..\".repeat(200)));
        out
    }

    #[test]
    fn an_unplaced_take_naming_another_computer_a_device_or_a_url_is_refused_unopened() {
        let dir = TempDir::new("hostile-takes");
        let session = dir.0.join("Song.transit");
        let probes = Probes::on();
        for path in hostile_entries(&session) {
            let entry = format!("{path}{HOLDS}");
            let mut s = Session { unplaced_takes: vec![entry.clone()], ..Session::default() };
            let (said, placed) = place_unplaced(&mut s, &session);
            assert_eq!(probes.take(), Vec::<PathBuf>::new(), "{path:?}: nothing reaches the file system");
            assert!(!placed && s.tracks.is_empty() && s.sources.is_empty(), "{path:?}");
            assert_eq!(s.unplaced_takes, std::slice::from_ref(&entry), "{path:?}: the take stays listed");
            assert!(said.iter().any(|m| m.contains(&entry) && m.contains("is not looked for: not opened")), "{path:?}: {said:?}");
        }
    }

    #[test]
    fn an_unplaced_take_with_a_relative_path_is_looked_for_in_the_session_folder_only() {
        let dir = TempDir::new("relative-takes");
        let session = dir.0.join("Song.transit");
        let take = dir.0.join("Audio Files").join("Take_01.wav");
        let probes = Probes::on();
        let entries = [
            "Audio Files/Take_01.wav".to_string(),
            r"Audio Files\Take_01.wav".to_string(),
            r"./Audio Files/..\Audio Files/Take_01.wav".to_string(),
            take.display().to_string(),
        ];
        for path in entries {
            let entry = format!("{path}{HOLDS}");
            let mut s = Session { unplaced_takes: vec![entry.clone()], ..Session::default() };
            let (said, placed) = place_unplaced(&mut s, &session);
            assert_eq!(probes.take(), std::slice::from_ref(&take), "{path:?}: taken from the session's folder, never the working directory");
            assert!(!placed && said.is_empty(), "{path:?}: a file not there is the plain never-placed case: {said:?}");
            assert_eq!(s.unplaced_takes, [entry], "the take stays listed");
        }
        // On the session's own network share the entry is the session's, as for every stored path
        // (checked without touching it: a share path is not absolute on Unix, so refused there).
        let shared = Path::new(r"\\host\share\Song\Song.transit");
        let mut s = Session { unplaced_takes: vec![format!(r"\\host\share\Song\Audio Files\Take_01.wav{HOLDS}")], ..Session::default() };
        let (said, _) = place_unplaced(&mut s, shared);
        if cfg!(windows) {
            assert_eq!(probes.take(), [PathBuf::from(r"\\host\share\Song\Audio Files\Take_01.wav")], "{said:?}");
        } else {
            assert!(probes.take().is_empty() && said.iter().any(|m| m.contains("is not looked for")), "{said:?}");
        }
        // A take on another share than the session's is another computer.
        let mut s = Session { unplaced_takes: vec![format!(r"\\host\other\Take_01.wav{HOLDS}")], ..Session::default() };
        let (said, _) = place_unplaced(&mut s, shared);
        assert!(probes.take().is_empty() && said.iter().any(|m| m.contains("the path is on another computer")), "{said:?}");
    }

    #[test]
    fn a_session_whose_unplaced_take_names_another_computer_opens_and_lists_it_untouched() {
        let dir = TempDir::new("hostile-open");
        let mut e = Engine::default();
        let remote = format!(r"\\host\share\Take_01.wav{HOLDS}");
        let near = format!("Take_02.wav{HOLDS}");
        e.session_mut().unplaced_takes = vec![remote.clone(), near.clone()];
        let session = dir.0.join("Hostile.transit");
        save_session(&mut e, &session.to_string_lossy()).unwrap();

        let probes = Probes::on();
        let mut r = Engine::default();
        open_session(&mut r, &session.to_string_lossy()).unwrap();
        assert_eq!(probes.take(), [dir.0.join("Take_02.wav")], "only the session folder's file is looked for");
        assert_eq!(r.session().unplaced_takes, [remote.clone(), near.clone()], "both stay listed");
        assert!(r.messages.iter().any(|m| m.contains(&remote) && m.contains("is not looked for: not opened")), "{:?}", r.messages);
        for entry in [&remote, &near] {
            assert!(r.messages.iter().any(|m| m.contains(transit_model::UNPLACED_NOTE) && m.contains(entry.as_str())), "{:?}", r.messages);
        }
        assert!(!r.is_dirty(), "nothing was placed, so the session opens as saved");
    }
}
