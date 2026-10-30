//! Linux: the host's `clap.timer-support` and `clap.posix-fd-support` (issue #147).
//!
//! CLAP plug-ins on Linux drive their GUI (and sometimes more) through host timers and handlers
//! for file descriptors (their own X11 connection, typically). The registrations go to the plug-in
//! host thread's run loop ([`transit_rt::run_loop`]), whose message queue waits on the
//! descriptors and the next timer and calls the plug-in's `on_timer` and `on_fd` between jobs, on
//! the instance's main thread (ADR 0018). Those calls run under the host thread's busy sections
//! like a window message, so a plug-in that never returns from one is a stuck call (issue #205).
//!
//! A registration holds the instance's host state weakly: once the instance is gone it removes
//! itself instead of calling in. The instance removes what the plug-in left registered after
//! `destroy` ([`HostState::close_run_loop`]). Registrations from another thread than the
//! instance's are refused (they would never be called there).

use super::*;
use clap_sys::ext::posix_fd_support::{
    CLAP_EXT_POSIX_FD_SUPPORT, CLAP_POSIX_FD_ERROR, CLAP_POSIX_FD_READ, CLAP_POSIX_FD_WRITE, clap_host_posix_fd_support,
    clap_plugin_posix_fd_support, clap_posix_fd_flags,
};
use clap_sys::ext::timer_support::{CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support};
use std::rc::Rc;
use std::sync::Weak;
use std::time::Duration;
use transit_rt::run_loop::{self as host_loop, Ready, Token};

/// An instance's registrations with the run loop.
#[derive(Default)]
pub(crate) struct Registrations {
    next_timer: clap_id,
    timers: Vec<(clap_id, Token)>,
    fds: Vec<(i32, Token)>,
    /// The instance was destroyed: nothing more is registered.
    closed: bool,
}

impl HostState {
    fn regs(&self) -> std::sync::MutexGuard<'_, Registrations> {
        self.run_loop.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why a registration is refused now, if it is.
    fn refused(&self) -> Option<&'static str> {
        if self.regs().closed {
            return Some("after the plug-in was destroyed");
        }
        match self.link.get().and_then(Weak::upgrade) {
            Some(link) if link.main != std::thread::current().id() => Some("from another thread than the plug-in's main thread"),
            _ => None,
        }
    }

    /// After the plug-in was destroyed: what it left registered goes, and nothing more is taken.
    /// On the instance's thread (elsewhere the registrations remove themselves at their next call).
    pub(crate) fn close_run_loop(&self) {
        let (timers, fds) = {
            let mut r = self.regs();
            r.closed = true;
            (std::mem::take(&mut r.timers), std::mem::take(&mut r.fds))
        };
        for t in timers.into_iter().map(|(_, t)| t).chain(fds.into_iter().map(|(_, t)| t)) {
            let _ = host_loop::remove(t);
        }
    }
}

impl GuiLink {
    /// `clap_plugin_timer_support.on_timer`; false once the plug-in is gone or has no such call.
    fn on_timer(&self, id: clap_id) -> bool {
        let g = self.lock();
        if !g.alive {
            return false;
        }
        // SAFETY: `plugin` is valid while `alive` (under the lock); the id is a static string.
        let ext = unsafe { (*g.plugin).get_extension.map(|f| f(g.plugin, CLAP_EXT_TIMER_SUPPORT.as_ptr())) };
        // SAFETY: null or the plug-in's static extension vtable.
        let Some(on_timer) = ext.and_then(|e| unsafe { e.cast::<clap_plugin_timer_support>().as_ref() }).and_then(|t| t.on_timer) else {
            return false;
        };
        // SAFETY: live plug-in, on its main thread (the run loop of the thread that registered).
        unsafe { on_timer(g.plugin, id) };
        true
    }

    /// `clap_plugin_posix_fd_support.on_fd`; false once the plug-in is gone or has no such call.
    fn on_fd(&self, fd: i32, flags: clap_posix_fd_flags) -> bool {
        let g = self.lock();
        if !g.alive {
            return false;
        }
        // SAFETY: as in `on_timer`.
        let ext = unsafe { (*g.plugin).get_extension.map(|f| f(g.plugin, CLAP_EXT_POSIX_FD_SUPPORT.as_ptr())) };
        // SAFETY: null or the plug-in's static extension vtable.
        let Some(on_fd) = ext.and_then(|e| unsafe { e.cast::<clap_plugin_posix_fd_support>().as_ref() }).and_then(|t| t.on_fd) else {
            return false;
        };
        // SAFETY: as in `on_timer`.
        unsafe { on_fd(g.plugin, fd, flags) };
        true
    }
}

