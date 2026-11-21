//! One direction of the session's client as an [`ab::Stream`], moved out of `session.rs` as a
//! child module, so it reads the session's private state as before.

use super::{Session, Slot, Timing, lock};
use crate::api::JackApi;
use crate::engine::{self, Shared};
use crate::links;
use std::sync::Arc;
use std::time::Duration;
use transit_audio_backend as ab;

/// One direction of the session's client. Dropping it detaches the direction; the last one
/// deactivates and closes the client, and joins the watcher: drop it on a control thread.
pub(crate) struct JackStream {
    pub(super) slot: Slot,
    pub(super) api: Arc<dyn JackApi>,
    pub(super) session: u64,
    pub(super) generation: u64,
    pub(super) direction: ab::Direction,
    pub(super) shared: Arc<Shared>,
    pub(super) timing: Timing,
}

impl JackStream {
    fn what(&self) -> &'static str {
        if self.direction == ab::Direction::Output { "render" } else { "capture" }
    }
}

impl ab::Stream for JackStream {
    /// Starts this direction (activating the client with the first one) and connects its ports.
    fn start(&self) -> Result<(), ab::OpenError> {
        let mut guard = lock(&self.slot);
        let session = guard
            .as_mut()
            .filter(|s| s.id == self.session)
            .ok_or_else(|| ab::OpenError::DeviceNotAvailable("JACK: the client of this stream is gone".into()))?;
        session.start(self.generation, &self.slot, &self.api, self.timing)
    }

    /// Stops this direction for good; returns once no process callback is inside it.
    fn stop(&self) -> Result<(), ab::OpenError> {
        let watcher = {
            let mut guard = lock(&self.slot);
            guard.as_mut().filter(|s| s.id == self.session).and_then(|s| s.end(self.generation, engine::STOPPED))
        };
        if let Some(watcher) = watcher {
            watcher.finish();
        }
        Ok(())
    }

    /// Port latencies are slice 3.
    fn latency(&self) -> Option<Duration> {
        None
    }

    /// The buffer size the start settled from the active client (ADR 0021, amendment for #760,
    /// rule 1); `None` before the start, or when it latched a rebuild.
    fn period(&self) -> Option<u32> {
        let guard = lock(&self.slot);
        guard.as_ref().filter(|s| s.id == self.session && s.settled && !s.rebuild).map(|s| s.buffer)
    }

    /// The start found the graph at another rate than the one the caller prepared for: no
    /// direction started, nothing connected; the caller reopens.
    fn needs_rebuild(&self) -> bool {
        lock(&self.slot).as_ref().is_some_and(|s| s.id == self.session && s.rebuild)
    }

    /// The graph, the latency reported at startup and the startup link check (display only).
    fn device_lines(&self) -> Vec<String> {
        lock(&self.slot).as_ref().filter(|s| s.id == self.session).map(Session::device_lines).unwrap_or_default()
    }

    /// The server's rate as its sample rate callback last published it (`None` until known).
    fn rate(&self) -> Option<u32> {
        Some(self.shared.sample_rate()).filter(|r| *r != engine::UNKNOWN)
    }

    /// The server refused the buffer size the open asked for (`jack_set_buffer_size` failed).
    fn period_declined(&self) -> bool {
        lock(&self.slot).as_ref().is_some_and(|s| s.id == self.session && (s.declined || s.not_kept))
    }

    /// The process thread and its priority are the server's.
    fn thread_priority(&self) -> ab::WorkerPriority {
        ab::WorkerPriority::Unobserved
    }

    /// This direction's process callbacks as wakes, server xruns as overloads, rate and quantum
    /// changes and the server's return as resets, a server shutdown or a panic as a fault.
    fn stats(&self) -> Option<ab::StreamStats> {
        Some(self.shared.stats(self.direction == ab::Direction::Output))
    }

    /// The links of every port of this stream's client that are not transit's own (#431, rule 6),
    /// for the reopen operation to hold while this client closes. A control-thread call, before
    /// the reopen stops or drops anything; never a JACK callback or an error callback.
    fn snapshot_links(&self) -> Option<ab::LinkSnapshot> {
        let mut guard = lock(&self.slot);
        let session = guard.as_mut().filter(|s| s.id == self.session)?;
        let mut snapshot = session.snapshot()?;
        if session.pipewire {
            *session.remembered.early() = Some(snapshot.early());
        }
        Some(Box::new(snapshot))
    }

    /// Restores a snapshot of an earlier client of this backend on this stream's started client,
    /// once (it is consumed here). Another backend's snapshot is discarded.
    ///
    /// Links the server accepted but its graph does not show yet are read again once per period
    /// until they show or [`links::CONFIRM_LIMIT`] has passed (a graph switch the server postponed;
    /// see `links`). The wait is on this control thread with the session's lock released: the lock
    /// is held for the restore and then for each read only, as long as one read-back held it
    /// before.
    ///
    /// A client that latched a rebuild ([`ab::Stream::needs_rebuild`]) restores nothing: it is
    /// silent and about to be replaced. It keeps the snapshot, and the reopen that replaces it
    /// takes it back ([`ab::Stream::snapshot_links`]) for the rebuilt client to restore.
    fn restore_links(&self, snapshot: ab::LinkSnapshot) -> Option<String> {
        let snapshot = snapshot.downcast::<links::Snapshot>().ok()?;
        let (mut done, period) = {
            let mut guard = lock(&self.slot);
            let session = guard.as_mut().filter(|s| s.id == self.session)?;
            if session.rebuild {
                log::info!("JACK: the client latched a rebuild; {} links made outside transit wait for the rebuilt client", snapshot.links.len());
                session.held = Some(*snapshot);
                return None;
            }
            (session.restore(&snapshot)?, session.period())
        };
        let limit = u64::try_from(links::CONFIRM_LIMIT.as_micros()).unwrap_or(u64::MAX);
        let start = self.api.now_us();
        let mut why = None;
        while !done.pending.is_empty() && self.api.now_us().saturating_sub(start) < limit {
            self.api.await_graph(period);
            let guard = lock(&self.slot);
            if guard.as_ref().filter(|s| s.id == self.session).and_then(|s| s.confirm(&mut done)).is_none() {
                why = Some("transit's client stopped before the server showed it");
                break;
            }
        }
        links::finish(&mut done, why);
        done.note()
    }
}

impl Drop for JackStream {
    fn drop(&mut self) {
        let (closing, watcher) = {
            let mut guard = lock(&self.slot);
            let Some(session) = guard.as_mut().filter(|s| s.id == self.session) else { return };
            let watcher = session.end(self.generation, engine::DETACHED);
            session.sides.retain(|s| s.generation != self.generation);
            if session.sides.is_empty() { (guard.take(), watcher) } else { (None, watcher) }
        };
        if let Some(watcher) = watcher {
            watcher.finish();
        }
        if let Some(session) = closing {
            log::debug!("JACK: the last stream ({}) closed; closing the client", self.what());
            session.close();
        }
    }
}
