//! The External Instrument's Measure (ADR 0020 decision 3, slice 2) in the studio UI: the card's
//! Measure button and `instrument.measure_external`. The engine checks what it can see (the track,
//! its port and return, the transport, the arm); the live Player sends the notes through the MIDI
//! output thread and records the return ([`transit_playback::external_probe`]), so the app runs
//! the command itself. A run takes about 13 s and never blocks: the card shows "Measuring… 3/8"
//! with Cancel, a per-frame step collects the result, and it is stored per machine in
//! `IoMap.external_latency` under (port, return, rate, buffer), next to the hardware insert table.
//! A spread above 1 ms is shown and stored only on Keep; every refusal is said in the words of
//! decision 7's table, and nothing is stored for it.

use crate::StudioApp;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use transit_model::{ExternalLatency, ExternalSource, IoDirection, MidiOutPort, MidiOutput, TrackId};
use transit_playback::external_probe::{MAX_SPREAD_MS, MeasureError, Measured, NOTES, Progress};

/// The refusal of a transport start while a Measure runs: playback would sound on the synth and
/// bleed into the return.
pub(crate) const REFUSED_TRANSPORT: &str = "Stop the External Instrument measurement first";

/// Why a Measure ended when the audio device was re-opened under it.
pub(crate) const CANCELLED_REOPEN: &str = "Measurement cancelled: the audio device was re-opened. Nothing was stored.";

/// What Cancel leaves on the card.
pub(crate) const CANCELLED: &str = "Measurement cancelled. Nothing was stored.";

/// The extra time past a run's length before a silent device counts as gone.
const GRACE: Duration = Duration::from_secs(5);

/// A Measure the card started and has not collected yet.
#[derive(Debug, Clone)]
pub struct Running {
    pub(crate) track: TrackId,
    port: MidiOutPort,
    ret: String,
    rate: u32,
    buffer: u32,
    began: Instant,
    wait: Duration,
}

/// What the card says after a Measure, until the next one on that track.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub track: TrackId,
    /// The result or the refusal, as said.
    pub text: String,
    /// A result whose spread is above 1 ms, stored only when the user presses Keep.
    pub keep: Option<ExternalLatency>,
    /// The result was stored: the card's state line says it ("Measured ... on ..."), not this.
    pub stored: bool,
}

/// A stand-in for the Player's Measure, driven by tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeMeasure {
    pub started: Vec<(TrackId, Vec<u16>)>,
    pub progress: Option<Progress>,
    pub result: Option<Result<Measured, MeasureError>>,
    pub cancelled: usize,
    /// The device buffer the fake device runs.
    pub buffer: Option<u32>,
    /// The output map has not settled.
    pub unsettled: bool,
}

/// The card's Measure state.
#[derive(Debug, Default)]
pub struct MeasureState {
    pub running: Option<Running>,
    pub outcome: Option<Outcome>,
    #[cfg(test)]
    pub(crate) fake: Option<FakeMeasure>,
}

/// What Measure needs, as the app sees it now (pure inputs of [`measure_blocker`]).
#[derive(Debug, Clone, Default)]
pub struct Conditions {
    /// An audio device is open.
    pub live: bool,
    /// Its input is open (a duplex device).
    pub duplex: bool,
    pub playing: bool,
    pub armed: bool,
    /// `None`: a port is chosen and present; `Some("")`: none chosen; `Some(name)`: missing.
    pub port: Option<String>,
    /// The return path, and whether it is an input path of this device.
    pub ret: String,
    pub ret_found: bool,
    /// The device clock's output map is valid (the notes can be stamped).
    pub settled: bool,
    /// Another measurement runs (this one, a hardware insert's, a loop calibration).
    pub busy: bool,
}

/// Why Measure is off, in decision 3's and decision 7's words; `None` when it can run.
#[must_use]
pub fn measure_blocker(c: &Conditions) -> Option<String> {
    Some(if !c.live {
        "No audio device".into()
    } else if !c.duplex {
        "Needs a duplex device".into()
    } else if c.playing {
        "Stop the transport to measure.".into()
    } else if c.armed {
        "Disarm the track to measure: live input would play the synth.".into()
    } else if let Some(port) = &c.port {
        if port.is_empty() { "No MIDI port: choose one on the Out button".into() } else { format!("{port} missing: notes are not sent") }
    } else if c.ret.is_empty() {
        "Choose the return input".into()
    } else if !c.ret_found {
        format!("Return {} is not on this interface", c.ret)
    } else if !c.settled {
        "Timing approximate: device clock not settled".into()
    } else if c.busy {
        "A measurement is already running.".into()
    } else {
        return None;
    })
}