/// The instance's editor link, once it exists and while the instance does.
fn link_of(state: &Weak<HostState>) -> Option<Arc<GuiLink>> {
    state.upgrade()?.link.get()?.upgrade()
}

fn interest(flags: clap_posix_fd_flags) -> Ready {
    Ready { read: flags & CLAP_POSIX_FD_READ != 0, write: flags & CLAP_POSIX_FD_WRITE != 0, error: flags & CLAP_POSIX_FD_ERROR != 0 }
}

fn flags_of(ready: Ready) -> clap_posix_fd_flags {
    (if ready.read { CLAP_POSIX_FD_READ } else { 0 })
        | (if ready.write { CLAP_POSIX_FD_WRITE } else { 0 })
        | (if ready.error { CLAP_POSIX_FD_ERROR } else { 0 })
}

unsafe extern "C" fn host_register_timer(host: *const clap_host, period_ms: u32, timer_id: *mut clap_id) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(s) = (unsafe { state(host) }) else { return false };
    if timer_id.is_null() {
        return false;
    }
    if let Some(why) = s.refused() {
        log::warn!("clap: register_timer {why}: refused");
        return false;
    }
    let mut r = s.regs();
    r.next_timer = r.next_timer.wrapping_add(1);
    let id = r.next_timer;
    let me = s.me.clone();
    let token = host_loop::add_timer(Duration::from_millis(u64::from(period_ms)), Rc::new(move || link_of(&me).is_some_and(|l| l.on_timer(id))));
    let Some(token) = token else { return false };
    r.timers.push((id, token));
    // SAFETY: a writable out-pointer from the plugin (non-null, checked).
    unsafe { *timer_id = id };
    log::debug!("clap: register_timer {period_ms} ms -> {id}");
    true
}

unsafe extern "C" fn host_unregister_timer(host: *const clap_host, timer_id: clap_id) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(s) = (unsafe { state(host) }) else { return false };
    let token = {
        let mut r = s.regs();
        let Some(i) = r.timers.iter().position(|&(id, _)| id == timer_id) else { return false };
        r.timers.remove(i).1
    };
    // Outside the lock: the run loop drops the registration, which may run plug-in code.
    host_loop::remove(token)
}

unsafe extern "C" fn host_register_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(s) = (unsafe { state(host) }) else { return false };
    if let Some(why) = s.refused() {
        log::warn!("clap: register_fd {why}: refused");
        return false;
    }
    let mut r = s.regs();
    if r.fds.iter().any(|&(f, _)| f == fd) {
        return false;
    }
    let me = s.me.clone();
    let token = host_loop::add_fd(fd, interest(flags), Rc::new(move |fd, ready| link_of(&me).is_some_and(|l| l.on_fd(fd, flags_of(ready)))));
    let Some(token) = token else { return false };
    r.fds.push((fd, token));
    log::debug!("clap: register_fd {fd} flags {flags}");
    true
}

unsafe extern "C" fn host_modify_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(s) = (unsafe { state(host) }) else { return false };
    let r = s.regs();
    r.fds.iter().find(|&&(f, _)| f == fd).is_some_and(|&(_, t)| host_loop::modify_fd(t, interest(flags)))
}

unsafe extern "C" fn host_unregister_fd(host: *const clap_host, fd: i32) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    let Some(s) = (unsafe { state(host) }) else { return false };
    let token = {
        let mut r = s.regs();
        let Some(i) = r.fds.iter().position(|&(f, _)| f == fd) else { return false };
        r.fds.remove(i).1
    };
    host_loop::remove(token)
}

pub(super) static HOST_TIMER_SUPPORT: clap_host_timer_support =
    clap_host_timer_support { register_timer: Some(host_register_timer), unregister_timer: Some(host_unregister_timer) };
pub(super) static HOST_POSIX_FD_SUPPORT: clap_host_posix_fd_support =
    clap_host_posix_fd_support { register_fd: Some(host_register_fd), modify_fd: Some(host_modify_fd), unregister_fd: Some(host_unregister_fd) };

