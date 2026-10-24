//! Windows shell identity: the explicit `AppUserModelID` (AUMID).
//!
//! Windows groups taskbar buttons, and caches their icons, by the process AUMID. Without an
//! explicit one it derives an ID from the executable path or the launching shortcut, so the
//! taskbar can show a stale or default icon. Set it once, early in `main`, to the same string the
//! installer puts on the Start Menu shortcut (`System.AppUserModel.ID`).

/// Sets the process's explicit `AppUserModelID`. Windows only; returns `false` elsewhere, for an
/// ID with an interior NUL, or when the shell refuses it. Call before the first window exists.
#[must_use]
pub fn set_app_user_model_id(id: &str) -> bool {
    imp(id)
}

#[cfg(windows)]
fn imp(id: &str) -> bool {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    if id.contains('\0') {
        return false;
    }
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call, and the function
    // only reads it; it has no other preconditions.
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
    hr >= 0
}

#[cfg(not(windows))]
fn imp(_id: &str) -> bool {
    false
}

/// Gives the window `hwnd` (and, through the shared window class, every other window of the
/// same class in the process, such as a second viewport) the icon embedded in the executable as
/// resource 1: both the big (taskbar, Alt-Tab) and the small (title bar) slot. winit only sets
/// the small slot from `with_icon` and registers its window class without an icon, so the big
/// slot is otherwise left to the shell's icon lookup, which sometimes comes up empty. Windows
/// only; returns `false` elsewhere, for a null handle, or when the exe carries no icon resource.
#[must_use]
pub fn set_window_icons_from_exe(hwnd: isize) -> bool {
    icons_imp(hwnd)
}

#[cfg(windows)]
fn icons_imp(hwnd: isize) -> bool {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GCLP_HICON, GCLP_HICONSM, GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON,
        SM_CYSMICON, SendMessageW, SetClassLongPtrW, WM_SETICON,
    };
    if hwnd == 0 {
        return false;
    }
    // SAFETY: a null module name asks for this process's own module; no preconditions.
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let load = |w: i32, h: i32| {
        // SAFETY: the resource id 1 is passed as the MAKEINTRESOURCE pseudo-pointer the API
        // documents; `module` is this exe. No LR_SHARED, so the two HICONs are owned and are
        // intentionally never destroyed: the window and its class use them for the life of the
        // process (two handles per process; the creation closure runs once on desktop). A failure
        // is a null handle, checked by the caller.
        unsafe { LoadImageW(module, std::ptr::without_provenance::<u16>(1), IMAGE_ICON, w, h, LR_DEFAULTCOLOR) }
    };
    // SAFETY: `GetSystemMetrics` has no preconditions.
    let (bw, bh, sw, sh) =
        unsafe { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON), GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON)) };
    let big = load(bw, bh);
    let small = load(sw, sh);
    if big.is_null() || small.is_null() {
        return false;
    }
    // SAFETY: `hwnd` is a window handle from the caller; WM_SETICON and the class setters only
    // read the icon handles, which stay loaded for the life of the process (never destroyed).
    unsafe {
        SendMessageW(hwnd as _, WM_SETICON, ICON_BIG as usize, big as isize);
        SendMessageW(hwnd as _, WM_SETICON, ICON_SMALL as usize, small as isize);
        SetClassLongPtrW(hwnd as _, GCLP_HICON, big as isize);
        SetClassLongPtrW(hwnd as _, GCLP_HICONSM, small as isize);
    }
    true
}

#[cfg(not(windows))]
fn icons_imp(_hwnd: isize) -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn interior_nul_is_refused() {
        assert!(!super::set_app_user_model_id("a\0b"));
    }

    #[test]
    fn a_null_window_gets_no_icons() {
        assert!(!super::set_window_icons_from_exe(0));
    }
}
