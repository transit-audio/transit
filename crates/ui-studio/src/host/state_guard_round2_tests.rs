//! Issue #574, round 2: the routes the review found (External Instrument, a width change, an Undo
//! History jump), the bounded wait and the refusal, the "not refreshed" notice on Delete, and the
//! timed autosave's read of plug-ins that never report a change.

use super::*;
use std::sync::atomic::AtomicU64;
use transit_playback::backend::null::PaceClock;

/// Probe P1: replacing a hosted instrument with an External Instrument, then Undo, brings the
/// plug-in back with the state it had at the replacement.
#[test]
fn undo_of_an_external_instrument_replacement_restores_the_state_at_the_replacement() {
    let mut r = rig(true);
    let (_, live) = r.made.last();
    live.set(b"edited");
    let t = r.t;
    r.app.set_external_instrument(t).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(2);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"edited"[..]), "made again with the state at the replacement");
}

/// Probe P4: a width change makes the hosted instance again at once; it keeps the live state.
#[test]
fn a_width_change_keeps_the_live_state_of_the_hosted_plugin() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"tweaked");
    let t = r.t;
    let mono = r.app.engine.session().track(t).unwrap().format == transit_model::ChannelFormat::Mono;
    let format = if mono { "stereo" } else { "mono" };
    r.app.run("track.change_width", json!({"tracks": [t.0], "format": format})).unwrap();
    r.until_loaded(2);
    let (made_with, live) = r.made.last();
    assert_eq!(made_with.as_deref(), Some(&b"tweaked"[..]), "made again with the live state");
    assert_eq!(*live.state.lock().unwrap(), b"tweaked");
}

/// Adds a second fake in insert slot `slot` of the rig's track as one undo step and waits for it.
fn add_fake(r: &mut Rig, slot: usize) {
    let t = r.t;
    let n = r.made.count() + 1;
    let before = r.app.engine.session_arc();
    r.app.engine.session_mut().track_mut(t).unwrap().mixer.inserts[slot] = Some(transit_model::Insert::new(ID));
    r.app.engine.push_undo("Insert", before);
    r.wait_made(n, |app| app.player.as_ref().unwrap().live_name(t, slot).is_some() && app.player.as_ref().unwrap().plays(&app.engine.session_arc()));
}

/// Probe P2: an Undo History jump over two steps that each take a plug-in away does both steps
/// (one capture before the jump), and Redo brings each back with its state at the jump.
#[test]
fn an_undo_history_jump_over_two_plugin_drops_completes() {
    let mut r = rig(false);
    add_fake(&mut r, 1);
    let (_, second) = r.made.last();
    add_fake(&mut r, 2);
    let (_, third) = r.made.last();
    second.set(b"second tweaked");
    third.set(b"third tweaked");
    let keep = r.app.engine.undo_history().len() - 2;
    let n = r.app.undo_to(keep);
    assert_eq!((n, r.app.status.as_str()), (2, "Undid 2 steps"));
    let made = r.made.count();
    r.frames(2);
    r.app.run("edit.redo", json!({})).unwrap();
    r.wait_made(made + 1, |_| true);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"second tweaked"[..]));
    r.frames(2);
    r.app.run("edit.redo", json!({})).unwrap();
    r.wait_made(made + 2, |_| true);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"third tweaked"[..]));
}

/// A jump whose capture is refused undoes nothing and says why.
#[test]
fn an_undo_history_jump_whose_capture_is_refused_undoes_nothing() {
    let mut r = rig(false);
    add_fake(&mut r, 1);
    r.app.state_guard.plan_wait = Duration::ZERO;
    // A document change the audio side has not applied yet.
    r.app.run("mix.mute", json!({"track": r.t.0})).unwrap();
    let before = r.app.engine.undo_history().len();
    let n = r.app.undo_to(before - 2);
    assert_eq!(n, 0);
    assert_eq!(r.app.engine.undo_history().len(), before, "nothing undone");
    assert!(r.app.status.starts_with("Nothing undone: Can't do that yet"), "{}", r.app.status);
}

