//! Every file a saved session names is in its own folder after Save, Save As, Save Copy In, the
//! first save of an untitled session and a save after crash recovery (#572, #573). A long take is
//! made the way a recording makes one: a take file over the stream setting, which these tests set
//! to half a second (`disk.stream_minutes`), so no test needs a ten-minute file. No devices, no clock.

use super::*;
use crate::autosave::{Autosaver, discard, leftovers, recover};
use crate::io::{add_recording, open_session, save_session, save_session_copy};
use serde_json::json;

/// A scratch folder from the test guard, removed when the test ends, pass or fail.
struct Scratch(transit_test_support::TestDir);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        Scratch(transit_test_support::TestDir::created(&format!("save-media-{tag}")))
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn file(&self, rel: &str) -> String {
        self.path(rel).to_string_lossy().into_owned()
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
    record_long_of(e, t, path, 48_000)
}

/// [`record_long`] with a take of `frames` frames (over half a second).
fn record_long_of(e: &mut Engine, t: TrackId, path: &Path, frames: usize) -> Vec<u8> {
    let bytes = write_take(path, frames);
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

#[test]
fn two_different_takes_of_the_same_length_are_never_taken_for_each_other_on_save_issue_573() {
    // 300,000 mono float frames: 1.2 MB, past the first 1 MiB chunk of the comparison, so a
    // difference beyond it is found only by reading on across the chunk boundary.
    const FRAMES: usize = 300_000;
    let header = transit_audio_io::wav_float_header(1, 48_000, FRAMES as u64).unwrap().len();
    let len = header + FRAMES * 4;
    assert!(len > CHUNK + 4_096, "the take spans more than one chunk");
    // Where the file already in the target `Audio Files` differs from the take: one sample early
    // in the file, only the last byte, only a byte beyond the first chunk; `None`: the same bytes.
    for (what, flip) in
        [("a sample", Some(header + 4 * 1_000)), ("past the first chunk", Some(CHUNK + 4_096)), ("the last byte", Some(len - 1)), ("nothing", None)]
    {
        let Some((mut e, t)) = engine() else { return };
        let dir = Scratch::new("same-length");
        save_session(&mut e, &dir.file("A/Song.transit")).unwrap();
        let take = record_long_of(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"), FRAMES);
        assert_eq!(take.len(), len);
        save_session(&mut e, &dir.file("A/Song.transit")).unwrap();
        // A file of the take's name, length and header is already in the new folder.
        let mut there = take.clone();
        if let Some(k) = flip {
            there[k] ^= 0x01;
        }
        let existing = dir.path("B/Audio Files/Vox_01.wav");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, &there).unwrap();

        let done = e.execute("session.save_as", &json!({"path": dir.file("B/Song.transit")})).unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), there, "{what}: the file already there is never replaced");
        let copy = dir.path("B/Audio Files/Vox_01_01.wav");
        if flip.is_some() {
            assert_eq!(done["audio_files_written"], 1, "{what}: {done}");
            assert_eq!(source_path(&e, "Vox_01"), "Audio Files/Vox_01_01.wav", "{what}: the session names its own copy");
            assert_eq!(std::fs::read(&copy).unwrap(), take, "{what}: the copy is the take");
        } else {
            assert_eq!(done["audio_files_written"], 0, "{what}: {done}");
            assert_eq!(source_path(&e, "Vox_01"), "Audio Files/Vox_01.wav", "{what}: the identical file is reused");
            assert!(!copy.exists(), "{what}: nothing new is written");
        }
    }
}

