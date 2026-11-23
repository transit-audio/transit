//! The Windows stretch for plug-in editors that cannot scale themselves (issue #153): the content
//! container the plug-in parents its view in is a DPI-unaware window inside the per-monitor-aware
//! frame, so Windows draws the plug-in's 100 percent bitmap stretched by the display's scale and
//! maps pointer input back into its coordinates.
//!
//! What Windows documents (Win32, "Mixed-Mode DPI Scaling and DPI-aware APIs";
//! `SetThreadDpiHostingBehavior`; `SetThreadDpiAwarenessContext`):
//!
//! - `SetThreadDpiAwarenessContext`: "Set the DPI awareness for the current thread to the provided
//!   value." A window takes the DPI awareness of the thread context it was created in, and keeps it.
//! - `DPI_AWARENESS_UNAWARE`: "DPI unaware. This app does not scale for DPI changes and is always
//!   assumed to have a scale factor of 100% (96 DPI). It will be automatically scaled by the system
//!   on any other DPI setting." The system's bitmap stretch is by the DPI of the monitor the window
//!   is on over 96, which is the display's scale and nothing else.
//! - `SetThreadDpiHostingBehavior`, `DPI_HOSTING_BEHAVIOR_MIXED`: "Mixed DPI behavior. This
//!   enables the creation and re-parenting of child windows with different DPI awareness. This
//!   context is applied when the parent window was created." With the default behavior "The
//!   associated window behaves as normal, and cannot create or re-parent child windows with a
//!   different `DPI_AWARENESS_CONTEXT`." It needs Windows 10 version 1803; the app runs from 1607
//!   (docs/building.md), so it is looked up at run time and the stretch is unavailable without it.
//!
//! So the frame is made with the mixed hosting behavior (it hosts the unaware container), the
//! container in the unaware context with the default behavior (the plug-in's windows under it
//! cannot differ from it), and the plug-in makes its windows inside [`ThreadDpi::unaware`] (VST3 `attached`,
//! CLAP `set_parent` and `show`), so the DPI it reads is 96. Only that frame and container are
//! touched; editors that are not stretched are made exactly as before.

use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_UNAWARE, DPI_HOSTING_BEHAVIOR, DPI_HOSTING_BEHAVIOR_MIXED, GetWindowDpiAwarenessContext,
    SetThreadDpiAwarenessContext,
};

/// `SetThreadDpiHostingBehavior` (user32, Windows 10 version 1803).
type SetHosting = unsafe extern "system" fn(DPI_HOSTING_BEHAVIOR) -> DPI_HOSTING_BEHAVIOR;

/// `SetThreadDpiHostingBehavior`, when this Windows has it.
fn set_hosting() -> Option<SetHosting> {
    static FN: OnceLock<Option<SetHosting>> = OnceLock::new();
    *FN.get_or_init(|| {
        let user32: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated module name; user32 is loaded in every process that links it
        // (this crate does), and a missing module gives null, handled below.
        let module = unsafe { GetModuleHandleW(user32.as_ptr()) };
        if module.is_null() {
            return None;
        }
        // SAFETY: a live module handle and a NUL-terminated ASCII export name; `None` when the
        // export does not exist (Windows before 1803).
        let f = unsafe { GetProcAddress(module, c"SetThreadDpiHostingBehavior".as_ptr().cast()) }?;
        // SAFETY: the export's documented signature is `DPI_HOSTING_BEHAVIOR
        // SetThreadDpiHostingBehavior(DPI_HOSTING_BEHAVIOR value)` with the WINAPI (system) calling
        // convention; `DPI_HOSTING_BEHAVIOR` is a 32-bit enum (`i32` in windows-sys). Both function
        // pointer types are pointer-sized.
        Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SetHosting>(f) })
    })
}

/// Whether this Windows can stretch an editor (the mixed hosting behavior exists).
#[must_use]
pub fn available() -> bool {
    set_hosting().is_some()
}

/// While alive, windows this thread creates get the mixed DPI hosting behavior; the previous
/// behavior comes back on drop. `None` when this Windows has no such call.
pub(super) struct Mixed {
    previous: DPI_HOSTING_BEHAVIOR,
    set: SetHosting,
}

impl Mixed {
    pub(super) fn enter() -> Option<Mixed> {
        let set = set_hosting()?;
        // SAFETY: the documented signature (see `set_hosting`); it changes the calling thread's
        // hosting behavior only and returns the previous one.
        let previous = unsafe { set(DPI_HOSTING_BEHAVIOR_MIXED) };
        Some(Mixed { previous, set })
    }
}

impl Drop for Mixed {
    fn drop(&mut self) {
        // SAFETY: as in `enter`; restores what this thread had.
        unsafe { (self.set)(self.previous) };
    }
}

/// While alive, this thread runs in another DPI awareness context; the previous one comes back
/// on drop. [`ThreadDpi::unaware`]: the DPI-unaware context (windows the thread creates are
/// DPI-unaware and DPI queries answer 96). [`ThreadDpi::of_window`]: the context of a window of
/// ours, for frame work reached from a DPI-unaware window procedure (a plug-in's synchronous
/// resize request, #153 review), so physical sizes are not virtualized. Inert (changes nothing)
/// when made with `on == false`.
pub struct ThreadDpi {
    previous: Option<DPI_AWARENESS_CONTEXT>,
}

impl ThreadDpi {
    pub(super) fn unaware(on: bool) -> ThreadDpi {
        ThreadDpi::set(on.then_some(DPI_AWARENESS_CONTEXT_UNAWARE))
    }

    pub(super) fn of_window(hwnd: windows_sys::Win32::Foundation::HWND, on: bool) -> ThreadDpi {
        // SAFETY: accepts any window handle (null context for an invalid one, then nothing is set).
        let ctx = if on { unsafe { GetWindowDpiAwarenessContext(hwnd) } } else { std::ptr::null_mut() };
        ThreadDpi::set((!ctx.is_null()).then_some(ctx))
    }

    fn set(ctx: Option<DPI_AWARENESS_CONTEXT>) -> ThreadDpi {
        let Some(ctx) = ctx else { return ThreadDpi { previous: None } };
        // SAFETY: a predefined context value or one Windows returned for a live window; it changes
        // the calling thread's context only and returns the previous one (null if the value were
        // invalid, which leaves the thread as it was: nothing to restore then).
        let previous = unsafe { SetThreadDpiAwarenessContext(ctx) };
        ThreadDpi { previous: (!previous.is_null()).then_some(previous) }
    }

    /// Whether the scope changed the thread's context.
    #[must_use]
    pub fn active(&self) -> bool {
        self.previous.is_some()
    }
}

impl Drop for ThreadDpi {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            // SAFETY: the context `SetThreadDpiAwarenessContext` returned on this thread (a scope
            // is not `Send`: it holds a raw pointer, so it is dropped where it was made).
            unsafe { SetThreadDpiAwarenessContext(previous) };
        }
    }
}

/// Physical pixels for `px` plug-in pixels stretched at `dpi`.
#[must_use]
pub(super) fn physical(px: i32, dpi: u32) -> i32 {
    let dpi = i64::from(if dpi == 0 { 96 } else { dpi.min(960) });
    i32::try_from((i64::from(px) * dpi + 48) / 96).unwrap_or(px)
}
