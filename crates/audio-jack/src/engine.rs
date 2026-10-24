//! What the JACK callbacks do, in safe code over [`OutputPlanes`], so it runs on plain vectors in
//! tests: the process shell and the notification handlers. `crate::callback` calls it from libjack's
//! threads with the real port buffers. Nothing here allocates, locks, logs or formats (clippy denies
//! `format!`, `to_string` and the log macros in this module; a `CountingAlloc` test drives it).
//!
//! Every handler only stores atomics (ADR 0021, decision 4): the buffer size, sample rate, xrun and
//! shutdown callbacks are treated as real-time context on every server, and the shutdown handler
//! must behave like an async-signal handler (jack.h, `jack_on_info_shutdown`). The watcher thread of
//! `crate::session` reads them and acts.
//!
//! Slice 1 runs no user callback: the process shell counts the cycle and writes silence to every
//! output port. The duplex engine (conversion, stamps, the render and capture callbacks) is slice 2.
#![deny(clippy::disallowed_macros, clippy::disallowed_methods)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// What the callbacks and the control side share for one client. Relaxed counters, except the
/// shutdown flag (release by the handler, acquire by the watcher).
#[derive(Debug)]
pub(crate) struct Shared {
    /// Process callbacks served.
    cycles: AtomicU64,
    /// The `nframes` of the last process callback.
    last_frames: AtomicU32,
    /// Xruns the server reported.
    xruns: AtomicU64,
    /// The server shut the client down (`jack_on_info_shutdown`).
    shutdown: AtomicBool,
    /// The server's quantum: `jack_get_buffer_size` at open, then each buffer size callback.
    buffer_size: AtomicU32,
    /// Buffer size callbacks that changed the quantum.
    buffer_changes: AtomicU64,
    /// The server's rate: `jack_get_sample_rate` at open, then each sample rate callback.
    sample_rate: AtomicU32,
    /// Reset requests the watcher delivered (a sample rate change).
    resets: AtomicU64,
    /// Faults the watcher delivered (the server stopped).
    faults: AtomicU64,
}

impl Shared {
    pub(crate) fn new(sample_rate: u32, buffer_size: u32) -> Self {
        Self {
            cycles: AtomicU64::new(0),
            last_frames: AtomicU32::new(0),
            xruns: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            buffer_size: AtomicU32::new(buffer_size),
            buffer_changes: AtomicU64::new(0),
            sample_rate: AtomicU32::new(sample_rate),
            resets: AtomicU64::new(0),
            faults: AtomicU64::new(0),
        }
    }

    /// The info shutdown callback: one atomic store, nothing else (async-signal safe).
    pub(crate) fn on_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// The xrun callback.
    pub(crate) fn on_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// The buffer size callback: the quantum the next process callbacks get.
    pub(crate) fn on_buffer_size(&self, frames: u32) {
        if self.buffer_size.swap(frames, Ordering::Relaxed) != frames {
            self.buffer_changes.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The sample rate callback.
    pub(crate) fn on_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    pub(crate) fn shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) fn cycles(&self) -> u64 {
        self.cycles.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn last_frames(&self) -> u32 {
        self.last_frames.load(Ordering::Relaxed)
    }

    pub(crate) fn xruns(&self) -> u64 {
        self.xruns.load(Ordering::Relaxed)
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        self.buffer_size.load(Ordering::Relaxed)
    }

    pub(crate) fn buffer_changes(&self) -> u64 {
        self.buffer_changes.load(Ordering::Relaxed)
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub(crate) fn resets(&self) -> u64 {
        self.resets.load(Ordering::Relaxed)
    }

    pub(crate) fn faults(&self) -> u64 {
        self.faults.load(Ordering::Relaxed)
    }

    /// The watcher delivered a reset request.
    pub(crate) fn count_reset(&self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }

    /// The watcher delivered a fault.
    pub(crate) fn count_fault(&self) {
        self.faults.fetch_add(1, Ordering::Relaxed);
    }
}

/// The output port buffers of one process callback, in port order (`out_1` first).
pub(crate) trait OutputPlanes {
    fn count(&self) -> usize;
    /// The buffer of output port `index` for this cycle, `None` when the server gave none.
    fn plane(&mut self, index: usize) -> Option<&mut [f32]>;
}

/// One process callback of `frames` frames: counts it and writes silence to every output port.
pub(crate) fn process<P: OutputPlanes + ?Sized>(shared: &Shared, frames: u32, outputs: &mut P) {
    shared.cycles.fetch_add(1, Ordering::Relaxed);
    shared.last_frames.store(frames, Ordering::Relaxed);
    let frames = frames as usize;
    for index in 0..outputs.count() {
        if let Some(plane) = outputs.plane(index) {
            let n = frames.min(plane.len());
            plane[..n].fill(0.0);
        }
    }
}
