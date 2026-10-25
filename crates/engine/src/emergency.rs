//! Bounded shutdown (issue #104): the quit, a lost graphics device and a panic on the UI thread
//! all end the process through here.
//!
//! The order of the work is the caller's (the app's quit order, E5); this module bounds it and
//! decides how the process ends. A watchdog thread ends the process if the work has not finished
//! within the bound. The process then exits normally only when plug-in hosting shut down cleanly
//! ([`crate::shutdown_plugin_hosts`] returned true). Otherwise it is terminated without exit
//! handlers ([`transit_rt::terminate_process`]): a plug-in binary that never had its module exit
//! would run its static destructors at exit, after the exit has killed its worker threads, and a
//! destructor that waits for one of them never returns.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// How long the shutdown work may take before the watchdog ends the process. It covers the
/// autosave (with a capture of plug-in state), closing the input, dropping the player and the
/// plug-in host thread's own 5 s shutdown wait, with room to spare. A normal quit takes well under
/// a second.
// timing: stall-bound a ceiling for a hung plug-in or driver, far above a normal shutdown
pub const SHUTDOWN_BOUND: Duration = Duration::from_secs(15);

/// Exit code after an emergency shutdown that finished cleanly (the process exited normally).
pub const EXIT_AFTER_SHUTDOWN: i32 = 70;
/// Exit code when plug-in hosting did not shut down cleanly and the process was terminated
/// without exit handlers.
pub const EXIT_TERMINATED: i32 = 71;
/// Exit code when the watchdog ended the process because the shutdown work overran its bound.
pub const EXIT_WATCHDOG: i32 = 72;

/// How long the watchdog waits for its own log line before terminating anyway (the logger could
/// be the thing that is stuck).
const LOG_WAIT: Duration = Duration::from_secs(1);

/// Runs `work` (true when it shut down cleanly) under a watchdog: if `work` has not returned
/// within `bound`, the watchdog logs it and terminates the process with [`EXIT_WATCHDOG`]. A panic
/// in `work` counts as unclean. Returns what `work` returned.
pub fn run_bounded(what: &str, bound: Duration, work: impl FnOnce() -> bool) -> bool {
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let watchdog = {
        let done = Arc::clone(&done);
        let what = what.to_owned();
        std::thread::Builder::new().name("shutdown-watchdog".into()).spawn(move || {
            let (lock, finished) = &*done;
            let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (guard, _) = finished.wait_timeout_while(guard, bound, |done| !*done).unwrap_or_else(PoisonError::into_inner);
            if !*guard {
                drop(guard);
                log_bounded(format!("{what} did not finish within {} s; ending the process now", bound.as_secs()));
                transit_rt::terminate_process(EXIT_WATCHDOG);
            }
        })
    };
    if let Err(e) = &watchdog {
        log::error!("{what}: no watchdog thread ({e}); the shutdown is not bounded");
    }
    let clean = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| {
        log::error!("{what}: the shutdown work panicked");
        false
    });
    let (lock, finished) = &*done;
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
    finished.notify_all();
    if let Ok(watchdog) = watchdog {
        let _ = watchdog.join();
    }
    clean
}

/// Ends the process after the shutdown work: a normal exit with [`EXIT_AFTER_SHUTDOWN`] when it
/// was clean, otherwise a termination without exit handlers or DLL unload code, with
/// [`EXIT_TERMINATED`]. The log is flushed first.
pub fn end_process(clean: bool) -> ! {
    if clean {
        log::logger().flush();
        std::process::exit(EXIT_AFTER_SHUTDOWN);
    }
    log::error!("plug-in hosting did not shut down cleanly; ending the process without running plug-in unload code");
    log::logger().flush();
    transit_rt::terminate_process(EXIT_TERMINATED)
}

/// The whole emergency path for a caller with nothing to show the user: log `reason`, run
/// `work` under the watchdog ([`run_bounded`], [`SHUTDOWN_BOUND`]) and end the process
/// ([`end_process`]).
pub fn emergency_shutdown(reason: &str, work: impl FnOnce() -> bool) -> ! {
    log::error!("emergency shutdown: {reason}");
    let clean = run_bounded("emergency shutdown", SHUTDOWN_BOUND, work);
    end_process(clean)
}

/// Logs `message` at error level and flushes, from a helper thread, waiting at most
/// [`LOG_WAIT`] for it.
fn log_bounded(message: String) {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("shutdown-log".into()).spawn(move || {
        log::error!("{message}");
        log::logger().flush();
        let _ = tx.send(());
    });
    if spawned.is_ok() {
        let _ = rx.recv_timeout(LOG_WAIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn run_bounded_returns_what_the_work_returned_and_a_panic_is_unclean() {
        assert!(run_bounded("test", SHUTDOWN_BOUND, || true));
        assert!(!run_bounded("test", SHUTDOWN_BOUND, || false));
        let ran = AtomicBool::new(false);
        let clean = run_bounded("test", SHUTDOWN_BOUND, || {
            ran.store(true, Ordering::SeqCst);
            std::panic::panic_any("shutdown work failed")
        });
        assert!(ran.load(Ordering::SeqCst) && !clean);
    }
}
