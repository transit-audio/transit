//! Unsaved work outlives a crash or an exit without Quit and is offered at the next launch.
//! Ported from the retired legacy UI's `recovery` tests (#177): the engine and autosave behavior
//! is shared, so the headless parts run on `StudioApp`.

use crate::{Services, StudioApp};
use serde_json::json;
use std::path::Path;
use transit_engine::autosave::leftovers;

fn folder(name: &str) -> transit_test_support::TestDir {
    let dir = transit_test_support::TestDir::new(&format!("scratch-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn autosaved(root: &Path) -> String {
    let folders = std::fs::read_dir(root).into_iter().flatten().flatten();
    folders.map(|d| std::fs::read_to_string(d.path().join("session.transit")).unwrap_or_default()).collect()
}

fn launch(root: &Path) -> StudioApp {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
    app.autosave_dir = Some(root.to_path_buf());
    app.leftovers = leftovers(root);
    app
}

#[test]
fn an_exit_without_quit_offers_unsaved_changes_at_the_next_launch() {
    let dir = folder("recovery-quit");
    let root = dir.join("Autosave");
    let path = dir.join("Song.transit");
    let mut first = launch(&root);
    first.run("session.save_as", json!({"path": path})).unwrap();
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    drop(first);

    let mut next = launch(&root);
    assert_eq!(next.leftovers.len(), 1, "the next launch is offered the unsaved work");
    let left = next.leftovers[0].clone();
    next.recover(&left);
    let e = &next.engine;
    assert!(e.session().track_by_name("Vocal").is_some() && e.is_dirty());
    assert_eq!(e.path, Some(path.to_string_lossy().into_owned()));
    assert!(leftovers(&root).is_empty());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    next.run("session.save", json!({})).unwrap();
    // The save closed the folder on the writer thread; `flush` returns once that job is done.
    next.autosaver.as_ref().expect("the recovered session has an autosaver").flush().unwrap();
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_change_is_autosaved_within_a_second() {
    let dir = folder("recovery-changes");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    for name in ["Vocal", "Bass"] {
        app.run("track.new", json!({"name": name})).unwrap();
        app.autosave_changes();
        // `flush` returns when the writer thread has written what was handed over: it waits on the
        // thread's own completion signal, not on the clock (E62).
        app.autosaver.as_ref().expect("a change hands a write to the autosaver").flush().unwrap();
        assert!(autosaved(&root).contains(name), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovering_keeps_the_unsaved_work_it_replaces() {
    let dir = folder("recovery-replace");
    let root = dir.join("Autosave");
    let mut first = launch(&root);
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    first.close_autosave();
    let left = leftovers(&root);
    assert_eq!((left.len(), left[0].name.as_str(), left[0].origin.as_deref()), (1, "Untitled", None));
    let mut next = launch(&root);
    next.run("track.new", json!({"name": "Drums"})).unwrap();
    next.recover(&left[0]);
    assert!(next.engine.session().track_by_name("Vocal").is_some() && next.engine.path.is_none());
    let kept = leftovers(&root);
    assert_eq!(kept.len(), 1);
    let mut third = launch(&root);
    third.recover(&kept[0]);
    assert!(third.engine.session().track_by_name("Drums").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recovering_an_early_take_keeps_its_listing_and_audio_after_saving() {
    let dir = folder("recovery-early-take");
    let root = dir.join("Autosave");
    let mut first = launch(&root);
    first.run("track.new", json!({"name": "Vocal"})).unwrap();
    let track = first.engine.session().track_by_name("Vocal").unwrap().id;
    let path = root.join("Untitled/Audio Files/Vocal_01.wav");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = transit_audio_io::wav_float_header(1, 48_000, 480).unwrap();
    bytes.extend((0..480).flat_map(|_| 0.25_f32.to_le_bytes()));
    std::fs::write(&path, bytes).unwrap();
    let plan = transit_engine::io::Recording::new(-960);
    assert!(first.place_takes(&plan, vec![(track, path)], -480, 1).is_empty());
    first.close_autosave();
    drop(first);
    let left = leftovers(&root);
    assert_eq!(left.len(), 1);
    let mut next = launch(&root);
    next.recover(&left[0]);
    assert_eq!(next.engine.session().unplaced_takes.len(), 1);
    assert_eq!(next.engine.session().sources.len(), 1);
    assert!(next.engine.session().track(track).unwrap().clips().is_empty());
    let saved = dir.join("Saved/Song.transit");
    next.run("session.save_as", json!({"path": saved})).unwrap();
    next.close_autosave();
    let s = next.engine.session();
    let transit_model::UnplacedTake::File { path, source, .. } = &s.unplaced_takes[0] else { panic!("structured entry required") };
    assert_eq!(*source, Some(s.sources[0].id));
    assert_eq!(path, &s.sources[0].path);
    assert!(saved.parent().unwrap().join(path).is_file());
    assert_eq!(s.pool.get(s.sources[0].id).unwrap().frames(), 480);
}

#[test]
fn closing_the_session_or_quitting_drops_its_autosave() {
    let dir = folder("recovery-close");
    let root = dir.join("Autosave");
    for end in ["session.close", "app.quit"] {
        let mut first = launch(&root);
        first.run("track.new", json!({"name": "Vocal"})).unwrap();
        first.close_autosave();
        let mut next = launch(&root);
        let left = next.leftovers[0].clone();
        next.recover(&left);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1, "{end}");
        next.run(end, json!({})).unwrap();
        next.close_autosave();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "{end}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quitting_while_recording_keeps_the_take_for_the_next_launch() {
    let dir = folder("recovery-recording");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
    app.engine.transport.recording = true;
    assert!(app.run("app.quit", json!({})).is_err(), "Quit waits for the take on the run path");
    app.quit_requested = true;
    app.close_autosave();
    assert!(!app.engine.transport.recording);
    assert_eq!(leftovers(&root).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ported from the legacy `bounce` tests (#177): while a bounce holds its own copy of the session,
/// a session-replacing command is refused, and Quit cancels the bounce.
#[test]
fn the_session_cannot_be_replaced_while_it_bounces() {
    let dir = folder("bounce-refuses");
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.open_bounce();
    let form = app.finish.bounce_form.as_mut().expect("the bounce form opens");
    form.range = crate::finish::BounceRange::Whole;
    form.path = dir.join("Mix.wav").to_string_lossy().into_owned();
    app.start_bounce().unwrap();
    assert!(app.finish.job.is_some(), "{}", app.status);
    let before = app.engine.session().name.clone();
    for id in ["session.new", "session.open", "session.close", "session.revert"] {
        assert!(app.run(id, json!({})).is_err(), "{id}");
        assert_eq!(app.status, "Finish or cancel the bounce first");
        assert_eq!(app.engine.session().name, before, "{id}");
    }
    app.run("app.quit", json!({})).unwrap();
    assert!(app.finish.job.is_none(), "Quit cancels the bounce");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ported from the legacy `a_panic_in_a_frame_autosaves_and_the_app_carries_on` (#177).
#[test]
fn a_panic_in_a_frame_autosaves_and_the_app_carries_on() {
    let dir = folder("recovery-panic");
    let root = dir.join("Autosave");
    let mut app = launch(&root);
    app.run("track.new", json!({"name": "Vocal"})).unwrap();
    app.guarded(|_| std::panic::panic_any("a bug in a frame"));
    assert!(app.status.contains("autosaved"), "{}", app.status);
    app.guarded(|app| {
        app.run("track.new", json!({"name": "Bass"})).unwrap();
    });
    assert!(app.engine.session().track_by_name("Bass").is_some());
    app.autosaver.as_ref().unwrap().flush().unwrap();
    drop(app);
    let left = leftovers(&root);
    assert_eq!(left.len(), 1);
    let mut next = launch(&root);
    next.recover(&left[0]);
    assert!(next.engine.session().track_by_name("Vocal").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A recovered MIDI take, from a journal of a recording that ran after a count-in of half a second,
/// with a note-on at each of `offsets` (samples from the latch); the status line after recovery.
fn recovered_status(name: &str, offsets: &[i64]) -> String {
    use transit_engine::midi_journal::{Header, Latch, Record, Writer};
    use transit_engine::midi_take::Msg;
    let dir = folder(name);
    let root = dir.join("Autosave");
    let mut first = launch(&root);
    first.run("track.new", json!({"kind": "midi", "name": "Keys"})).unwrap();
    let track = first.engine.session().track_by_name("Keys").unwrap().id;
    first.close_autosave();
    drop(first);
    let mut left = leftovers(&root).remove(0);
    let journal = root.join("Untitled/Audio Files/Keys.midijournal");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    let plan = transit_engine::io::Recording { count_in: Some(24_000), ..transit_engine::io::Recording::new(0) };
    let h = Header::new(Latch { output_frame: 100_000, position: 0, device_rate: 48_000, session_rate: 48_000 }, &plan, None, vec![track]);
    let mut records = Vec::new();
    for (i, off) in offsets.iter().enumerate() {
        let frame = u64::try_from(100_000 + off).unwrap();
        let pitch = 60 + u8::try_from(i).unwrap();
        let on = Record { frame, track, channel: 0, approximate: false, msg: Msg::NoteOn { pitch, velocity: 90 } };
        records.push(on);
        records.push(Record { frame: frame + 4_000, msg: Msg::NoteOff { pitch, velocity: 0 }, ..on });
    }
    let mut w = Writer::new(journal.clone());
    w.append(&h, &records, records.len()).unwrap();
    w.flush().unwrap();
    left.midi_journal = Some(journal);
    let mut next = launch(&root);
    next.recover(&left);
    let status = next.status.clone();
    let _ = std::fs::remove_dir_all(&dir);
    status
}

/// Recovery says how many notes played before the count-in started were left out, in the words a
/// normal Stop uses (#704); with none left out it says nothing of the kind.
#[test]
fn a_recovered_take_says_how_many_notes_before_the_count_in_were_left_out() {
    let said = recovered_status("recovery-before-count-in", &[-36_000, -30_000, -12_000, 12_000]);
    assert!(said.contains("1 recovered MIDI clip from the interrupted recording"), "{said}");
    assert!(said.ends_with("; 2 notes played before recording began were not recorded."), "{said}");
    let said = recovered_status("recovery-no-early-notes", &[-12_000, 12_000]);
    assert!(said.contains("1 recovered MIDI clip") && !said.contains("not recorded"), "{said}");
}
