//! Every file a saved session names is in its own folder after Save, Save As, Save Copy In, the
//! first save of an untitled session and a save after crash recovery (#572, #573). A long take is
//! made the way a recording makes one: a take file over the stream setting, which these tests set
//! to half a second (`disk.stream_minutes`), so no test needs a ten-minute file. No devices, no clock.

use super::*;
use crate::autosave::{Autosaver, discard, leftovers, recover};
use crate::io::{add_recording, open_session, save_session, save_session_copy};
use serde_json::json;

/// A scratch folder, removed when the test ends, pass or fail.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("transit-save-media-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn file(&self, rel: &str) -> String {
        self.path(rel).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A mono float take of `frames` frames at 48 kHz; returns its bytes.
fn write_take(path: &Path, frames: usize) -> Vec<u8> {
    let mut bytes = transit_audio_io::wav_float_header(1, 48_000, frames as u64).unwrap();
    bytes.extend((0..frames).flat_map(|i| (i as f32 / frames as f32 - 0.5).to_le_bytes()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

/// An engine whose takes over half a second play from disk; `None` when the process-wide override
/// is set (it would decide instead of the session's setting).
fn engine() -> Option<(Engine, TrackId)> {
    if std::env::var(transit_model::disk::STREAM_MINUTES_ENV).is_ok() {
        return None;
    }
    let mut e = Engine::default();
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.5 / 60.0);
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    Some((e, t))
}

/// Record the take file at `path` (one second: over the setting) onto `t`; it is placed disk-backed.
fn record_long(e: &mut Engine, t: TrackId, path: &Path) -> Vec<u8> {
    let bytes = write_take(path, 48_000);
    assert_eq!(add_recording(e, 0, vec![(t, path.to_path_buf())], None).len(), 1);
    let src = e.session().sources.last().unwrap();
    assert!(src.disk && e.session().pool.get(src.id).is_some_and(|a| a.is_disk()), "a long take plays from its file");
    bytes
}

/// Open `path`; the engine and what it reported missing.
fn reopen(path: &str) -> (Engine, Vec<String>) {
    let mut r = Engine::default();
    let missing = open_session(&mut r, path).unwrap();
    (r, missing)
}

fn source_path(e: &Engine, name: &str) -> String {
    e.session().sources.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no source {name}")).path.clone()
}

#[test]
fn save_as_to_another_folder_copies_a_long_take_and_the_first_folder_can_go_issue_573() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("save-as");
    let a = dir.file("A/Song.transit");
    save_session(&mut e, &a).unwrap();
    let bytes = record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    assert_eq!(save_session(&mut e, &a).unwrap(), 0, "a take already in the folder is not copied");
    assert_eq!(source_path(&e, "Vox_01"), "Audio Files/Vox_01.wav");

    let b = dir.file("B/Song.transit");
    let done = e.execute("session.save_as", &json!({"path": b})).unwrap();
    assert_eq!(done["audio_files_written"], 1, "{done}");
    assert_eq!(source_path(&e, "Vox_01"), "Audio Files/Vox_01.wav", "the session names its own copy");
    assert_eq!(std::fs::read(dir.path("B/Audio Files/Vox_01.wav")).unwrap(), bytes, "a complete, byte-for-byte copy");
    assert_eq!(save_session(&mut e, &b).unwrap(), 0, "a save in place copies nothing again");
    drop(e);

    std::fs::remove_dir_all(dir.path("A")).unwrap();
    let (r, missing) = reopen(&b);
    assert_eq!(missing, Vec::<String>::new(), "nothing the session names was in the first folder");
    let src = r.session().sources.iter().find(|s| s.name == "Vox_01").unwrap();
    assert!(src.disk && r.session().pool.get(src.id).is_some_and(|a| a.is_disk() && a.frames() == 48_000));
}

#[test]
fn the_first_save_and_a_save_copy_bring_in_a_long_take_from_the_unsaved_folder_issue_573() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("untitled");
    // Where an untitled session records (the host's unsaved folder under the autosave root).
    let bytes = record_long(&mut e, t, &dir.path("Unsaved/Untitled/Audio Files/Vox_01.wav"));
    assert!(Path::new(&source_path(&e, "Vox_01")).is_absolute());

    let song = dir.file("Song/Song.transit");
    assert_eq!(save_session(&mut e, &song).unwrap(), 1);
    assert_eq!(source_path(&e, "Vox_01"), "Audio Files/Vox_01.wav");
    assert_eq!(std::fs::read(dir.path("Song/Audio Files/Vox_01.wav")).unwrap(), bytes);
    let copy = dir.file("Copy/Copy.transit");
    assert_eq!(save_session_copy(&e, &copy).unwrap(), 1, "Save Copy In makes a normal session folder");
    assert_eq!(std::fs::read(dir.path("Copy/Audio Files/Vox_01.wav")).unwrap(), bytes);
    drop(e);

    std::fs::remove_dir_all(dir.path("Unsaved")).unwrap();
    let (_, missing) = reopen(&song);
    assert_eq!(missing, Vec::<String>::new());
    std::fs::remove_dir_all(dir.path("Song")).unwrap();
    let (r, missing) = reopen(&copy);
    assert_eq!(missing, Vec::<String>::new(), "the copy needs neither the unsaved folder nor the session's");
    assert_eq!(source_path(&r, "Vox_01"), "Audio Files/Vox_01.wav");
}

