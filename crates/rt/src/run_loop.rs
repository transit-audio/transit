//! The plug-in run loop of a Linux window-hosting thread (issue #147, ADR 0018): file-descriptor
//! handlers and timers that plug-ins register with the host and that the plug-in host thread runs
//! between its jobs.
//!
//! On Linux a plug-in's editor has no OS message loop to ride on: VST3 views ask the host for
//! `Steinberg::Linux::IRunLoop` (timers, and handlers for file descriptors such as their own X11
//! connection), and CLAP plug-ins use the host's `clap.timer-support` and `clap.posix-fd-support`.
//! The format hosts register those here; [`crate::gui_thread::MessageQueue`] waits on the
//! descriptors and the next timer in its `wait`, and runs the handlers in its `dispatch`, on the
//! thread that registered them.
//!
//! Contract:
//! - Everything acts on the **calling** thread's registry (a thread-local): a handler runs only on
//!   the thread that registered it, inside that thread's `dispatch`, never inside `wait`.
//! - A handler is called with nothing here borrowed, so it may register and remove entries
//!   (itself included). It returns whether it stays registered. A removed handler is dropped
//!   after the registry is released too, so a handler whose drop runs plug-in code (a VST3
//!   handler's last `release`) may call back in. During the thread's exit the registry is gone and
//!   every call does nothing.
//! - A descriptor the kernel reports invalid (`POLLNVAL`: closed without being removed) is dropped
//!   from the registry with a warning, so it cannot spin the loop.
//! - A timer fires at most once per `dispatch` and is rescheduled one period after it last fired
//!   (missed periods are skipped, never fired in a burst).
//! - Nothing here panics. The one system call (`poll(2)`) lives in the private `sys` module, the
//!   only `unsafe` of this module.

// With `force-fallback` the message queue does not wait on or dispatch the run loop.
#![cfg_attr(feature = "force-fallback", allow(dead_code))]

use std::cell::RefCell;
use std::os::fd::RawFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

mod sys;

/// Readiness of a descriptor: what a handler is registered for, and what [`run`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // the three poll(2) conditions, not a state
pub struct Ready {
    /// Readable (or the peer hung up).
    pub read: bool,
    /// Writable.
    pub write: bool,
    /// An error condition (always reported, whatever the interest).
    pub error: bool,
}

impl Ready {
    /// Readable only: what a VST3 `IEventHandler` is registered for.
    pub const READ: Ready = Ready { read: true, write: false, error: false };
}

/// A descriptor handler: called with the descriptor and what is ready; returns whether it stays
/// registered.
pub type FdHandler = Rc<dyn Fn(RawFd, Ready) -> bool>;
/// A timer handler: returns whether the timer stays registered.
pub type TimerHandler = Rc<dyn Fn() -> bool>;

/// A registration of the calling thread's run loop, for [`remove`] and [`modify_fd`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token(u64);

/// The shortest timer period: a plug-in asking for 0 ms gets this, so it cannot spin the thread.
pub const MIN_PERIOD: Duration = Duration::from_millis(1);
/// The longest timer period kept as asked; longer ones are clamped (a bogus huge value).
pub const MAX_PERIOD: Duration = Duration::from_secs(3600);
/// Most registrations of each kind per thread (a plug-in that registers without end is refused).
pub const MAX_ENTRIES: usize = 4096;

struct FdEntry {
    token: Token,
    fd: RawFd,
    interest: Ready,
    handler: FdHandler,
}

struct TimerEntry {
    token: Token,
    period: Duration,
    due: Instant,
    handler: TimerHandler,
}

#[derive(Default)]
struct Registry {
    next: u64,
    fds: Vec<FdEntry>,
    timers: Vec<TimerEntry>,
}

impl Registry {
    fn token(&mut self) -> Token {
        self.next = self.next.wrapping_add(1);
        Token(self.next)
    }

