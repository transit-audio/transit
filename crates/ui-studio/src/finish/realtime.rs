//! File > Bounce in real time (ADR 0027, ADR 0020 slice 3): a mix with an External Instrument is
//! rendered by playing it through the live device, so the bounce is what the user hears. The
//! engine checks what it can see (`transit_engine::cmd::realtime_plan`); the app checks the
//! device (open, duplex, no measurement running, the ports present), then the Player plays the
//! range from its pre-roll and keeps Main and the returns ([`transit_playback::realtime_render`]).
//! The dialog shows "Rendering in real time… bar N of M" with Cancel; Cancel, a failure and a
//! device loss stop playback, release the synths' notes and remove the partial files.

use crate::StudioApp;
use serde_json::{Value, json};
use std::path::PathBuf;
use transit_model::{MidiOutput, TrackId};
use transit_playback::realtime_render::{FileSink, RealtimeError, RealtimeSpec, Rendered, StemSpec};

/// The refusal of a transport start while a real-time render plays.
pub(crate) const REFUSED_TRANSPORT: &str = "Rendering in real time: cancel the render first";

/// What Cancel leaves on the status line.
pub(crate) const CANCELLED: &str = "Render cancelled. Nothing was written.";

/// What All MIDI Notes Off during a render leaves: the render is cancelled, not finished with its
/// notes cut off.
pub(crate) const CANCELLED_PANIC: &str = "Render cancelled by All MIDI Notes Off. Nothing was written.";

/// Why a render ended when the audio device was re-opened under it.
pub(crate) const CANCELLED_REOPEN: &str = "Render stopped: the audio device was re-opened. Nothing was written.";

/// The commands a real-time render refuses while it plays: they would move or stop the playhead.
const TRANSPORT: [&str; 15] = [
    "transport.play",
    "transport.toggle",
    "transport.record",
    "transport.half_speed",
    "transport.play_selection",
    "transport.stop",
    "transport.locate",
    "transport.rtz",
    "transport.pause",
    "transport.rewind",
    "transport.fast_forward",
    "transport.go_to_end",
    "transport.loop",
    "edit.play_timeline",
    "instrument.measure_external",
];

/// A real-time render the dialog started.
#[derive(Debug, Clone)]
pub struct Rendering {
    pub path: PathBuf,
    pub stems: Vec<PathBuf>,
    pub range: transit_time::Range,
    pub tracks: Vec<TrackId>,
    pub uncompensated: Vec<String>,
}

/// A stand-in for the Player's real-time render, driven by tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FakeRender {
    pub started: Vec<RealtimeSpec>,
    pub done: u64,
    pub result: Option<Result<Rendered, RealtimeError>>,
    pub cancelled: usize,
    pub no_device: bool,
    pub output_only: bool,
    pub measuring: bool,
}

/// The real-time render's state in the bounce dialog.
#[derive(Debug, Default)]
pub struct RealtimeState {
    pub running: Option<Rendering>,
    /// How the last render ended, when something other than its own outcome ended it.
    pub ended: Option<String>,
    #[cfg(test)]
    pub(crate) fake: Option<FakeRender>,
}

/// What a real-time render needs of the device, as the app sees it now.
#[derive(Debug, Clone, Default)]
pub struct Conditions {
    pub live: bool,
    pub duplex: bool,
    pub playing: bool,
    /// A measurement runs (an External Instrument's, a hardware insert's, a loop calibration).
    pub measuring: bool,
    /// The External Instrument whose MIDI port is missing, by its port's label.
    pub missing_port: Option<String>,
}

/// Why the device cannot render in real time now, in ADR 0020 decision 7's words; `None` when it can.
#[must_use]
pub fn device_blocker(c: &Conditions) -> Option<String> {
    Some(if !c.live {
        "No audio device".into()
    } else if !c.duplex {
        "Needs a duplex device".into()
    } else if c.playing {
        "Stop the transport to render in real time.".into()
    } else if c.measuring {
        "A measurement is running: wait for it or cancel it.".into()
    } else if let Some(port) = &c.missing_port {
        format!("{port} missing: notes are not sent")
    } else {
        return None;
    })
}

/// "Rendering in real time… bar 3 of 8".
#[must_use]
pub fn progress_text(bar: i64, of: i64) -> String {
    format!("Rendering in real time\u{2026} bar {bar} of {of}")
}

/// The message of an engine refusal, without the command's prefix.
fn said(e: &transit_engine::EngineError) -> String {
    match e {
        transit_engine::EngineError::BadParams(_, m) => m.clone(),
        other => other.to_string(),
    }
}