/// The host's timer and descriptor extensions for `get_extension`.
pub(super) fn extension(id: &CStr) -> Option<*const c_void> {
    if id == CLAP_EXT_TIMER_SUPPORT {
        Some((&raw const HOST_TIMER_SUPPORT).cast())
    } else if id == CLAP_EXT_POSIX_FD_SUPPORT {
        Some((&raw const HOST_POSIX_FD_SUPPORT).cast())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;
    use transit_rt::gui_thread::MessageQueue;

    /// What the fake plug-in's calls saw: timer calls, and the last descriptor call's flags.
    static TIMER_CALLS: AtomicU32 = AtomicU32::new(0);
    static FD_FLAGS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn on_timer(_: *const clap_plugin, _: clap_id) {
        TIMER_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn on_fd(_: *const clap_plugin, _: i32, flags: clap_posix_fd_flags) {
        FD_FLAGS.store(flags | 0x100, Ordering::Relaxed);
    }
    static TIMER: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };
    static FD: clap_plugin_posix_fd_support = clap_plugin_posix_fd_support { on_fd: Some(on_fd) };
    unsafe extern "C" fn get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
        // SAFETY: a NUL-terminated id from the host.
        let id = unsafe { CStr::from_ptr(id) };
        if id == CLAP_EXT_TIMER_SUPPORT {
            (&raw const TIMER).cast()
        } else if id == CLAP_EXT_POSIX_FD_SUPPORT {
            (&raw const FD).cast()
        } else {
            std::ptr::null()
        }
    }

    fn pump(q: &MessageQueue, until: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            q.dispatch(64);
            if until() {
                return true;
            }
            q.wait(Duration::from_millis(15));
        }
        false
    }

    /// A plug-in's timer and descriptor registrations call it back on this (its main) thread's
    /// message queue, stop when unregistered, and stop by themselves once the instance is gone.
    #[test]
    fn timers_and_descriptors_call_the_plugin_on_its_thread() {
        let q = MessageQueue::attach();
        // SAFETY: an all-null plug-in vtable but `get_extension` (the only call made), kept alive
        // for the test.
        let mut vt: clap_plugin = unsafe { std::mem::zeroed() };
        vt.get_extension = Some(get_extension);
        let state = Arc::new_cyclic(|me| HostState { me: me.clone(), ..HostState::default() });
        let host = clap_host {
            clap_version: CLAP_VERSION,
            host_data: Arc::as_ptr(&state).cast_mut().cast(),
            name: c"test".as_ptr(),
            vendor: c"test".as_ptr(),
            url: c"test".as_ptr(),
            version: c"0".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        };
        let link = Arc::new(GuiLink {
            main: std::thread::current().id(),
            inner: Mutex::new(GuiInner {
                plugin: &raw const vt,
                gui: std::ptr::null(),
                state: std::ptr::null(),
                alive: true,
                created: false,
                visible: false,
                window: None,
            }),
            host: Arc::clone(&state),
            header: transit_rt::editor_window::HeaderCell::new(),
        });
        let _ = state.link.set(Arc::downgrade(&link));
        // SAFETY: valid ids; our host struct.
        let (timers, fds) = unsafe {
            (
                host_get_extension(&host, CLAP_EXT_TIMER_SUPPORT.as_ptr()).cast::<clap_host_timer_support>().as_ref().unwrap(),
                host_get_extension(&host, CLAP_EXT_POSIX_FD_SUPPORT.as_ptr()).cast::<clap_host_posix_fd_support>().as_ref().unwrap(),
            )
        };
        let (mut tx, rx) = UnixStream::pair().unwrap();
        let mut id = 0;
        // SAFETY: our host struct and a writable id, on this (the instance's) thread.
        unsafe {
            assert!((timers.register_timer.unwrap())(&host, 5, &mut id));
            assert!((fds.register_fd.unwrap())(&host, rx.as_raw_fd(), CLAP_POSIX_FD_READ));
            assert!(!(fds.register_fd.unwrap())(&host, rx.as_raw_fd(), CLAP_POSIX_FD_READ), "once per descriptor");
        }
        assert!(pump(&q, || TIMER_CALLS.load(Ordering::Relaxed) >= 3), "the timer fired");
        tx.write_all(b"x").unwrap();
        assert!(pump(&q, || FD_FLAGS.load(Ordering::Relaxed) == (CLAP_POSIX_FD_READ | 0x100)), "on_fd with READ");
        // SAFETY: as above.
        unsafe {
            assert!((timers.unregister_timer.unwrap())(&host, id));
            assert!(!(timers.unregister_timer.unwrap())(&host, id), "once");
            assert!((fds.modify_fd.unwrap())(&host, rx.as_raw_fd(), CLAP_POSIX_FD_READ | CLAP_POSIX_FD_WRITE));
        }
        assert_eq!(host_loop::counts(), (1, 0));
        // The instance is gone (`alive` cleared, as `GuiLink::kill` does): the registration
        // removes itself at its next call instead of calling in.
        link.lock().alive = false;
        assert!(pump(&q, || host_loop::counts() == (0, 0)), "the descriptor registration removed itself");
        state.close_run_loop();
        let mut id = 0;
        // SAFETY: as above.
        unsafe { assert!(!(timers.register_timer.unwrap())(&host, 5, &mut id), "nothing after the instance is destroyed") };
    }
}
