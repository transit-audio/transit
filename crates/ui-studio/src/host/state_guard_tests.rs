//! Issue #574, through a device-free Player and an inline plug-in host with a fake hosted plug-in
//! whose state the session cannot see: a change it reports marks the session modified (save
//! question, autosave), a change it does not report is caught by the save question, and an undo
//! of a removal, a track deletion, an instrument replacement or a redo brings the plug-in back with
//! the state it had at that moment.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{Dialog, Services};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_dsp::{Plugin, PluginEditor, PluginStateReader};
use transit_playback::PluginTestHooks;
use transit_plugin_host::{Config, Host};

const ID: &str = "clap:test.state574";

/// One fake instance's state as the plug-in holds it, and its "state changed" notice.
#[derive(Default)]
struct Live {
    state: Mutex<Vec<u8>>,
    changed: AtomicBool,
}

impl Live {
    fn set(&self, bytes: &[u8]) {
        bytes.clone_into(&mut self.state.lock().unwrap());
    }
}

/// The instances made so far, newest last, with the stored state each was made with.
#[derive(Default)]
struct Made {
    instances: Mutex<Vec<(Option<Vec<u8>>, Arc<Live>)>>,
}

impl Made {
    fn count(&self) -> usize {
        self.instances.lock().unwrap().len()
    }
    fn last(&self) -> (Option<Vec<u8>>, Arc<Live>) {
        let list = self.instances.lock().unwrap();
        let (state, live) = list.last().expect("an instance was made");
        (state.clone(), Arc::clone(live))
    }
}

/// A gain whose state is `live` (the stored state at creation, else "factory").
struct Fake {
    inner: Box<dyn Plugin>,
    live: Arc<Live>,
}

impl Plugin for Fake {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, max: usize, ch: usize) {
        self.inner.prepare(sr, max, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        self.live.set(data);
        true
    }
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(Editor { live: Arc::clone(&self.live) }))
    }
    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        let live = Arc::clone(&self.live);
        Some(Box::new(move || Some(live.state.lock().unwrap().clone())))
    }
}

/// A closed, headless editor handle that passes on the fake's notice.
struct Editor {
    live: Arc<Live>,
}

