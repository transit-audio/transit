//! Linux: the host's `Steinberg::Linux::IRunLoop` (issue #147), answered by the plug frame.
//!
//! A Linux VST3 view has no OS message loop to ride on: it asks its `IPlugFrame` for `IRunLoop`
//! (`queryInterface`) and registers timers and handlers for file descriptors (its own X11
//! connection, typically). The registrations go to the plug-in host thread's run loop
//! ([`transit_rt::run_loop`]), whose message queue waits on the descriptors and the next timer and
//! calls the handlers between jobs, on the thread that owns the editor (ADR 0018). The host
//! thread's busy sections cover those calls like any window message (a handler that never returns
//! is a stuck "window message", issue #205).
//!
//! Each registration holds its own reference to the plug-in's handler, released when it is
//! unregistered, when the view is torn down (anything the plug-in left registered), or when the
//! thread exits. Registrations from a thread other than the editor's are refused: they would never
//! be called there.

use super::*;
use std::rc::Rc;
use transit_rt::run_loop::{self as host_loop, Ready, Token};
use vst3::Steinberg::Linux::{FileDescriptor, IEventHandler, IEventHandlerTrait, IRunLoopTrait, ITimerHandler, ITimerHandlerTrait, TimerInterval};

/// The run-loop side of a plug frame.
pub(crate) struct RunLoop {
    /// The thread whose run loop takes the registrations (the editor's).
    owner: std::thread::ThreadId,
    /// The plug-in's handlers (by address) and their registrations.
    regs: Mutex<Vec<(usize, Token)>>,
    /// The view was torn down: nothing more is registered.
    closed: AtomicBool,
}

