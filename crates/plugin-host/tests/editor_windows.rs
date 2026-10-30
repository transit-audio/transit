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
use transit_dsp::EditorWindowRequest as R;
#[cfg(any(windows, target_os = "linux"))]
use transit_dsp::{EditorWindowState as S, PluginEditor};
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
fn until(mut done: impl FnMut() -> bool) -> bool {
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
/// The UI's calls run on a helper thread the test waits for with a stall bound: a call that
/// waited for the held thread fails the test instead of hanging it (#213 review minor D).
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
    // Everything the UI does for the grouping, on a stand-in UI thread, while it is held.
    let ui_host = host.clone();
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let posted: Vec<usize> =
            [R::Follow(true), R::Park, R::Unpark, R::Front, R::Follow(false)].into_iter().map(|r| ui_host.editor_windows(r)).collect();
        editor.raise();
        let seen = (editor.window_state(), editor.is_open());
        let _ = done.send((posted, seen, editor));
    });
    let (posted, seen, editor) = finished.recv_timeout(Duration::from_secs(10)).expect("a UI call waited for the held host thread");
    assert_eq!(posted, [1; 5], "each posted to the one host thread");
    assert_eq!(seen, (None, true), "nothing published yet; the open on its way counts as open");
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

#[cfg(any(windows, target_os = "linux"))]
mod windows {
    //! The whole path on a real window: a fake plug-in whose editor is an editor window of ours on
    //! the host thread (no plug-in code), driven the way the UI drives it. On Linux an X11 window
    //! (issue #147): it needs an X server (CI runs it under `xvfb-run -a`) and is skipped without.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use transit_dsp::{Category, Plugin, PluginInfo};
    use transit_rt::editor_window::EditorWindow;

    /// Linux needs an X server for these windows: true (with a message) when there is none, so
    /// the test returns. CI's plain test step has no `DISPLAY`; its Xvfb step runs them.
    fn no_display(test: &str) -> bool {
        let missing = cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none();
        if missing {
            eprintln!("{test}: skipped: no DISPLAY (run under `xvfb-run -a`)");
        }
        missing
    }

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

    /// #173 review: thread A is held while the user minimizes and restores. When A catches up and
    /// runs its stale park, the process-wide state stays as the UI set it, and an editor that
    /// opens on thread B meanwhile is shown. A is held again between its stale park and unpark
    /// (an editor open queued between them waits on a channel), the moment the old code got wrong.
    #[test]
    fn a_thread_that_catches_up_never_parks_an_editor_opening_elsewhere() {
        if no_display("a_thread_that_catches_up_never_parks_an_editor_opening_elsewhere") {
            return;
        }
        let _g = globals();
        let timeout = Duration::from_millis(200);
        let host = Host::new(Config { timeout, creation_timeout: Duration::from_secs(60), inline: false, ..Config::default() });
        host.editor_windows(R::Follow(true));
        // A plug-in on thread A, then A held by a slow creation; the one queued behind it moves
        // to a fresh thread B (issue #82), which becomes current.
        let log = Arc::new(Log::default());
        let l = Arc::clone(&log);
        let mut on_a = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(10)).unwrap();
        let (release_a, held_a) = mpsc::channel::<()>();
        let slow = host.submit(move |_| {
            let _ = held_a.recv();
        });
        let behind = host.submit(|_| std::thread::current().id());
        let mut b_thread = None;
        assert!(
            until(|| {
                if let transit_plugin_host::Status::Ready(id) = behind.poll() {
                    b_thread = Some(id);
                }
                b_thread.is_some()
            }),
            "the queued creation moved to a fresh thread"
        );
        assert_ne!(b_thread, log.thread_of("create"), "B is another thread");
        assert_eq!(host.wedged(), 0, "A is slow, not stuck: it still gets requests");
        // The user minimizes; A's queue then holds the park and, behind it, an editor open that
        // waits on a channel; then the user restores.
        assert_eq!(host.editor_windows(R::Park), 2);
        let (release_open, open_gate) = mpsc::channel::<()>();
        *log.open_gate.lock().unwrap() = Some(open_gate);
        let mut editor = on_a.editor().unwrap();
        editor.open().unwrap();
        assert_eq!(host.editor_windows(R::Unpark), 2);
        assert!(!transit_rt::editor_window::parked());
        // A catches up: its stale park runs, then it is held in the open, before its unpark.
        drop(release_a);
        assert!(until(|| log.thread_of("editor_open").is_some()), "A ran its stale park and is in the open");
        assert!(!transit_rt::editor_window::parked(), "the stale park did not undo the restore");
        assert!(transit_rt::editor_window::follows());
        // An editor opening on B now is shown.
        let made = Arc::new(AtomicUsize::new(0));
        let m = Arc::clone(&made);
        let mut on_b = host.submit(move |ctx| ctx.adopt(Box::new(Windowed(m)))).wait(Duration::from_secs(10)).unwrap();
        let mut b_editor = on_b.editor().unwrap();
        b_editor.open().unwrap();
        assert!(until(|| b_editor.window_state() == Some(S::Shown)), "shown, not parked: {:?}", b_editor.window_state());
        drop(release_open);
        assert!(slow.wait(Duration::from_secs(10)).is_some());
        b_editor.close();
        host.editor_windows(R::Follow(false));
        drop((editor, b_editor, on_a, on_b));
        assert!(host.shutdown(Duration::from_secs(10)));
    }

    #[test]
    fn the_editor_window_parks_comes_back_and_raises_without_a_duplicate() {
        if no_display("the_editor_window_parks_comes_back_and_raises_without_a_duplicate") {
            return;
        }
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
