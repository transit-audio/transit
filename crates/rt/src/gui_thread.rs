//! The OS message queue of a thread that hosts windows it does not get from the UI toolkit: the
//! plug-in host thread (ADR 0018). Plug-in editors and the hidden windows plug-in frameworks
//! create at load time belong to the thread that created them, and that thread must dispatch
//! their messages or they freeze.
//!
//! | Platform | [`MessageQueue::attach`] | [`MessageQueue::wait`] | [`MessageQueue::dispatch`] |
//! |---|---|---|---|
//! | Windows | `OleInitialize` (a single-threaded apartment, as winit gives the UI thread) and the thread's message queue | `MsgWaitForMultipleObjectsEx` on any input | `PeekMessageW`, `TranslateMessage`, `DispatchMessageW` |
//! | Linux | a wake pipe | `poll(2)` on the wake pipe, the thread's X11 connection and the plug-ins' descriptors ([`crate::run_loop`]), until the next plug-in timer at the latest | the X11 events of this thread's editor windows, then the plug-ins' ready descriptor handlers and due timers (issue #147) |
//! | elsewhere | nothing | `std::thread::park_timeout` until a wake or the deadline | nothing (no window here needs dispatching) |
//!
//! Contract, on every platform:
//! - Everything acts on the **calling** thread: attach, wait and dispatch on the thread that
//!   owns the windows. [`MessageQueue`] is not `Send`, so it cannot leave that thread, and its
//!   drop (which ends the apartment) runs there.
//! - [`QueueWaker::wake`] may be called from any thread. It makes the next or the current
//!   [`MessageQueue::wait`] return early; wakes coalesce, so a burst posts one OS message.
//! - Nothing here panics. A wait never blocks longer than its timeout.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The calling thread's message queue (see the module docs). Not `Send`: it belongs to the
/// thread that attached it.
pub struct MessageQueue {
    shared: Arc<Shared>,
    /// Windows: `OleInitialize` succeeded, so drop must call `OleUninitialize`.
    apartment: bool,
    _not_send: PhantomData<*const ()>,
}

/// Wakes a thread waiting in [`MessageQueue::wait`]; `Send`, `Sync` and cheap to clone.
#[derive(Clone)]
pub struct QueueWaker {
    shared: Arc<Shared>,
}

struct Shared {
    /// A wake is pending that no wait has consumed yet.
    woken: AtomicBool,
    thread: std::thread::Thread,
    /// Windows: the OS id of the owning thread, for `PostThreadMessageW` (0 elsewhere).
    #[cfg_attr(not(windows), allow(dead_code))]
    os_thread: u32,
    /// Linux: the wake pipe (nothing elsewhere).
    #[cfg_attr(not(all(target_os = "linux", not(feature = "force-fallback"))), allow(dead_code))]
    wake: imp::Wake,
}

impl MessageQueue {
    /// Makes the calling thread a window-hosting thread: on Windows, initializes OLE (an STA)
    /// and creates the thread's message queue so a wake can be posted before the first wait.
    #[must_use]
    pub fn attach() -> MessageQueue {
        let (apartment, os_thread, wake) = imp::attach();
        MessageQueue {
            shared: Arc::new(Shared { woken: AtomicBool::new(false), thread: std::thread::current(), os_thread, wake }),
            apartment,
            _not_send: PhantomData,
        }
    }

    /// Whether the thread joined a single-threaded OLE apartment (Windows only; `false`
    /// elsewhere and when OLE refused, for example because the thread was already in a
    /// multithreaded apartment).
    #[must_use]
    pub fn apartment(&self) -> bool {
        self.apartment
    }

    /// A waker for this queue, to hand to the threads that send it work.
    #[must_use]
    pub fn waker(&self) -> QueueWaker {
        QueueWaker { shared: Arc::clone(&self.shared) }
    }

    /// Dispatches up to `max` pending window messages of this thread to their window procedures
    /// and returns how many it took. Window procedures (plug-in code) run inside this call.
    // The count is diagnostic; a pump loop ignores it.
    #[allow(clippy::must_use_candidate)]
    pub fn dispatch(&self, max: usize) -> usize {
        imp::dispatch(max)
    }

    /// Returns when a window message arrives for this thread, a [`QueueWaker::wake`] happens (or
    /// happened since the last wait), or `timeout` passes, whichever is first.
    pub fn wait(&self, timeout: Duration) {
        if self.shared.woken.swap(false, Ordering::AcqRel) {
            imp::consume(&self.shared);
            return;
        }
        imp::wait(&self.shared, timeout);
    }
}

impl Drop for MessageQueue {
    fn drop(&mut self) {
        imp::detach(self.apartment);
    }
}

impl QueueWaker {
    /// Makes the owning thread's current or next [`MessageQueue::wait`] return. Any thread.
    pub fn wake(&self) {
        if !self.shared.woken.swap(true, Ordering::AcqRel) {
            imp::post(&self.shared);
            self.shared.thread.unpark();
        }
    }
}

