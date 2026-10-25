//! The Player's side of a real-time render (ADR 0027): refuse what cannot render, arm the tap,
//! start the writer thread and play from the pre-roll; then, polled by the app, stop at the end,
//! and on Cancel, a failure or a device loss stop and release every External Instrument's notes
//! through the MIDI output thread. The audio thread does what live playback does plus the tap's
//! copy ([`crate::realtime_render`]).

use super::Player;
use crate::realtime_render::{RealtimeError, RealtimeSpec, RenderProgress, RenderSink, Rendered, TapState};
use soundcraft_model::TrackId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A real-time render under way.
pub(super) struct Job {
    tracks: Vec<TrackId>,
    len: u64,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    writer: Option<std::thread::JoinHandle<Result<Rendered, RealtimeError>>>,
    /// The transport was stopped (the end, or a failure).
    stopped: bool,
    lost: bool,
}

impl Job {
    /// Stop the writer (its sink removes the part files) and wait for it.
    fn abandon(&mut self) -> Option<Result<Rendered, RealtimeError>> {
        self.cancel.store(true, Ordering::Release);
        let writer = self.writer.take()?;
        writer.thread().unpark();
        Some(writer.join().unwrap_or_else(|_| Err(RealtimeError::Sink("the file writer panicked".into()))))
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let _ = self.abandon();
    }
}

impl Player {
    /// Start a real-time render of `spec` into `sink`: plays from `spec.start - spec.preroll` at
    /// speed 1 through the open device, sending each External Instrument's MIDI as playback does,
    /// and keeps Main and the returns. Poll [`Self::realtime_render_result`]. Refuses with no
    /// device, no live input (not duplex), a playing transport, a speed other than 1x, a
    /// measurement running, a render running, a stem whose MIDI port the output thread does not
    /// have open or whose return names a channel the device did not open, or too many channels.
    /// Control side only.
    pub fn start_realtime_render(&self, spec: &RealtimeSpec, sink: Box<dyn RenderSink>) -> Result<(), String> {
        let tap = self.render_tap.as_ref().filter(|_| !self.silent).ok_or("No audio device")?;
        let io = self.io_map();
        if !self.plan_input.get()
            && let Some(ring) = self.with_held_input(|recorder| Arc::clone(&recorder.monitor))
        {
            // The returns are read from the live input ring: the held input feeds the plan.
            self.set_input(ring).map_err(|e| format!("the held input could not feed the render ({e:?})"))?;
        }
        if !self.plan_input.get() || io.inputs == 0 {
            return Err("Needs a duplex device".into());
        }
        if self.is_playing() {
            return Err("Stop the transport to render in real time.".into());
        }
        if self.speed() != 1.0 {
            return Err("Set the playback speed to 1x to render in real time.".into());
        }
        if self.hardware_probe_running() || self.loop_calibration_running() || self.external_measure_running() {
            return Err("A measurement is running: wait for it or cancel it.".into());
        }
        if self.render_job.try_borrow().map_or(true, |j| j.is_some()) || tap.busy() {
            return Err("A real-time render is already running.".into());
        }
        let mut ret = Vec::new();
        for stem in &spec.stems {
            if stem.channels.is_empty() || stem.channels.iter().any(|c| *c >= io.inputs) {
                return Err("the return names a channel the device did not open".into());
            }
            self.external_port_open(stem.track)?;
            ret.extend_from_slice(&stem.channels);
        }
        let reach = spec.stems.iter().map(|s| u64::from(s.latency)).max().unwrap_or(0);
        if !tap.arm(spec.start, spec.len, reach, spec.main_channels, &ret) {
            return Err("the render has no range, or more channels than a render keeps".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let writer = match crate::realtime_render::spawn(Arc::clone(tap), spec, sink, Arc::clone(&cancel), Arc::clone(&done)) {
            Ok(writer) => writer,
            Err(e) => {
                tap.cancel();
                return Err(format!("the file writer could not start: {e}"));
            }
        };
        let mut job = Job {
            tracks: spec.stems.iter().map(|s| s.track).collect(),
            len: spec.len,
            cancel,
            done,
            writer: Some(writer),
            stopped: false,
            lost: false,
        };
        if let Err(e) = self.play(spec.start.saturating_sub(spec.preroll.max(0)), None, None) {
            tap.cancel();
            let _ = job.abandon();
            return Err(format!("playback could not start: {e:?}"));
        }
        if let Ok(mut slot) = self.render_job.try_borrow_mut() {
            *slot = Some(job);
            Ok(())
        } else {
            tap.cancel();
            Err("the Player is busy".into())
        }
    }

    /// Whether a real-time render is under way.
    pub fn realtime_render_running(&self) -> bool {
        self.render_job.try_borrow().is_ok_and(|j| j.is_some())
    }

    /// Mix frames written of the range; `None` when no render runs.
    pub fn realtime_render_progress(&self) -> Option<RenderProgress> {
        let job = self.render_job.try_borrow().ok()?;
        let job = job.as_ref()?;
        Some(RenderProgress { done: job.done.load(Ordering::Relaxed), of: job.len })
    }

    /// Stop the transport and release what every External Instrument of the render holds (note-off,
    /// Sustain off, All Notes Off on its port, through the output thread).
    fn stop_and_release(&self, tracks: &[TrackId]) {
        let _ = self.stop();
        for t in tracks {
            self.release_external(*t);
        }
    }

    /// The render's outcome once its writer is done (`None` while it runs, and after it was
    /// taken). Stops the transport when the capture is complete; a failure or a device loss
    /// stops it and releases the External Instruments' notes, and nothing is written.
    pub fn realtime_render_result(&self) -> Option<Result<Rendered, RealtimeError>> {
        let tap = self.render_tap.as_ref()?;
        let mut slot = self.render_job.try_borrow_mut().ok()?;
        let job = slot.as_mut()?;
        if !job.lost && (self.fault.gone.load(Ordering::Acquire) || self.fault.failed.load(Ordering::Acquire)) {
            job.lost = true;
            tap.cancel();
            job.cancel.store(true, Ordering::Release);
            for t in &job.tracks {
                self.release_external(*t);
            }
        }
        let state = tap.state();
        if !job.stopped && matches!(state, TapState::Captured | TapState::Failed(_)) {
            job.stopped = true;
            let tracks = job.tracks.clone();
            if matches!(state, TapState::Failed(_)) {
                self.stop_and_release(&tracks);
            } else {
                let _ = self.stop();
            }
        }
        if !job.writer.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            return None;
        }
        let mut job = slot.take()?;
        let lost = job.lost;
        let result = job.abandon()?;
        tap.cancel();
        Some(if lost { Err(RealtimeError::DeviceLost) } else { result })
    }

    /// Cancel the render (Cancel, the window closing, a device reopen): playback stops, every
    /// External Instrument of the render is released through the output thread, and the part
    /// files are removed before this returns. Nothing is written.
    pub fn cancel_realtime_render(&self) {
        let Some(mut job) = self.render_job.try_borrow_mut().ok().and_then(|mut j| j.take()) else { return };
        if let Some(tap) = &self.render_tap {
            tap.cancel();
        }
        self.stop_and_release(&job.tracks);
        let _ = job.abandon();
    }
}
