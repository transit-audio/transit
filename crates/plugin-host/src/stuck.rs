//! What a host thread is busy with, for the watchdog's log and the diagnostics report (#205).
//!
//! Every busy section (a job, a creation, a window-message dispatch, a tick) carries a [`Busy`]:
//! the plug-in it runs and the call. When [`crate::Host::check`] finds a section past its budget it
//! logs both at once, and again every [`RELOG_EVERY`] while the section stays stuck, so a log of
//! a hang always names the plug-in and the call it did not return from. [`crate::Host::stuck`]
//! lists the stuck sections for the UI and the diagnostics report ([`StuckCall::line`]).

use std::sync::Arc;
use std::time::Duration;

/// How often the watchdog logs a call again while it stays stuck.
pub const RELOG_EVERY: Duration = Duration::from_secs(60);

/// The calls a section can run, as the log and the report name them.
pub mod call {
    /// A creation: module entry, instance, parameter table and state restore.
    pub const CREATING: &str = "creating it (module load, instance, state restore)";
    pub const EDITOR_OPEN: &str = "opening its editor";
    pub const EDITOR_CLOSE: &str = "closing its editor";
    pub const EDITOR_PARAM: &str = "showing a parameter change in its editor";
    pub const EDITOR_HEADER: &str = "updating its editor's header strip";
    pub const STATE_READ: &str = "reading its state";
    pub const PRESET_JOB: &str = "a preset save or load";
    pub const DESTROY: &str = "destroying it";
    pub const RELEASE: &str = "releasing its editor and state handles";
    /// A window message: a click or key in a plug-in's own window (its editor, or a dialog it
    /// opened) runs the plug-in's code inside it.
    pub const WINDOW_MESSAGE: &str = "a window message (its editor or a dialog it opened)";
    pub const EDITOR_IDLE: &str = "its editor idle (and its restartComponent servicing)";
    pub const SERVICING: &str = "request servicing (restartComponent, request_callback, parameter rescans)";
    pub const START: &str = "starting the host thread";
    pub const SHUTDOWN: &str = "shutting the host thread down";
    /// A job that names no call (a test, a format's own request).
    pub const REQUEST: &str = "a request";
}

/// The plug-in a section runs when it runs none in particular.
pub const ANY_PLUGIN: &str = "a plug-in";
/// The plug-in a dispatch runs when no editor is open.
pub const NO_EDITOR: &str = "no plug-in editor open";

/// What a busy section runs: the plug-in (its name) and the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Busy {
    pub plugin: Arc<str>,
    pub call: &'static str,
}

impl Busy {
    #[must_use]
    pub fn new(plugin: &Arc<str>, call: &'static str) -> Busy {
        Busy { plugin: Arc::clone(plugin), call }
    }

    /// A section for no plug-in in particular.
    #[must_use]
    pub fn any(call: &'static str) -> Busy {
        Busy { plugin: Arc::from(ANY_PLUGIN), call }
    }
}

/// A host thread stuck in a plug-in call ([`crate::Host::stuck`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StuckCall {
    /// The host thread's name (`plugin-host-0`).
    pub thread: String,
    pub plugin: Arc<str>,
    pub call: &'static str,
    /// How long the call has run.
    pub stuck_for: Duration,
    /// The user gave up on the thread ([`crate::Host::give_up`]): its plug-ins are quarantined.
    pub given_up: bool,
    /// How many plug-in instances the thread hosts (a Give up quarantines them all).
    pub hosted: usize,
}

impl StuckCall {
    /// The plug-ins on the thread besides the stuck one (a creation's plug-in is not hosted yet).
    #[must_use]
    pub fn others(&self) -> usize {
        if self.call == call::CREATING { self.hosted } else { self.hosted.saturating_sub(1) }
    }

    /// The diagnostics report's line: `Plug-in host: Guitar Rig 7 stuck in a window message (its
    /// editor or a dialog it opened) for 240 s`.
    #[must_use]
    pub fn line(&self) -> String {
        let given_up = if self.given_up { " (given up: its plug-ins are quarantined until transit restarts)" } else { "" };
        format!("Plug-in host: {} stuck in {} for {} s{given_up}", self.plugin, self.call, self.stuck_for.as_secs())
    }
}