/// Control C2's target: a refused capture stops the removal (the plug-in stays, no undo step) and
/// says so in the removal's own words.
#[test]
fn a_removal_whose_capture_is_refused_changes_nothing_and_says_why() {
    let mut r = rig(false);
    r.app.state_guard.plan_wait = Duration::ZERO;
    let t = r.t;
    r.app.run("mix.mute", json!({"track": t.0})).unwrap();
    let steps = r.app.engine.undo_history().len();
    let refused = r.app.remove_device(t, 0);
    assert!(refused.is_err(), "refused");
    assert!(r.app.engine.session().track(t).unwrap().mixer.inserts[0].is_some(), "the plug-in stays");
    assert_eq!(r.app.engine.undo_history().len(), steps, "no undo step");
    assert!(r.app.status.contains("could not be read yet"), "{}", r.app.status);
    assert!(!r.app.status.contains("saving"), "{}", r.app.status);
}

/// A free-running silent device: every block is due as soon as the last one ends (1 ms apart), so
/// the audio side applies a new document within a few milliseconds of real time.
struct FreeClock(AtomicU64);

impl PaceClock for FreeClock {
    fn now(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::Acquire))
    }
    fn wait_until(&self, deadline: Duration, stop: &AtomicBool) {
        if !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.0.fetch_max(u64::try_from(deadline.as_nanos()).unwrap_or(u64::MAX), Ordering::AcqRel);
    }
}

/// The bounded wait: a removal right after another edit waits for the audio side to apply it,
/// then captures and removes; Undo brings the plug-in back with its state. The wait is made
/// unbounded here, so the test ends on the handshake (`plays`), not on a time.
#[test]
fn a_removal_right_after_an_edit_waits_for_the_plan_and_goes_ahead() {
    let engine = transit_engine::Engine::default();
    let clock: Arc<dyn PaceClock> = Arc::new(FreeClock(AtomicU64::new(0)));
    let player = transit_playback::Player::with_null_device_clock(engine.session_arc(), clock);
    let mut r = rig_with(engine, player, false, false);
    r.app.state_guard.plan_wait = Duration::from_secs(600);
    let (_, live) = r.made.last();
    live.set(b"edited");
    let t = r.t;
    r.app.run("mix.mute", json!({"track": t.0})).unwrap();
    r.app.remove_device(t, 0).unwrap();
    assert!(r.app.engine.session().track(t).unwrap().mixer.inserts[0].is_none(), "removed");
    // Let the plan drop the instance, so Undo has to make it again.
    for _ in 0..2000 {
        if r.app.player.as_ref().unwrap().live_name(t, 0).is_none() {
            break;
        }
        r.frame();
    }
    assert!(r.app.player.as_ref().unwrap().live_name(t, 0).is_none(), "the instance is gone");
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(2);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"edited"[..]), "made again with the state at the removal");
}

/// Delete with a plug-in that does not answer within the deadline: the track goes with that
/// plug-in's stored state, and the status says so after "Deleted".
#[test]
fn delete_names_a_plugin_whose_state_was_not_refreshed() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.busy.store(true, Ordering::Relaxed);
    let t = r.t;
    r.app.delete_tracks(&[t]).unwrap();
    assert!(r.app.status.starts_with("Deleted"), "{}", r.app.status);
    assert!(r.app.status.contains("not refreshed"), "{}", r.app.status);
}

/// Decision 2 (b): a plug-in that never reports a change is still read by the timed autosave,
/// also on a clean session, and the changed state is stored and autosaved.
#[test]
fn the_timed_autosave_reads_a_silent_plugin_on_a_clean_session() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"silent change");
    let dir = transit_test_support::TestDir::new("state574-timed-silent");
    r.app.autosave_dir = Some(dir.join("Autosave"));
    r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
    r.app.autosave();
    assert_eq!(r.stored(false).as_deref(), Some(&b"silent change"[..]), "read on a clean session");
    assert_eq!(r.app.state_guard.timed_writes, 1, "and autosaved");
    r.app.release_autosave(false);
}

