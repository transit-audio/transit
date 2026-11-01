//! Host faults count once even when several tempo-map sub-blocks miss the same deadline.
use super::*;

impl StripPool {
    pub(crate) fn host_entry(&mut self, frames: usize, rate: u32) {
        self.host_period = block_period(frames, rate);
        #[cfg(feature = "testing")]
        self.set.install_test_seams();
    }

    pub(crate) fn measurement_requested(&self) -> bool { self.set.measurement_requested() }
    pub(crate) fn capture_measurement(&mut self, settled: bool) { self.set.capture_measurement(settled); }
    pub(crate) fn begin_host_block(&mut self) {
        self.host_seen.fill(0);
        self.host_counted = false;
        self.host_overshot = false;
        self.last_overruns = 0;
    }

    pub(crate) fn end_host_block(&mut self, session: &Session) {
        if self.host_counted {
            self.note_held(session);
        }
    }

    pub(super) fn note_host_overshoot(&mut self) {
        if !self.host_overshot {
            self.counters.overshoot.fetch_add(1, Ordering::Relaxed);
            self.host_overshot = true;
        }
    }

    pub(super) fn note_host_outcomes(&mut self) {
        if !self.host_counted {
            self.counters.blocks.fetch_add(1, Ordering::Relaxed);
            self.host_counted = true;
        }
        for (i, seen) in self.host_seen.iter_mut().enumerate() {
            let Some(Outcome::Overrun(cause)) = self.set.outcome(i) else { continue };
            let (bit, counter) = match cause {
                OverrunCause::Busy => (1, &self.counters.overruns_busy),
                OverrunCause::Late => (1, &self.counters.overruns_late),
                OverrunCause::Unstarted => (1, &self.counters.overruns_unstarted),
                OverrunCause::UpstreamMissed => (2, &self.counters.upstream_missed),
            };
            if *seen & bit != 0 {
                continue;
            }
            *seen |= bit;
            counter.fetch_add(1, Ordering::Relaxed);
            if bit == 1 {
                if self.last_overruns == 0 {
                    self.counters.overrun_blocks.fetch_add(1, Ordering::Relaxed);
                }
                self.last_overruns = self.last_overruns.saturating_add(1);
            }
        }
    }
}
