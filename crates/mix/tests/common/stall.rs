//! Last-resort hang detection for test handshakes, independent of total hold duration.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Ten minutes without progress is a last-resort hang ceiling, not a render-time requirement.
/// A scheduled hold may span arbitrarily slow blocks; completed blocks renew this budget.
pub const HANG: Duration = Duration::from_secs(600);

pub struct Watchdog {
    progress: usize,
    since: Duration,
}

impl Watchdog {
    pub fn new(progress: usize) -> Self {
        Self { progress, since: Duration::ZERO }
    }

    /// Read time before progress for expiry, but renew at a fresh time after observing progress.
    /// Either read can be separated from the other by an arbitrarily long scheduler pause.
    /// Inject the reads themselves so tests cover those interleavings without sleeping.
    pub fn stalled(&mut self, progress: impl FnOnce() -> usize, mut elapsed: impl FnMut() -> Duration) -> bool {
        let before = elapsed();
        let observed = progress();
        if observed != self.progress {
            self.progress = observed;
            self.since = elapsed();
            return false;
        }
        before.saturating_sub(self.since) >= HANG
    }
}

/// Only the owner releases a hold. Total time in `process` does not decide its outcome.
/// The no-progress ceiling catches an owner that never resumes (including a failed test).
pub fn wait_for_release(hold: &AtomicBool, progress: &AtomicUsize, pause: impl Fn()) {
    // The initial progress read must also precede its clock origin; otherwise a pause between
    // them could charge the new observation for time that passed before it was observed.
    let mut watchdog = Watchdog::new(progress.load(Ordering::Acquire));
    let start = Instant::now();
    while hold.load(Ordering::Acquire) {
        let stalled = watchdog.stalled(|| progress.load(Ordering::Acquire), || start.elapsed());
        // Recheck the release after reading the clock: the owner may have released the hold
        // while this worker was descheduled between the loop condition and the readout.
        assert!(!stalled || !hold.load(Ordering::Acquire), "hold owner made no progress for ten minutes");
        pause();
    }
}
