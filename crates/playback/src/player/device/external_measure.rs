//! The Player's side of an External Instrument's Measure (ADR 0020 decision 3, slice 2): arm the
//! probe for a track, report progress, collect the result, and on every early exit release the
//! note through the MIDI output thread ([`crate::external_probe`] has the timing).

use super::Player;
use crate::external_probe::{self, ExtProbe, MeasureError, Measured, Progress};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use transit_model::{MidiOutChannel, MidiOutput, TrackId};

impl Player {
    /// Start measuring the External Instrument on `track`: its MIDI port gets one warm-up note and
    /// then eight notes 1.5 s apart (note 60, velocity 100, on the track's channel, channel 1 when
    /// it sends as recorded), and the probe records the device input channels `ret` (0-based) by
    /// plan frame. Poll [`Self::external_measure_result`]; it takes about 13 s. Refuses with no
    /// device, no live input, a playing transport, a speed other than 1x, an armed track, a track
    /// without an External Instrument or an external MIDI port, a port the output thread does not
    /// have open, an unknown channel, or another measurement running. Control side only.
    pub fn start_external_measure(&self, track: TrackId, ret: &[u16]) -> Result<(), String> {
        let probe = self.ext_probe.as_ref().filter(|_| !self.silent).ok_or("there is no audio device to measure through")?;
        let io = self.io_map();
        if !self.plan_input.get()
            && let Some(ring) = self.with_held_input(|recorder| Arc::clone(&recorder.monitor))
        {
            // The probe reads the live input ring: the held input feeds the plan for it.
            self.set_input(ring).map_err(|e| format!("the held input could not feed the measurement ({e:?})"))?;
        }
        if !self.plan_input.get() || io.inputs == 0 {
            return Err("this device has no live input open (an External Instrument needs a duplex device)".into());
        }
        if self.is_playing() {
            return Err("stop the transport before measuring".into());
        }
        if self.speed() != 1.0 {
            return Err("set the playback speed to 1x before measuring".into());
        }
        if ret.is_empty() || ret.iter().any(|c| *c >= io.inputs) {
            return Err("the return names a channel the device did not open".into());
        }
        if self.hardware_probe_running() || self.loop_calibration_running() || probe.armed() {
            return Err("a measurement is already running".into());
        }
        let session = Arc::clone(self.owner().map_err(|e| format!("{e:?}"))?.requested_session());
        let t = session.track(track).ok_or("no such track")?;
        if t.external_instrument().is_none() {
            return Err("the track has no External Instrument".into());
        }
        if t.mixer.record_arm {
            return Err("disarm the track to measure: live input would play the synth".into());
        }
        let channel = match &t.midi_output {
            MidiOutput::External { channel: MidiOutChannel::Channel(c), .. } => *c,
            MidiOutput::External { channel: MidiOutChannel::AsRecorded, .. } => 0,
            MidiOutput::Internal => return Err("the track sends its MIDI to no port".into()),
        };
        self.external_port_open(track)?;
        if !probe.arm(track.0, channel, external_probe::NOTE, external_probe::VELOCITY, ret) {
            return Err("a measurement is already running".into());
        }
        Ok(())
    }

    /// Whether the output thread has `track`'s MIDI port open.
    pub(super) fn external_port_open(&self, track: TrackId) -> Result<(), String> {
        use crate::player::midi_out::DestinationState;
        let midi = self.midi.try_borrow().map_err(|_| "MIDI is busy")?;
        let native = midi.as_ref().ok_or("MIDI is unavailable")?;
        match native.output_status().track(track) {
            Some(DestinationState::Connected) => Ok(()),
            _ => Err("the track's MIDI port is not open".into()),
        }
    }

    /// Ask the output thread to release what `track` holds (note-off, Sustain off, All Notes Off).
    pub(super) fn release_external(&self, track: TrackId) {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(native) = midi.as_ref()
        {
            native.release_track(track);
        }
    }

    /// Whether a Measure is running.
    pub fn external_measure_running(&self) -> bool {
        self.ext_probe.as_ref().is_some_and(|p| p.armed())
    }

    /// Where the running Measure is (notes sent of eight, the return's level); `None` when none
    /// runs. Reads atomics only.
    pub fn external_measure_progress(&self) -> Option<Progress> {
        self.ext_probe.as_ref().filter(|p| p.armed()).map(|p| p.progress())
    }

    /// How long a Measure takes on this Player.
    pub fn external_measure_duration(&self) -> std::time::Duration {
        self.ext_probe.as_ref().map_or(std::time::Duration::ZERO, |p| p.duration())
    }

    /// The Measure's outcome once it is done (`None` while it runs, and after it was taken). A
    /// device that went away during it ends it with [`MeasureError::DeviceLost`], after the output
    /// thread was asked to release the note.
    pub fn external_measure_result(&self) -> Option<Result<Measured, MeasureError>> {
        let probe: &ExtProbe = self.ext_probe.as_ref()?;
        if probe.armed() && (self.fault.gone.load(Ordering::Acquire) || self.fault.failed.load(Ordering::Acquire)) {
            probe.abandon();
            self.release_external(TrackId(probe.track()));
            let lost = Err(MeasureError::DeviceLost);
            log_result(probe, &lost);
            return Some(lost);
        }
        let result = probe.take_result()?;
        log_result(probe, &result);
        Some(result)
    }

    /// Stop the Measure (Cancel, the window closing, a device reopen): the audio thread sends the
    /// note-off and All Notes Off at its next block, and the output thread releases the track now
    /// in case it does not run again. Nothing is measured.
    pub fn cancel_external_measure(&self) {
        if let Some(p) = &self.ext_probe
            && p.cancel()
        {
            self.release_external(TrackId(p.track()));
        }
    }
}

/// The Measure's log line (control thread, INFO): the outcome and the input ring's backlog B by
/// note window, so a refusal on hardware can be read back from transit.log (#98).
fn log_result(probe: &ExtProbe, result: &Result<Measured, MeasureError>) {
    let outcome = match result {
        Ok(m) => format!("C {} frames, spread {:.1}, {} of {} notes, latencies {:?}", m.frames, m.spread, m.found, m.sent, m.latencies),
        Err(MeasureError::InputAlignmentChanged { first, last }) => format!("refused: input alignment changed (window mean B {first} -> {last})"),
        Err(e) => format!("refused: {e}"),
    };
    let windows: Vec<String> = probe.backlog_windows().iter().map(ToString::to_string).collect();
    log::info!(
        "External Instrument Measure at {} Hz: {outcome}; input ring backlog B by note window (warm-up, notes 1-{}), mean/min/max input frames (blocks): {}; tolerance {} frames",
        probe.rate(),
        external_probe::NOTES,
        windows.join(", "),
        probe.backlog_tolerance()
    );
}