/// A plug-in that reports a change every interval while its state reads the same causes one
/// autosave (the session became modified), not one per interval.
#[test]
fn a_constantly_reporting_plugin_with_unchanged_state_is_not_autosaved_every_interval() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    let dir = transit_test_support::TestDir::new("state574-timed-constant");
    r.app.autosave_dir = Some(dir.join("Autosave"));
    for _ in 0..3 {
        live.changed.store(true, Ordering::Relaxed);
        r.frames(2);
        r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
        r.app.autosave();
    }
    assert_eq!(r.app.state_guard.timed_writes, 1, "one write for three intervals");
    r.app.release_autosave(false);
}

/// The timed read stops at its budget and goes on where it stopped: with a zero budget each
/// interval reads one plug-in, and the next interval reads the other.
#[test]
fn the_timed_read_stops_at_its_budget_and_resumes_next_interval() {
    let mut r = rig(false);
    add_fake(&mut r, 1);
    let (_, second) = r.made.last();
    let first = Arc::clone(&r.made.instances.lock().unwrap()[0].1);
    first.set(b"first changed");
    second.set(b"second changed");
    r.app.state_guard.timed_budget = Duration::ZERO;
    let dir = transit_test_support::TestDir::new("state574-timed-budget");
    r.app.autosave_dir = Some(dir.join("Autosave"));
    let stored = |r: &Rig| -> Vec<Option<Vec<u8>>> {
        let tr = r.app.engine.session().track(r.t).unwrap();
        (0..2).map(|i| tr.mixer.inserts[i].as_ref().and_then(transit_model::Insert::state_bytes)).collect()
    };
    r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
    r.app.autosave();
    let once = stored(&r);
    let changed = once.iter().filter(|s| matches!(s.as_deref(), Some(b"first changed" | b"second changed"))).count();
    assert_eq!(changed, 1, "one plug-in read within the budget: {once:?}");
    // The stored state is a new document: let the player apply it before the next read.
    r.until_loaded(2);
    r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
    r.app.autosave();
    assert_eq!(stored(&r), vec![Some(b"first changed".to_vec()), Some(b"second changed".to_vec())], "the other one next interval");
    r.app.release_autosave(false);
}

/// A removal with a plug-in that does not answer within the deadline goes ahead with that
/// plug-in's stored state, and the status names it.
#[test]
fn a_removal_names_a_plugin_whose_state_was_not_refreshed() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.busy.store(true, Ordering::Relaxed);
    let t = r.t;
    r.app.remove_device(t, 0).unwrap();
    assert!(r.app.engine.session().track(t).unwrap().mixer.inserts[0].is_none(), "removed");
    assert!(r.app.status.contains("not refreshed"), "{}", r.app.status);
}

/// Ungroup with a group device that does not answer within the deadline: the status says so
/// after "Ungrouped".
#[test]
fn ungroup_names_a_plugin_whose_state_was_not_refreshed() {
    let mut r = rig(false);
    let t = r.t;
    let g = r.app.group_tracks(&[t]).unwrap();
    r.app.engine.session_mut().track_mut(g).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new(ID));
    r.wait_made(2, |app| app.player.as_ref().unwrap().live_name(g, 0).is_some() && app.player.as_ref().unwrap().plays(&app.engine.session_arc()));
    let (_, live) = r.made.last();
    live.busy.store(true, Ordering::Relaxed);
    r.app.ungroup(&[g]).unwrap();
    assert!(r.app.status.starts_with("Ungrouped"), "{}", r.app.status);
    assert!(r.app.status.contains("not refreshed"), "{}", r.app.status);
}

/// A plug-in with no editor handle reports its own state change through its state handle, and
/// the session reads modified.
#[test]
fn a_plugin_without_an_editor_handle_marks_the_session_modified() {
    let mut r = rig(false);
    r.made.headless.store(true, Ordering::Relaxed);
    add_fake(&mut r, 1);
    assert!(!r.app.player.as_ref().unwrap().has_editor(r.t, 1), "no editor handle");
    r.app.capture_plugin_states().unwrap();
    r.app.engine.mark_clean();
    let (_, live) = r.made.last();
    live.changed.store(true, Ordering::Relaxed);
    r.frames(2);
    assert!(r.app.engine.is_dirty(), "the change reached the session");
}