/// The disk-backed audio the source `name` plays.
fn disk_of(e: &Engine, name: &str) -> Arc<transit_model::disk::DiskSource> {
    let src = e.session().sources.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no source {name}"));
    e.session().pool.get(src.id).and_then(|a| a.disk.clone()).unwrap_or_else(|| panic!("{name} is not disk-backed"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).ok().is_some_and(|x| std::fs::canonicalize(b).ok().is_some_and(|y| x == y))
}

/// The long take `name` (a [`write_take`] ramp of `frames` frames) plays and renders: every long
/// take primes, its ring holds the take's samples, and an offline render's check finds it readable.
fn plays(e: &Engine, name: &str, frames: usize) {
    assert_eq!(transit_model::disk::prime_session(e.session(), 0), 0, "{name}: every long take is ready to play");
    let src = e.session().sources.iter().find(|s| s.name == name).unwrap().id;
    let clip = e.session().tracks.iter().flat_map(|t| t.clips().iter()).find(|c| c.source() == Some(src)).unwrap();
    let map = transit_model::disk::StreamMap::of(clip).unwrap();
    let disk = disk_of(e, name);
    let v = disk.stream(&map, 0, 64).unwrap_or_else(|| panic!("{name}: no ring after the prime"));
    for k in 0..64 {
        let want = map.sample(k, |x| (x < frames).then(|| x as f32 / frames as f32 - 0.5));
        assert_eq!(v.get(0, k), want, "{name}: clip time {k}");
    }
    drop(v);
    let (_, lines) = crate::io::DiskReads::check(e.session(), None, transit_time::Range::new(0, frames as i64));
    assert_eq!(lines, Vec::<String>::new(), "{name}: an offline render can read it");
    assert_eq!(disk.fault(), None, "{name}");
}

/// Move the folder at `from` away and delete it, as a user does with a session folder they no
/// longer need. On Windows a folder holding a file some reader still has open cannot be renamed.
fn move_away_and_delete(dir: &Scratch, from: &str) {
    let gone = dir.path(&format!("{from} moved"));
    std::fs::rename(dir.path(from), &gone).unwrap_or_else(|e| panic!("the folder {from} could not be moved: {e}"));
    std::fs::remove_dir_all(&gone).unwrap_or_else(|e| panic!("the folder {from} could not be deleted: {e}"));
}

#[test]
fn after_save_as_a_long_take_plays_from_its_copy_and_the_old_folder_can_go_while_the_session_is_open_issue_629() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("follow");
    let a = dir.file("A/Song.transit");
    save_session(&mut e, &a).unwrap();
    record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    save_session(&mut e, &a).unwrap();
    // The take plays and has been read offline: readers of the old file are open.
    plays(&e, "Vox_01", 48_000);
    disk_of(&e, "Vox_01").with_sync(|get| assert!(get(0, 100).is_some()));
    // An edit the save does not end: undo and redo reach across the save.
    e.execute("mix.volume", &json!({"tracks": [t.0], "db": -6.0})).unwrap();

    let b = dir.file("B/Song.transit");
    e.execute("session.save_as", &json!({"path": b})).unwrap();
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &dir.path("B/Audio Files/Vox_01.wav")), "the open session reads the copy");
    move_away_and_delete(&dir, "A");
    plays(&e, "Vox_01", 48_000);

    assert!(e.undo());
    plays(&e, "Vox_01", 48_000);
    assert_eq!(save_session(&mut e, &b).unwrap(), 0, "a save while the edit is undone needs nothing from the old folder");
    assert!(e.redo());
    plays(&e, "Vox_01", 48_000);
    assert_eq!(save_session(&mut e, &b).unwrap(), 0);

    let c = dir.file("C/Song.transit");
    let done = e.execute("session.save_as", &json!({"path": c})).unwrap();
    assert_eq!(done["audio_files_written"], 1, "a further Save As copies the take from the folder it plays from: {done}");
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &dir.path("C/Audio Files/Vox_01.wav")));
    move_away_and_delete(&dir, "B");
    plays(&e, "Vox_01", 48_000);
}

#[test]
fn a_save_copies_from_the_file_the_session_names_when_the_one_played_is_gone_issue_629() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("named");
    let a = dir.file("A/Song.transit");
    save_session(&mut e, &a).unwrap();
    let bytes = record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    save_session(&mut e, &a).unwrap();
    e.execute("session.save_as", &json!({"path": dir.file("B/Song.transit")})).unwrap();
    move_away_and_delete(&dir, "A");
    // A take left reading its original, now gone, while the session names its copy (what every
    // build before this fix did after Save As).
    let disk = disk_of(&e, "Vox_01");
    disk.retarget(dir.path("A/Audio Files/Vox_01.wav"), Arc::new(|| Err("the folder was deleted".to_string())));

    let done = e.execute("session.save_as", &json!({"path": dir.file("C/Song.transit")})).unwrap();
    assert_eq!(done["audio_files_written"], 1, "{done}");
    assert_eq!(std::fs::read(dir.path("C/Audio Files/Vox_01.wav")).unwrap(), bytes, "copied from the file the session names");
    assert!(same_file(&disk.path(), &dir.path("C/Audio Files/Vox_01.wav")), "and played from its copy");
    plays(&e, "Vox_01", 48_000);
}

