//! Help > Debug Logging (issue #84) and the per-run overload line.
//!
//! The logger lives in the app (`apps/transit/src/logging.rs`); the studio reaches it through
//! [`LogControl`], handed in as [`crate::Services::log`]. The toggle is session-scoped: it raises
//! the filter to debug for the rest of the run and is never saved, so the next launch starts from
//! the flag, `RUST_LOG` or the default again.

use crate::StudioApp;
use serde_json::{Value, json};
use soundcraft_playback::telemetry::CallbackTelemetry;
use std::path::PathBuf;

/// What the studio asks of the app's logger.
pub trait LogControl {
    /// Debug output is on (the `--verbose` flag, or the Help toggle).
    fn debug(&self) -> bool;
    /// Raise (or restore) the filter without a restart.
    fn set_debug(&self, on: bool);
    /// The log file of this run, if there is one.
    fn file(&self) -> Option<PathBuf>;
}

/// The status line after the toggle: `Debug logging on. Log file: <path>`.
pub fn status_line(on: bool, file: Option<&std::path::Path>) -> String {
    match (on, file) {
        (true, Some(path)) => format!("Debug logging on. Log file: {}", path.display()),
        (true, None) => "Debug logging on (this run writes no log file; lines go to standard error).".to_owned(),
        (false, _) => "Debug logging off.".to_owned(),
    }
}

/// Counters that grow during a run of the transport, read when it starts and when it stops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunCounts {
    /// Callbacks whose work exceeded the nominal frame budget (`CallbackTelemetry::over_budget_callbacks`).
    pub over_budget: u64,
    /// Overloads the driver reported (ASIO `kAsioOverload`; 0 elsewhere).
    pub driver_overloads: u64,
    /// Stream error callbacks of all kinds (not necessarily xruns).
    pub stream_errors: u64,
}

impl RunCounts {
    pub fn of(t: &CallbackTelemetry) -> RunCounts {
        RunCounts { over_budget: t.over_budget_callbacks, driver_overloads: t.backend.map_or(0, |b| b.overloads), stream_errors: t.backend_errors }
    }
}

/// The INFO line written at Stop, or `None` when nothing grew during the run:
/// `transport stopped: 3 over-budget callbacks, 1 driver overloads, 0 stream errors during this run`.
pub fn stop_line(start: RunCounts, stop: RunCounts) -> Option<String> {
    let over = stop.over_budget.saturating_sub(start.over_budget);
    let driver = stop.driver_overloads.saturating_sub(start.driver_overloads);
    let errors = stop.stream_errors.saturating_sub(start.stream_errors);
    (over + driver + errors > 0)
        .then(|| format!("transport stopped: {over} over-budget callbacks, {driver} driver overloads, {errors} stream errors during this run"))
}

impl StudioApp {
    /// `help.debug_logging {on?}`: set (or, without `on`, flip) debug logging for the rest of the session.
    pub(crate) fn set_debug_logging(&mut self, on: Option<bool>) -> Result<Value, String> {
        let Some(log) = self.services.log.as_ref() else {
            self.status = "Debug logging is not available in this build.".into();
            return Err(self.status.clone());
        };
        let on = on.unwrap_or(!log.debug());
        log.set_debug(on);
        let file = log.file();
        log::info!("debug logging {}", if on { "on (Help > Debug Logging)" } else { "off (Help > Debug Logging)" });
        self.status = status_line(on, file.as_deref());
        Ok(json!({"debug_logging": on, "log_file": file.map(|p| p.display().to_string())}))
    }
}

/// Once a frame, from the player read-back: remember the counters when the transport starts and
/// write the INFO line when it stops.
pub(crate) fn track_run_counts(counts: &mut Option<RunCounts>, p: &soundcraft_playback::Player, playing: bool) {
    if playing {
        counts.get_or_insert_with(|| RunCounts::of(&p.callback_telemetry()));
    } else if let Some(start) = counts.take()
        && let Some(line) = stop_line(start, RunCounts::of(&p.callback_telemetry()))
    {
        log::info!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[derive(Default)]
    struct Fake {
        on: Cell<bool>,
        file: RefCell<Option<PathBuf>>,
    }

    struct Shared(Rc<Fake>);

    impl LogControl for Shared {
        fn debug(&self) -> bool {
            self.0.on.get()
        }
        fn set_debug(&self, on: bool) {
            self.0.on.set(on);
        }
        fn file(&self) -> Option<PathBuf> {
            self.0.file.borrow().clone()
        }
    }

    fn app(fake: &Rc<Fake>) -> StudioApp {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.services.log = Some(Box::new(Shared(Rc::clone(fake))));
        app
    }

    #[test]
    fn the_command_toggles_debug_logging_and_the_status_names_the_log_file() {
        let fake = Rc::new(Fake::default());
        *fake.file.borrow_mut() = Some(PathBuf::from("/settings/logs/transit.log"));
        let mut app = app(&fake);
        let out = app.run("help.debug_logging", json!({})).expect("toggle on");
        assert_eq!(out["debug_logging"], true);
        assert!(fake.on.get());
        assert_eq!(app.status, status_line(true, Some(std::path::Path::new("/settings/logs/transit.log"))));
        assert!(app.status.contains("transit.log"), "{}", app.status);
        let out = app.run("help.debug_logging", json!({})).expect("toggle off");
        assert_eq!(out["debug_logging"], false);
        assert!(!fake.on.get());
        assert_eq!(app.status, "Debug logging off.");
        app.run("help.debug_logging", json!({"on": true})).expect("explicit on");
        app.run("help.debug_logging", json!({"on": true})).expect("explicit on again stays on");
        assert!(fake.on.get());
    }

    #[test]
    fn without_a_logger_the_command_says_so() {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        let e = app.run("help.debug_logging", json!({})).expect_err("no logger");
        assert!(e.contains("not available"), "{e}");
    }

    #[test]
    fn the_stop_line_counts_only_what_grew_during_the_run() {
        let start = RunCounts { over_budget: 5, driver_overloads: 1, stream_errors: 0 };
        assert_eq!(stop_line(start, start), None);
        let stop = RunCounts { over_budget: 8, driver_overloads: 1, stream_errors: 2 };
        assert_eq!(
            stop_line(start, stop).as_deref(),
            Some("transport stopped: 3 over-budget callbacks, 0 driver overloads, 2 stream errors during this run")
        );
        // A reopened player starts its counters again: never a negative or a wrapped count.
        assert_eq!(stop_line(stop, RunCounts::default()), None);
    }
}