/// "48 kHz", "44.1 kHz".
#[must_use]
pub fn rate_text(rate: u32) -> String {
    let k = f64::from(rate) / 1000.0;
    if (k - k.round()).abs() < 1e-9 { format!("{k:.0} kHz") } else { format!("{k:.1} kHz") }
}

/// Milliseconds of `frames` at `rate`.
fn ms(frames: f64, rate: u32) -> f64 {
    frames * 1000.0 / f64::from(rate.max(1))
}

/// The date of a measurement, `YYYY-MM-DD` in UTC ("unknown date" without one).
#[must_use]
pub fn date_text(measured_at: u64) -> String {
    if measured_at == 0 { "an unknown date".into() } else { transit_playback::devices::utc_date(measured_at) }
}

/// "Measured 19.9 ms on 2026-10-24 at 48 kHz/64": a measurement of the current key.
#[must_use]
pub fn measured_text(m: &ExternalLatency) -> String {
    format!("Measured {:.1} ms on {} at {}/{}", ms(f64::from(m.frames), m.rate), date_text(m.measured_at), rate_text(m.rate), m.buffer)
}

/// "Measured at 48 kHz / 64, now 44.1 kHz / 128: measure again": the newest measurement of the
/// chain, made at another rate or buffer (used, rescaled by time across rates; labeled amber).
#[must_use]
pub fn stale_text(m: &ExternalLatency, rate: u32, buffer: Option<u32>) -> String {
    let now = buffer.map_or_else(|| rate_text(rate), |b| format!("{} / {b}", rate_text(rate)));
    format!("Measured at {} / {}, now {now}: measure again", rate_text(m.rate), m.buffer)
}

/// "22.1 ms (1061 frames), spread 0.4 ms, 8 of 8 notes".
#[must_use]
pub fn result_text(m: &Measured, rate: u32) -> String {
    format!("{:.1} ms ({} frames), spread {:.1} ms, {} of {NOTES} notes", ms(f64::from(m.frames), rate), m.frames, m.spread_ms(rate), m.found)
}

/// Decision 7's warning for a spread above 1 ms.
#[must_use]
pub fn spread_text(spread_ms: f64) -> String {
    format!("Spread {spread_ms:.1} ms is high (expected under {MAX_SPREAD_MS:.0} ms): the synth's attack or the driver is unsteady.")
}

/// Decision 7's refusal texts.
#[must_use]
pub fn error_text(e: &MeasureError, ret: &str) -> String {
    match e {
        MeasureError::NoSound { .. } => {
            format!("No sound came back on {ret} within 1 s. Check the synth's audio goes to {ret}, its MIDI channel and its volume.")
        }
        MeasureError::NotQuiet { .. } => "The return is not quiet before the note: stop the synth's sound (held note, arpeggiator, drone).".into(),
        MeasureError::Clipped { .. } => "The return clipped: lower the synth or the input gain and measure again.".into(),
        MeasureError::DeviceLost => "Measurement stopped: the audio device went away. Nothing was stored.".into(),
        MeasureError::InputAlignmentChanged { .. } => "Input alignment changed during the measurement: measure again.".into(),
    }
}

/// "Measuring… 3/8".
#[must_use]
pub fn progress_text(p: Progress) -> String {
    format!("Measuring\u{2026} {}/{}", p.note, p.of)
}

