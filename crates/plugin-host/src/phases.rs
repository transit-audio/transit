//! The phases of a plug-in load and of an editor open, with their durations (issue #67).
//!
//! A load runs in four phases: waiting for the host thread, creating the instance (module load,
//! its parameter table, the instance itself), restoring the stored state, and binding to the plan
//! (made, waiting for the compile to take it). The creation job marks the first and last; the
//! factory marks the middle ones with [`report_phase`], which finds the load it runs for through
//! a thread-local the job installs ([`scope`]). Every mark takes its timestamp from an injected
//! [`Clock`], so a test drives the phases on a manual clock and never sleeps. Nothing here runs on
//! the audio thread.

use crate::lock;
use soundcraft_dsp::EditorOpenReport;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The time marks are taken on ([`Instant::now`]; a test sets a manual one).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Where a load is, in the order it passes through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoadPhase {
    /// Submitted; the host thread has not started it (it is busy with an earlier job).
    Queued,
    /// The module is loaded, its parameter table read and the instance created.
    Creating,
    /// The stored state is being restored into the instance, which is prepared.
    Restoring,
    /// Made; waiting for the next plan to take it.
    Binding,
}

impl LoadPhase {
    /// What the tooltip and the log say.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            LoadPhase::Queued => "waiting for the plug-in host thread",
            LoadPhase::Creating => "creating instance",
            LoadPhase::Restoring => "restoring state",
            LoadPhase::Binding => "binding to the plan",
        }
    }
}

struct Marks {
    current: LoadPhase,
    since: Instant,
    done: Vec<(LoadPhase, Duration)>,
}

/// The phases one load has passed through, published by the thread that runs it and read by the
/// UI's polling.
pub struct PhaseLog {
    marks: Mutex<Marks>,
}

impl PhaseLog {
    /// A load that starts [`LoadPhase::Queued`] at `at`.
    #[must_use]
    pub fn new(at: Instant) -> Arc<PhaseLog> {
        Arc::new(PhaseLog { marks: Mutex::new(Marks { current: LoadPhase::Queued, since: at, done: Vec::new() }) })
    }

    /// Moves to `phase` at `at`, closing the one before with its duration. Only forward: a phase
    /// not after the current one (a repeated mark, or a factory that reports out of order) is
    /// ignored. Returns whether it moved.
    pub fn enter(&self, phase: LoadPhase, at: Instant) -> bool {
        let mut m = lock(&self.marks);
        if phase <= m.current {
            return false;
        }
        let took = at.saturating_duration_since(m.since);
        let left = m.current;
        m.done.push((left, took));
        m.current = phase;
        m.since = at;
        true
    }

    /// The phase it is in now.
    #[must_use]
    pub fn current(&self) -> LoadPhase {
        lock(&self.marks).current
    }

    /// Closes the current phase at `at` and returns every phase with its duration, in order.
    #[must_use]
    pub fn finish(&self, at: Instant) -> Vec<(LoadPhase, Duration)> {
        let mut m = lock(&self.marks);
        let took = at.saturating_duration_since(m.since);
        let left = m.current;
        m.done.push((left, took));
        m.since = at;
        std::mem::take(&mut m.done)
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<(Arc<PhaseLog>, Clock)>> = const { RefCell::new(None) };
}

/// Runs `f` with `log` as the load this thread works for, so [`report_phase`] inside it marks
/// `log` at `clock`'s time. The previous one (none, normally) comes back afterwards, also on a
/// panic.
pub fn scope<R>(log: &Arc<PhaseLog>, clock: &Clock, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<(Arc<PhaseLog>, Clock)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            ACTIVE.with(|a| *a.borrow_mut() = previous);
        }
    }
    let previous = ACTIVE.with(|a| a.borrow_mut().replace((Arc::clone(log), Arc::clone(clock))));
    let _restore = Restore(previous);
    f()
}

/// Marks the load this thread is working for (if any) as having entered `phase`. A factory calls
/// it between its steps; it does nothing where no load is being timed.
pub fn report_phase(phase: LoadPhase) {
    ACTIVE.with(|a| {
        if let Some((log, clock)) = &*a.borrow() {
            log.enter(phase, clock());
        }
    });
}