impl StudioApp {
    /// What a real-time render needs of the device now.
    pub(crate) fn realtime_conditions(&self) -> Conditions {
        let s = self.engine.session();
        let view = crate::midi_output::OutputView::read(self.player.as_ref());
        let missing_port = s
            .tracks
            .iter()
            .filter(|t| !t.inactive && t.freeze.is_none() && t.external_instrument().is_some())
            .find(|t| matches!(t.midi_output, MidiOutput::External { .. }) && view.missing(t.id, &t.midi_output))
            .map(|t| view.port_label(&t.midi_output));
        let player = self.player.as_ref().filter(|p| !p.silent);
        let c = Conditions {
            live: player.is_some(),
            duplex: player.is_some_and(transit_playback::Player::duplex),
            playing: self.is_playing() || self.engine.transport.recording,
            measuring: self.external_measure.running.is_some()
                || self.audio_devices.measuring.is_some()
                || player.is_some_and(|p| p.external_measure_running() || p.hardware_probe_running() || p.loop_calibration_running()),
            missing_port,
        };
        #[cfg(test)]
        if let Some(f) = &self.finish.realtime.fake {
            return Conditions { live: !f.no_device, duplex: !f.output_only, measuring: f.measuring, missing_port: None, ..c };
        }
        c
    }

    /// Why `p` (a `file.bounce_mix` in real time) cannot start now: the engine's checks first,
    /// then the device's. `None` when it can.
    pub(crate) fn realtime_blocker(&self, p: &Value) -> Option<String> {
        if let Err(e) = transit_engine::cmd::realtime_plan(&self.engine, p) {
            return Some(said(&e));
        }
        device_blocker(&self.realtime_conditions())
    }

    /// Commands a running real-time render refuses, and `file.bounce_mix {mode: "realtime"}`,
    /// which the app runs itself. `None`: not for this module.
    pub(crate) fn realtime_intercept(&mut self, id: &str, p: &Value) -> Option<Result<Value, String>> {
        if self.finish.realtime.running.is_some() && id == "event.all_notes_off" {
            // A panic cancels the render (release, partial files removed), then runs as usual.
            self.cancel_realtime();
            self.finish.realtime.ended = Some(CANCELLED_PANIC.into());
            self.status = CANCELLED_PANIC.into();
            if matches!(self.dialog, Some(crate::Dialog::Bounce)) {
                self.dialog = None;
            }
            return None;
        }
        if self.finish.realtime.running.is_some() && TRANSPORT.contains(&id) {
            self.status = REFUSED_TRANSPORT.into();
            return Some(Err(self.status.clone()));
        }
        let realtime =
            id == "file.bounce_mix" && transit_engine::cmd::realtime_plan_mode(p).is_ok_and(|m| m == transit_engine::cmd::RenderMode::Realtime);
        realtime.then(|| {
            let r = self.start_realtime(p);
            if let Err(why) = &r {
                self.status.clone_from(why);
            }
            r
        })
    }

    /// Start `p` on the live Player and return at once.
    fn start_realtime(&mut self, p: &Value) -> Result<Value, String> {
        if self.finish.realtime.running.is_some() {
            return Err("A real-time render is already running.".into());
        }
        self.finish.realtime.ended = None;
        let plan = transit_engine::cmd::realtime_plan(&self.engine, p).map_err(|e| said(&e))?;
        if let Some(why) = device_blocker(&self.realtime_conditions()) {
            return Err(why);
        }
        // The plan must have this machine's table (the returns' channels and C) as it is now.
        self.sync_io();
        let s = self.engine.session_arc();
        let rate = s.sample_rate.hz();
        // C as the live plan compensates it: decision 4's lookup through the Player's own table.
        let io = self.player.as_ref().map(transit_playback::Player::io_map);
        let reports = io.as_deref().map(|io| transit_mix::external_report(&s, io, io.inputs, true, rate, &[])).unwrap_or_default();
        let stems: Vec<StemSpec> = plan
            .stems
            .iter()
            .map(|st| StemSpec {
                track: st.track,
                channels: st.channels.clone(),
                latency: reports.iter().find(|r| r.track == st.track).and_then(|r| r.latency).or(st.latency).unwrap_or(0),
                gain: transit_dsp::db_to_gain(st.gain_db),
            })
            .collect();
        let block = i64::from(self.device_buffer().unwrap_or(512));
        let reach = stems.iter().map(|st| i64::from(st.latency)).max().unwrap_or(0);
        let spec = RealtimeSpec {
            start: plan.range.start,
            len: u64::try_from(plan.range.len()).unwrap_or(0),
            main_channels: u16::try_from(plan.channels).unwrap_or(2),
            preroll: reach.max(block),
            stems,
        };
        let files: Vec<Option<(PathBuf, usize)>> = plan.stems.iter().map(|st| st.path.clone().map(|p| (p, st.channels.len()))).collect();
        let rendering = Rendering {
            path: plan.path.clone(),
            stems: plan.stems.iter().filter_map(|st| st.path.clone()).collect(),
            range: plan.range,
            tracks: plan.stems.iter().map(|st| st.track).collect(),
            uncompensated: plan.uncompensated(),
        };
        #[cfg(test)]
        let fake = self.finish.realtime.fake.as_mut().map(|f| f.started.push(spec.clone())).is_some();
        #[cfg(not(test))]
        let fake = false;
        if !fake {
            let sink = FileSink::create(&plan.path, plan.channels, Some(plan.mask), &files, &plan.opts, rate, spec.len)?;
            let player = self.player.as_ref().filter(|p| !p.silent).ok_or("No audio device")?;
            player.start_realtime_render(&spec, Box::new(sink))?;
        }
        let out = json!({
            "running": true,
            "path": rendering.path.to_string_lossy(),
            "stems": rendering.stems.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "uncompensated": rendering.uncompensated,
        });
        self.finish.realtime.running = Some(rendering);
        Ok(out)
    }

