//! The note for a JACK loopback calibration that does not apply (#683): on JACK a calibration is
//! keyed by the output and the input port group that opened, so one measured through another
//! group (or saved before the key named the group, when it measured the output's own) is not
//! used, and the Audio Devices notes and the status line say so. A record an earlier build stored
//! lacks the buffer size the server ran and is not applied either (ADR 0021, amendment for #760,
//! rule 7). No note asks for a new measurement: nothing on the JACK system can measure a loop (the
//! app's Measure is native ASIO only), so none names an action (#760).

use crate::StudioApp;
use transit_playback::devices;

impl StudioApp {
    /// The note when the open JACK pair has no calibration but its output has one, at this mode,
    /// rate and buffer, measured through another input group; `None` otherwise (and on every
    /// other system, and without an open input, whose own note says why).
    pub(crate) fn jack_calibration_note(&self) -> Option<String> {
        let player = self.player.as_ref().filter(|p| !p.silent && p.calibration.is_none() && devices::is_jack_system(Some(&p.device_system)))?;
        let key = player.calibration_key.as_ref()?;
        let opened = key.input.as_deref()?;
        // Stored by an earlier build, keyed by the requested buffer whatever the server ran: kept,
        // not applied (ADR 0021, amendment for #760, rule 7).
        if self.ui.audio.jack_calibration_unproven(key).is_some() {
            return Some(format!(
                "The loopback calibration of “{}” was saved without the buffer size the JACK server ran, so it is not applied.",
                key.device
            ));
        }
        // Measured through this input group, over other ports of a virtual device (or before its
        // ports were recorded): not applied (amendment for #431, rule 13).
        if self.ui.audio.jack_calibration_ports_changed(key).is_some() {
            return Some(format!(
                "The loopback calibration of “{}” was measured through other ports of input “{opened}”, so it is not applied.",
                key.device
            ));
        }
        let saved = self.ui.audio.jack_calibration_elsewhere(key)?;
        let measured = saved.input.as_deref().unwrap_or(&key.device);
        Some(format!("The loopback calibration of “{}” was measured with input “{measured}” and does not apply to input “{opened}”.", key.device))
    }
}

/// The status line for `notes`: joined, then "Choose another…", unless every note is the
/// calibration note `calibration`, which another device choice does not answer.
pub(super) fn notes_status(notes: &[String], calibration: Option<&String>) -> String {
    let joined = notes.join(" ");
    if notes.iter().all(|n| Some(n) == calibration) { joined } else { format!("{joined} {}", super::CHOOSE_ANOTHER) }
}