    /// Time until the earliest timer is due (zero when one is overdue).
    fn next_due(&self, now: Instant) -> Option<Duration> {
        self.timers.iter().map(|t| t.due.saturating_duration_since(now)).min()
    }
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Runs `f` on the calling thread's registry; the default when it is in use further up this
/// thread's stack (a handler's drop calling back in) or gone (the thread is exiting).
fn with<R: Default>(f: impl FnOnce(&mut Registry) -> R) -> R {
    REGISTRY.try_with(|r| r.try_borrow_mut().map(|mut r| f(&mut r)).ok()).ok().flatten().unwrap_or_default()
}

/// Registers `handler` for `fd` on the calling thread, called from its `dispatch` when the
/// descriptor is ready for what `interest` names (errors are always reported). `None` for a
/// negative descriptor or when the registry is full.
pub fn add_fd(fd: RawFd, interest: Ready, handler: FdHandler) -> Option<Token> {
    if fd < 0 {
        return None;
    }
    with(|r| {
        if r.fds.len() >= MAX_ENTRIES {
            return None;
        }
        let token = r.token();
        r.fds.push(FdEntry { token, fd, interest, handler });
        Some(token)
    })
}

/// Changes what the descriptor registration `token` waits for. False when it is not registered
/// on the calling thread.
#[must_use]
pub fn modify_fd(token: Token, interest: Ready) -> bool {
    with(|r| r.fds.iter_mut().find(|e| e.token == token).map(|e| e.interest = interest).is_some())
}

/// Registers a timer on the calling thread that calls `handler` every `period` (clamped to
/// [`MIN_PERIOD`]..=[`MAX_PERIOD`]), first one period from now. `None` when the registry is full.
pub fn add_timer(period: Duration, handler: TimerHandler) -> Option<Token> {
    let period = period.clamp(MIN_PERIOD, MAX_PERIOD);
    with(|r| {
        if r.timers.len() >= MAX_ENTRIES {
            return None;
        }
        let token = r.token();
        r.timers.push(TimerEntry { token, period, due: Instant::now() + period, handler });
        Some(token)
    })
}

/// Removes a registration of the calling thread. False when there is none (already removed, or
/// made on another thread).
#[must_use = "false: nothing was registered under the token on this thread"]
pub fn remove(token: Token) -> bool {
    // Taken out under the borrow, dropped after it (a handler's drop may call back in).
    let (fds, timers): (Vec<FdEntry>, Vec<TimerEntry>) = with(|r| {
        let fds = r.fds.iter().position(|e| e.token == token).map(|i| r.fds.remove(i)).into_iter().collect();
        let timers = r.timers.iter().position(|e| e.token == token).map(|i| r.timers.remove(i)).into_iter().collect();
        (fds, timers)
    });
    let removed = !fds.is_empty() || !timers.is_empty();
    drop((fds, timers));
    removed
}

/// The calling thread's registrations: (descriptors, timers).
#[must_use]
pub fn counts() -> (usize, usize) {
    with(|r| (r.fds.len(), r.timers.len()))
}

fn events(interest: Ready) -> i16 {
    (if interest.read { sys::POLLIN } else { 0 }) | (if interest.write { sys::POLLOUT } else { 0 })
}

/// Milliseconds for `poll(2)`, rounded up so a wait never returns before what is due.
fn millis(d: Duration) -> i32 {
    i32::try_from(d.as_micros().div_ceil(1000)).unwrap_or(i32::MAX)
}

/// Waits until one of `extra` (read interest) or a registered descriptor is ready, the next timer
/// of the calling thread is due, or `timeout` passes. Runs no handler. An interrupted wait
/// (`EINTR`) goes on for the time left.
pub(crate) fn wait(extra: &[RawFd], timeout: Duration) {
    let start = Instant::now();
    let mut fds: Vec<sys::pollfd> = extra.iter().filter(|&&fd| fd >= 0).map(|&fd| sys::pollfd { fd, events: sys::POLLIN, revents: 0 }).collect();
    let next = with(|r| {
        fds.extend(r.fds.iter().map(|e| sys::pollfd { fd: e.fd, events: events(e.interest), revents: 0 }));
        r.next_due(start)
    });
    let limit = next.map_or(timeout, |n| n.min(timeout));
    loop {
        let left = limit.saturating_sub(start.elapsed());
        match sys::poll(&mut fds, millis(left)) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted && !left.is_zero() => {}
            Err(e) => {
                log::debug!("run loop: poll failed ({e}); waiting out the timeout");
                std::thread::sleep(left.min(Duration::from_millis(10)));
                return;
            }
            Ok(_) => return,
        }
    }
}

