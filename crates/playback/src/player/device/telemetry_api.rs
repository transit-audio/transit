//! The Player's stream telemetry: the output callback's counters, the device loop's, the live
//! engine's render-worker faults (ADR 0029), and a MIDI pass's overrun marks for its journal.

use super::{Player, telemetry};

impl Player {
    /// Start the since-open peak load over; the output callback applies it at its next block.
    pub fn reset_peak_load(&self) {
        self.fault.telemetry.reset_peak();
    }
    pub fn callback_telemetry(&self) -> telemetry::CallbackTelemetry {
        telemetry::CallbackTelemetry { backend: self.backend_stats(), strip_faults: self.strip_faults(), ..self.fault.telemetry.snapshot() }
    }
    /// The device loop's counters of the output stream (relaxed reads: call from the UI or control
    /// side), when the backend keeps them; the native WASAPI backend does, cpal does not.
    pub fn backend_stats(&self) -> Option<telemetry::StreamStats> {
        self.stream.as_ref().and_then(|stream| stream.stats())
    }
    /// The live engine's render-worker stream faults (strip overruns by cause, late results,
    /// deferred-MIDI overflows and silences, suspect strips; ADR 0029), `None` while the strips
    /// render serially (the default; `TRANSIT_RENDER_POOL=1` opts in). Relaxed reads.
    pub fn strip_faults(&self) -> Option<transit_mix::StripFaults> {
        self.owner().ok().and_then(|owner| owner.strip_faults())
    }
    /// The overrun marks of the MIDI pass under way (blocks in which render-worker strips were
    /// silence while it recorded, ADR 0029), oldest first, for its take journal. Drains the
    /// capture queue like [`Player::midi_recording_snapshot`]; nothing is consumed.
    pub fn midi_recording_overruns(&self) -> Vec<crate::midi_capture::OverrunMark> {
        let Ok(mut capture) = self.capture.try_borrow_mut() else { return Vec::new() };
        let Some(capture) = capture.as_mut() else { return Vec::new() };
        capture.poll();
        capture.marks().to_vec()
    }
}
