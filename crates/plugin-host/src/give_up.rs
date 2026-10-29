//! Giving up on a host thread stuck inside a plug-in (issue #205, ADR 0018 amendment).
//!
//! A wedged host thread cannot be stopped, joined or reused: it is inside a plug-in's code. What
//! [`Host::give_up`] does is quarantine it, the way a hung render worker is quarantined (K89):
//!
//! - the thread is marked abandoned: no job is sent to it again (its instances' requests fail at
//!   once, their destruction is leaked), so its plug-ins are never called, freed or reused by
//!   transit again, even if the call returns;
//! - its windows (the plug-in's editor) are asked to hide from the calling thread with
//!   [`transit_rt::editor_window::hide_thread_windows`], which never waits for the hung thread:
//!   the hide takes effect when that thread next retrieves messages, and while it stays hung
//!   Windows keeps its "not responding" ghost in the window's place;
//! - new plug-ins already load on the fresh thread that took over when the thread wedged.
//!
//! If the call returns after all, the thread [`park`]s: it hides its own windows, keeps every
//! plug-in object it held (leaked), never runs a job again and only retrieves window messages,
//! so its hidden windows never ghost.

use crate::{Ctx, Host, JobCell, Worker, lock};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use transit_rt::editor_window::{current_thread_id, hide_thread_windows};
use transit_rt::gui_thread::MessageQueue;

/// What [`Host::give_up`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GaveUp {
    /// The plug-in each abandoned thread was stuck in, one per thread.
    pub plugins: Vec<Arc<str>>,
    /// The windows of those threads asked to hide.
    pub windows: usize,
}

impl Host {
    /// Gives up on every host thread that is stuck now (see the module docs): quarantines it and
    /// its plug-ins and asks its windows to hide. Never waits for a plug-in. Threads that respond
    /// are left alone; in inline mode there is nothing to give up on.
    pub fn give_up(&self) -> GaveUp {
        let mut out = GaveUp::default();
        let workers: Vec<Arc<Worker>> = lock(&self.inner.workers).clone();
        for w in workers {
            if !w.running.load(Ordering::Acquire) || !w.is_wedged() || w.abandoned.swap(true, Ordering::AcqRel) {
                continue;
            }
            let stuck = w.stuck_for().unwrap_or_default();
            let busy = w.busy_label();
            let windows = hide_thread_windows(w.os_thread.load(Ordering::Acquire));
            log::error!(
                "plug-in host: gave up on {} ({}, stuck in {} for {} s): its plug-ins are quarantined (never called, freed or reused again); {windows} of its windows asked to hide",
                busy.plugin,
                w.name,
                busy.call,
                stuck.as_secs()
            );
            out.plugins.push(busy.plugin);
            out.windows += windows;
            // New creations never go to an abandoned thread.
            let current = lock(&self.inner.current).clone();
            if current.is_some_and(|c| Arc::ptr_eq(&c, &w)) {
                self.take_over(&w);
            }
        }
        out
    }

    /// How many host threads the user gave up on in this process.
    #[must_use]
    pub fn given_up(&self) -> usize {
        lock(&self.inner.workers).iter().filter(|w| w.abandoned.load(Ordering::Acquire)).count()
    }
}

/// The end of an abandoned thread whose call returned (see the module docs). Never returns: the
/// thread retrieves window messages until the process ends.
pub(crate) fn park(worker: &Worker, rx: mpsc::Receiver<Arc<JobCell>>, queue: &MessageQueue, ctx: Ctx) -> ! {
    worker.running.store(false, Ordering::Release);
    log::warn!(
        "plug-in host: {} returned after it was given up on; its plug-ins stay quarantined and its windows hidden until transit exits",
        worker.name
    );
    // Quarantine: the instances' handles and every queued job are kept, never dropped here.
    std::mem::forget(ctx);
    std::mem::forget(rx);
    let _ = hide_thread_windows(current_thread_id());
    loop {
        let _ = catch_unwind(AssertUnwindSafe(|| queue.dispatch(256)));
        queue.wait(Duration::from_millis(250));
    }
}
