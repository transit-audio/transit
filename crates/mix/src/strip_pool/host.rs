//! Host faults count once even when several tempo-map sub-blocks miss the same deadline.
use super::*;

impl StripPool {
    pub(crate) fn host_entry(&mut self, frames: usize, rate: u32) {
        self.host_period = block_period(frames, rate);
        #[cfg(feature = "testing")]
        self.set.install_test_seams();
    }

    /// The block's render path code (`crate::render_path`): one relaxed load, and a store only
    /// when it changed. Allocation-free.
    pub(crate) fn note_render_path(&self, code: u32) {
        if self.counters.render_path.load(Ordering::Relaxed) != code {
            self.counters.render_path.store(code, Ordering::Relaxed);
        }
    }

    pub(crate) fn measurement_requested(&self) -> bool {
        self.set.measurement_requested()
    }
    pub(crate) fn capture_measurement(&mut self, settled: bool) {
        self.set.capture_measurement(settled);
        self.counters.measurement_status.store(self.set.measurement_status(), Ordering::Relaxed);
    }
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
            // The tail acceptance rule's `H`: an aux task Late or Busy in any sub-block is one hole task-block of the host
            // block (`H`), and its first own overrun of the host block is an aux task overrun.
            let aux = i >= self.tail_from;
            if aux && matches!(cause, OverrunCause::Busy | OverrunCause::Late) && *seen & 4 == 0 {
                *seen |= 4;
                self.counters.aux_holes.fetch_add(1, Ordering::Relaxed);
            }
            if aux && bit == 1 && *seen & 1 == 0 {
                match cause {
                    OverrunCause::Busy => self.counters.aux_overruns_busy.fetch_add(1, Ordering::Relaxed),
                    OverrunCause::Late => self.counters.aux_overruns_late.fetch_add(1, Ordering::Relaxed),
                    _ => 0,
                };
            }
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

impl MixEngine {
    /// Takes the pool's single measurement consumer, on the control thread before rendering.
    /// Requests capture at a subsequent host boundary, before any new publication.
    pub fn render_measurement_reader(&mut self) -> Option<transit_rt::worker_set::queue::MeasurementReader> {
        self.pool.as_mut()?.set.measurement_reader()
    }
}