/// What the card's state line says about the latency of a playing device: the manual value, the
/// measurement of this key, or one made at another rate or buffer; `amber` for the last.
#[must_use]
pub fn latency_text(frames: u32, source: &ExternalSource, rate: u32, buffer: Option<u32>) -> (String, bool) {
    match source {
        ExternalSource::Manual => (format!("Manual: {:.1} ms", ms(f64::from(frames), rate)), false),
        ExternalSource::Measured(m) => (measured_text(m), false),
        ExternalSource::Stale(m) => (stale_text(m, rate, buffer), true),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl StudioApp {
    /// The buffer the open device runs (part of a measurement's key).
    pub(crate) fn device_buffer(&self) -> Option<u32> {
        #[cfg(test)]
        if let Some(f) = &self.external_measure.fake {
            return f.buffer;
        }
        self.player.as_ref().filter(|p| !p.silent).and_then(|p| p.device_buffer)
    }

    /// What Measure on `t` needs, as things are now.
    pub(crate) fn measure_conditions(&self, t: TrackId) -> Conditions {
        let s = self.engine.session();
        let Some(track) = s.track(t) else { return Conditions::default() };
        let ret = track.external_instrument().map(|e| e.ret.clone()).unwrap_or_default();
        let ret_found = self.engine.io_map().path(&ret).is_some_and(|p| p.direction == IoDirection::Input);
        let view = crate::midi_output::OutputView::read(self.player.as_ref());
        let port = match &track.midi_output {
            MidiOutput::Internal => Some(String::new()),
            out if view.missing(t, out) => Some(view.port_label(out)),
            _ => None,
        };
        let c = Conditions {
            live: self.player.as_ref().is_some_and(|p| !p.silent),
            duplex: self.player.as_ref().is_some_and(|p| p.duplex()),
            playing: self.is_playing() || self.engine.transport.recording,
            armed: track.mixer.record_arm,
            port,
            ret,
            ret_found,
            settled: self
                .player
                .as_ref()
                .and_then(|p| p.midi_status().ok())
                .and_then(|m| m.timing_quality)
                .is_some_and(|q| q != transit_playback::device_clock::Quality::Invalid),
            busy: self.external_measure.running.is_some()
                || self.audio_devices.measuring.is_some()
                || self.player.as_ref().is_some_and(|p| p.external_measure_running() || p.hardware_probe_running() || p.loop_calibration_running()),
        };
        #[cfg(test)]
        if let Some(f) = &self.external_measure.fake {
            let port = if matches!(track.midi_output, MidiOutput::External { .. }) { None } else { c.port.clone() };
            return Conditions { live: true, duplex: true, settled: !f.unsettled, busy: self.external_measure.running.is_some(), port, ..c };
        }
        c
    }

    /// Why Measure on `t` is off now; `None` when it can run.
    pub(crate) fn external_measure_blocker(&self, t: TrackId) -> Option<String> {
        measure_blocker(&self.measure_conditions(t))
    }

    /// The card's Measure (and `instrument.measure_external`): check, then start the run and
    /// return at once. Errors say why it did not start.
    pub(crate) fn start_external_measure(&mut self, t: TrackId) -> Result<(), String> {
        transit_engine::cmd::external_measure_blocker(&self.engine, t)?;
        if let Some(why) = self.external_measure_blocker(t) {
            return Err(why);
        }
        let s = self.engine.session();
        let track = s.track(t).ok_or("no such track")?;
        let ret = track.external_instrument().map(|e| e.ret.clone()).unwrap_or_default();
        let MidiOutput::External { port, .. } = track.midi_output.clone() else { return Err("No MIDI port: choose one on the Out button".into()) };
        let channels = self.engine.io_map().path(&ret).map(|p| p.channels.clone()).unwrap_or_default();
        let rate = s.sample_rate.hz();
        // The plan must know the table as it is now (the return's channels).
        self.sync_io();
        let buffer = self.device_buffer().unwrap_or(0);
        #[cfg(test)]
        let fake = self.external_measure.fake.as_mut().map(|f| f.started.push((t, channels.clone()))).is_some();
        #[cfg(not(test))]
        let fake = false;
        let wait = if fake {
            Duration::from_secs(14)
        } else {
            let player = self.player.as_ref().filter(|p| !p.silent).ok_or("No audio device")?;
            player.start_external_measure(t, &channels)?;
            player.external_measure_duration()
        };
        self.external_measure.outcome = None;
        self.external_measure.running = Some(Running { track: t, port, ret, rate, buffer, began: Instant::now(), wait: wait + GRACE });
        Ok(())
    }

    /// Where the running Measure is.
    pub(crate) fn external_measure_progress(&self) -> Option<Progress> {
        self.external_measure.running.as_ref()?;
        #[cfg(test)]
        if let Some(f) = &self.external_measure.fake {
            return Some(f.progress.unwrap_or(Progress { note: 0, of: NOTES as u32, level: 0.0 }));
        }
        Some(self.player.as_ref().and_then(|p| p.external_measure_progress()).unwrap_or(Progress { note: 0, of: NOTES as u32, level: 0.0 }))
    }

    fn poll_external(&mut self, r: &Running) -> Option<Result<Measured, MeasureError>> {
        #[cfg(test)]
        if let Some(f) = self.external_measure.fake.as_mut() {
            return f.result.take();
        }
        let Some(player) = self.player.as_ref() else { return Some(Err(MeasureError::DeviceLost)) };
        if let Some(done) = player.external_measure_result() {
            return Some(done);
        }
        if r.began.elapsed() > r.wait {
            // The device stopped calling back without saying so.
            player.cancel_external_measure();
            return Some(Err(MeasureError::DeviceLost));
        }
        None
    }

    /// Per frame: collect a finished Measure, store it (or hold it for Keep) and say it.
    pub(crate) fn step_external_measure(&mut self) {
        let Some(r) = self.external_measure.running.clone() else { return };
        let Some(done) = self.poll_external(&r) else { return };
        self.external_measure.running = None;
        let outcome = match done {
            Ok(m) => {
                let record = ExternalLatency {
                    port_id: r.port.id.clone(),
                    port_name: r.port.name.clone(),
                    ret: r.ret.clone(),
                    rate: r.rate,
                    buffer: r.buffer,
                    frames: m.frames,
                    spread_frames: m.spread.round() as u32,
                    notes: m.found,
                    measured_at: now_secs(),
                };
                let said = result_text(&m, r.rate);
                if m.needs_keep(r.rate) {
                    Outcome { track: r.track, text: format!("{said}. {}", spread_text(m.spread_ms(r.rate))), keep: Some(record), stored: false }
                } else {
                    self.store_external(record);
                    Outcome { track: r.track, text: said, keep: None, stored: true }
                }
            }
            Err(e) => Outcome { track: r.track, text: error_text(&e, &r.ret), keep: None, stored: false },
        };
        self.status.clone_from(&outcome.text);
        self.external_measure.outcome = Some(outcome);
    }

    fn store_external(&mut self, record: ExternalLatency) {
        self.engine.store_external_latency(record);
        self.sync_io();
        self.save_audio_preferences();
    }

    /// Keep: store the held result of `t`'s last Measure (its spread was above 1 ms).
    pub(crate) fn keep_external_measure(&mut self, t: TrackId) -> Result<(), String> {
        let held = self.external_measure.outcome.as_mut().filter(|o| o.track == t).and_then(|o| o.keep.take());
        let record = held.ok_or("No measurement is waiting to be kept.")?;
        let text = format!("Kept: {}", measured_text(&record));
        self.store_external(record);
        if let Some(o) = self.external_measure.outcome.as_mut() {
            o.text.clone_from(&text);
            o.stored = true;
        }
        self.status = text;
        Ok(())
    }

    /// Cancel the running Measure (Cancel, a device reopen) and leave `say` on the card. The
    /// Player releases the note through the MIDI output thread. Nothing is stored.
    pub(crate) fn cancel_external_measure(&mut self, say: &str) {
        let Some(r) = self.external_measure.running.take() else { return };
        #[cfg(test)]
        if let Some(f) = self.external_measure.fake.as_mut() {
            f.cancelled += 1;
            self.external_measure.outcome = Some(Outcome { track: r.track, text: say.into(), keep: None, stored: false });
            return;
        }
        if let Some(p) = &self.player {
            p.cancel_external_measure();
        }
        self.external_measure.outcome = Some(Outcome { track: r.track, text: say.into(), keep: None, stored: false });
    }

    /// `instrument.measure_external {track?, action?}`: start (returns at once), cancel, keep, or
    /// status.
    pub(crate) fn measure_external_command(&mut self, p: &Value) -> Result<Value, String> {
        let action = p.get("action").and_then(Value::as_str).unwrap_or("start");
        let t = match p.get("track").and_then(Value::as_u64) {
            Some(id) => TrackId(id),
            None => self.focus_track().ok_or("`track` required")?,
        };
        let result = match action {
            "start" => self.start_external_measure(t),
            "cancel" => {
                self.cancel_external_measure(CANCELLED);
                Ok(())
            }
            "keep" => self.keep_external_measure(t),
            "status" => Ok(()),
            _ => Err("`action` is start, cancel, keep or status".into()),
        };
        if let Err(why) = &result {
            self.status.clone_from(why);
        }
        result?;
        let progress = self.external_measure_progress();
        let outcome = self.external_measure.outcome.as_ref().filter(|o| o.track == t);
        Ok(json!({
            "running": progress.is_some(),
            "note": progress.map_or(0, |p| p.note),
            "of": NOTES,
            "result": outcome.map(|o| o.text.clone()),
            "keep": outcome.is_some_and(|o| o.keep.is_some()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(rate: u32, buffer: u32, frames: u32) -> ExternalLatency {
        ExternalLatency {
            port_id: String::new(),
            port_name: "MODEL D".into(),
            ret: "In 1".into(),
            rate,
            buffer,
            frames,
            spread_frames: 19,
            notes: 8,
            measured_at: 1_792_800_000,
        }
    }

    #[test]
    fn the_blocker_says_why_in_the_adr_words_in_order() {
        let ok = Conditions { live: true, duplex: true, ret: "In 1".into(), ret_found: true, settled: true, ..Conditions::default() };
        assert_eq!(measure_blocker(&ok), None);
        let say = |f: &dyn Fn(&mut Conditions)| {
            let mut c = ok.clone();
            f(&mut c);
            measure_blocker(&c).unwrap()
        };
        assert_eq!(say(&|c| c.live = false), "No audio device");
        assert_eq!(say(&|c| c.duplex = false), "Needs a duplex device");
        assert_eq!(say(&|c| c.playing = true), "Stop the transport to measure.");
        assert_eq!(say(&|c| c.armed = true), "Disarm the track to measure: live input would play the synth.");
        assert_eq!(say(&|c| c.port = Some(String::new())), "No MIDI port: choose one on the Out button");
        assert_eq!(say(&|c| c.port = Some("MODEL D".into())), "MODEL D missing: notes are not sent");
        assert_eq!(say(&|c| c.ret_found = false), "Return In 1 is not on this interface");
        assert_eq!(say(&|c| c.ret.clear()), "Choose the return input");
        assert_eq!(say(&|c| c.settled = false), "Timing approximate: device clock not settled");
        assert_eq!(say(&|c| c.busy = true), "A measurement is already running.");
    }

    #[test]
    fn the_card_texts() {
        assert_eq!((rate_text(48_000), rate_text(44_100), rate_text(96_000)), ("48 kHz".into(), "44.1 kHz".into(), "96 kHz".into()));
        assert_eq!(measured_text(&record(48_000, 64, 955)), "Measured 19.9 ms on 2026-10-24 at 48 kHz/64");
        assert_eq!(stale_text(&record(48_000, 64, 955), 44_100, Some(128)), "Measured at 48 kHz / 64, now 44.1 kHz / 128: measure again");
        let m = Measured { frames: 1061, spread: 19.2, found: 8, sent: 8, latencies: Vec::new(), backlog: None };
        assert_eq!(result_text(&m, 48_000), "22.1 ms (1061 frames), spread 0.4 ms, 8 of 8 notes");
        assert_eq!(spread_text(1.3), "Spread 1.3 ms is high (expected under 1 ms): the synth's attack or the driver is unsteady.");
        assert_eq!(
            error_text(&MeasureError::NoSound { found: 2, sent: 8 }, "In 1"),
            "No sound came back on In 1 within 1 s. Check the synth's audio goes to In 1, its MIDI channel and its volume."
        );
        assert_eq!(
            error_text(&MeasureError::NotQuiet { peak: 0.2 }, "In 1"),
            "The return is not quiet before the note: stop the synth's sound (held note, arpeggiator, drone)."
        );
        assert_eq!(
            error_text(&MeasureError::Clipped { peak: 1.0 }, "In 1"),
            "The return clipped: lower the synth or the input gain and measure again."
        );
        assert_eq!(progress_text(Progress { note: 3, of: 8, level: 0.0 }), "Measuring\u{2026} 3/8");
        assert_eq!(latency_text(960, &ExternalSource::Manual, 48_000, Some(64)), ("Manual: 20.0 ms".into(), false));
        assert!(latency_text(955, &ExternalSource::Stale(record(48_000, 64, 955)), 48_000, Some(128)).1, "amber");
    }
}
