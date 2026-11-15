//! Issue #574, round 3: the plan wait leaves a pending transport request alone, the timed read is
//! deferred while playing or mid-gesture and skips noisy plug-ins on a clean session, the jump flag
//! is reset, a jump carries no state into a frozen track, and the "not refreshed" notice survives a
//! jump and a browser instrument drop.

use super::round2::add_fake;
use super::*;

/// The timed autosave with an interval just passed.
fn timed(r: &mut Rig) {
    r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
    r.app.autosave();
}

/// A rig with an autosave folder.
fn autosaving(instrument: bool, tag: &str) -> (Rig, transit_test_support::TestDir) {
    let mut r = rig(instrument);
    let dir = transit_test_support::TestDir::new(tag);
    r.app.autosave_dir = Some(dir.join("Autosave"));
    (r, dir)
}

/// F1: with Play pending, the plan wait neither publishes the unapplied document nor polls: the
/// capture's own refusal stands and the player still has the older document.
#[test]
fn the_plan_wait_publishes_nothing_while_play_is_pending() {
    let mut r = rig(false);
    r.app.state_guard.plan_wait = Duration::from_millis(20);
    let t = r.t;
    r.app.run("mix.mute", json!({"track": t.0})).unwrap();
    r.app.player_requests.play = Some(transit_playback::action_lane::Sequence::default());
    assert!(r.app.remove_device(t, 0).is_err(), "refused while Play is pending");
    let published = r.app.player.as_ref().unwrap().has_requested_session(&r.app.engine.session_arc());
    r.app.player_requests.play = None;
    assert!(!published, "the wait published the document around the pending Play");
    assert!(r.app.engine.session().track(t).unwrap().mixer.inserts[0].is_some(), "the plug-in stays");
}

/// F2: the timed read waits while the transport plays and runs on the first autosave after Stop.
#[test]
fn the_timed_read_waits_for_stop() {
    let (mut r, _dir) = autosaving(false, "state574-r3-playing");
    r.app.run("transport.play", json!({})).unwrap();
    for _ in 0..200 {
        if r.app.is_playing() {
            break;
        }
        r.frame();
    }
    assert!(r.app.is_playing(), "playing");
    let (_, live) = r.made.last();
    live.set(b"while playing");
    timed(&mut r);
    assert_eq!(r.stored(false).as_deref(), Some(&b"factory"[..]), "not read while playing");
    r.app.run("transport.stop", json!({})).unwrap();
    for _ in 0..200 {
        if !r.app.is_playing() && r.app.player.as_ref().unwrap().plays(&r.app.engine.session_arc()) {
            break;
        }
        r.frame();
    }
    r.app.autosave();
    assert_eq!(r.stored(false).as_deref(), Some(&b"while playing"[..]), "read right after Stop");
    r.app.release_autosave(false);
}

/// F2: the timed read waits while a merge gesture is held (the automation write pass) and runs
/// once it is let go.
#[test]
fn the_timed_read_waits_for_an_open_gesture() {
    let (mut r, _dir) = autosaving(false, "state574-r3-gesture");
    let (_, live) = r.made.last();
    live.set(b"mid gesture");
    r.app.engine.hold_merge(Some("pass"));
    timed(&mut r);
    assert_eq!(r.stored(false).as_deref(), Some(&b"factory"[..]), "not read mid-gesture");
    r.app.engine.hold_merge(None);
    r.app.autosave();
    assert_eq!(r.stored(false).as_deref(), Some(&b"mid gesture"[..]), "read once the gesture closed");
    r.app.release_autosave(false);
}

/// F2: on a clean session, a plug-in whose state changed on two timed reads in a row (no notice,
/// no edit) is skipped by later timed reads; a save-path capture still reads it.
#[test]
fn a_noisy_plugin_is_skipped_by_later_timed_reads() {
    let (mut r, _dir) = autosaving(false, "state574-r3-noisy");
    let (_, live) = r.made.last();
    for bytes in [&b"n1"[..], b"n2"] {
        live.set(bytes);
        timed(&mut r);
        assert_eq!(r.stored(false).as_deref(), Some(bytes), "read");
        r.until_loaded(1);
    }
    live.set(b"n3");
    timed(&mut r);
    assert_eq!(r.stored(false).as_deref(), Some(&b"n2"[..]), "skipped once noisy");
    r.app.capture_plugin_states().unwrap();
    assert_eq!(r.stored(false).as_deref(), Some(&b"n3"[..]), "the full capture still reads it");
    r.app.release_autosave(false);
}

