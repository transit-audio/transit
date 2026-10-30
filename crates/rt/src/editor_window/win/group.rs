//! Editor windows follow the main window by hand (issue #173, ADR 0030): the requests a host
//! thread applies to its own editor windows, and the state it reads back from them.
//!
//! Every function here acts only on windows of the calling thread (frames this module's parent
//! made, checked with `data`), so a request is applied by the thread that owns the windows and
//! never by the UI thread: the UI posts it to that thread's queue and does not wait (ADR 0018's
//! hang isolation). The calls are the ones a thread makes on its own windows (`ShowWindow`,
//! `SetWindowPos`, `SetWindowLongPtrW`); none of them is ever made on another thread's window.

use super::{data, set_content};
use crate::editor_window::{Group, WindowState, note_group};
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOP, IsIconic, IsWindowVisible, SW_HIDE, SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WS_EX_TOOLWINDOW,
};

/// The extended style of an editor that follows the main window: no taskbar button, not in
/// Alt-Tab.
pub(super) const TOOL: u32 = WS_EX_TOOLWINDOW;

thread_local! {
    /// Frames made on this thread since the last [`take_created`].
    static CREATED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Notes a frame `EditorWindow::new` made on this thread.
pub(super) fn created(hwnd: HWND) {
    CREATED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() < 64 {
            c.push(hwnd as usize);
        }
    });
}

/// The editor windows made on the calling thread since the last call (handles as numbers, for
/// [`raise_window`] and [`window_state`]): the plug-in host learns which window an editor open
/// made by taking them right after the open.
#[must_use]
pub fn take_created() -> Vec<usize> {
    CREATED.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// The editor frames of the calling thread, in the order Windows lists them (top of the z-order
/// first, in practice).
fn frames() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, found: LPARAM) -> windows_sys::core::BOOL {
        // SAFETY: `found` is the `Vec` passed to `EnumThreadWindows` below, alive and not otherwise
        // borrowed for the whole enumeration, which calls this on the calling thread.
        let found = unsafe { &mut *(found as *mut Vec<HWND>) };
        if data(hwnd).is_some() {
            found.push(hwnd);
        }
        1
    }
    let mut found: Vec<HWND> = Vec::new();
    // SAFETY: the calling thread's own id; a callback of the documented signature and an `LPARAM`
    // pointing to `found`, which outlives the call. The enumeration runs on this thread and sends
    // no message.
    unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(collect), (&raw mut found) as LPARAM) };
    found
}

fn visible(hwnd: HWND) -> bool {
    // SAFETY: accepts any handle; it reads the window's style and sends no message.
    unsafe { IsWindowVisible(hwnd) != 0 }
}

/// Brings a frame of this thread to the top of the z-order (and shows it if `show`) without
/// activating it: the main window keeps the keyboard focus.
fn to_front(hwnd: HWND, show: bool) {
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { 0 };
    // SAFETY: a live frame of this thread (callers checked it with `data`); only its z-order (and
    // its visibility with `show`) changes.
    unsafe { SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, flags) };
}

