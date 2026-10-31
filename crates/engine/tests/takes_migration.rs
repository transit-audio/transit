//! Pre-#25 files migrate on open, without changing current sessions or losing recordings.

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use transit_engine::{Engine, io};
use transit_model::{ClipContent, ClipId, Frozen, Session, SourceId, TrackId};
use transit_time::Range;

const OLD: &str = include_str!("../../model/tests/corpus/v0.5.1-pre-take-lanes.transit");
const CURRENT: &str = include_str!("../../model/tests/corpus/v0.6.0-takes.transit");
const SUMMARY: &str = "2 track(s) recorded with an earlier build now show their laps as takes";

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("transit-take-migration-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn session(&self, text: &str) -> PathBuf {
        let s = Session::from_json(text).unwrap();
        let path = self.0.join(format!("{}.transit", s.name));
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn open(path: &Path) -> Engine {
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    e
}

fn from_bytes(text: &str) -> Engine {
    let mut e = Engine::default();
    io::open_session_bytes(&mut e, "takes.transit", text.as_bytes()).unwrap();
    e
}

fn assert_migrated(e: &Engine) {
    assert!(e.is_dirty());
    assert_eq!(e.undo_history(), ["Migrate takes"]);
    assert!(e.messages.iter().any(|m| m == SUMMARY), "{:?}", e.messages);
    for name in ["Vox", "Keys"] {
        let t = e.session().track_by_name(name).unwrap();
        assert_eq!(t.active_playlist, 0);
        let stacks = t.take_stacks();
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].takes.iter().map(|k| k.number).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(stacks[0].explicit());
        assert_eq!(stacks[0].active(), Some(3));
        assert!(e.messages.iter().any(|m| m.contains(&format!("track \"{name}\": migrated"))), "{:?}", e.messages);
    }
    let vox = e.session().track(TrackId(2)).unwrap();
    assert_eq!(vox.playlist().unwrap().clip_at(1_000).unwrap().take, Some(3));
    assert_eq!(vox.playlist().unwrap().clip_at(30_000).unwrap().take, Some(2), "the previous lap surrounds the partial last lap");
    assert_eq!(vox.playlist().unwrap().clip_at(200_000).unwrap().id, ClipId(5), "the rest of the original playlist plays");
    let keys = e.session().track(TrackId(9)).unwrap();
    let ClipContent::Midi { sequence } = &keys.clips()[0].content else { panic!("MIDI take") };
    assert_eq!(sequence.notes[0].pitch, 64);
}

#[test]
fn the_old_file_migrates_in_one_step_undo_restores_it_exactly_and_redo_restores_the_migration() {
    let dir = Dir::new();
    let path = dir.session(OLD);
    let mut e = open(&path);
    assert_migrated(&e);
    let after = e.session().to_json().unwrap();
    assert!(e.undo());
    assert_eq!(e.session().to_json().unwrap(), Session::from_json(OLD).unwrap().to_json().unwrap());
    assert!(!e.is_dirty());
    assert!(!e.can_undo(), "all tracks were one step");
    assert!(e.redo());
    assert_eq!(e.session().to_json().unwrap(), after);
    e.execute("session.save", &json!({})).unwrap();
    let reopened = open(&path);
    assert!(!reopened.is_dirty() && !reopened.can_undo(), "migration is idempotent");
    assert_eq!(reopened.session().to_json().unwrap(), after);
}

#[test]
fn opening_bytes_also_migrates_and_the_newest_audio_is_what_renders() {
    let mut e = from_bytes(OLD);
    assert_migrated(&e);
    // Attach deterministic decoded audio: each lap has its own constant level.
    let samples: Vec<f32> = (0..96_000).map(|i| (i / 32_000 + 1) as f32 / 10.0).collect();
    e.session_mut().pool.insert(
        SourceId(1),
        std::sync::Arc::new(transit_model::SourceAudio::new(transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![samples] })),
    );
    let level = |at| transit_mix::render_track_pre_fader(e.session(), TrackId(2), Range::new(at, at + 64))[0][32];
    let original = level(200_000);
    assert!(original > 0.0);
    assert!((level(1_000) / original - 3.0).abs() < 1e-5, "newest lap plays");
    assert!((level(30_000) / original - 2.0).abs() < 1e-5, "previous lap plays after the partial lap");
    // Every original take can still be chosen, including joining the split second lap.
    for number in 1..=3 {
        e.execute("takes.set_active", &json!({"track": 2, "take": number})).unwrap();
        let vox = e.session().track(TrackId(2)).unwrap();
        let clip = vox.playlist().unwrap().clip_at(1_000).unwrap();
        let old = Session::from_json(OLD).unwrap();
        let original = &old.track(TrackId(2)).unwrap().playlists[number as usize - 1].clips[0];
        assert!(transit_model::same_material(clip, original), "take {number} survived whole");
    }
}

#[test]
fn frozen_tracks_are_reported_and_unchanged_while_other_tracks_migrate() {
    let mut s = Session::from_json(OLD).unwrap();
    let vox = s.track_mut(TrackId(2)).unwrap();
    vox.freeze = Some(Frozen { source: SourceId(1), start: 0, length: 24_000, hash: vox.freeze_hash(), revision: 0, latency: 0 });
    let expected = vox.clone();
    let e = from_bytes(&s.to_json().unwrap());
    assert_eq!(e.session().track(TrackId(2)), Some(&expected));
    assert!(e.messages.iter().any(|m| m.contains("track \"Vox\"") && m.contains("frozen")), "{:?}", e.messages);
    assert!(e.messages.iter().any(|m| m == "1 track(s) recorded with an earlier build now show their laps as takes"));
    s.tracks.retain(|t| t.id == TrackId(2));
    let e = from_bytes(&s.to_json().unwrap());
    assert!(!e.is_dirty() && !e.can_undo());
    assert_eq!(e.session().tracks, s.tracks);
}

#[test]
fn a_single_clip_without_a_stack_or_with_only_a_copy_is_untouched() {
    for duplicate in [false, true] {
        let mut s = Session::from_json(OLD).unwrap();
        s.tracks.retain(|t| t.id == TrackId(9));
        let t = &mut s.tracks[0];
        let mut c = t.clips()[0].clone();
        c.id = ClipId(10);
        if !duplicate {
            c.start = 96_000;
        }
        t.playlists[0].clips = vec![c];
        t.playlists[1].clips.clear();
        let text = s.to_json().unwrap();
        let e = from_bytes(&text);
        assert!(!e.is_dirty() && !e.can_undo());
        assert_eq!(e.session().to_json().unwrap(), text);
    }
}

#[test]
fn current_takes_and_an_ordinary_session_load_and_save_byte_identically() {
    let dir = Dir::new();
    let mut plain = Session::default();
    plain.add_track(transit_model::TrackKind::Midi, transit_model::ChannelFormat::Mono, Some("Plain"));
    // Git can check out the corpus with CRLF; session files are written with LF on every OS.
    // Normalize the input fixture only, then compare the saved file's bytes without changes.
    for text in [CURRENT.replace("\r\n", "\n"), plain.to_json().unwrap()] {
        let path = dir.session(&text);
        let mut e = open(&path);
        assert!(!e.is_dirty() && !e.can_undo());
        e.execute("session.save", &json!({})).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), text.as_bytes());
    }
}