#[test]
fn a_save_that_fails_half_way_leaves_the_open_session_reading_the_files_it_had_issue_629() {
    let Some((mut e, t)) = engine() else { return };
    let g = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Gtr"));
    let dir = Scratch::new("half-way");
    let a = dir.file("A/Song.transit");
    save_session(&mut e, &a).unwrap();
    record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    record_long(&mut e, g, &dir.path("A/Audio Files/Gtr_01.wav"));
    save_session(&mut e, &a).unwrap();
    // The second take's file goes: Save As copies the first, then fails on the second.
    std::fs::remove_file(dir.path("A/Audio Files/Gtr_01.wav")).unwrap();
    let err = save_session(&mut e, &dir.file("B/Song.transit")).unwrap_err().to_string();
    assert!(err.contains("Gtr_01.wav"), "{err}");
    assert!(dir.path("B/Audio Files/Vox_01.wav").is_file(), "the first take was copied before the failure");
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &dir.path("A/Audio Files/Vox_01.wav")), "the open session still reads its own file");
    assert_eq!(e.path.as_deref(), Some(a.as_str()));
}

#[test]
fn a_take_recorded_before_the_first_save_plays_from_the_session_folder_and_save_copy_in_changes_nothing_issue_629() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("first-save");
    record_long(&mut e, t, &dir.path("Unsaved/Untitled/Audio Files/Vox_01.wav"));
    plays(&e, "Vox_01", 48_000);
    save_session(&mut e, &dir.file("Song/Song.transit")).unwrap();
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &dir.path("Song/Audio Files/Vox_01.wav")));
    move_away_and_delete(&dir, "Unsaved");
    plays(&e, "Vox_01", 48_000);

    assert_eq!(save_session_copy(&e, &dir.file("Copy/Copy.transit")).unwrap(), 1);
    assert!(
        same_file(&disk_of(&e, "Vox_01").path(), &dir.path("Song/Audio Files/Vox_01.wav")),
        "Save Copy In leaves the session reading its own files"
    );
    move_away_and_delete(&dir, "Copy");
    plays(&e, "Vox_01", 48_000);
}

#[test]
fn a_take_whose_move_fails_at_save_as_is_said_and_moved_at_the_next_plain_save_issue_629() {
    let Some((mut e, t)) = engine() else { return };
    let dir = Scratch::new("refollow");
    let a = dir.file("A/Song.transit");
    save_session(&mut e, &a).unwrap();
    record_long(&mut e, t, &dir.path("A/Audio Files/Vox_01.wav"));
    save_session(&mut e, &a).unwrap();
    plays(&e, "Vox_01", 48_000);
    let old = disk_of(&e, "Vox_01").path();
    let copy = dir.path("B/Audio Files/Vox_01.wav");

    // Save As whose move onto the copy fails (the copy cannot be opened right after it is written).
    let b = dir.file("B/Song.transit");
    let mark = e.message_pushes();
    let refuse: Follow = &|_, _| Err("the copy is locked".to_string());
    assert_eq!(crate::io::session::save_session_following(&mut e, &b, refuse).unwrap(), 1, "the save itself succeeds and copies the take");
    let said = e.messages_since(mark).to_vec();
    assert_eq!(said.len(), 1, "one note for the one take: {said:?}");
    let note = &said[0];
    assert!(note.starts_with("The long take \"Vox_01\" still plays from "), "{note}");
    assert!(note.contains(&old.display().to_string()), "it names the file the take still reads: {note}");
    assert!(note.contains("could not be opened (the copy is locked). The next save tries again."), "{note}");
    assert_eq!(e.path.as_deref(), Some(b.as_str()));
    assert!(copy.is_file());
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &old), "a failed move leaves the take on the file it read");
    plays(&e, "Vox_01", 48_000);

    // Save Copy In and the autosave copy never move it, even with a move owed.
    save_session_copy(&e, &dir.file("Copy/Copy.transit")).unwrap();
    crate::io::save_copy(&e, &dir.file("Auto/Auto.transit")).unwrap();
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &old), "Save Copy In and autosave leave the take where it reads");

    // A plain Save into the same folder: the copy is there already, nothing is copied, and the
    // take is moved onto it without a note.
    let mark = e.message_pushes();
    assert_eq!(save_session(&mut e, &b).unwrap(), 0);
    assert_eq!(e.messages_since(mark), &[] as &[String]);
    assert!(same_file(&disk_of(&e, "Vox_01").path(), &copy), "the next plain save moves the take onto its copy");
    move_away_and_delete(&dir, "Copy");
    move_away_and_delete(&dir, "A");
    plays(&e, "Vox_01", 48_000);
}
