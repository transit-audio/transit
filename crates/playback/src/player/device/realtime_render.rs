//! The Player's side of a real-time render (ADR 0027): refuse what cannot render, arm the tap,
//! start the writer thread and play from the pre-roll; then, polled by the app, stop at the end,
//! and on Cancel, a failure or a device loss stop and release every External Instrument's notes
//! through the MIDI output thread. The audio thread does what live playback does plus the tap's
//! copy ([`crate::realtime_render`]).
//!
//! The render's transport requests are the caller's to track (ADR 0062 section 11): the start
//! returns the Play's sequence, and the job keeps every Stop it submits from the moment it submits
//! it, records a Stop it could not submit as owed, and reports each once, on the poll after it
//! (with or without a result) or in what a cancel or a disposal returns.

use super::{Player, PlayerError};
use crate::action_lane::Sequence;
use crate::realtime_render::{RealtimeError, RealtimeSpec, RenderProgress, RenderReport, RenderSink, RenderStops, Rendered, TapState};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use transit_model::TrackId;

type Writer = std::thread::JoinHandle<Result<Rendered, RealtimeError>>;

/// A real-time render under way.
pub(super) struct Job {
    tracks: Vec<TrackId>,
    len: u64,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    writer: Option<Writer>,
    /// The transport was stopped (the end, or a failure).
    stopped: bool,
    lost: bool,
    /// Every Stop the job submitted, in order; the first `reported` were reported.
    stops: Vec<Sequence>,
    reported: usize,
    /// A Stop the job could not submit, not yet reported.
    owed: bool,
}

impl Job {
    fn new(tracks: Vec<TrackId>, len: u64, cancel: Arc<AtomicBool>, done: Arc<AtomicU64>, writer: Writer) -> Self {
        Self { tracks, len, cancel, done, writer: Some(writer), stopped: false, lost: false, stops: Vec::new(), reported: 0, owed: false }
    }

    /// Keep a Stop's submission: its sequence, or an owed Stop when the Player refused it without
    /// a fault (a fault retires every request instead).
    fn stopped_with(&mut self, stop: Result<Sequence, PlayerError>) {
        match stop {
            Ok(sequence) => self.stops.push(sequence),
            Err(PlayerError::Faulted) => {}
            Err(_) => self.owed = true,
        }
    }

    /// The Stops submitted and owed since the last report; each is reported once.
    fn report(&mut self) -> RenderStops {
        let submitted = self.stops.get(self.reported..).unwrap_or_default().to_vec();
        self.reported = self.stops.len();
        RenderStops { submitted, owed: std::mem::take(&mut self.owed) }
    }