impl PluginEditor for Editor {
    fn has_gui(&self) -> bool {
        false
    }
    fn open(&mut self) -> Result<(), String> {
        Err("no window".into())
    }
    fn close(&mut self) {}
    fn is_open(&self) -> bool {
        false
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
    fn take_state_changed(&mut self) -> bool {
        self.live.changed.swap(false, Ordering::Relaxed)
    }
}

struct Rig {
    app: StudioApp,
    ctx: egui::Context,
    made: Arc<Made>,
    t: TrackId,
}

/// An app whose track `t` holds the fake (`instrument`: as its instrument on an instrument track,
/// else as insert 1 of an audio track), loaded, its state stored and the session clean, as after
/// a save.
fn rig(instrument: bool) -> Rig {
    let engine = transit_engine::Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let now = Instant::now();
    let made = Arc::new(Made::default());
    let m = Arc::clone(&made);
    player.set_plugin_test_hooks(PluginTestHooks {
        host: Host::new(Config { inline: true, ..Config::default() }),
        factory: Arc::new(move |spec: &transit_mix::InstanceSpec| {
            let live = Arc::new(Live::default());
            live.set(spec.state.as_deref().unwrap_or(b"factory"));
            m.instances.lock().unwrap().push((spec.state.as_deref().map(<[u8]>::to_vec), Arc::clone(&live)));
            transit_mix::create_instance("gain", None, 48_000.0, 512, spec.ch)
                .map(|n| transit_mix::NewInstance { plugin: Box::new(Fake { inner: n.plugin, live }), ..n })
        }),
        deferred: Arc::new(|id: &str| id == ID),
        clock: Arc::new(move || now),
        slow_after: Duration::from_secs(10),
    });
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let kind = if instrument { "instrument" } else { "audio" };
    let r = app.run("track.new", json!({"kind": kind})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    let tr = app.engine.session_mut().track_mut(t).unwrap();
    if instrument {
        tr.instrument = Some(transit_model::Insert::new(ID));
    } else {
        tr.mixer.inserts[0] = Some(transit_model::Insert::new(ID));
    }
    let mut r = Rig { app, ctx: egui::Context::default(), made, t };
    r.until_loaded(1);
    r.app.capture_plugin_states().unwrap();
    r.app.engine.mark_clean();
    // The capture stored a state: a new document, which the player confirms in a few frames.
    r.until_loaded(1);
    assert_eq!(r.stored(instrument).as_deref(), Some(&b"factory"[..]), "the session stores the plug-in's state");
    assert!(!r.app.unsaved_changes(), "clean after the save");
    r
}

impl Rig {
    /// One frame of the app, the device-free player's "audio callback" first.
    fn frame(&mut self) {
        self.app.player.as_ref().unwrap().step_device_free();
        let mut out = self.ctx.run_ui(egui::RawInput::default(), |ui| self.app.logic(ui.ctx()));
        out.textures_delta.clear();
    }

    /// Frames until `n` instances were made and the newest is in the plan.
    fn until_loaded(&mut self, n: usize) {
        for _ in 0..200 {
            self.frame();
            let loaded = self.app.player.as_ref().unwrap().live_name(self.t, 0).is_some()
                || self.app.player.as_ref().unwrap().live_name(self.t, transit_mix::INSTRUMENT_SLOT).is_some();
            if self.made.count() >= n && loaded && self.app.player.as_ref().unwrap().plays(&self.app.engine.session_arc()) {
                return;
            }
        }
        panic!("instance {n} did not load (made {})", self.made.count());
    }

    /// Frames until `n` instances were made and `ready` holds.
    fn wait_made(&mut self, n: usize, ready: impl Fn(&StudioApp) -> bool) {
        for _ in 0..200 {
            self.frame();
            if self.made.count() >= n && ready(&self.app) {
                return;
            }
        }
        panic!("instance {n} was not made (made {})", self.made.count());
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    /// The state the session stores for the fake.
    fn stored(&self, instrument: bool) -> Option<Vec<u8>> {
        let tr = self.app.engine.session().track(self.t)?;
        let ins = if instrument { tr.instrument.as_ref() } else { tr.mixer.inserts[0].as_ref() };
        ins?.state_bytes()
    }
}

/// Expected (1): a change the plug-in reports (a patch loaded in its own browser, no parameter
/// event) marks the session modified at once; Quit asks to save; the autosave due within two
/// minutes runs and carries the new state.
#[test]
fn a_reported_plugin_state_change_marks_the_session_modified_and_is_autosaved() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"patch B");
    live.changed.store(true, Ordering::Relaxed);
    r.frames(2);
    assert!(r.app.engine.is_dirty(), "the plug-in's own change is unsaved work");
    let dir = transit_test_support::TestDir::new("state574-autosave");
    r.app.autosave_dir = Some(dir.join("Autosave"));
    r.app.last_autosave = Instant::now().checked_sub(Duration::from_secs(121));
    r.app.autosave();
    // Let go of the folder, as the app does when the session closes with unsaved changes.
    r.app.release_autosave(true);
    let left = transit_engine::autosave::leftovers(&dir.join("Autosave"));
    assert_eq!(left.len(), 1, "the two-minute autosave ran");
    let mut back = transit_engine::Engine::default();
    transit_engine::autosave::recover(&mut back, &left[0]).unwrap();
    let state = back.session().track(r.t).and_then(|t| t.mixer.inserts[0].as_ref()).and_then(transit_model::Insert::state_bytes);
    assert_eq!(state.as_deref(), Some(&b"patch B"[..]), "the autosave holds the plug-in's new state");
    r.app.run_or_ask("app.quit", json!({}));
    assert!(matches!(r.app.dialog, Some(Dialog::SaveChanges { .. })), "Quit asks to save");
}

/// A capture that reads the state the session already holds leaves the document as it is (no new
/// revision), so another capture right after it is not refused as still updating.
#[test]
fn a_capture_of_unchanged_states_leaves_the_document_alone() {
    let mut r = rig(false);
    let (doc, rev) = (r.app.engine.session_arc(), r.app.engine.revision);
    r.app.capture_plugin_states().unwrap();
    assert!(Arc::ptr_eq(&doc, &r.app.engine.session_arc()), "the same document");
    assert_eq!(r.app.engine.revision, rev);
    let (_, live) = r.made.last();
    live.set(b"changed");
    r.app.capture_plugin_states().unwrap();
    assert_eq!(r.stored(false).as_deref(), Some(&b"changed"[..]), "a changed state is stored");
}

/// A notice from a plug-in of the session being replaced does not mark the new session modified.
#[test]
fn a_notice_from_the_replaced_session_does_not_mark_the_new_one() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.changed.store(true, Ordering::Relaxed);
    r.app.run("session.new", json!({})).unwrap();
    r.frames(4);
    assert!(!r.app.engine.is_dirty(), "the new session is clean");
}

/// A change the plug-in does not report is still caught by the save question: its live state is
/// read first.
#[test]
fn an_unreported_plugin_state_change_is_caught_by_the_save_question() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"patch C");
    r.frames(2);
    assert!(!r.app.engine.is_dirty(), "nothing told the session");
    r.app.run_or_ask("app.quit", json!({}));
    assert!(matches!(r.app.dialog, Some(Dialog::SaveChanges { .. })), "Quit asks to save");
    assert_eq!(r.stored(false).as_deref(), Some(&b"patch C"[..]), "the question read the live state");
}

/// Expected (2): remove the device, Undo: the plug-in comes back with the state it had at the
/// removal, not the state of the last save.
#[test]
fn undo_of_a_device_removal_restores_the_state_at_the_removal() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"edited");
    let t = r.t;
    r.app.remove_device(t, 0).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(2);
    let (made_with, live) = r.made.last();
    assert_eq!(made_with.as_deref(), Some(&b"edited"[..]), "made again with the state at the removal");
    assert_eq!(*live.state.lock().unwrap(), b"edited");
}