#[test]
fn a_save_after_recovery_keeps_long_take_and_missing_paths_in_the_session_folder_issue_572() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("recover");
    let song = dir.file("Song/Song.transit");
    save_session(&mut e, &song).unwrap();
    record_long(&mut e, t, &dir.path("Song/Audio Files/Vox_01.wav"));
    let gone = add_source(e.session_mut(), "Gone", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 480]] }, None, FileFormat::Wav);
    let id = e.session_mut().new_clip_id();
    crate::edit::place_clip(e.session_mut(), t, Clip::audio(id, "Gone", gone, 96_000, 0, 480));
    save_session(&mut e, &song).unwrap();
    assert_eq!(source_path(&e, "Gone"), "Audio Files/Gone.wav");
    drop(e);
    // A file that is offline when the session is opened, and stays so through the crash.
    std::fs::remove_file(dir.path("Song/Audio Files/Gone.wav")).unwrap();
    let (mut r, missing) = reopen(&song);
    assert_eq!(missing.len(), 1, "{missing:?}");

    // An edit, autosaved; the app goes down without saving.
    r.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();
    let root = dir.path("Autosave");
    let w = Autosaver::new(root.clone());
    w.write(r.session_arc(), r.path.clone(), None);
    w.flush().unwrap();
    drop(w);
    drop(r);

    let left = leftovers(&root);
    assert_eq!(left.len(), 1);
    let mut x = Engine::default();
    recover(&mut x, &left[0]).unwrap();
    // The host deletes the leftover folder once it is recovered.
    discard(&left[0]);
    assert!(!left[0].dir.exists());
    assert_eq!(x.path.as_deref(), Some(song.as_str()));
    save_session(&mut x, &song).unwrap();
    assert_eq!(source_path(&x, "Vox_01"), "Audio Files/Vox_01.wav", "the long take keeps its path in the session folder");
    assert_eq!(source_path(&x, "Gone"), "Audio Files/Gone.wav", "the offline file keeps its path in the session folder");
    drop(x);

    let (r, missing) = reopen(&song);
    assert_eq!(missing.len(), 1, "only the offline file, as before the crash: {missing:?}");
    assert!(missing[0].contains("Gone.wav") && !missing[0].contains("Autosave"), "{missing:?}");
    assert_eq!(r.session().track(t).unwrap().mixer.volume_db, -6.0, "the recovered edit was saved");
    let src = r.session().sources.iter().find(|s| s.name == "Vox_01").unwrap();
    assert!(r.session().pool.get(src.id).is_some_and(|a| a.is_disk() && a.frames() == 48_000), "the long take opens from its file");
}