/// F2: a change between edits is not noise: an edit after the first changed read resets the count.
#[test]
fn a_plugin_that_changes_between_edits_is_not_noisy() {
    let (mut r, _dir) = autosaving(false, "state574-r3-not-noisy");
    let (_, live) = r.made.last();
    let t = r.t;
    live.set(b"c1");
    timed(&mut r);
    r.until_loaded(1);
    r.app.run("mix.mute", json!({"track": t.0})).unwrap();
    r.until_loaded(1);
    live.set(b"c2");
    timed(&mut r);
    r.until_loaded(1);
    live.set(b"c3");
    timed(&mut r);
    assert_eq!(r.stored(false).as_deref(), Some(&b"c3"[..]), "still read");
    r.app.release_autosave(false);
}

/// F3: after a jump the per-step guard is back on: a later removal captures again.
#[test]
fn a_removal_after_a_jump_still_captures() {
    let mut r = rig(false);
    add_fake(&mut r, 1);
    let keep = r.app.engine.undo_history().len() - 1;
    assert_eq!(r.app.undo_to(keep), 1);
    r.until_loaded(2);
    let first = Arc::clone(&r.made.instances.lock().unwrap()[0].1);
    first.set(b"after the jump");
    let t = r.t;
    r.app.remove_device(t, 0).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(3);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"after the jump"[..]), "the removal captured");
}

/// F4: a jump whose steps pass a document where the plug-in's track is frozen completes; nothing
/// is written into the frozen track.
#[test]
fn a_jump_across_a_frozen_track_completes() {
    let mut r = rig(false);
    let t = r.t;
    let frozen = transit_model::Frozen { source: transit_model::SourceId(1), start: 0, length: 10, hash: 0, revision: 0, latency: 0 };
    let before = r.app.engine.session_arc();
    r.app.engine.session_mut().track_mut(t).unwrap().freeze = Some(frozen);
    r.app.engine.push_undo("Freeze", before);
    r.frames(2);
    let before = r.app.engine.session_arc();
    r.app.engine.session_mut().track_mut(t).unwrap().freeze = None;
    r.app.engine.push_undo("Unfreeze", before);
    r.until_loaded(1);
    add_fake(&mut r, 1);
    for (_, live) in r.made.instances.lock().unwrap().iter() {
        live.set(b"tweaked");
    }
    let keep = r.app.engine.undo_history().len() - 3;
    let n = r.app.undo_to(keep);
    assert_eq!((n, r.app.status.as_str()), (3, "Undid 3 steps"));
}

/// F5: the "not refreshed" notice is kept after a jump's status.
#[test]
fn a_jump_names_a_plugin_whose_state_was_not_refreshed() {
    let mut r = rig(false);
    add_fake(&mut r, 1);
    let first = Arc::clone(&r.made.instances.lock().unwrap()[0].1);
    first.busy.store(true, Ordering::Relaxed);
    let keep = r.app.engine.undo_history().len() - 1;
    assert_eq!(r.app.undo_to(keep), 1);
    assert!(r.app.status.starts_with("Undid 1 step"), "{}", r.app.status);
    assert!(r.app.status.contains("not refreshed"), "{}", r.app.status);
}

/// F5: the "not refreshed" notice is kept after a browser instrument drop's status.
#[test]
fn a_browser_instrument_drop_names_a_plugin_whose_state_was_not_refreshed() {
    let mut r = rig(true);
    let (_, live) = r.made.last();
    live.busy.store(true, Ordering::Relaxed);
    let t = r.t;
    r.app.drop_plugin(crate::macro_osc_panel::MACRO_OSC, "Macro Oscillator", true, crate::browser::Target::Track { track: t, at: None }).unwrap();
    assert!(r.app.status.starts_with("Loaded Macro Oscillator"), "{}", r.app.status);
    assert!(r.app.status.contains("not refreshed"), "{}", r.app.status);
}