/// Gives a frame of this thread the tool-window style (`tool`) or takes it away, keeping the
/// plug-in's content size. A shown window is hidden around the change, so the taskbar notices.
/// `hwnd` is a live frame of this thread (the callers checked it with `data`).
fn restyle(hwnd: HWND, tool: bool) {
    // SAFETY: a style read of a live frame of this thread.
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let want = if tool { ex | TOOL } else { ex & !TOOL };
    if want == ex {
        return;
    }
    let shown = visible(hwnd);
    // SAFETY: a live frame of this thread: hidden (when shown), restyled, its frame recomputed and
    // shown again without activation. Its children (the strip, the plug-in's content) are kept.
    unsafe {
        if shown {
            ShowWindow(hwnd, SW_HIDE);
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want as isize);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    // The sends above ran window procedures (the plug-in's children among them): take the frame's
    // data again, and stop if the frame is gone.
    let Some(d) = data(hwnd) else { return };
    // The tool caption is shorter: the frame is resized so the plug-in keeps its pixel size.
    let (w, h) = d.size.get();
    set_content(hwnd, d, w, h);
    if shown {
        // SAFETY: as above.
        unsafe { ShowWindow(hwnd, SW_SHOWNA) };
    }
}

/// Records `request` for the whole process ([`note_group`]) and applies it to every editor window
/// of the calling thread ([`apply`]): where the thread that records it owns the windows (inline
/// mode, the tests). Returns how many windows it changed.
#[must_use = "the count tells the tests what changed; `let _ =` where it does not matter"]
pub fn group(request: Group) -> usize {
    note_group(request);
    apply(request)
}

/// Applies `request` to every editor window of the calling thread without recording anything: what
/// a plug-in host thread runs from its queue, after the UI recorded the request when it posted it
/// (the UI is the only writer of the process-wide state). Each window is brought to the state the
/// process is in now ([`follows`], [`parked`]), not the one the request was posted for, so a thread
/// that was stuck and applies its queued requests late never undoes what the UI did since (#173
/// review): a stale park while the main window is up hides nothing, and a stale follow change
/// restyles to the current setting. Park, Unpark and Front do nothing while editors do not follow
/// the main window ([`Group::Follow`]`(false)`, the behavior before #173). Returns how many
/// windows it changed.
///
/// [`follows`]: crate::editor_window::follows
/// [`parked`]: crate::editor_window::parked
#[must_use = "the count tells the tests what changed; `let _ =` where it does not matter"]
pub fn apply(request: Group) -> usize {
    let follow = crate::editor_window::follows();
    let parked = crate::editor_window::parked();
    let mut changed = 0;
    let frames = frames();
    let request = match request {
        Group::Follow(_) => Group::Follow(follow),
        Group::Park if !parked => return 0,
        Group::Unpark if parked => return 0,
        r => r,
    };
    match request {
        Group::Park if follow => {
            for &hwnd in &frames {
                let Some(d) = data(hwnd) else { continue };
                if visible(hwnd) {
                    d.parked.set(true);
                    // SAFETY: a live frame of this thread.
                    unsafe { ShowWindow(hwnd, SW_HIDE) };
                    changed += 1;
                }
            }
        }
        Group::Unpark | Group::Follow(false) => {
            for &hwnd in &frames {
                let Some(d) = data(hwnd) else { continue };
                if d.parked.replace(false) {
                    // Shown where it was, in its current state, without taking the focus.
                    // SAFETY: a live frame of this thread.
                    unsafe { ShowWindow(hwnd, SW_SHOWNA) };
                    changed += 1;
                }
            }
            if request == Group::Follow(false) {
                for &hwnd in &frames {
                    if data(hwnd).is_some() {
                        restyle(hwnd, false);
                    }
                }
            }
        }
        Group::Front if follow => {
            // Bottom first, so the editors keep their order among themselves on top.
            for &hwnd in frames.iter().rev() {
                if data(hwnd).is_some() && visible(hwnd) {
                    to_front(hwnd, false);
                    changed += 1;
                }
            }
        }
        Group::Follow(true) => {
            for &hwnd in &frames {
                if data(hwnd).is_some() {
                    restyle(hwnd, true);
                    changed += 1;
                }
            }
        }
        Group::Park | Group::Front => {}
    }
    changed
}

/// Brings the editor window `window` (a handle from [`take_created`]) to the front without
/// activating it: selecting its plug-in in the device chain or the mixer (issue #173). A parked
/// window stays parked (it comes back with the main window), and one the user closed stays
/// closed. False when `window` is not a live editor window of the calling thread or it did not
/// move.
#[must_use = "false: the window was not raised"]
pub fn raise_window(window: usize) -> bool {
    let hwnd = window as HWND;
    let Some(d) = data(hwnd) else { return false };
    if d.parked.get() || !visible(hwnd) {
        return false;
    }
    to_front(hwnd, true);
    true
}

/// The state of the editor window `window` (a handle from [`take_created`]); `None` when it is
/// not a live editor window of the calling thread.
#[must_use]
pub fn window_state(window: usize) -> Option<WindowState> {
    let hwnd = window as HWND;
    let d = data(hwnd)?;
    // SAFETY: accepts any handle; a style read that sends no message.
    let iconic = unsafe { IsIconic(hwnd) } != 0;
    Some(if d.parked.get() {
        WindowState::Parked
    } else if iconic {
        WindowState::Minimized
    } else if visible(hwnd) {
        WindowState::Shown
    } else {
        WindowState::Hidden
    })
}

#[cfg(test)]
#[path = "group_tests.rs"]
mod tests;
