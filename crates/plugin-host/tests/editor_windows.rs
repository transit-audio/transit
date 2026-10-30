//! Issue #173 (ADR 0030): editor windows follow the main window through the host thread's queue,
//! with no ownership link. The UI posts and never waits: a host thread held inside a plug-in
//! leaves every call here returning at once, and the requests apply when it returns. Fake
//! plug-ins block on channels the test releases (E49); waits are completion signals with stall
//! bounds, never wall-clock assertions (E62).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use transit_dsp::{EditorWindowRequest as R, EditorWindowState as S, PluginEditor};
use transit_plugin_host::{Config, Host};

/// The follow state is process-wide (`transit_rt::editor_window`): the tests here take turns.
static GLOBALS: Mutex<()> = Mutex::new(());

fn globals() -> MutexGuard<'static, ()> {
    GLOBALS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A host whose requests have the app's 10 s budget: nothing here wedges a thread.
fn host() -> Host {
    Host::new(Config { inline: false, ..Config::default() })
}

/// Waits (a stall bound, polling once per 5 ms frame) until `done` holds; false if it never did.
fn until(done: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    // timing: stall-bound ten seconds for the host thread's next tick to publish the state
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// The host thread is held inside a plug-in (its editor open waits on a channel): every call the
/// UI makes for the grouping returns at once, and nothing is applied until the thread returns.
/// Without the fire-and-forget path, the first call below would wait for the held thread and the
/// test would never reach the release.
#[test]
fn a_held_host_thread_never_holds_the_ui_and_applies_the_requests_when_it_returns() {
    let _g = globals();
    let host = host();
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(10)).unwrap();
    let (release, gate) = mpsc::channel::<()>();
    *log.open_gate.lock().unwrap() = Some(gate);
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    // The open is running and blocked: the thread is inside the plug-in.
    assert!(until(|| log.thread_of("editor_open").is_some()), "the open started");
    // Everything the UI does for the grouping, on this (the UI's) thread, while it is held.
    for request in [R::Follow(true), R::Park, R::Unpark, R::Front, R::Follow(false)] {
        assert_eq!(host.editor_windows(request), 1, "posted to the one host thread");
    }
    editor.raise();
    assert_eq!(editor.window_state(), None, "nothing published yet");
    assert!(editor.is_open(), "the open on its way counts as open");
    // A request queued behind them (a completion signal, not a clock): it runs after them.
    let l = Arc::clone(&log);
    let behind = host.submit(move |_| {
        l.note("behind");
    });
    assert!(log.thread_of("behind").is_none(), "held: nothing behind the open has run");
    drop(release);
    assert!(behind.wait(Duration::from_secs(10)).is_some(), "the thread returned and ran the queue in order");
    assert_eq!(host.wedged(), 0);
    drop(editor);
    drop(plugin);
}

/// A thread given up on (#205) is never sent a grouping request: posting skips it.
#[test]
fn a_thread_given_up_on_is_not_sent_the_requests() {
    let _g = globals();
    let timeout = Duration::from_millis(200);
    let host = Host::new(Config { timeout, creation_timeout: timeout, inline: false, ..Config::default() });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(10)).unwrap();
    let (release, gate) = mpsc::channel::<()>();
    *log.open_gate.lock().unwrap() = Some(gate);
    let mut editor = plugin.editor().unwrap();
    editor.open().unwrap();
    assert!(
        until(|| {
            host.check();
            host.wedged() == 1
        }),
        "the held thread wedged"
    );
    let gave = host.give_up();
    assert_eq!(gave.plugins.len(), 1);
    // The fresh thread that took over is the only one asked.
    assert_eq!(host.editor_windows(R::Front), 1);
    editor.raise();
    drop(release);
    drop(editor);
    drop(plugin);
}

#[cfg(windows)]
mod windows {
    //! The whole path on a real window: a fake plug-in whose editor is an editor window of ours on
    //! the host thread (no plug-in code), driven the way the UI drives it.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use transit_dsp::{Category, Plugin, PluginInfo};
    use transit_rt::editor_window::EditorWindow;

    static INFO: PluginInfo = PluginInfo {
        id: "clap:test.window",
        name: "Windowed",
        short_name: "Windowed",
        category: Category::Other,
        params: &[],
        is_instrument: false,
        audiosuite: false,
    };

    /// Counts the windows its editor made.
    struct Windowed(Arc<AtomicUsize>);

    impl Plugin for Windowed {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            false
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
            Some(Box::new(WindowEditor { window: None, made: Arc::clone(&self.0) }))
        }
    }

    /// Opens an editor window the way the format hosts do: made hidden, then raised; an open of
    /// an open editor raises it again; a window the user closed is torn down on the next idle.
    struct WindowEditor {
        window: Option<EditorWindow>,
        made: Arc<AtomicUsize>,
    }

    impl PluginEditor for WindowEditor {
        fn open(&mut self) -> Result<(), String> {
            if self.window.as_ref().is_some_and(EditorWindow::is_visible) {
                self.window.as_ref().unwrap().raise();
                return Ok(());
            }
            if let Some(w) = self.window.take() {
                w.close();
            }
            let w = EditorWindow::new("Windowed", 200.0, 100.0, None)?;
            self.made.fetch_add(1, Ordering::SeqCst);
            w.raise();
            self.window = Some(w);
            Ok(())
        }
        fn close(&mut self) {
            if let Some(w) = self.window.take() {
                w.close();
            }
        }
        fn is_open(&self) -> bool {
            self.window.as_ref().is_some_and(EditorWindow::is_visible)
        }
        fn idle(&mut self) -> Vec<(String, f32)> {
            if self.window.as_ref().is_some_and(|w| !w.is_visible()) {
                self.close();
            }
            Vec::new()
        }
    }

    #[test]
    fn the_editor_window_parks_comes_back_and_raises_without_a_duplicate() {
        let _g = globals();
        let host = host();
        host.editor_windows(R::Follow(true));
        let made = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&made);
        let mut plugin = host.submit(move |ctx| ctx.adopt(Box::new(Windowed(m)))).wait(Duration::from_secs(10)).unwrap();
        let mut editor = plugin.editor().unwrap();
        editor.open().unwrap();
        assert!(until(|| editor.window_state() == Some(S::Shown)), "opened and shown: {:?}", editor.window_state());
        // Minimize the main window: the editor is parked but still open (not torn down).
        host.editor_windows(R::Park);
        assert!(until(|| editor.window_state() == Some(S::Parked)), "{:?}", editor.window_state());
        assert!(editor.is_open(), "parked is still open");
        // Restore: back in place.
        host.editor_windows(R::Unpark);
        assert!(until(|| editor.window_state() == Some(S::Shown)), "{:?}", editor.window_state());
        // Selecting the plug-in raises its window; opening it again makes no second window.
        editor.raise();
        host.editor_windows(R::Front);
        editor.open().unwrap();
        // A completion signal: the job queued behind the second open has run, so the open has.
        assert!(host.submit(|_| ()).wait(Duration::from_secs(10)).is_some());
        assert_eq!(made.load(Ordering::SeqCst), 1, "no second window: the existing one was raised");
        assert!(until(|| editor.window_state() == Some(S::Shown)));
        // Closing the editor: no window state, and nothing else goes with it.
        editor.close();
        assert!(until(|| editor.window_state().is_none()), "{:?}", editor.window_state());
        host.editor_windows(R::Follow(false));
        drop(editor);
        drop(plugin);
        assert!(host.shutdown(Duration::from_secs(10)));
    }
}
