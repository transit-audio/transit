//! A recorded take over the decode limit fails loudly and stays reachable from the session (#192).
//! One test in its own binary owns the process-wide logger.

use std::io::Write;
use std::sync::Mutex;
use transit_engine::{Engine, io::add_recording_take};
use transit_model::{ChannelFormat, Session, TrackKind};

static LOG: TestLog = TestLog(Mutex::new(Vec::new()));

struct TestLog(Mutex<Vec<String>>);

impl log::Log for TestLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if let Ok(mut records) = self.0.lock() {
            records.push(format!("{}: {}", record.level(), record.args()));
        }
    }

    fn flush(&self) {}
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
fn a_take_over_the_limit_is_refused_loudly_unread_and_remembered_by_the_session() {
    log::set_logger(&LOG).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    let dir = std::env::temp_dir().join(format!("long-take-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Loopback_01.wav");
    header_only_take(&path, transit_audio_io::MAX_DECODE_SAMPLES + 1_000);

    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let before = e.session().sources.len();
    assert!(add_recording_take(&mut e, 0, vec![(t, path.clone())], None, false, None).is_empty());

    // Nothing placed, nothing to undo; the file is still there.
    assert_eq!(e.session().sources.len(), before);
    assert!(e.undo_history().is_empty());
    assert!(path.is_file());

    // The message says what and how long, and where the file is.
    let shown = path.display().to_string();
    let said = e.messages.last().cloned().unwrap();
    assert!(said.contains("6.2 h of mono at 48 kHz") && said.contains("the limit is 6.2 h"), "{said}");
    assert!(said.contains(&format!("The file is kept at {shown}")), "{said}");

    // The log has it as a warning with the same hours.
    let lines = LOG.0.lock().unwrap().clone();
    let warn = lines.iter().find(|l| l.starts_with("WARN") && l.contains("6.2 h of mono at 48 kHz")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(warn.contains(&shown) && warn.contains("1073742824 frames") && warn.contains("the limit is 6.2 h"), "{warn}");

    // The session keeps a reference that survives a save and a load, and the open says so.
    assert_eq!(e.session().unplaced_takes.len(), 1);
    let back = Session::from_json(&e.session().to_json().unwrap()).unwrap();
    assert_eq!(back.unplaced_takes, e.session().unplaced_takes);
    assert!(back.load_notes.iter().any(|n| n.contains(&shown) && n.contains("6.2 h")), "{:?}", back.load_notes);

    // The same take refused again is listed once.
    assert!(add_recording_take(&mut e, 0, vec![(t, path.clone())], None, false, None).is_empty());
    assert_eq!(e.session().unplaced_takes.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