/// Expected (2) for a track deletion.
#[test]
fn undo_of_a_track_deletion_restores_the_state_at_the_deletion() {
    let mut r = rig(false);
    let (_, live) = r.made.last();
    live.set(b"edited");
    let t = r.t;
    r.app.delete_tracks(&[t]).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(2);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"edited"[..]), "made again with the state at the deletion");
}

/// Expected (2) for an instrument replaced by another.
#[test]
fn undo_of_an_instrument_replacement_restores_the_state_at_the_replacement() {
    let mut r = rig(true);
    let (_, live) = r.made.last();
    live.set(b"edited");
    let t = r.t;
    r.app.set_instrument(t, crate::macro_osc_panel::MACRO_OSC).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.until_loaded(2);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"edited"[..]), "made again with the state at the replacement");
}

/// Expected (2) for a group track's own device, which Ungroup deletes with the group track.
#[test]
fn undo_of_an_ungroup_restores_the_group_devices_state() {
    let mut r = rig(false);
    let t = r.t;
    let g = r.app.group_tracks(&[t]).unwrap();
    r.app.engine.session_mut().track_mut(g).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new(ID));
    r.wait_made(2, |app| app.player.as_ref().unwrap().live_name(g, 0).is_some() && app.player.as_ref().unwrap().plays(&app.engine.session_arc()));
    let (_, live) = r.made.last();
    live.set(b"grouped");
    r.app.ungroup(&[g]).unwrap();
    r.frames(2);
    r.app.run("edit.undo", json!({})).unwrap();
    r.wait_made(3, |_| true);
    assert_eq!(r.made.last().0.as_deref(), Some(&b"grouped"[..]), "made again with the state at the ungroup");
}

/// An undo that takes a plug-in away (the undo of its insertion), then Redo: it comes back with
/// the state it had when it was undone.
#[test]
fn redo_after_an_undo_that_removed_a_plugin_restores_its_state() {
    let mut r = rig(false);
    let t = r.t;
    // A second fake in insert 2, as one undo step.
    let before = r.app.engine.session_arc();
    r.app.engine.session_mut().track_mut(t).unwrap().mixer.inserts[1] = Some(transit_model::Insert::new(ID));
    r.app.engine.push_undo("Insert", before);
    for _ in 0..200 {
        r.frame();
        if r.made.count() == 2 && r.app.player.as_ref().unwrap().live_name(t, 1).is_some() {
            break;
        }
    }
    assert_eq!(r.made.count(), 2, "the second instance loaded");
    let (_, live) = r.made.last();
    live.set(b"tweaked");
    r.app.run("edit.undo", json!({})).unwrap();
    r.frames(2);
    r.app.run("edit.redo", json!({})).unwrap();
    for _ in 0..200 {
        r.frame();
        if r.made.count() == 3 {
            break;
        }
    }
    assert_eq!(r.made.last().0.as_deref(), Some(&b"tweaked"[..]), "made again with the state it had when undone");
}

/// The predicate: which commands may drop a hosted device. Built-ins have no state to lose.
#[test]
fn only_changes_that_drop_a_hosted_device_capture_first() {
    let mut app = crate::tests::app();
    let r = app.run("track.new", json!({"kind": "audio", "count": 2})).unwrap();
    let (a, b) = (r["tracks"][0].as_u64().unwrap(), r["tracks"][1].as_u64().unwrap());
    app.engine.session_mut().track_mut(TrackId(a)).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new(ID));
    app.engine.session_mut().track_mut(TrackId(b)).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new("eq7"));
    for (id, p, drops) in [
        ("track.delete", json!({"tracks": [a]}), true),
        ("track.delete", json!({"tracks": [b]}), false),
        ("mix.insert_remove", json!({"track": a, "slot": 0}), true),
        ("mix.insert_remove", json!({"track": b, "slot": 0}), false),
        ("mix.insert", json!({"track": a, "slot": 0, "plugin": "eq7"}), true),
        ("mix.insert", json!({"track": a, "slot": "a", "plugin": "eq7"}), true),
        ("mix.insert", json!({"track": a, "slot": 1, "plugin": "eq7"}), false),
        ("mix.insert_remove", json!({"track": a, "slot": 1}), false),
        ("mix.insert", json!({"track": a, "plugin": "eq7"}), false),
        // Track `a` hosts an insert, not an instrument: replacing its instrument drops nothing hosted.
        ("mix.instrument", json!({"track": a, "plugin": "macro_oscillator"}), false),
        ("mix.mute", json!({"track": a}), false),
        // Undoing the new tracks would take the hosted insert away with them.
        ("edit.undo", json!({}), true),
    ] {
        assert_eq!(app.drops_live_state(id, &p), drops, "{id} {p}");
    }
    let with = app.engine.session_arc();
    let mut without = (*with).clone();
    without.track_mut(TrackId(a)).unwrap().mixer.inserts[0] = None;
    assert!(drops_hosted(&with, &without));
    assert!(!drops_hosted(&without, &with));
}
