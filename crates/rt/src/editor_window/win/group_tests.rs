//! Issue #173: the grouping requests on real windows of the test's thread (no plug-in).
#![allow(clippy::unwrap_used)]

use super::*;
use crate::editor_window::{EditorWindow, HeaderCell, TEST_GLOBALS};
use std::sync::{MutexGuard, PoisonError};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::WindowsAndMessaging::{GW_HWNDNEXT, GW_OWNER, GetClientRect, GetWindow, GetWindowRect, SendMessageW, WM_CLOSE};

/// Holds the process-wide follow state for one test and puts it back (off, not parked) after.
struct Globals {
    _lock: MutexGuard<'static, ()>,
}

impl Globals {
    fn follow(on: bool) -> Globals {
        let g = Globals { _lock: TEST_GLOBALS.lock().unwrap_or_else(PoisonError::into_inner) };
        note_group(Group::Unpark);
        note_group(Group::Follow(on));
        g
    }
}

impl Drop for Globals {
    fn drop(&mut self) {
        note_group(Group::Follow(false));
    }
}

fn rect(hwnd: *mut std::ffi::c_void) -> (i32, i32, i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; `r` is writable.
    unsafe { GetWindowRect(hwnd, &raw mut r) };
    (r.left, r.top, r.right, r.bottom)
}

fn client(hwnd: *mut std::ffi::c_void) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; `r` is writable.
    unsafe { GetClientRect(hwnd, &raw mut r) };
    (r.right - r.left, r.bottom - r.top)
}

fn ex_style(hwnd: *mut std::ffi::c_void) -> u32 {
    // SAFETY: a style read of a live window of this thread.
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

/// Whether `upper` is above `lower` in the z-order.
fn above(upper: *mut std::ffi::c_void, lower: *mut std::ffi::c_void) -> bool {
    let mut w = upper;
    while !w.is_null() {
        // SAFETY: a query on a window handle (any value is accepted).
        w = unsafe { GetWindow(w, GW_HWNDNEXT) };
        if w == lower {
            return true;
        }
    }
    false
}

fn close_by_user(w: &EditorWindow) {
    // SAFETY: a live window of this thread; the frame's close only hides it.
    unsafe { SendMessageW(w.raw(), WM_CLOSE, 0, 0) };
}

fn shown(title: &str) -> EditorWindow {
    let w = EditorWindow::new(title, 240.0, 120.0, Some(HeaderCell::new())).unwrap();
    w.raise();
    w
}

#[test]
fn minimizing_parks_the_shown_editors_and_restoring_brings_them_back_in_place() {
    let _g = Globals::follow(true);
    let a = shown("A");
    let closed = shown("closed by the user");
    close_by_user(&closed);
    assert_eq!(closed.state(), WindowState::Hidden);
    let at = rect(a.raw());
    assert_eq!(group(Group::Park), 1, "only the shown editor is parked");
    assert_eq!(a.state(), WindowState::Parked);
    assert!(!visible(a.raw()), "hidden with the main window");
    assert!(a.is_visible(), "but still open for the user: its editor is not torn down");
    assert!(!closed.is_visible(), "the closed one stays closed");
    // An editor that opens while the main window is minimized waits with the others.
    let late = shown("opened while minimized");
    assert_eq!(late.state(), WindowState::Parked);
    assert_eq!(group(Group::Unpark), 2);
    assert_eq!((a.state(), late.state(), closed.state()), (WindowState::Shown, WindowState::Shown, WindowState::Hidden));
    assert_eq!(rect(a.raw()), at, "back in its place");
    for w in [a, closed, late] {
        w.close();
    }
}

#[test]
fn without_following_nothing_is_parked_or_restyled() {
    let _g = Globals::follow(false);
    let a = shown("A");
    assert_eq!(ex_style(a.raw()) & TOOL, 0, "its own taskbar button, as before #173");
    assert_eq!(group(Group::Park), 0);
    assert_eq!(a.state(), WindowState::Shown, "today's behavior: minimizing transit leaves it up");
    assert_eq!(group(Group::Front), 0);
    a.close();
}

#[test]
fn following_gives_the_tool_window_style_and_keeps_the_content_size() {
    let _g = Globals::follow(false);
    let a = shown("A");
    let content = client(a.parent());
    assert_eq!(group(Group::Follow(true)), 1);
    assert_ne!(ex_style(a.raw()) & TOOL, 0, "no taskbar button, not in Alt-Tab");
    assert_eq!(a.state(), WindowState::Shown, "still shown after the restyle");
    assert_eq!(client(a.parent()), content, "the plug-in keeps its pixel size");
    // SAFETY: a query on a live window of this thread.
    assert!(unsafe { GetWindow(a.raw(), GW_OWNER) }.is_null(), "still no owner (ADR 0018 section 7)");
    let b = EditorWindow::new("B", 100.0, 100.0, None).unwrap();
    assert_ne!(ex_style(b.raw()) & TOOL, 0, "a window made later takes the style");
    assert_eq!(group(Group::Park), 1, "B was never shown");
    assert_eq!(a.state(), WindowState::Parked);
    // Turning it off brings a parked editor back and gives back its own taskbar button.
    assert_eq!(group(Group::Follow(false)), 1, "the parked one came back");
    assert_eq!(a.state(), WindowState::Shown);
    assert_eq!(ex_style(a.raw()) & TOOL, 0);
    assert_eq!(client(a.parent()), content);
    a.close();
    b.close();
}

#[test]
fn raising_one_editor_brings_it_above_the_others_without_a_new_window() {
    let _g = Globals::follow(true);
    let a = shown("A");
    let b = shown("B");
    let _ = take_created();
    assert!(above(b.raw(), a.raw()), "B was shown last");
    assert!(raise_window(a.raw() as usize));
    assert!(above(a.raw(), b.raw()), "A is on top now");
    assert!(take_created().is_empty(), "raising made no new window");
    // Front keeps their order among themselves.
    assert_eq!(group(Group::Front), 2);
    assert!(above(a.raw(), b.raw()));
    // A parked or closed editor is not raised; another thread's window is not touched.
    assert_eq!(group(Group::Park), 2);
    assert!(!raise_window(a.raw() as usize));
    assert_eq!(group(Group::Unpark), 2);
    let raw = a.raw() as usize;
    let other = std::thread::spawn(move || (raise_window(raw), window_state(raw))).join().unwrap();
    assert_eq!(other, (false, None), "another thread gets nothing");
    close_by_user(&b);
    assert!(!raise_window(b.raw() as usize), "closed by the user: not brought back");
    a.close();
    b.close();
}

#[test]
fn the_windows_an_open_made_are_handed_out_once() {
    let _ = take_created();
    let a = EditorWindow::new("A", 100.0, 100.0, None).unwrap();
    assert_eq!(take_created(), vec![a.raw() as usize]);
    assert!(take_created().is_empty(), "once");
    let other = std::thread::spawn(take_created).join().unwrap();
    assert!(other.is_empty(), "per thread");
    a.close();
}
