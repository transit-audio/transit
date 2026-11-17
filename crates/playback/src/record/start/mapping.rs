//! Latch input placement in the independent clock worker before disk work.
use super::{Reader, Status};
use crate::device_clock::{ClockKey, ClockMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;
/// Placed by the degraded policy: the audio thread's own frame choice, no host-time map.
const READY_APPROXIMATE: u8 = 4;
/// How long a placement waits for an input clock map that is missing or cannot place the start
/// before it falls back (#587), as the audio side waits a second for the output clock.
pub(crate) const WAIT_NS: u64 = 1_000_000_000;

struct Shared {
    state: AtomicU8,
    frame: AtomicU64,
}

pub(crate) struct Mapping {
    start: Reader,
    input: ClockKey,
    shared: Arc<Shared>,
    /// The newest validated input map this placement was offered: what a start falls back to
    /// when no current map can place it (#587).
    last: Option<ClockMap>,
    /// Host time after which a start that no current map places stops waiting for one.
    deadline: Option<u64>,
    /// The input clock's invalidation count when `last` was offered: a change drops `last`, so the
    /// fallback never projects across a discontinuity (#587).
    invalidations: Option<u64>,
}

pub(crate) struct InputStart {
    shared: Arc<Shared>,
    finished: Option<Arc<AtomicBool>>,
}

pub(crate) fn pair(start: Reader, input: ClockKey) -> (Mapping, InputStart) {
    let shared = Arc::new(Shared { state: AtomicU8::new(PENDING), frame: AtomicU64::new(0) });
    (Mapping { start, input, shared: Arc::clone(&shared), last: None, deadline: None, invalidations: None }, InputStart { shared, finished: None })
}

impl Mapping {
    /// [`Mapping::service_counted`] without an invalidation count (tests).
    #[cfg(test)]
    pub(crate) fn service(&mut self, map: Option<ClockMap>, now: Option<u64>) -> bool {
        self.service_counted(map, now, None)
    }

    /// Called by clock maintenance, never by the audio callback or disk writer, with the input
    /// clock's invalidation count (see [`crate::device_clock::FitCounts::invalidations`]) when the
    /// caller has it. Returns true after the one immutable result has been published.
    pub(crate) fn service_counted(&mut self, map: Option<ClockMap>, now: Option<u64>, invalidations: Option<u64>) -> bool {
        if self.shared.state.load(Ordering::Acquire) != PENDING {
            return true;
        }
        if let Some(count) = invalidations {
            if self.invalidations.is_some_and(|seen| seen != count) {
                self.last = None;
            }
            self.invalidations = Some(count);
        }
        if let Some(map) = map {
            self.last = Some(map);
        }
        let point = match self.start.status() {
            Status::Pending | Status::CountingIn | Status::Waiting => return false,
            Status::Started(point) => point,
            Status::Failed(_) | Status::Cancelled => {
                self.shared.state.store(FAILED, Ordering::Release);
                return true;
            }
        };
        // A start the input clock has not observed up to yet (the output's latency puts it ahead of
        // the input's last block) waits for a later map; a map that goes away ends the wait below.
        if point.host_ns != 0
            && let (Some(map), Some(now)) = (map, now)
            && map.is_ahead(self.input, point.host_ns, now)
        {
            return false;
        }
        // A zero host time means the output had no validated map at the latch.
        let mapped =
            (point.host_ns != 0).then(|| map.zip(now)).flatten().and_then(|(map, now)| map.host_to_frame(self.input, point.host_ns, now).ok());
        if let Some(frame) = mapped {
            self.shared.frame.store(frame, Ordering::Relaxed);
            self.shared.state.store(READY, Ordering::Release);
            return true;
        }
        let ordinal = self.start.fallback().and_then(|f| f.input_frame);
        // No current map places a start the output did map, and it has no duplex input ordinal (a
        // separate input, #587: a late input callback rejects the fit, or the map expires): wait a
        // bounded time for one, then place it by the newest map this placement saw. A duplex start
        // falls back to its ordinal at once, as before.
        if point.host_ns != 0 && ordinal.is_none() {
            if !self.waited(now) {
                return false;
            }
            if let Some(frame) = self.projected(point.host_ns) {
                log::warn!("recording input placed by the last validated input clock map (no current map placed the start); the take is approximate");
                self.place_approximately(frame);
                return true;
            }
        }
        match ordinal {
            Some(frame) => {
                log::warn!("recording input placed by input frame ordinal (no validated device clock); the take is approximate");
                self.place_approximately(frame);
            }
            None => self.shared.state.store(FAILED, Ordering::Release),
        }
        true
    }

    fn place_approximately(&self, frame: u64) {
        self.start.mark_approximate();
        self.shared.frame.store(frame, Ordering::Relaxed);
        self.shared.state.store(READY_APPROXIMATE, Ordering::Release);
    }

    /// Has the start waited its bounded time for a current map? The wait starts at the first
    /// service that could not place it; without a current host time it does not wait.
    fn waited(&mut self, now: Option<u64>) -> bool {
        let Some(now) = now else { return true };
        now >= *self.deadline.get_or_insert(now.saturating_add(WAIT_NS))
    }

    /// The start's input frame by the newest validated map offered, projected past its window.
    fn projected(&self, host_ns: u64) -> Option<u64> {
        self.last?.project_frame(self.input, host_ns).ok()
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // Service failure, shutdown and abandoned queued requests cannot strand
        // a stopped disk writer waiting for an acknowledgment that will not come.
        let _ = self.shared.state.compare_exchange(PENDING, FAILED, Ordering::Release, Ordering::Relaxed);
    }
}

impl InputStart {
    pub(crate) fn with_lifetime(mut self, finished: Arc<AtomicBool>) -> Self {
        self.finished = Some(finished);
        self
    }
    pub(crate) fn frame(&self) -> Result<Option<u64>, &'static str> {
        let mut state = self.shared.state.load(Ordering::Acquire);
        if state == PENDING && self.finished.as_ref().is_some_and(|finished| finished.load(Ordering::Acquire)) {
            // Completion may have published between the first read and worker
            // teardown. Its release precedes `finished`; revalidate once.
            state = self.shared.state.load(Ordering::Acquire);
            if state == PENDING {
                state = FAILED;
            }
        }
        match state {
            READY | READY_APPROXIMATE => Ok(Some(self.shared.frame.load(Ordering::Relaxed))),
            PENDING => Ok(None),
            _ => Err("recording start could not be mapped by the input clock worker"),
        }
    }
}