    /// Bars done and bars in all of the running render; `None` when none runs.
    pub(crate) fn realtime_progress(&self) -> Option<(i64, i64, f32)> {
        let r = self.finish.realtime.running.as_ref()?;
        #[cfg(test)]
        let done = self.finish.realtime.fake.as_ref().map(|f| f.done);
        #[cfg(not(test))]
        let done: Option<u64> = None;
        let done = done.or_else(|| self.player.as_ref().and_then(|p| p.realtime_render_progress()).map(|p| p.done)).unwrap_or(0);
        let s = self.engine.session();
        let bar = |at: i64| s.tempo.bar_beat_at(at.max(0), s.sample_rate).bar;
        let first = bar(r.range.start);
        let of = bar((r.range.end - 1).max(r.range.start)) - first + 1;
        let at = r.range.start.saturating_add(i64::try_from(done).unwrap_or(0)).min(r.range.end - 1);
        let fraction = (done as f64 / r.range.len().max(1) as f64).clamp(0.0, 1.0) as f32;
        Some(((bar(at) - first + 1).clamp(1, of.max(1)), of.max(1), fraction))
    }

    fn poll_realtime(&mut self) -> Option<Result<Rendered, RealtimeError>> {
        #[cfg(test)]
        if let Some(f) = self.finish.realtime.fake.as_mut() {
            return f.result.take();
        }
        let Some(player) = self.player.as_ref() else { return Some(Err(RealtimeError::DeviceLost)) };
        if let Some(done) = player.realtime_render_result() {
            return Some(done);
        }
        if !player.realtime_render_running() {
            // Another Player (a device reopen) has no render: the old one removed its files.
            return Some(Err(RealtimeError::Sink(CANCELLED_REOPEN.into())));
        }
        None
    }

    /// Per frame: collect a finished real-time render and say it.
    pub(crate) fn step_realtime(&mut self) {
        let Some(r) = self.finish.realtime.running.clone() else { return };
        let Some(done) = self.poll_realtime() else { return };
        self.finish.realtime.running = None;
        self.status = match done {
            Ok(d) => {
                let secs = self.engine.session().sample_rate.seconds(r.range.len());
                let peak = transit_dsp::gain_to_db(d.peak);
                let path = r.path.to_string_lossy().into_owned();
                self.finish.layout.last_bounce = Some(
                    json!({"path": path, "seconds": secs, "peak_db": peak, "realtime": true, "stems": r.stems, "uncompensated": r.uncompensated}),
                );
                let stems = if r.stems.is_empty() {
                    String::new()
                } else {
                    format!(" and {} stem{}", r.stems.len(), if r.stems.len() == 1 { "" } else { "s" })
                };
                let late = if r.uncompensated.is_empty() { String::new() } else { format!(" Uncompensated: {}.", r.uncompensated.join(", ")) };
                format!("Rendered in real time to {path}{stems} ({secs:.1} s, peak {peak:.1} dBFS).{late}")
            }
            Err(RealtimeError::Sink(m)) if m == CANCELLED_REOPEN => m,
            Err(e) => e.to_string(),
        };
        if matches!(self.dialog, Some(crate::Dialog::Bounce)) {
            self.dialog = None;
        }
    }

    /// Cancel the running real-time render: the Player stops, releases the synths' notes and
    /// removes the partial files before this returns.
    pub(crate) fn cancel_realtime(&mut self) {
        if self.finish.realtime.running.take().is_none() {
            return;
        }
        #[cfg(test)]
        if let Some(f) = self.finish.realtime.fake.as_mut() {
            f.cancelled += 1;
            self.status = CANCELLED.into();
            return;
        }
        if let Some(p) = &self.player {
            p.cancel_realtime_render();
        }
        self.status = CANCELLED.into();
    }
}

#[cfg(test)]
mod tests;