/// Runs the calling thread's handlers whose descriptors are ready now, then its timers that are
/// due, at most `max` calls in all. Returns how many it called.
pub(crate) fn run(max: usize) -> usize {
    let mut done = 0;
    // Descriptors: one zero-timeout poll over the registry, then the handlers of the ready ones.
    let mut polled: Vec<(Token, sys::pollfd)> =
        with(|r| r.fds.iter().map(|e| (e.token, sys::pollfd { fd: e.fd, events: events(e.interest), revents: 0 })).collect());
    if !polled.is_empty() {
        let mut fds: Vec<sys::pollfd> = polled.iter().map(|(_, p)| *p).collect();
        if sys::poll(&mut fds, 0).is_ok() {
            for ((_, p), f) in polled.iter_mut().zip(&fds) {
                p.revents = f.revents;
            }
        } else {
            polled.clear();
        }
    }
    for (token, p) in polled {
        if done >= max {
            return done;
        }
        if p.revents == 0 {
            continue;
        }
        if p.revents & sys::POLLNVAL != 0 {
            log::warn!("run loop: descriptor {} was closed while registered; its handler is dropped", p.fd);
            let _ = remove(token);
            continue;
        }
        let ready = Ready {
            read: p.revents & (sys::POLLIN | sys::POLLHUP) != 0,
            write: p.revents & sys::POLLOUT != 0,
            error: p.revents & (sys::POLLERR | sys::POLLHUP) != 0,
        };
        // Still registered (an earlier handler of this round may have removed it)?
        let Some(handler) = with(|r| r.fds.iter().find(|e| e.token == token).map(|e| Rc::clone(&e.handler))) else { continue };
        done += 1;
        if !handler(p.fd, ready) {
            let _ = remove(token);
        }
    }
    // Timers: the due ones, each rescheduled before it runs.
    let now = Instant::now();
    let due: Vec<(Token, TimerHandler)> = with(|r| {
        r.timers
            .iter_mut()
            .filter(|t| t.due <= now)
            .map(|t| {
                let next = t.due + t.period;
                t.due = if next > now { next } else { now + t.period };
                (t.token, Rc::clone(&t.handler))
            })
            .collect()
    });
    for (token, handler) in due {
        if done >= max {
            break;
        }
        if !with(|r| r.timers.iter().any(|t| t.token == token)) {
            continue;
        }
        done += 1;
        if !handler() {
            let _ = remove(token);
        }
    }
    done
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::cell::Cell;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_timer_fires_when_due_and_reschedules_without_a_burst() {
        let count = Rc::new(Cell::new(0));
        let c = Rc::clone(&count);
        let t = add_timer(
            Duration::from_millis(20),
            Rc::new(move || {
                c.set(c.get() + 1);
                true
            }),
        )
        .unwrap();
        assert_eq!(run(16), 0, "not due yet");
        let start = Instant::now();
        wait(&[], Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(2), "the wait ends at the timer");
        // timing: sleep-ok makes the timer overdue by several periods; the assertion is on the count
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(run(16), 1, "one call however late");
        assert_eq!(run(16), 0, "rescheduled a period ahead");
        assert!(remove(t));
        assert!(!remove(t));
        assert_eq!(counts(), (0, 0));
    }

    #[test]
    fn a_ready_descriptor_runs_its_handler_on_this_thread_and_a_handler_can_remove_itself() {
        let (mut a, b) = UnixStream::pair().unwrap();
        let seen = Rc::new(Cell::new(0));
        let s = Rc::clone(&seen);
        let fd = b.as_raw_fd();
        add_fd(
            fd,
            Ready::READ,
            Rc::new(move |got, ready| {
                assert_eq!(got, fd);
                assert!(ready.read);
                s.set(s.get() + 1);
                false
            }),
        )
        .unwrap();
        assert_eq!(run(16), 0, "nothing to read");
        a.write_all(b"x").unwrap();
        let start = Instant::now();
        wait(&[], Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(2), "the wait ends at the descriptor");
        assert_eq!(run(16), 1);
        assert_eq!(seen.get(), 1);
        assert_eq!(counts(), (0, 0), "the handler asked to go");
        // Another thread's registry is its own.
        std::thread::spawn(|| assert_eq!(counts(), (0, 0))).join().unwrap();
    }

    #[test]
    fn a_closed_descriptor_is_dropped_not_spun_on() {
        // A number no descriptor of this process has (closing a real one here would race with the
        // other tests' threads, which may get its number again at once).
        let fd = 999_983;
        add_fd(fd, Ready::READ, Rc::new(|_, _| true)).unwrap();
        assert_eq!(run(16), 0);
        assert_eq!(counts(), (0, 0));
        assert!(add_fd(-1, Ready::READ, Rc::new(|_, _| true)).is_none());
    }

    /// A handler whose drop calls back in (a VST3 handler's last `release` unregistering its other
    /// registrations) neither panics nor deadlocks.
    #[test]
    fn a_handler_dropped_on_removal_may_call_back_in() {
        struct CallsBack(Cell<Option<Token>>);
        impl Drop for CallsBack {
            fn drop(&mut self) {
                if let Some(other) = self.0.take() {
                    assert!(remove(other), "the other registration goes from inside the drop");
                }
            }
        }
        let other = add_timer(Duration::from_secs(60), Rc::new(|| true)).unwrap();
        let guard = CallsBack(Cell::new(Some(other)));
        let first = add_timer(
            Duration::from_secs(60),
            Rc::new(move || {
                let _ = &guard;
                true
            }),
        )
        .unwrap();
        assert!(remove(first));
        assert_eq!(counts(), (0, 0));
    }

    #[test]
    fn an_extra_descriptor_ends_the_wait_and_the_timeout_holds_without_one() {
        let (mut a, b) = UnixStream::pair().unwrap();
        let start = Instant::now();
        wait(&[b.as_raw_fd()], Duration::from_millis(50));
        assert!(start.elapsed() >= Duration::from_millis(40), "nothing ready: the timeout");
        a.write_all(b"x").unwrap();
        let start = Instant::now();
        wait(&[b.as_raw_fd()], Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