/// `0.4 s`, `12.3 s`: one decimal.
#[must_use]
pub fn seconds_text(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

/// The tooltip's phase part: `Loading Jup-8 V… 12.3 s (creating instance)`.
#[must_use]
pub fn loading_text(name: &str, elapsed: Duration, phase: LoadPhase) -> String {
    format!("Loading {name}… {} ({})", seconds_text(elapsed), phase.label())
}

/// The INFO lines for one finished load, one per phase and a total: `plug-in load: Jup-8 V:
/// creating instance 11200 ms`, then `plug-in loaded: Jup-8 V in 14100 ms`.
#[must_use]
pub fn log_lines(name: &str, total: Duration, phases: &[(LoadPhase, Duration)]) -> Vec<String> {
    let mut lines: Vec<String> = phases.iter().map(|(p, d)| format!("plug-in load: {name}: {} {} ms", p.label(), d.as_millis())).collect();
    lines.push(format!("plug-in loaded: {name} in {} ms", total.as_millis()));
    lines
}

/// The INFO line for an editor open: `plug-in editor opened: Jup-8 V in 3200 ms (request queued
/// 100 ms, window created and plug-in attached 3100 ms)`.
#[must_use]
pub fn open_log_line(name: &str, report: EditorOpenReport) -> String {
    format!(
        "plug-in editor opened: {name} in {} ms (request queued {} ms, window created and plug-in attached {} ms)",
        report.total().as_millis(),
        report.queued.as_millis(),
        report.opening.as_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn phases_are_published_in_order_with_their_durations() {
        let base = Instant::now();
        let log = PhaseLog::new(base);
        assert_eq!(log.current(), LoadPhase::Queued);
        assert!(log.enter(LoadPhase::Creating, at(base, 300)));
        assert!(log.enter(LoadPhase::Restoring, at(base, 11_500)));
        assert!(log.enter(LoadPhase::Binding, at(base, 13_500)));
        assert_eq!(log.current(), LoadPhase::Binding);
        let done = log.finish(at(base, 14_100));
        let ms = |p, d| (p, Duration::from_millis(d));
        assert_eq!(
            done,
            vec![ms(LoadPhase::Queued, 300), ms(LoadPhase::Creating, 11_200), ms(LoadPhase::Restoring, 2_000), ms(LoadPhase::Binding, 600)]
        );
    }

    #[test]
    fn a_mark_that_does_not_move_forward_is_ignored() {
        let base = Instant::now();
        let log = PhaseLog::new(base);
        assert!(log.enter(LoadPhase::Restoring, at(base, 10)));
        assert!(!log.enter(LoadPhase::Creating, at(base, 20)), "out of order");
        assert!(!log.enter(LoadPhase::Restoring, at(base, 30)), "repeated");
        assert_eq!(log.current(), LoadPhase::Restoring);
        let done = log.finish(at(base, 40));
        assert_eq!(done, vec![(LoadPhase::Queued, Duration::from_millis(10)), (LoadPhase::Restoring, Duration::from_millis(30))]);
    }

    #[test]
    fn report_phase_marks_the_load_of_the_scope_at_the_scope_clock() {
        let base = Instant::now();
        let now = Arc::new(Mutex::new(base));
        let clock: Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || *lock(&now))
        };
        let log = PhaseLog::new(base);
        report_phase(LoadPhase::Creating);
        assert_eq!(log.current(), LoadPhase::Queued, "outside a scope nothing is marked");
        scope(&log, &clock, || {
            *lock(&now) = at(base, 250);
            report_phase(LoadPhase::Creating);
            *lock(&now) = at(base, 1_250);
            report_phase(LoadPhase::Restoring);
        });
        report_phase(LoadPhase::Binding);
        assert_eq!(log.current(), LoadPhase::Restoring, "the scope ended with the job");
        let done = log.finish(at(base, 2_000));
        assert_eq!(done[1], (LoadPhase::Creating, Duration::from_millis(1_000)));
        assert_eq!(done[2], (LoadPhase::Restoring, Duration::from_millis(750)));
    }

    #[test]
    fn the_scope_is_restored_when_the_job_panics() {
        let base = Instant::now();
        let clock: Clock = Arc::new(move || base);
        let log = PhaseLog::new(base);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scope(&log, &clock, || panic!("factory"))));
        report_phase(LoadPhase::Creating);
        assert_eq!(log.current(), LoadPhase::Queued);
    }

    #[test]
    fn the_texts_name_the_plug_in_the_time_and_the_phase() {
        assert_eq!(loading_text("Jup-8 V", Duration::from_millis(12_300), LoadPhase::Creating), "Loading Jup-8 V… 12.3 s (creating instance)");
        let lines = log_lines("Jup-8 V", Duration::from_millis(14_100), &[(LoadPhase::Creating, Duration::from_millis(11_200))]);
        assert_eq!(lines, ["plug-in load: Jup-8 V: creating instance 11200 ms", "plug-in loaded: Jup-8 V in 14100 ms"]);
        let report = EditorOpenReport { queued: Duration::from_millis(500), opening: Duration::from_secs(2) };
        assert_eq!(
            open_log_line("Jup-8 V", report),
            "plug-in editor opened: Jup-8 V in 2500 ms (request queued 500 ms, window created and plug-in attached 2000 ms)"
        );
    }
}