#[cfg(all(windows, not(feature = "force-fallback")))]
mod imp {
    use super::Shared;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use windows_sys::Win32::System::Ole::{OleInitialize, OleUninitialize};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_NOREMOVE, PM_REMOVE, PeekMessageW, PostThreadMessageW,
        QS_ALLINPUT, TranslateMessage, WM_NULL, WM_QUIT, WM_USER,
    };

    /// Nothing: the thread's message queue carries the wake.
    pub(super) struct Wake;

    pub(super) fn attach() -> (bool, u32, Wake) {
        // SAFETY: `OleInitialize` takes a reserved pointer that must be null and acts on the
        // calling thread only; S_OK and S_FALSE both mean a reference this thread must release
        // with `OleUninitialize` (done in `detach`, on this thread, because `MessageQueue` is not
        // `Send`); a failure (RPC_E_CHANGED_MODE) takes no reference.
        let hr = unsafe { OleInitialize(std::ptr::null()) };
        let mut msg = empty_msg();
        // SAFETY: `msg` is a valid, writable MSG; a null window and a WM_USER-only filter with
        // PM_NOREMOVE only make Windows create this thread's message queue (documented for
        // PostThreadMessage) and leave every message where it is.
        unsafe { PeekMessageW(&raw mut msg, std::ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
        // SAFETY: no arguments, no preconditions.
        let id = unsafe { GetCurrentThreadId() };
        (hr >= 0, id, Wake)
    }

    pub(super) fn detach(apartment: bool) {
        if apartment {
            // SAFETY: balances the successful `OleInitialize` of `attach`, on the same thread
            // (`MessageQueue` is not `Send`, so its drop runs where it was attached).
            unsafe { OleUninitialize() };
        }
    }

    fn empty_msg() -> MSG {
        MSG { hwnd: std::ptr::null_mut(), message: 0, wParam: 0, lParam: 0, time: 0, pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 } }
    }

    pub(super) fn dispatch(max: usize) -> usize {
        let mut done = 0;
        while done < max {
            let mut msg = empty_msg();
            // SAFETY: `msg` is a valid, writable MSG; a null window with no filter takes any
            // message of the calling thread's queue (window and thread messages alike).
            if unsafe { PeekMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                break;
            }
            done += 1;
            if msg.message == WM_QUIT {
                // Plug-in code asked this thread to quit; the host decides when it ends.
                continue;
            }
            // SAFETY: `msg` was just filled by `PeekMessageW` on this thread, which is what
            // `TranslateMessage` and `DispatchMessageW` require; a thread message (null window)
            // is ignored by `DispatchMessageW`.
            unsafe {
                TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
        done
    }

    /// Returns on any input in the thread's queue (a window message or the wake's `WM_NULL`) or
    /// at the timeout; the queue itself holds any wake, so the flag only resets coalescing.
    pub(super) fn wait(shared: &Shared, timeout: Duration) {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: zero handles with a null array is the documented way to wait on the message
        // queue alone; MWMO_INPUTAVAILABLE also returns for input already in the queue.
        unsafe { MsgWaitForMultipleObjectsEx(0, std::ptr::null(), ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        shared.woken.store(false, Ordering::Release);
    }

    /// The wake's posted message stays queued for `dispatch`; nothing else to consume.
    pub(super) fn consume(_shared: &Shared) {}

    pub(super) fn post(shared: &Shared) {
        let thread = shared.os_thread;
        // SAFETY: posts a WM_NULL thread message (no pointers in it) to the owning thread's
        // queue, created in `attach`. A failure (the thread ended, or its queue is full) only
        // means no early wake; the wait still ends at its timeout.
        unsafe { PostThreadMessageW(thread, WM_NULL, 0, 0) };
    }
}

#[cfg(any(not(any(windows, target_os = "linux")), feature = "force-fallback"))]
mod imp {
    use super::Shared;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    /// Nothing: a wake unparks the thread.
    pub(super) struct Wake;

    pub(super) fn attach() -> (bool, u32, Wake) {
        (false, 0, Wake)
    }

    pub(super) fn detach(_apartment: bool) {}

    pub(super) fn dispatch(_max: usize) -> usize {
        0
    }

    /// Parks until a wake sets `woken` or the deadline passes, and returns whether a wake ended
    /// it. `park_timeout` may return spuriously, or on an unpark that is not a wake (any code can
    /// unpark this thread), so only the flag ends the wait early. A wake that lands after the
    /// deadline stays set for the next wait.
    pub(super) fn wait(shared: &Shared, timeout: Duration) -> bool {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if shared.woken.swap(false, Ordering::Acquire) {
                // The wake's unpark may still be pending; take it so the next wait parks.
                consume(shared);
                return true;
            }
            let remaining = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                None => timeout,
            };
            if remaining.is_zero() {
                return false;
            }
            std::thread::park_timeout(remaining);
        }
    }

    /// Takes the unpark token of the wake that ended a wait early, so the next wait waits.
    pub(super) fn consume(_shared: &Shared) {
        std::thread::park_timeout(Duration::ZERO);
    }

    pub(super) fn post(_shared: &Shared) {}
}

/// Linux (issue #147): the plug-in host thread waits in `poll(2)` on a wake pipe, its X11
/// connection and the descriptors plug-ins registered ([`crate::run_loop`]), and dispatches the
/// X11 events of its editor windows and the plug-ins' handlers and timers.
#[cfg(all(target_os = "linux", not(feature = "force-fallback")))]
mod imp {
    use super::Shared;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// The wake pipe: [`post`] writes a byte to the second end, the wait polls the first. `None`
    /// when the pair could not be made (no descriptors left); then a wait lasts at most
    /// [`NO_PIPE_WAIT`].
    pub(super) struct Wake(Option<(UnixStream, UnixStream)>);

    /// The longest wait without a wake pipe: a wake is noticed this late at worst.
    const NO_PIPE_WAIT: Duration = Duration::from_millis(5);

    pub(super) fn attach() -> (bool, u32, Wake) {
        let pair = UnixStream::pair().and_then(|(rx, tx)| {
            rx.set_nonblocking(true)?;
            tx.set_nonblocking(true)?;
            Ok((rx, tx))
        });
        let wake = match pair {
            Ok(p) => Wake(Some(p)),
            Err(e) => {
                log::warn!("plug-in host thread: no wake pipe ({e}); waits are cut to {} ms", NO_PIPE_WAIT.as_millis());
                Wake(None)
            }
        };
        (false, 0, wake)
    }

    pub(super) fn detach(_apartment: bool) {}

    /// The X11 events of this thread's editor windows, then the plug-ins' handlers and timers.
    pub(super) fn dispatch(max: usize) -> usize {
        let done = crate::editor_window::pump(max);
        done + crate::run_loop::run(max.saturating_sub(done))
    }

    pub(super) fn wait(shared: &Shared, timeout: Duration) {
        let mut extra = Vec::with_capacity(2);
        let timeout = match &shared.wake.0 {
            Some((rx, _)) => {
                extra.push(rx.as_raw_fd());
                timeout
            }
            None => timeout.min(NO_PIPE_WAIT),
        };
        if let Some(fd) = crate::editor_window::connection_fd() {
            extra.push(fd);
        }
        crate::run_loop::wait(&extra, timeout);
        // The flag is cleared before the pipe is drained: a wake landing in between leaves the
        // flag set, so the next wait returns at once and nothing is lost.
        shared.woken.store(false, Ordering::Release);
        consume(shared);
    }

    /// Drains the wake pipe.
    pub(super) fn consume(shared: &Shared) {
        if let Some((rx, _)) = &shared.wake.0 {
            let mut buf = [0u8; 64];
            while matches!((&*rx).read(&mut buf), Ok(n) if n > 0) {}
        }
    }

    pub(super) fn post(shared: &Shared) {
        if let Some((_, tx)) = &shared.wake.0 {
            // A full pipe (would block) already holds a wake; any other failure only means no
            // early wake, and the wait still ends at its timeout.
            let _ = (&*tx).write(&[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_wake_before_the_wait_returns_at_once() {
        let q = MessageQueue::attach();
        q.waker().wake();
        let t = Instant::now();
        q.wait(Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2));
        // Consumed (and the wake's message dispatched): the next wait runs to its timeout.
        q.dispatch(64);
        let t = Instant::now();
        q.wait(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(30));
        q.dispatch(64);
    }

    #[test]
    fn a_wake_from_another_thread_ends_a_wait() {
        let q = MessageQueue::attach();
        let w = q.waker();
        let t = Instant::now();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            w.wake();
        });
        q.wait(Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2));
        h.join().unwrap();
    }

    #[test]
    fn a_spurious_unpark_does_not_end_the_wait() {
        let q = MessageQueue::attach();
        // An unpark that is not a wake (as `park_timeout` may also return on its own).
        std::thread::current().unpark();
        let t = Instant::now();
        q.wait(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(30));
        q.dispatch(64);
    }

    #[test]
    fn a_wake_during_the_wait_ends_it_once() {
        let q = MessageQueue::attach();
        let w = q.waker();
        let h = std::thread::spawn(move || {
            // timing: sleep-ok lets the main thread enter its wait first; nothing measures this sleep
            std::thread::sleep(Duration::from_millis(20));
            w.wake();
        });
        let t = Instant::now();
        q.wait(Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2));
        h.join().unwrap();
        // The wake was consumed: the next wait, with no wake, runs to its timeout.
        q.dispatch(64);
        let t = Instant::now();
        q.wait(Duration::from_millis(50));
        assert!(t.elapsed() >= Duration::from_millis(30));
        q.dispatch(64);
    }

    #[test]
    fn an_empty_queue_dispatches_nothing() {
        let q = MessageQueue::attach();
        while q.dispatch(64) > 0 {}
        assert_eq!(q.dispatch(64), 0);
    }

    #[cfg(all(windows, not(feature = "force-fallback")))]
    #[test]
    fn a_posted_thread_message_is_taken_by_dispatch_and_ole_is_entered() {
        let q = MessageQueue::attach();
        assert!(q.apartment(), "a fresh test thread can enter an STA");
        q.waker().wake();
        q.wait(Duration::from_secs(1));
        assert!(q.dispatch(64) >= 1, "the wake's WM_NULL is in the queue");
    }
}