impl RunLoop {
    pub(super) fn new() -> RunLoop {
        RunLoop { owner: std::thread::current().id(), regs: Mutex::new(Vec::new()), closed: AtomicBool::new(false) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(usize, Token)>> {
        self.regs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why a registration is refused now, if it is.
    fn refused(&self) -> Option<&'static str> {
        if std::thread::current().id() != self.owner {
            Some("from another thread than the editor's")
        } else if self.closed.load(Ordering::Acquire) {
            Some("after the view was removed")
        } else {
            None
        }
    }

    fn add(&self, name: &str, what: &str, handler: usize, token: Option<Token>) -> sb::tresult {
        let Some(token) = token else {
            log::warn!("vst3 editor \"{name}\": IRunLoop {what}: refused (the run loop is full or the descriptor is invalid)");
            return sb::kResultFalse;
        };
        self.lock().push((handler, token));
        log::debug!("vst3 editor \"{name}\": IRunLoop {what}");
        sb::kResultTrue
    }

    /// Unregisters every registration of `handler`.
    fn remove(&self, handler: usize) -> sb::tresult {
        let tokens: Vec<Token> = {
            let mut regs = self.lock();
            let (gone, keep): (Vec<_>, Vec<_>) = regs.drain(..).partition(|&(h, _)| h == handler);
            *regs = keep;
            gone.into_iter().map(|(_, t)| t).collect()
        };
        // Outside the lock: dropping a registration releases the plug-in's handler, whose last
        // `release` may call back in.
        let found = !tokens.is_empty();
        for t in tokens {
            let _ = host_loop::remove(t);
        }
        if found { sb::kResultTrue } else { sb::kInvalidArgument }
    }

    /// The view was torn down (after `removed` and `setFrame(null)`): what the plug-in left
    /// registered is removed, and nothing more is taken. On the editor's thread (elsewhere the
    /// registrations stay until that thread exits).
    pub(super) fn close(&self, name: &str) {
        self.closed.store(true, Ordering::Release);
        let left: Vec<Token> = std::mem::take(&mut *self.lock()).into_iter().map(|(_, t)| t).collect();
        if !left.is_empty() {
            log::debug!("vst3 editor \"{name}\": {} run-loop registrations left at teardown; removed", left.len());
        }
        for t in left {
            let _ = host_loop::remove(t);
        }
    }
}

impl IRunLoopTrait for PlugFrame {
    unsafe fn registerEventHandler(&self, handler: *mut IEventHandler, fd: FileDescriptor) -> sb::tresult {
        if let Some(why) = self.run_loop.refused() {
            log::warn!("vst3 editor \"{}\": IRunLoop registerEventHandler {why}: refused", self.name);
            return sb::kResultFalse;
        }
        // SAFETY: the plug-in passes its live handler (or null, refused), valid for this call;
        // `to_com_ptr` takes a reference of ours, released with the registration.
        let Some(h) = (unsafe { vst3::ComRef::from_raw(handler) }).map(|r| r.to_com_ptr()) else { return sb::kInvalidArgument };
        let token = host_loop::add_fd(
            fd,
            Ready::READ,
            Rc::new(move |fd, _| {
                // SAFETY: our counted reference keeps the handler alive; called on the editor's
                // thread, from its run loop, as the SDK's own run loop calls it.
                unsafe { h.onFDIsSet(fd) };
                true
            }),
        );
        self.run_loop.add(&self.name, &format!("registerEventHandler fd {fd}"), handler.addr(), token)
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut IEventHandler) -> sb::tresult {
        if handler.is_null() {
            return sb::kInvalidArgument;
        }
        self.run_loop.remove(handler.addr())
    }

    unsafe fn registerTimer(&self, handler: *mut ITimerHandler, milliseconds: TimerInterval) -> sb::tresult {
        if let Some(why) = self.run_loop.refused() {
            log::warn!("vst3 editor \"{}\": IRunLoop registerTimer {why}: refused", self.name);
            return sb::kResultFalse;
        }
        // SAFETY: as in `registerEventHandler`.
        let Some(h) = (unsafe { vst3::ComRef::from_raw(handler) }).map(|r| r.to_com_ptr()) else { return sb::kInvalidArgument };
        let token = host_loop::add_timer(
            Duration::from_millis(milliseconds),
            Rc::new(move || {
                // SAFETY: as in `registerEventHandler`.
                unsafe { h.onTimer() };
                true
            }),
        );
        self.run_loop.add(&self.name, &format!("registerTimer {milliseconds} ms"), handler.addr(), token)
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> sb::tresult {
        if handler.is_null() {
            return sb::kInvalidArgument;
        }
        self.run_loop.remove(handler.addr())
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
    use vst3::Steinberg::Linux::IRunLoop;

    struct Counter(AtomicU32);

    impl Class for Counter {
        type Interfaces = (ITimerHandler, IEventHandler);
    }

    impl ITimerHandlerTrait for Counter {
        unsafe fn onTimer(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl IEventHandlerTrait for Counter {
        unsafe fn onFDIsSet(&self, _fd: FileDescriptor) {
            self.0.fetch_add(1000, Ordering::Relaxed);
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

    /// A view's `queryInterface` on the frame finds `IRunLoop`; a timer and a descriptor it
    /// registers run on this (the editor's) thread's message queue; unregistering stops them, and
    /// the teardown removes what is left and refuses more.
    #[test]
    fn the_frame_is_the_views_run_loop() {
        let q = MessageQueue::attach();
        let frame = ComWrapper::new(PlugFrame::new(std::ptr::null_mut(), "run loop"));
        let plug_frame = frame.as_com_ref::<sb::IPlugFrame>().unwrap();
        let run_loop = plug_frame.cast::<IRunLoop>().expect("IRunLoop through queryInterface");
        let counter = ComWrapper::new(Counter(AtomicU32::new(0)));
        let timer = counter.as_com_ref::<ITimerHandler>().unwrap().as_ptr();
        let events = counter.as_com_ref::<IEventHandler>().unwrap().as_ptr();
        let (mut tx, rx) = UnixStream::pair().unwrap();
        // SAFETY: live handlers of ours, on the frame's thread.
        unsafe {
            assert_eq!(run_loop.registerTimer(timer, 5), sb::kResultTrue);
            assert_eq!(run_loop.registerEventHandler(events, rx.as_raw_fd()), sb::kResultTrue);
        }
        assert!(pump(&q, || counter.0.load(Ordering::Relaxed) >= 3), "the timer fired");
        assert!(counter.0.load(Ordering::Relaxed) < 1000, "nothing to read yet");
        tx.write_all(b"x").unwrap();
        assert!(pump(&q, || counter.0.load(Ordering::Relaxed) >= 1000), "the descriptor's handler ran");
        // SAFETY: as above.
        unsafe {
            assert_eq!(run_loop.unregisterTimer(timer), sb::kResultTrue);
            assert_eq!(run_loop.unregisterTimer(timer), sb::kInvalidArgument, "once");
        }
        assert_eq!(host_loop::counts(), (1, 0));
        // Registered from another thread: refused (it would never run there).
        let other = RunLoop::new();
        assert!(other.refused().is_none());
        assert!(std::thread::spawn(move || other.refused().is_some()).join().unwrap());
        frame.run_loop.close("run loop");
        assert_eq!(host_loop::counts(), (0, 0), "the teardown removed the descriptor handler");
        // SAFETY: as above.
        unsafe { assert_eq!(run_loop.registerTimer(timer, 5), sb::kResultFalse, "nothing after the teardown") };
        drop(run_loop);
    }

    /// The fixture's view opens in an X11 window on this thread (`X11EmbedWindowID`), at the size
    /// it reports, hidden until raised, and closes with its window. Needs an X server (CI runs
    /// these tests under `xvfb-run -a`); skipped without one.
    #[test]
    fn an_editor_opens_in_an_x11_window_and_closes() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("an_editor_opens_in_an_x11_window_and_closes: skipped: no DISPLAY (run under `xvfb-run -a`)");
            return;
        }
        let inst = crate::ffi::tests::gain_instance();
        let link = inst.editor().unwrap();
        link.open_with("x11", |_| {}).unwrap();
        {
            let g = link.lock();
            let (win, view) = (g.window.as_ref().unwrap(), g.view.as_ref().unwrap());
            assert!(!win.parent().is_null(), "the content window's id is the view's parent");
            let (w, h) = win.content_size();
            assert_eq!(Some((f64::from(w), f64::from(h))), crate::ffi::view::view_size(view), "the window takes the view's size");
            assert!(g.frame.as_ref().unwrap().as_com_ref::<sb::Linux::IRunLoop>().is_some(), "the view's frame is its run loop");
        }
        assert!(!link.is_open(), "made hidden; shown by the raise of `open`");
        link.close();
        assert!(link.lock().window.is_none());
    }
}
