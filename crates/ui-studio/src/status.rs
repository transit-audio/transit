//! The status line's lifetime (#203): a message ages out after [`STATUS_SECS`] of the frame clock
//! the UI already has, so a refused command's error does not sit in the bar until the next
//! message. The last message is kept, with its time, for the Copy diagnostics report.

/// How long a status message stays, seconds of the frame clock.
pub const STATUS_SECS: f64 = 8.0;

#[derive(Debug)]
pub struct StatusAge {
    /// The message the clock is counting, so a different text (a newer message) restarts it.
    seen: String,
    since: f64,
    last: Option<(u64, String)>,
    /// How long a message stays, seconds of the frame clock; `None` holds it until replaced.
    span: Option<f64>,
}

impl Default for StatusAge {
    /// The app's: a message ages out after [`STATUS_SECS`].
    fn default() -> Self {
        StatusAge { seen: String::new(), since: 0.0, last: None, span: Some(STATUS_SECS) }
    }
}

/// A message that states a transport wait, not an outcome: the transport code clears it itself.
fn is_state(text: &str) -> bool {
    text == "Count-in" || text.ends_with(" pending")
}

impl StatusAge {
    /// A message stays until the next one; the last one is still kept for the report. The crate's
    /// tests use it: their harnesses frame while they wait on background threads for a wall-clock
    /// span, so 8 s of their frame clock is wall time, and a status assertion after such a wait
    /// would depend on the host's speed (E49). The tests of the aging opt in with `default()`.
    pub fn held() -> Self {
        StatusAge { span: None, ..Self::default() }
    }

    /// Once a frame. `now` is the frame clock, `epoch` the wall time stamped on the kept message.
    /// Returns the seconds until `status` is due to clear, when it is waiting to.
    pub fn tick(&mut self, status: &mut String, now: f64, epoch: u64) -> Option<f64> {
        if *status != self.seen {
            self.seen.clone_from(status);
            self.since = now;
            if !status.is_empty() {
                self.last = Some((epoch, status.clone()));
            }
        }
        let span = self.span?;
        if status.is_empty() || is_state(status) {
            return None;
        }
        let left = span - (now - self.since);
        if left <= 0.0 {
            status.clear();
            self.seen.clear();
            return None;
        }
        Some(left)
    }

    /// The last message shown and when it appeared (epoch seconds).
    pub fn last(&self) -> Option<&(u64, String)> {
        self.last.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_clears_after_the_span_of_frames() {
        let mut age = StatusAge::default();
        let mut status = String::from("Command failed: no such port");
        for frame in 0..=480 {
            age.tick(&mut status, f64::from(frame) / 60.0, 1000);
        }
        assert!(status.is_empty(), "8 s of 60 Hz frames clears it");
        let mut again = String::from("x");
        let mut a = StatusAge::default();
        a.tick(&mut again, 0.0, 1);
        a.tick(&mut again, STATUS_SECS - 0.1, 1);
        assert_eq!(again, "x", "still there just before the span");
    }

    #[test]
    fn a_newer_message_replaces_the_older_one_at_once_and_restarts_the_clock() {
        let mut age = StatusAge::default();
        let mut status = String::from("first");
        age.tick(&mut status, 0.0, 10);
        status = "second".into();
        age.tick(&mut status, 7.9, 11);
        assert_eq!(status, "second", "the replacement shows at once");
        age.tick(&mut status, 9.0, 12);
        assert_eq!(status, "second", "and has its own span, not the first one's remainder");
        age.tick(&mut status, 7.9 + STATUS_SECS + 0.1, 13);
        assert!(status.is_empty());
    }

    #[test]
    fn the_last_message_survives_the_clearing_with_its_time() {
        let mut age = StatusAge::default();
        let mut status = String::from("midi.map: no such control");
        age.tick(&mut status, 0.0, 1_700_000_000);
        age.tick(&mut status, 20.0, 1_700_000_020);
        assert!(status.is_empty());
        assert_eq!(age.last(), Some(&(1_700_000_000, "midi.map: no such control".to_string())));
    }

    #[test]
    fn transport_waits_stay_until_the_transport_code_clears_them() {
        let mut age = StatusAge::default();
        for text in ["Count-in", "Record pending"] {
            let mut status = text.to_string();
            age.tick(&mut status, 0.0, 1);
            age.tick(&mut status, 100.0, 2);
            assert_eq!(status, text);
        }
    }

    #[test]
    fn a_held_message_stays_until_replaced_and_is_still_kept_for_the_report() {
        let mut age = StatusAge::held();
        let mut status = String::from("Added /tmp/x to Locations");
        age.tick(&mut status, 0.0, 5);
        assert_eq!(age.tick(&mut status, 100.0, 6), None, "no clearing is scheduled");
        assert_eq!(status, "Added /tmp/x to Locations");
        assert_eq!(age.last(), Some(&(5, "Added /tmp/x to Locations".to_string())));
    }

    #[test]
    fn the_report_carries_the_last_error() {
        let report = crate::diagnostics::Report { last_status: Some((1_700_000_000, "bad midi.map".into())), ..Default::default() };
        let text = report.render(&crate::diagnostics::Redaction::default());
        assert!(text.contains("Last status message (2023-11-14 22:13 UTC): bad midi.map"), "{text}");
        let text = crate::diagnostics::Report::default().render(&crate::diagnostics::Redaction::default());
        assert!(text.contains("Last status message: none"), "{text}");
    }
}
