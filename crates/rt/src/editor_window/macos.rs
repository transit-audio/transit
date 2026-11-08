//! macOS plug-in editor windows: an `NSWindow` whose content view is the plug-in view's parent.
//!
//! Moved from `transit-vst3-host` so the VST3 and CLAP hosts share one window (ADR 0013: new
//! `unsafe` lives in this crate). `AppKit` allows windows on the process main thread only; on macOS
//! plug-in editors run inline on the UI thread, which is that thread (ADR 0018 decision 7). Every
//! method checks `MainThreadMarker` and does nothing elsewhere. There is no header strip here, as on
//! Linux: the plug-in's view fills the window.

use super::HeaderCell;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSFloatingWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;

thread_local! {
    /// The windows [`EditorWindow::new`] made on this thread (the main thread) that
    /// [`EditorWindow::close`] has not released: the only ones [`resize_content`] touches.
    static LIVE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// An `NSWindow` (titled, closable, floating above the app's windows) whose content view is the
/// plug-in view's parent. Holds one retain of the window; main thread only.
pub struct EditorWindow {
    window: *mut NSWindow,
}

// SAFETY: the pointer is only dereferenced on the main thread (every method checks
// `MainThreadMarker`); the struct merely carries the retain between threads' locks.
unsafe impl Send for EditorWindow {}

impl EditorWindow {
    fn get(&self) -> Option<&NSWindow> {
        MainThreadMarker::new()?;
        // SAFETY: we hold a retain of this window (from `new`) until `close`.
        unsafe { self.window.as_ref() }
    }

    /// A hidden window with a `w` by `h` point content area, centered. `_header`: no strip on this
    /// platform.
    ///
    /// # Errors
    /// Off the main thread.
    pub fn new(title: &str, w: f64, h: f64, _header: Option<Arc<HeaderCell>>) -> Result<EditorWindow, String> {
        let mtm = MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string())?;
        let _app = NSApplication::sharedApplication(mtm);
        let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(w, h));
        // Plugin editors are transient windows. In particular they must not be
        // minimizable: minimized NSWindows become persistent thumbnails in the macOS Dock,
        // which made every editor visited in a session look like another running app.
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        // SAFETY: a freshly allocated window on the main thread; valid rect and style.
        let window =
            unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false) };
        // SAFETY: we keep our own retain and release it in `close`, so AppKit must not
        // release the window when the user closes it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(title));
        window.setLevel(NSFloatingWindowLevel);
        window.setHidesOnDeactivate(true);
        window.setExcludedFromWindowsMenu(true);
        window.setCollectionBehavior(NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle);
        window.center();
        let window = Retained::into_raw(window);
        LIVE.with(|live| live.borrow_mut().push(window.addr()));
        Ok(EditorWindow { window })
    }

    /// The raw `NSWindow*` (for [`resize_content`]).
    #[must_use]
    pub fn raw(&self) -> *mut c_void {
        self.window.cast()
    }

    /// The content view (`NSView*`) the plug-in view attaches to; null when unavailable.
    #[must_use]
    pub fn parent(&self) -> *mut c_void {
        self.get().and_then(NSWindow::contentView).map_or(std::ptr::null_mut(), |v| Retained::as_ptr(&v).cast_mut().cast())
    }

    /// Shows the window and makes it key.
    pub fn raise(&self) {
        if let Some(w) = self.get() {
            w.makeKeyAndOrderFront(None);
        }
    }

    /// Whether the window is on screen (false once the user closed it).
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.get().is_some_and(NSWindow::isVisible)
    }

    /// 1.0: views work in points and read the backing scale themselves (issue #101).
    #[must_use]
    pub fn scale(&self) -> f64 {
        1.0
    }

    /// `None`: see [`Self::scale`].
    #[must_use]
    pub fn take_scale_change(&self) -> Option<f64> {
        None
    }

    /// The content area in points; `(0, 0)` off the main thread.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // window sizes in points fit an i32
    pub fn content_size(&self) -> (i32, i32) {
        self.get().and_then(NSWindow::contentView).map_or((0, 0), |v| {
            let s = v.frame().size;
            (s.width.round() as i32, s.height.round() as i32)
        })
    }

    /// Nothing to keep in step: no strip here.
    pub fn sync(&self) {}

    /// Hides and closes the window and releases its retain (main thread only).
    pub fn close(self) {
        if let Some(w) = self.get() {
            w.orderOut(None);
            w.close();
        }
        if MainThreadMarker::new().is_some() {
            LIVE.with(|live| live.borrow_mut().retain(|&w| w != self.window.addr()));
            // SAFETY: balances the retain taken in `new`; nothing uses the pointer after this
            // (`resize_content` no longer finds it in `LIVE`).
            drop(unsafe { Retained::from_raw(self.window) });
        }
    }
}

/// Resizes a window's content (`window` from [`EditorWindow::raw`]) to `w` by `h` points; main
/// thread only. Returns whether it resized: false for any pointer that is not a live window of
/// this thread (closed, foreign or invalid), which it never dereferences.
pub fn resize_content(window: *mut c_void, w: f64, h: f64) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    if !LIVE.with(|live| live.borrow().contains(&window.addr())) {
        return false;
    }
    // SAFETY: a window `new` made on this thread and `close` has not released (`LIVE`), so we
    // still hold its retain: valid and non-null.
    let Some(win) = (unsafe { window.cast::<NSWindow>().as_ref() }) else { return false };
    win.setContentSize(NSSize::new(w, h));
    true
}
