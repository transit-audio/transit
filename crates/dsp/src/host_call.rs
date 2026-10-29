//! Host calls into plug-in code on this thread's stack (issue #205).
//!
//! A plug-in's main thread runs both the host's calls into it (a creation, a state load, an
//! editor open, a window message for its editor) and the host's own servicing of what plug-ins
//! asked for (VST3 `restartComponent`, CLAP `request_restart` and `request_callback`, parameter
//! rescans) and each editor's `idle`. Servicing deactivates and reactivates instances, so it must
//! never run inside a call into a plug-in: a call that pumps the message loop (a progress dialog,
//! a wait that dispatches messages) could otherwise reach the servicing and call `setActive(false)`
//! on an instance in the middle of a preset load, which a plug-in holding its own lock turns into
//! a deadlock.
//!
//! The plug-in host enters a [`HostCall`] around every call it makes into plug-in code; the
//! formats' servicing and editor `idle` check [`active`] first and, while a call is on the stack,
//! leave the requests for the next idle and count the deferral ([`deferred`]). The scope is per
//! thread and covers every instance: two instances of one plug-in often share its library's
//! locks, so servicing a sibling inside the call is the same hazard.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

static DEFERRED: AtomicU64 = AtomicU64::new(0);

/// A host call into plug-in code is on this thread's stack while this lives. Not `Send`: it
/// belongs to the thread that entered it.
#[must_use = "the call is on the stack only while the scope lives"]
pub struct HostCall {
    _thread: PhantomData<*const ()>,
}

/// Enters a host call on this thread (nested calls count).
pub fn enter() -> HostCall {
    DEPTH.with(|d| d.set(d.get().saturating_add(1)));
    HostCall { _thread: PhantomData }
}

impl Drop for HostCall {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Whether a host call into plug-in code is on this thread's stack: servicing and editor `idle`
/// must wait for the next idle.
#[must_use]
pub fn active() -> bool {
    DEPTH.with(Cell::get) > 0
}

/// Counts one servicing (or editor idle) left for the next idle because a host call was on the
/// stack.
pub fn defer() {
    DEFERRED.fetch_add(1, Ordering::Relaxed);
}

/// How many servicings and editor idles were deferred in this process (the diagnostics report).
#[must_use]
pub fn deferred() -> u64 {
    DEFERRED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_marks_this_thread_only_and_nests() {
        assert!(!active());
        let outer = enter();
        assert!(active());
        {
            let _inner = enter();
            assert!(active());
        }
        assert!(active(), "the outer call is still on the stack");
        assert!(!std::thread::spawn(active).join().unwrap_or(true), "another thread has no call on its stack");
        drop(outer);
        assert!(!active());
    }

    #[test]
    fn deferrals_are_counted() {
        let before = deferred();
        defer();
        assert!(deferred() > before);
    }
}