    fn writer_finished(&self) -> bool {
        self.writer.as_ref().is_some_and(std::thread::JoinHandle::is_finished)
    }

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
    /// Returns the render Play's sequence (ADR 0062 section 11). Control side only.
    pub fn start_realtime_render(&self, spec: &RealtimeSpec, sink: Box<dyn RenderSink>) -> Result<Sequence, String> {
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
        self.owner().and_then(|mut o| o.full_chain(true)).map_err(|e| format!("full render chain could not be requested: {e:?}"))?;
        if !tap.arm(spec.start, spec.len, reach, spec.main_channels, &ret) {
            let _ = self.owner().and_then(|mut o| o.full_chain(false));
            return Err("the render has no range, or more channels than a render keeps".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let writer = match crate::realtime_render::spawn(Arc::clone(tap), spec, sink, Arc::clone(&cancel), Arc::clone(&done)) {
            Ok(writer) => writer,
            Err(e) => {
                tap.cancel();
                let _ = self.owner().and_then(|mut o| o.full_chain(false));
                return Err(format!("the file writer could not start: {e}"));
            }
        };
        let mut job = Job::new(spec.stems.iter().map(|s| s.track).collect(), spec.len, cancel, done, writer);
        let play = match self.play(spec.start.saturating_sub(spec.preroll.max(0)), None, None) {
            Ok(play) => play,
            Err(e) => {
                tap.cancel();
                let _ = job.abandon();
                let _ = self.owner().and_then(|mut o| o.full_chain(false));
                return Err(format!("playback could not start: {e:?}"));
            }
        };
        if let Ok(mut slot) = self.render_job.try_borrow_mut() {
            *slot = Some(job);
            Ok(play)
        } else {
            tap.cancel();
            let _ = self.owner().and_then(|mut o| o.full_chain(false));
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
    /// Sustain off, All Notes Off on its port, through the output thread). The Stop's submission.
    fn stop_and_release(&self, tracks: &[TrackId]) -> Result<Sequence, PlayerError> {
        let stop = self.stop();
        self.release(tracks);
        stop
    }

    fn release(&self, tracks: &[TrackId]) {
        for t in tracks {
            self.release_external(*t);
        }
    }

    /// The one poll of a running render, each step: stops the transport when the capture is
    /// complete; a failure or a device loss stops it and releases the External Instruments' notes,
    /// and nothing is written. Reports the Stops submitted or owed since the last report even when
    /// no result is ready, and the render's outcome once its writer is done (then the job is
    /// taken: a later poll or cancel finds none). Empty when no render runs.
    pub fn realtime_render_poll(&self) -> RenderReport {
        self.poll_render().unwrap_or_default()
    }

    fn poll_render(&self) -> Option<RenderReport> {
        let tap = self.render_tap.as_ref()?;
        let mut slot = self.render_job.try_borrow_mut().ok()?;
        let job = slot.as_mut()?;
        if !job.lost && (self.fault.gone.load(Ordering::Acquire) || self.fault.failed.load(Ordering::Acquire)) {
            job.lost = true;
            tap.cancel();
            job.cancel.store(true, Ordering::Release);
            self.release(&job.tracks);
        }
        let state = tap.state();
        if !job.stopped && matches!(state, TapState::Captured | TapState::Failed(_)) {
            job.stopped = true;
            let tracks = job.tracks.clone();
            let stop = if matches!(state, TapState::Failed(_)) { self.stop_and_release(&tracks) } else { self.stop() };
            // Kept from the moment it is submitted, before the unfinished-writer return below.
            job.stopped_with(stop);
        }
        let stops = job.report();
        if !job.writer_finished() {
            return Some(RenderReport { stops, result: None });
        }
        let Some(mut job) = slot.take() else { return Some(RenderReport { stops, result: None }) };
        let lost = job.lost;
        let result = job.abandon();
        let _ = self.owner().and_then(|mut o| o.full_chain(false));
        let Some(result) = result else { return Some(RenderReport { stops, result: None }) };
        tap.cancel();
        Some(RenderReport { stops, result: Some(if lost { Err(RealtimeError::DeviceLost) } else { result }) })
    }

    /// Cancel the render (Cancel, the window closing, a device reopen): playback stops, every
    /// External Instrument of the render is released through the output thread, and the part
    /// files are removed before this returns. Returns the Stops not reported before (this
    /// cancel's own included, or owed) and, when the writer had already finished, its own result:
    /// the files it committed stay. Empty when no render runs (a poll collected it first).
    pub fn cancel_realtime_render(&self) -> RenderReport {
        self.end_render(true)
    }

    /// Dispose of the render after the Player refused its Play: as [`Self::cancel_realtime_render`]
    /// (the part files removed, the External Instruments released), without a Stop.
    pub fn dispose_realtime_render(&self) -> RenderReport {
        self.end_render(false)
    }

    fn end_render(&self, stop: bool) -> RenderReport {
        let Some(mut job) = self.render_job.try_borrow_mut().ok().and_then(|mut j| j.take()) else { return RenderReport::default() };
        // Read before the tap's cancel, which ends a writer still running.
        let finished = job.writer_finished();
        if let Some(tap) = &self.render_tap {
            tap.cancel();
        }
        if stop {
            let submitted = self.stop_and_release(&job.tracks);
            job.stopped_with(submitted);
        } else {
            self.release(&job.tracks);
        }
        let lost = job.lost;
        let result = job.abandon().filter(|_| finished).map(|r| if lost { Err(RealtimeError::DeviceLost) } else { r });
        let _ = self.owner().and_then(|mut o| o.full_chain(false));
        RenderReport { stops: job.report(), result }
    }

    /// The running render's writer has finished (tests: a cancel after the writer is done).
    #[cfg(test)]
    pub(super) fn realtime_writer_finished(&self) -> bool {
        self.render_job.try_borrow().is_ok_and(|j| j.as_ref().is_some_and(Job::writer_finished))
    }
}
