//! Plug-in editor windows follow the main window (issue #173, ADR 0030), without an OS owner.
//!
//! The editor windows have no owner window (ADR 0018 section 7): on Windows an owner on another
//! thread attaches the two threads' input queues, so a plug-in that stops answering would stop the
//! UI with it. Transit groups them by hand instead. The UI sees its main window's transitions and
//! calls [`Host::editor_windows`], which posts the request to every host thread's queue and
//! returns: nothing here waits for a host thread. Each thread applies the request to its own
//! windows ([`transit_rt::editor_window::group`]) between jobs, as it runs any job. A thread
//! stuck inside a plug-in simply does not run it (the request waits in its queue, or is dropped
//! with a thread given up on), so the UI stays responsive: hang isolation is the invariant kept.
//!
//! Each editor learns its window from the open job ([`EditorWindow`] tracks the windows made on
//! its thread); the thread's tick publishes the window's state ([`publish`]) and a raise is one
//! more fire-and-forget request for that editor.
//!
//! [`EditorWindow`]: transit_rt::editor_window::EditorWindow

use crate::hosted::EditorShared;
use crate::{Busy, Host, call, lock};
use std::sync::atomic::Ordering;
use transit_dsp::{EditorWindowRequest, EditorWindowState};
use transit_rt::editor_window::{self as window, Group, WindowState};

fn group_of(request: EditorWindowRequest) -> Group {
    match request {
        EditorWindowRequest::Park => Group::Park,
        EditorWindowRequest::Unpark => Group::Unpark,
        EditorWindowRequest::Front => Group::Front,
        EditorWindowRequest::Follow(on) => Group::Follow(on),
    }
}

/// The state as a byte for [`EditorShared`] (0: none known).
fn encode(state: Option<WindowState>) -> u8 {
    match state {
        None => 0,
        Some(WindowState::Shown) => 1,
        Some(WindowState::Parked) => 2,
        Some(WindowState::Minimized) => 3,
        Some(WindowState::Hidden) => 4,
    }
}

/// What [`encode`] stored.
pub(crate) fn decode(byte: u8) -> Option<EditorWindowState> {
    Some(match byte {
        1 => EditorWindowState::Shown,
        2 => EditorWindowState::Parked,
        3 => EditorWindowState::Minimized,
        4 => EditorWindowState::Hidden,
        _ => return None,
    })
}

impl Host {
    /// Posts `request` to every live host thread and returns at once (issue #173): each thread
    /// applies it to its own editor windows when it next runs a job. A thread stuck in a plug-in
    /// does not apply it (the main window stays responsive meanwhile); a thread given up on is not
    /// sent it. The process-wide part (editors made later follow it too) is recorded here first.
    /// Returns how many threads it was posted to (0 in inline mode, where it is applied at once on
    /// the calling thread, which owns the windows there).
    pub fn editor_windows(&self, request: EditorWindowRequest) -> usize {
        let group = group_of(request);
        window::note_group(group);
        if self.is_inline() {
            let _changed = window::group(group);
            return 0;
        }
        let workers = lock(&self.inner.workers).clone();
        let mut posted = 0;
        for w in workers.iter().filter(|w| w.running.load(Ordering::Acquire) && !w.is_abandoned()) {
            w.send_or_leak(Busy::any(call::EDITOR_WINDOWS), move |_| {
                let _changed = window::group(group);
            });
            posted += 1;
        }
        posted
    }
}

/// From an editor's open job, right after the editor's own `open` on its thread: the window that
/// open made, if it made one (an open of an editor already open made none and keeps the one it
/// has). Call [`forget_created`] before the open.
pub(crate) fn note_opened(shared: &EditorShared) {
    if let Some(&made) = window::take_created().last() {
        shared.window.store(made, Ordering::Release);
    }
}

/// Before an editor's open: windows made on this thread outside an open are nobody's.
pub(crate) fn forget_created() {
    let _ = window::take_created();
}

/// From the host thread's tick: the state of an editor's window (none once it is closed).
pub(crate) fn publish(shared: &EditorShared, open: bool) {
    if !open {
        shared.window.store(0, Ordering::Release);
    }
    let made = shared.window.load(Ordering::Acquire);
    let state = if made == 0 { None } else { window::window_state(made) };
    shared.window_state.store(encode(state), Ordering::Release);
}

/// From a raise request, on the editor's thread: its window to the front, unactivated.
pub(crate) fn raise(shared: &EditorShared) {
    let made = shared.window.load(Ordering::Acquire);
    if made != 0 {
        let _raised = window::raise_window(made);
    }
}
