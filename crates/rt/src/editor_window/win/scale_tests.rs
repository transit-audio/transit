//! Issue #153 on real windows of the test's thread (no plug-in): the Windows stretch (a
//! DPI-unaware content container in a per-monitor-aware frame) and the Scale submenu.
//!
//! The test thread runs in the per-monitor-aware (v2) context, as transit's threads do (the app is
//! per-monitor aware; a test binary is not). Sizes are asserted against the frame's own DPI, so the
//! tests hold on any display: at 100 percent the stretched sizes equal the plug-in's, at 150
//! percent (the reference machine) they are one and a half times them.
#![allow(clippy::unwrap_used)]

use super::*;
use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_UNAWARE,
    DPI_HOSTING_BEHAVIOR_DEFAULT, DPI_HOSTING_BEHAVIOR_MIXED, GetThreadDpiAwarenessContext, GetWindowDpiAwarenessContext,
    GetWindowDpiHostingBehavior, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetSubMenu, SendMessageW};

/// The thread in the per-monitor-aware (v2) context for one test, put back after.
struct PerMonitor(DPI_AWARENESS_CONTEXT);

impl PerMonitor {
    fn enter() -> PerMonitor {
        // SAFETY: a predefined context value; it changes this thread's context only.
        PerMonitor(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PerMonitor {
    fn drop(&mut self) {
        // SAFETY: the context this thread had, on this thread.
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

fn context_is(hwnd: HWND, ctx: DPI_AWARENESS_CONTEXT) -> bool {
    // SAFETY: both calls accept any window handle and predefined or returned context values.
    unsafe { AreDpiAwarenessContextsEqual(GetWindowDpiAwarenessContext(hwnd), ctx) != 0 }
}

fn thread_is(ctx: DPI_AWARENESS_CONTEXT) -> bool {
    // SAFETY: as above, for this thread's context.
    unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), ctx) != 0 }
}

/// A window's size as this (per-monitor-aware) thread sees it: physical pixels.
fn outer_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; `r` is writable.
    unsafe { GetWindowRect(hwnd, &raw mut r) };
    (r.right - r.left, r.bottom - r.top)
}

/// A window's client size as this thread sees it.
fn client(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; `r` is writable.
    unsafe { GetClientRect(hwnd, &raw mut r) };
    (r.right - r.left, r.bottom - r.top)
}

/// A stand-in for the plug-in's own window: a plain child of the content container.
fn plugin_window(parent: HWND, w: i32, h: i32) -> HWND {
    let class = classes().unwrap().content;
    // SAFETY: a registered class; the parent is our live content container on this thread.
    let child = unsafe {
        CreateWindowExW(
            0,
            usize::from(class) as *const u16,
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            w,
            h,
            parent,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    assert!(!child.is_null());
    child
}

/// The frame stays per-monitor aware and hosts the DPI-unaware container (mixed hosting); the
/// container is the plug-in's size times the frame's DPI over 96 on screen, and a resize the
/// plug-in asks for in its own pixels keeps that relation.
#[test]
fn a_stretched_window_hosts_a_dpi_unaware_container_at_the_display_scale() {
    let _pm = PerMonitor::enter();
    assert!(stretch::available(), "Windows 10 1803 or later on every test machine");
    let win = EditorWindow::new_stretched("stretched", 320.0, 200.0, Some(HeaderCell::new())).unwrap();
    let (frame, content) = (win.raw(), win.parent());
    assert!(context_is(frame, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2), "the frame (and its strip) stay sharp");
    assert!(context_is(content, DPI_AWARENESS_CONTEXT_UNAWARE), "the plug-in's container is DPI-unaware");
    // SAFETY: both accept any window handle.
    let (frame_hosting, content_hosting) = unsafe { (GetWindowDpiHostingBehavior(frame), GetWindowDpiHostingBehavior(content)) };
    assert_eq!(frame_hosting, DPI_HOSTING_BEHAVIOR_MIXED, "the frame may host a child of another awareness");
    assert_eq!(content_hosting, DPI_HOSTING_BEHAVIOR_DEFAULT, "the plug-in's windows cannot differ from the container");
    let dpi = dpi_of(frame);
    assert_eq!(dpi_of(content), 96, "the plug-in reads 96 DPI");
    assert_eq!(win.stretch(), Some(f64::from(dpi) / 96.0));
    assert_eq!(win.content_size(), (320, 200), "the plug-in's own pixels");
    let phys = |px| stretch::physical(px, dpi);
    assert_eq!(outer_size(content), (phys(320), phys(200)), "on screen: stretched by the frame's DPI over 96 ({dpi})");
    assert_eq!(client(frame), (phys(320), phys(200) + super::super::header_height(dpi)));
    assert!(resize_content(frame, 400.0, 300.0));
    assert_eq!(win.content_size(), (400, 300));
    assert_eq!(outer_size(content), (phys(400), phys(300)));
    win.close();
}

/// The plug-in makes its window inside the scope: it is DPI-unaware, the thread's context comes
/// back after, and the window follows the plug-in's child in the child's own pixels. A window the
/// plug-in makes later, outside the scope, still takes the container's awareness.
#[test]
fn the_plugin_window_is_dpi_unaware_and_followed_in_its_own_pixels() {
    let _pm = PerMonitor::enter();
    let win = EditorWindow::new_stretched("follow", 800.0, 500.0, Some(HeaderCell::new())).unwrap();
    let child = {
        let scope = win.plugin_scope();
        assert!(scope.active());
        assert!(thread_is(DPI_AWARENESS_CONTEXT_UNAWARE));
        plugin_window(win.parent(), 700, 480)
    };
    assert!(thread_is(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2), "the scope puts the thread's context back");
    assert!(context_is(child, DPI_AWARENESS_CONTEXT_UNAWARE));
    assert_eq!(dpi_of(child), 96);
    win.forget_resize();
    for _ in 0..Follow::STABLE {
        win.sync();
    }
    assert_eq!(win.content_size(), (700, 480), "followed in the plug-in's pixels, not the screen's");
    let dpi = dpi_of(win.raw());
    assert_eq!(outer_size(win.parent()), (stretch::physical(700, dpi), stretch::physical(480, dpi)));
    let later = plugin_window(child, 50, 50);
    assert!(context_is(later, DPI_AWARENESS_CONTEXT_UNAWARE), "a later window of the plug-in takes the container's awareness");
    win.close();
}

/// #153 review, item 6: a stretched plug-in's synchronous resize request arrives from its
/// DPI-unaware window procedure; `resize_content` called from a DPI-unaware caller sizes the frame
/// in the frame's own (physical) pixels all the same, and the caller's context is put back.
#[test]
fn a_resize_from_a_dpi_unaware_caller_sizes_the_stretched_window_right() {
    let _pm = PerMonitor::enter();
    let win = EditorWindow::new_stretched("unaware caller", 320.0, 200.0, Some(HeaderCell::new())).unwrap();
    {
        let _plugin = win.plugin_scope();
        assert!(thread_is(DPI_AWARENESS_CONTEXT_UNAWARE));
        assert!(resize_content(win.raw(), 400.0, 300.0));
        assert!(thread_is(DPI_AWARENESS_CONTEXT_UNAWARE), "the caller's context is put back");
    }
    let dpi = dpi_of(win.raw());
    assert_eq!(win.content_size(), (400, 300));
    assert_eq!(outer_size(win.parent()), (stretch::physical(400, dpi), stretch::physical(300, dpi)), "at {dpi} DPI");
    assert_eq!(client(win.raw()), (stretch::physical(400, dpi), stretch::physical(300, dpi) + super::super::header_height(dpi)));
    win.close();
}

/// A window that is not stretched is made exactly as before: one awareness, default hosting, an
/// inert scope, and the content in the plug-in's (physical) pixels.
#[test]
fn an_editor_that_is_not_stretched_is_made_as_before() {
    let _pm = PerMonitor::enter();
    let win = EditorWindow::new("plain", 320.0, 200.0, Some(HeaderCell::new())).unwrap();
    assert!(context_is(win.raw(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
    assert!(context_is(win.parent(), DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
    // SAFETY: accepts any window handle.
    assert_eq!(unsafe { GetWindowDpiHostingBehavior(win.raw()) }, DPI_HOSTING_BEHAVIOR_DEFAULT);
    assert_eq!(win.stretch(), None);
    assert!(!win.plugin_scope().active(), "nothing changes for the plug-in");
    assert!(thread_is(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2));
    assert_eq!(outer_size(win.parent()), (320, 200));
    win.close();
}

/// A DPI change keeps the plug-in's own size and stretches by the new DPI.
#[test]
fn a_dpi_change_restretches_the_container_and_keeps_the_plugin_size() {
    let _pm = PerMonitor::enter();
    let win = EditorWindow::new_stretched("dpi", 320.0, 200.0, Some(HeaderCell::new())).unwrap();
    let mut suggested = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; WM_DPICHANGED's wparam carries the DPI twice and its
    // lparam points to a suggested RECT, which outlives the call.
    unsafe {
        SendMessageW(win.raw(), WM_DPICHANGED, (0xC0 << 16) | 0xC0 /* 192 DPI */, (&raw mut suggested) as LPARAM)
    };
    assert_eq!(win.content_size(), (320, 200), "the plug-in's pixels");
    assert_eq!(win.stretch(), Some(2.0), "stretched by the new DPI over 96");
    // (The container's measured size is not checked: the message changes the DPI the frame
    // computes with, not the monitor's, so the frame's non-client metrics no longer match and its
    // client area comes out a few pixels off; the same caveat as the #101 DPI test in win.rs.)
    win.close();
}

/// The menu's Scale submenu is a real Win32 submenu with its entries.
#[test]
fn the_scale_submenu_is_built_as_a_win32_submenu() {
    let items = vec![
        MenuItem::Item { id: 1, label: "Save".into(), checked: false, enabled: true },
        MenuItem::Submenu {
            label: "Scale".into(),
            items: vec![
                MenuItem::Heading("In use: drawn by the plug-in at 150%".into()),
                MenuItem::Separator,
                super::super::scale_item(2, "Auto (150%)", true, true),
            ],
        },
    ];
    let menu = build_menu(&items);
    assert!(!menu.is_null());
    // SAFETY: the live menu just built; queries only.
    unsafe {
        assert_eq!(GetMenuItemCount(menu), 2);
        let sub = GetSubMenu(menu, 1);
        assert!(!sub.is_null(), "the second entry opens a submenu");
        assert_eq!(GetMenuItemCount(sub), 3);
        assert!(GetSubMenu(menu, 0).is_null(), "a plain entry has none");
        DestroyMenu(menu);
    }
}
