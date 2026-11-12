//! Replacing the session (open, new, close, revert) forgets copied audio clips, whose sources
//! belong to the old session's pool (#476). Copies made of values stay, and the same document
//! (save as, undo, redo) keeps its clipboard.

use crate::Engine;
use serde_json::json;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, Session, TrackKind};
use transit_time::TICKS_PER_QUARTER;

/// A session with one audio clip (frames 0..24000) on its track; the clip's range is selected.
fn session_with_audio() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let tone: Vec<f32> = (0..24_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let src = crate::io::add_source(&mut s, "tone", AudioBuffer { sample_rate: 48_000, channels: vec![tone] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, Clip::audio(id, "tone", src, 0, 0, 24_000));
    s.edit.selected_tracks = vec![t];
    s
}

fn copy_all(e: &mut Engine) {
    e.execute("edit.copy", &json!({"start": 0, "end": 24_000})).unwrap();
    assert!(!e.clipboard.tracks.is_empty());
}

fn clip_count(e: &Engine) -> usize {
    e.session().tracks.iter().filter_map(|t| t.playlist()).map(|p| p.clips.len()).sum()
}

/// What Paste answers on an engine that never copied anything.
fn empty_answer() -> String {
    let mut e = Engine::new(session_with_audio());
    e.execute("edit.paste", &json!({"at": 30_000})).unwrap_err().to_string()
}

/// Asserts the paste is refused as an empty clipboard, leaving the document and history alone.
fn assert_paste_refused(e: &mut Engine) {
    let clips = clip_count(e);
    let pushes = e.undo_pushes();
    let rev = e.session().clone();
    let err = e.execute("edit.paste", &json!({"at": 30_000})).unwrap_err().to_string();
    assert_eq!(err, empty_answer());
    assert_eq!(clip_count(e), clips);
    assert_eq!(e.undo_pushes(), pushes);
    assert!(crate::same_document(&rev, e.session()), "the session is unchanged");
}

fn other_session() -> Session {
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s
}

#[test]
fn replacing_the_session_forgets_copied_audio_and_paste_answers_as_empty() {
    let mut e = Engine::new(session_with_audio());
    copy_all(&mut e);
    e.replace_session(other_session());
    assert!(e.clipboard.tracks.is_empty());
    e.execute("edit.select", &json!({"tracks": [e.session().tracks[0].id.0]})).unwrap();
    assert_paste_refused(&mut e);
}

#[test]
fn new_and_close_forget_the_copy_too() {
    for command in ["session.new", "session.close"] {
        let mut e = Engine::new(session_with_audio());
        copy_all(&mut e);
        e.execute(command, &json!({"name": "B"})).unwrap();
        assert!(e.clipboard.tracks.is_empty(), "{command}");
    }
}

#[test]
fn open_and_revert_forget_the_copy() {
    let dir = std::env::temp_dir().join(format!("transit-clipboard-476-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("A.transit").to_string_lossy().into_owned();
    let mut e = Engine::new(session_with_audio());
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    copy_all(&mut e);
    e.execute("session.revert", &json!({})).unwrap();
    assert!(e.clipboard.tracks.is_empty(), "revert");
    copy_all(&mut e);
    e.execute("session.open", &json!({"path": path})).unwrap();
    assert!(e.clipboard.tracks.is_empty(), "open");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copy_and_paste_within_one_session_still_works_and_survives_undo_and_redo() {
    let mut e = Engine::new(session_with_audio());
    copy_all(&mut e);
    e.execute("edit.paste", &json!({"at": 30_000})).unwrap();
    assert_eq!(clip_count(&e), 2);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(clip_count(&e), 1);
    e.execute("edit.paste", &json!({"at": 30_000})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(clip_count(&e), 2);
    e.execute("edit.paste", &json!({"at": 60_000})).unwrap();
    assert_eq!(clip_count(&e), 3);
}

#[test]
fn save_as_keeps_the_copy() {
    let dir = std::env::temp_dir().join(format!("transit-clipboard-476-sa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("Same.transit").to_string_lossy().into_owned();
    let mut e = Engine::new(session_with_audio());
    copy_all(&mut e);
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    e.execute("edit.paste", &json!({"at": 30_000})).unwrap();
    assert_eq!(clip_count(&e), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copied_midi_is_values_and_pastes_into_a_new_session() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: TICKS_PER_QUARTER as _ });
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", 0, 24_000, seq));
    s.edit.selected_tracks = vec![t];
    let mut e = Engine::new(s);
    copy_all(&mut e);
    let mut b = Session::default();
    let tb = b.add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    b.edit.selected_tracks = vec![tb];
    e.replace_session(b);
    assert!(!e.clipboard.tracks.is_empty(), "MIDI clips hold no session ids");
    e.execute("edit.paste", &json!({"at": 0})).unwrap();
    assert_eq!(clip_count(&e), 1);
}