#[test]
fn a_save_that_cannot_copy_a_long_take_fails_naming_it_and_keeps_the_previous_session_file() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("fails");
    save_session(&mut e, &dir.file("A/Song.transit")).unwrap();
    record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    save_session(&mut e, &dir.file("A/Song.transit")).unwrap();
    // A session already at the target, which a failed save must leave as it is.
    let b = dir.file("B/Song.transit");
    save_session(&mut Engine::default(), &b).unwrap();
    let before = std::fs::read(&b).unwrap();
    // The take's file goes while the session is open (its folder deleted, a drive unplugged).
    std::fs::remove_file(dir.path("A/Audio Files/Vox_01.wav")).unwrap();

    let err = save_session(&mut e, &b).unwrap_err().to_string();
    assert!(err.contains("Vox_01.wav") && err.contains("could not be copied"), "{err}");
    assert_eq!(std::fs::read(&b).unwrap(), before, "the previous session file is intact");
    let left: Vec<_> = std::fs::read_dir(dir.path("B/Audio Files")).map(|d| d.flatten().map(|f| f.file_name()).collect()).unwrap_or_default();
    assert!(left.is_empty(), "nothing half-copied is left: {left:?}");
    assert_eq!(e.path.as_deref(), Some(dir.file("A/Song.transit").as_str()), "the open session still belongs to its folder");
}

#[test]
fn a_copy_never_replaces_a_file_reuses_an_identical_one_and_is_checked_against_its_source() {
    let dir = Scratch::new("bring");
    let take = dir.path("Elsewhere/Vox_01.wav");
    let bytes = write_take(&take, 4_800);
    let audio = dir.path("S/Audio Files");
    std::fs::create_dir_all(&audio).unwrap();
    // Another take of the same name is already in the folder: kept, the copy gets the next name.
    let other = write_take(&audio.join("Vox_01.wav"), 480);
    assert_eq!(bring_into(&audio, &take, 4_800, 1).unwrap(), ("Vox_01_01.wav".to_string(), true));
    assert_eq!(std::fs::read(audio.join("Vox_01.wav")).unwrap(), other, "never replaced");
    assert_eq!(std::fs::read(audio.join("Vox_01_01.wav")).unwrap(), bytes);
    // The same bytes already there are used as they are; the file itself, too.
    assert_eq!(bring_into(&audio, &take, 4_800, 1).unwrap(), ("Vox_01_01.wav".to_string(), false));
    assert_eq!(bring_into(&audio, &audio.join("Vox_01.wav"), 480, 1).unwrap(), ("Vox_01.wav".to_string(), false));
    // A file in the session folder or its `Audio Files` is the session's own; one deeper is not.
    assert_eq!(in_session_folder(&dir.path("S"), &audio.join("Vox_01.wav")).as_deref(), Some("Audio Files/Vox_01.wav"));
    assert_eq!(in_session_folder(&dir.path("Elsewhere"), &take).as_deref(), Some("Vox_01.wav"));
    assert_eq!(in_session_folder(&dir.0, &take), None, "a folder holding the take's folder does not own it");
    assert_eq!(in_session_folder(&dir.path("S"), &take), None);
    // A copy that does not read back as the source's audio is refused and leaves nothing.
    std::fs::create_dir_all(dir.path("T/Audio Files")).unwrap();
    let err = bring_into(&dir.path("T/Audio Files"), &take, 4_801, 1).map(|_| ()).unwrap_err().to_string();
    assert!(err.contains("Vox_01.wav") && err.contains("reads as 4800 frames"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path("T/Audio Files")).unwrap().count(), 0, "no reservation or partial copy is left");
}