/// The watchdog's line when a section first runs past its budget.
#[must_use]
pub fn first_line(busy: &Busy, thread: &str, budget: Duration) -> String {
    format!(
        "plug-in host: {} has not returned from {} for {} s ({thread}); a new host thread takes over new plug-ins, and the old one's plug-ins are unreachable until it returns",
        busy.plugin,
        busy.call,
        budget.as_secs()
    )
}

/// The watchdog's line when a thread that already handed new plug-ins over (it wedged once and
/// came back) runs past its budget again.
#[must_use]
pub fn older_line(busy: &Busy, thread: &str, budget: Duration) -> String {
    format!(
        "plug-in host: {} has not returned from {} for {} s ({thread}, an older host thread); its plug-ins are unreachable until it returns",
        busy.plugin,
        busy.call,
        budget.as_secs()
    )
}

/// The watchdog's line while a section stays stuck.
#[must_use]
pub fn again_line(busy: &Busy, thread: &str, stuck_for: Duration) -> String {
    format!("plug-in host: {} still stuck in {} after {} s ({thread})", busy.plugin, busy.call, stuck_for.as_secs())
}

/// Whether a stuck section that was last logged when it had run `logged` is due again at
/// `stuck_for`.
#[must_use]
pub fn relog_due(stuck_for: Duration, logged: Duration) -> bool {
    stuck_for.saturating_sub(logged) >= RELOG_EVERY
}

/// The names of the plug-ins whose editors are open, for a dispatch's label: `Guitar Rig 7`,
/// `Guitar Rig 7, Pro-Q 3`, or [`NO_EDITOR`].
#[must_use]
pub fn open_editors_label(names: &[&str]) -> Arc<str> {
    if names.is_empty() { Arc::from(NO_EDITOR) } else { Arc::from(names.join(", ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stuck_call_is_logged_at_first_and_every_minute_after() {
        let s = |secs| Duration::from_secs(secs);
        assert!(!relog_due(s(120), s(120)), "just logged");
        assert!(!relog_due(s(179), s(120)));
        assert!(relog_due(s(180), s(120)), "60 s later");
        assert!(relog_due(s(600), s(180)));
        assert!(!relog_due(s(100), s(120)), "a clock that reads earlier is not due");
    }

    #[test]
    fn the_lines_name_the_plug_in_and_the_call() {
        let busy = Busy::new(&Arc::from("Guitar Rig 7"), call::WINDOW_MESSAGE);
        assert_eq!(
            first_line(&busy, "plugin-host-0", Duration::from_secs(120)),
            "plug-in host: Guitar Rig 7 has not returned from a window message (its editor or a dialog it opened) for 120 s (plugin-host-0); a new host thread takes over new plug-ins, and the old one's plug-ins are unreachable until it returns"
        );
        assert_eq!(
            again_line(&busy, "plugin-host-0", Duration::from_secs(181)),
            "plug-in host: Guitar Rig 7 still stuck in a window message (its editor or a dialog it opened) after 181 s (plugin-host-0)"
        );
        let stuck = StuckCall {
            thread: "plugin-host-0".into(),
            plugin: Arc::from("Guitar Rig 7"),
            call: call::PRESET_JOB,
            stuck_for: Duration::from_millis(240_900),
            given_up: false,
            hosted: 6,
        };
        assert_eq!(stuck.others(), 5, "the stuck plug-in is one of them");
        assert_eq!(StuckCall { call: call::CREATING, ..stuck.clone() }.others(), 6, "a creation's is not hosted yet");
        assert_eq!(stuck.line(), "Plug-in host: Guitar Rig 7 stuck in a preset save or load for 240 s");
        assert_eq!(
            StuckCall { given_up: true, ..stuck }.line(),
            "Plug-in host: Guitar Rig 7 stuck in a preset save or load for 240 s (given up: its plug-ins are quarantined until transit restarts)"
        );
    }

    #[test]
    fn a_dispatch_names_the_open_editors() {
        assert_eq!(&*open_editors_label(&[]), NO_EDITOR);
        assert_eq!(&*open_editors_label(&["Guitar Rig 7"]), "Guitar Rig 7");
        assert_eq!(&*open_editors_label(&["Guitar Rig 7", "Pro-Q 3"]), "Guitar Rig 7, Pro-Q 3");
    }
}
