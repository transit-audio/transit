//! A recorded take over the decode limit is placed as one clip played from its file (ADR 0035,
//! #187; until then it was refused loudly and listed as unplaced, #192).
//! Non-Windows only: the fixture is a 6 GiB hole, which NTFS allocates (the engine unit tests cover
//! Windows with a faked file length).

#![cfg(not(windows))]

use std::io::Write;
use transit_engine::{Engine, io::add_recording_take};
use transit_model::{ChannelFormat, TrackKind};

/// Removes the directory when the test ends, passing or not.
struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A header-only take: an RF64 header for `frames` mono float frames at 48 kHz, then a file
/// length that holds them as a hole (sparse), never as written bytes.
fn header_only_take(path: &std::path::Path, frames: u64) {
    let header = transit_audio_io::wav_float_header(1, 48_000, frames).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&header).unwrap();
    f.set_len(header.len() as u64 + frames * 4).unwrap();
}

#[test]
fn a_take_of_one_and_a_half_gi_samples_is_one_disk_backed_clip_and_reopens_so() {
    let dir = std::env::temp_dir().join(format!("long-take-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = Cleanup(dir.clone());
    let path = dir.join("Loopback_01.wav");
    let frames = 3 * (1u64 << 29);
    assert!(frames > transit_audio_io::MAX_DECODE_SAMPLES);
    header_only_take(&path, frames);
    let len = std::fs::metadata(&path).unwrap().len();

    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let ids = add_recording_take(&mut e, 0, vec![(t, path.clone())], None, false, None);
    assert_eq!(ids.len(), 1, "{:?}", e.messages);
    assert!(e.session().unplaced_takes.is_empty());
    let src = e.session().sources.last().cloned().unwrap();
    assert!(src.disk && !src.unsaved && src.frames == frames, "{src:?}");
    assert!(e.session().pool.get(src.id).is_some_and(|a| a.is_disk()));
    let clip = e.session().find_clip(ids[0]).unwrap().1.clone();
    assert_eq!((clip.start, clip.length), (0, frames as i64));

    // Saved and opened again, it is still played from the file, which the save left alone.
    let session = dir.join("Long.transit").to_string_lossy().into_owned();
    e.execute("session.save_as", &serde_json::json!({"path": session})).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), len);
    let mut r = Engine::default();
    r.execute("session.open", &serde_json::json!({"path": session})).unwrap();
    let back = r.session().sources.last().cloned().unwrap();
    assert!(back.disk && back.frames == frames, "{back:?}");
    assert!(r.session().pool.get(back.id).is_some_and(|a| a.is_disk() && a.frames() as u64 == frames));
}
