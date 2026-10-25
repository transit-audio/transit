//! Ending the process without running exit handlers or DLL unload code (issue #104).
//!
//! `std::process::exit` runs the C runtime's exit handlers and, on Windows, `ExitProcess` first
//! kills every other thread and then calls each loaded DLL's `DllMain(PROCESS_DETACH)`, which
//! runs that DLL's static destructors. A plug-in whose destructor waits for one of its own worker
//! threads then waits forever: the thread is already gone. When plug-in hosting could not be shut
//! down cleanly, the app ends the process here instead, so no plug-in code runs at exit.

/// Ends the process at once with `code`: no exit handlers, no static destructors, no DLL unload
/// notifications, no unwinding. Windows: `TerminateProcess` on the current process. Linux and
/// macOS: `_exit`. Anything else: `std::process::abort` (the code is lost there).
///
/// Flush what must reach the disk (the log) before calling it; buffered output is lost.
pub fn terminate_process(code: i32) -> ! {
    imp(code)
}

#[cfg(windows)]
fn imp(code: i32) -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // The exit code is the 32-bit pattern of `code` (negative codes keep their bits, as
    // `std::process::exit` does on Windows).
    let status = u32::from_ne_bytes(code.to_ne_bytes());
    // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no closing and is always
    // valid for the calling process; `TerminateProcess` on it has no other preconditions. It does
    // not return when it succeeds.
    unsafe {
        TerminateProcess(GetCurrentProcess(), status);
    }
    // Only reached if the call failed, which the documentation does not foresee for the current
    // process. Abort rather than return into code that assumed the process was gone.
    std::process::abort()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn imp(code: i32) -> ! {
    // SAFETY: `_exit` is async-signal-safe, takes any status value and never returns; it runs no
    // `atexit` handlers or destructors, which is the point.
    unsafe { libc::_exit(code) }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn imp(_code: i32) -> ! {
    std::process::abort()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    const CHILD: &str = "TRANSIT_RT_TERMINATE_CHILD";

    /// Registered by the child: proves that no exit handler ran.
    extern "C" fn exit_handler() {
        println!("exit handler ran");
    }

    #[test]
    fn terminate_process_ends_with_the_code_and_runs_no_exit_handler() {
        if std::env::var_os(CHILD).is_some() {
            #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
            {
                unsafe extern "C" {
                    fn atexit(f: extern "C" fn()) -> std::ffi::c_int;
                }
                // SAFETY: the C library's `atexit` with its declared signature; the handler is a
                // plain function of this binary that lives as long as the process.
                assert_eq!(unsafe { atexit(exit_handler) }, 0);
            }
            println!("child terminating");
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
            super::terminate_process(42);
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process::tests::terminate_process_ends_with_the_code_and_runs_no_exit_handler", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("child terminating"), "the child never ran the case: {stdout}");
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert_eq!(out.status.code(), Some(42), "{stdout}");
            assert!(!stdout.contains("exit handler ran"), "an exit handler ran: {stdout}");
        }
    }
}
