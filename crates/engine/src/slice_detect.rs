//! Sampler slice detection off the control thread (ADR 0041 "Slice sources and Slice to MIDI" and
//! its amendment 2026-11-08, items 5 and 7; #269): the candidate marker set that
//! `mix.sampler_slice_detect` prepares and `mix.sampler_slice_replace` accepts.
//!
//! - **Off the control thread, never the audio thread.** Detect hands a [`Job`] (the loop region,
//!   the source's decoded audio as an `Arc`, the WAV file to read, the settings and the session
//!   tempo at the placement, all read where commands run) to a worker thread. The worker never
//!   touches the engine or the document, only the candidate map behind one mutex, which the audio
//!   thread never sees.
//! - **One candidate per track.** A newer Detect cancels the older one (its flag is checked
//!   between the analysis steps and before and after the transient detector, which runs whole and
//!   unchanged) and replaces it; a late result of a canceled job is dropped.
//! - **Stale.** A candidate keeps the patch it was detected from. [`Candidates::observe`] runs after
//!   every document change (commands, undo, redo): a changed patch marks the candidate stale for
//!   good, a removed or replaced instrument discards it. Replace also compares the patch itself.
//! - **Never in the document.** Candidates live on the engine: never saved, autosaved or put in
//!   the undo history, and emptied when the session is replaced (new, open, close, revert).
//! - **Deterministic.** The same audio, file and settings give the same markers: the analysis
//!   is plain arithmetic on the inputs, independent of thread timing.

use crate::cmd::{AtTempo, TempoOrigin, frames_at, grid_tempo, usable_acid};
use crate::file_access::AutomationRoots;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use transit_audio_io::{BadChunkReason, CueDiscardReason, MAX_CUE_POINTS, wav_cues, wav_tempo};
use transit_model::sampler_patch::{AutoSource, Division, MAX_SLICES, PatchV1, SliceLoop};
use transit_model::slicing::{GridError, grid_boundaries};
use transit_model::{Session, SourceAudio, TrackId};

/// Replace on a candidate detected before the patch last changed.
pub const STALE: &str = "The patch changed after this detection: detect again.";
/// Replace on a candidate whose analysis has not finished.
pub const PENDING: &str = "Still analyzing.";

/// The refusal for a result of more than [`MAX_SLICES`] slices (never truncated).
pub(crate) fn over_capacity(slices: usize) -> String {
    format!("over capacity: {slices} slices, at most {MAX_SLICES}")
}

/// The detector and grid settings a candidate was made with. They travel with it and Replace
/// stores them as the loop's.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    pub source: AutoSource,
    pub sensitivity: f32,
    pub division: Division,
    pub bars: Option<u8>,
    pub grid_bpm: Option<f64>,
}

impl Settings {
    /// The loop's stored settings.
    pub(crate) fn stored(l: &SliceLoop) -> Settings {
        Settings { source: l.detector.source, sensitivity: l.detector.sensitivity, division: l.division, bars: l.bars, grid_bpm: l.grid_bpm }
    }
}

/// A WAV file to read cue points and the `acid` tempo from, with the automation roots in force
/// where Detect ran.
pub(crate) struct FileRef {
    pub path: PathBuf,
    pub roots: Option<Arc<AutomationRoots>>,
}

/// Everything one detection reads, gathered where commands run.
pub(crate) struct Job {
    /// The loop region `[start, end)` in frames of the decoded audio.
    pub start: u64,
    pub end: u64,
    /// The source's decoded audio (at the session rate the source was loaded at).
    pub audio: Arc<SourceAudio>,
    /// The source's WAV file; `None` for audio without one (in memory, not a WAV file).
    pub file: Option<FileRef>,
    pub settings: Settings,
    /// The session tempo and meter at the placement `at`, when one was given.
    pub at: Option<AtTempo>,
}

/// A finished detection.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Found {
    /// The source that gave the markers (never `Auto`).
    pub source_used: AutoSource,
    /// The slice starts, strictly increasing, S first, each with its label.
    pub markers: Vec<(u64, Option<String>)>,
    /// Load notes for the user.
    pub notes: Vec<String>,
    /// The grid's tempo and its origin, when the grid gave the markers.
    pub tempo: Option<(f64, TempoOrigin)>,
}

/// Why an analysis stopped without a result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Stop {
    /// A newer Detect, a discard or a session change canceled it.
    Cancelled,
    Refused(String),
}

fn check(cancel: &AtomicBool) -> Result<(), Stop> {
    if cancel.load(Ordering::Relaxed) { Err(Stop::Cancelled) } else { Ok(()) }
}

/// The source file's bytes, read once and only when a step needs them.
struct FileBytes<'a> {
    file: Option<&'a FileRef>,
    read: Option<Vec<u8>>,
}

impl FileBytes<'_> {
    /// The bytes, `None` without a file; a file that cannot be read refuses the detection.
    fn get(&mut self) -> Result<Option<&[u8]>, Stop> {
        let Some(f) = self.file else { return Ok(None) };
        if self.read.is_none() {
            let read = || crate::io::read_file(&f.path);
            let bytes = match &f.roots {
                Some(r) => crate::file_access::confine(Some(r), read),
                None => crate::file_access::unconfined(read),
            }
            .map_err(|e| Stop::Refused(e.to_string()))?;
            self.read = Some(bytes);
        }
        Ok(self.read.as_deref())
    }
}

/// The detection of `job`: `auto` takes the first nonempty result of cues, transients, then the
/// grid; an explicit source gives its own result, empty or not. A result over capacity refuses.
pub(crate) fn analyze(job: &Job, cancel: &AtomicBool) -> Result<Found, Stop> {
    check(cancel)?;
    let mut file = FileBytes { file: job.file.as_ref(), read: None };
    let mut notes = Vec::new();
    let found = |source_used, markers, tempo, notes| Found { source_used, markers, notes, tempo };
    match job.settings.source {
        AutoSource::Cues => {
            let m = cues(job, &mut file, &mut notes, true)?;
            Ok(found(AutoSource::Cues, m, None, notes))
        }
        AutoSource::Transients => {
            let m = transients(job, cancel)?;
            Ok(found(AutoSource::Transients, m, None, notes))
        }
        AutoSource::Grid => {
            let (m, tempo) = grid(job, &mut file, &mut notes)?;
            Ok(found(AutoSource::Grid, m, Some(tempo), notes))
        }
        AutoSource::Auto => {
            let m = cues(job, &mut file, &mut notes, false)?;
            if m.len() > 1 {
                return Ok(found(AutoSource::Cues, m, None, notes));
            }
            check(cancel)?;
            let m = transients(job, cancel)?;
            if m.len() > 1 {
                return Ok(found(AutoSource::Transients, m, None, notes));
            }
            check(cancel)?;
            let (m, tempo) = grid(job, &mut file, &mut notes)?;
            Ok(found(AutoSource::Grid, m, Some(tempo), notes))
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The file's cue points inside the loop, S first: each frame scaled once from the file's rate to
/// the decoded audio's rate (`frames_at`'s rounding), kept inside the open interval `(start, end)`
/// (a cue at `start` labels S, one at `end` is the end boundary), one per frame (the first wins).
/// `explicit`: the user chose cues, so an empty result says why.
fn cues(job: &Job, file: &mut FileBytes<'_>, notes: &mut Vec<String>, explicit: bool) -> Result<Vec<(u64, Option<String>)>, Stop> {
    let (start, end) = (job.start, job.end);
    let mut out: Vec<(u64, Option<String>)> = vec![(start, None)];
    let Some(bytes) = file.get()? else {
        if explicit {
            notes.push("This source has no WAV file to read cue points from.".into());
        }
        return Ok(out);
    };
    let read = wav_cues(bytes);
    let to = job.audio.buffer.sample_rate;
    let from = if read.cues.is_empty() {
        to
    } else {
        transit_audio_io::probe(bytes, Some("wav")).map_err(|e| Stop::Refused(format!("the source file cannot be read: {e}")))?.sample_rate
    };
    let mut outside = read.discarded.iter().filter(|d| d.reason == CueDiscardReason::OutOfRange).count();
    let mut s_seen = false;
    for c in &read.cues {
        match frames_at(c.frame, from, to) {
            Some(f) if f == start => {
                if !s_seen {
                    s_seen = true;
                    out[0].1.clone_from(&c.label);
                }
            }
            Some(f) if f > start && f < end => {
                if out.last().is_some_and(|l| l.0 < f) {
                    out.push((f, c.label.clone()));
                }
            }
            Some(f) if f == end => {}
            _ => outside += 1,
        }
    }
    if outside > 0 {
        notes.push(format!("{} outside the loop left out.", plural(outside, "cue point", "cue points")));
    }
    if read.bad_chunks.iter().any(|b| b.reason == BadChunkReason::TooMany) {
        notes.push(format!("The file holds more than {MAX_CUE_POINTS} cue points; only the first {MAX_CUE_POINTS} were read."));
    }
    let malformed = read.bad_chunks.iter().filter(|b| b.reason != BadChunkReason::TooMany).count();
    if malformed > 0 {
        notes.push(format!(
            "{} read as far as {} go.",
            plural(malformed, "malformed cue chunk", "malformed cue chunks"),
            if malformed == 1 { "it" } else { "they" }
        ));
    }
    if out.len() > MAX_SLICES {
        // Every cue read counts, so the refusal carries the notes (a file past the reader's limit
        // says so here).
        let mut m = over_capacity(out.len());
        for n in notes.iter() {
            m.push(' ');
            m.push_str(n);
        }
        return Err(Stop::Refused(m));
    }
    if explicit && out.len() == 1 {
        notes.push("No usable cue points in this file.".into());
    }
    Ok(out)
}

/// `detect_transients` (unchanged, run whole) on the loop region of every channel, S added, kept
/// inside `(start, end)`, one per frame.
fn transients(job: &Job, cancel: &AtomicBool) -> Result<Vec<(u64, Option<String>)>, Stop> {
    check(cancel)?;
    let (start, end) = (job.start, job.end);
    let at = |f: u64| usize::try_from(f).unwrap_or(usize::MAX);
    let region: Vec<Vec<f32>> =
        job.audio.buffer.channels.iter().map(|c| c.get(at(start).min(c.len())..at(end).min(c.len())).unwrap_or(&[]).to_vec()).collect();
    let rate = job.audio.buffer.sample_rate as f32;
    let found = transit_dsp::offline::detect_transients(&region, rate, job.settings.sensitivity);
    check(cancel)?;
    let mut out: Vec<(u64, Option<String>)> = vec![(start, None)];
    for p in found {
        let f = start.saturating_add(p as u64);
        if f > start && f < end && out.last().is_some_and(|l| l.0 < f) {
            out.push((f, None));
        }
    }
    if out.len() > MAX_SLICES {
        return Err(Stop::Refused(over_capacity(out.len())));
    }
    Ok(out)
}

/// The beat grid at the tempo of item 5's priority chain: an explicit Bars count, else `grid_bpm`,
/// else the file's usable `acid` tempo, else the session tempo at the placement (refused without
/// one). The meter is the session's at the placement (only a bar count reads it).
fn grid(job: &Job, file: &mut FileBytes<'_>, notes: &mut Vec<String>) -> Result<(Vec<(u64, Option<String>)>, (f64, TempoOrigin)), Stop> {
    let s = &job.settings;
    let (start, end) = (job.start, job.end);
    // The file is read for its tempo only when nothing before it in the chain is set.
    let acid = if s.bars.is_none() && s.grid_bpm.is_none() { file.get()?.and_then(wav_tempo) } else { None };
    let l = SliceLoop { start, end, bars: s.bars, grid_bpm: s.grid_bpm, ..SliceLoop::default() };
    let rate = job.audio.buffer.sample_rate;
    let (bpm, origin) = grid_tempo(&l, rate, acid, job.at).map_err(|m| Stop::Refused(m.to_string()))?;
    if origin == TempoOrigin::Session
        && let Some(a) = acid.filter(|a| usable_acid(Some(*a)).is_none())
    {
        notes.push(format!("The file's acid tempo of {a:.2} BPM is outside 5 to 1000 BPM, so the session tempo is used."));
    }
    let meter = job.at.map_or((4, 4), |a| a.meter);
    match grid_boundaries(start, end, rate, bpm, s.division, meter, s.bars.map(u32::from)) {
        Ok(g) => {
            let starts = g.boundaries.len().saturating_sub(1);
            Ok((g.boundaries.into_iter().take(starts).map(|f| (f, None)).collect(), (bpm, origin)))
        }
        Err(GridError::OverCapacity) => {
            // The count for the refusal text: divisions of the loop at this tempo (the refusal
            // itself is the grid's own decision, so the count is at least one over the limit).
            let step = f64::from(rate) * 240.0 / (bpm * f64::from(s.division.per_whole()));
            let n = ((end - start) as f64 / step).ceil();
            let n = if n.is_finite() && n < usize::MAX as f64 { n as usize } else { usize::MAX };
            Err(Stop::Refused(over_capacity(n.max(MAX_SLICES + 1))))
        }
        Err(e) => Err(Stop::Refused(e.to_string())),
    }
}

/// Where a candidate is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum State {
    Pending,
    Ready(Found),
    Refused(String),
}

/// A track's candidate.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub id: u64,
    pub settings: Settings,
    /// The Auto set's slice count when Detect ran.
    pub current_count: usize,
    /// The patch Detect read.
    pub patch: PatchV1,
    /// The patch changed since (sticky: changing it back does not make the candidate fresh).
    pub stale: bool,
    pub state: State,
    cancel: Arc<AtomicBool>,
}

type Map = BTreeMap<TrackId, Candidate>;

fn lock(m: &Mutex<Map>) -> MutexGuard<'_, Map> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How a job runs: on a worker thread, or (crate tests) on the calling thread or held until the
/// test runs it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    Thread,
    Inline,
    Held,
}

/// A held job (crate tests).
#[cfg(test)]
pub(crate) struct Held {
    pub track: TrackId,
    pub id: u64,
    job: Job,
    cancel: Arc<AtomicBool>,
}

/// The engine's detection candidates, one per track at most.
pub struct Candidates {
    shared: Arc<Mutex<Map>>,
    /// The next candidate id; never reset, so an id from before a session change is never reused.
    next: u64,
    /// Jobs started since the engine was made (a test seam: opening a session starts none).
    started: u64,
    #[cfg(test)]
    run: Run,
    #[cfg(test)]
    held: Vec<Held>,
    #[cfg(test)]
    handles: Vec<std::thread::JoinHandle<()>>,
    /// The name of each worker thread a job ran on (crate tests).
    #[cfg(test)]
    threads: Arc<Mutex<Vec<Option<String>>>>,
}

/// The worker threads' name.
pub const WORKER: &str = "transit-slice-detect";

impl Default for Candidates {
    fn default() -> Self {
        Candidates {
            shared: Arc::default(),
            next: 1,
            started: 0,
            #[cfg(test)]
            run: Run::Inline,
            #[cfg(test)]
            held: Vec::new(),
            #[cfg(test)]
            handles: Vec::new(),
            #[cfg(test)]
            threads: Arc::default(),
        }
    }
}

impl Drop for Candidates {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Stores a finished job's outcome on its candidate, unless that candidate was replaced, discarded
/// or canceled meanwhile (the outcome is dropped then).
fn finish(shared: &Mutex<Map>, track: TrackId, id: u64, outcome: Result<Found, Stop>) {
    let mut g = lock(shared);
    let Some(c) = g.get_mut(&track).filter(|c| c.id == id && !c.cancel.load(Ordering::Relaxed)) else { return };
    match outcome {
        Ok(f) => c.state = State::Ready(f),
        Err(Stop::Refused(m)) => c.state = State::Refused(m),
        Err(Stop::Cancelled) => {}
    }
}

/// Runs one job, catching a panic as a refusal.
fn work(shared: &Mutex<Map>, track: TrackId, id: u64, job: &Job, cancel: &AtomicBool) -> Result<Found, Stop> {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| analyze(job, cancel)))
        .unwrap_or_else(|_| Err(Stop::Refused("internal error while detecting".into())));
    finish(shared, track, id, outcome.clone());
    outcome
}

impl Candidates {
    /// Starts detecting for `track` and returns the new candidate's id; an older candidate of the
    /// track is canceled and replaced. `patch` is the patch Detect read.
    pub(crate) fn start(&mut self, track: TrackId, patch: PatchV1, settings: Settings, current_count: usize, job: Job) -> u64 {
        let id = self.next;
        self.next += 1;
        self.started += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let c = Candidate { id, settings, current_count, patch, stale: false, state: State::Pending, cancel: Arc::clone(&cancel) };
        if let Some(old) = lock(&self.shared).insert(track, c) {
            old.cancel.store(true, Ordering::Relaxed);
        }
        #[cfg(test)]
        match self.run {
            Run::Inline => {
                let _ = work(&self.shared, track, id, &job, &cancel);
                return id;
            }
            Run::Held => {
                self.held.push(Held { track, id, job, cancel });
                return id;
            }
            Run::Thread => {}
        }
        let shared = Arc::clone(&self.shared);
        #[cfg(test)]
        let threads = Arc::clone(&self.threads);
        let spawned = std::thread::Builder::new().name(WORKER.into()).spawn({
            let cancel = Arc::clone(&cancel);
            move || {
                #[cfg(test)]
                threads.lock().unwrap_or_else(PoisonError::into_inner).push(std::thread::current().name().map(str::to_string));
                let _ = work(&shared, track, id, &job, &cancel);
            }
        });
        match spawned {
            #[cfg(test)]
            Ok(h) => self.handles.push(h),
            #[cfg(not(test))]
            Ok(_) => {}
            Err(err) => {
                log::warn!("slice detection worker not started ({err})");
                finish(&self.shared, track, id, Err(Stop::Refused(format!("the detection could not start: {err}"))));
            }
        }
        id
    }

    /// A copy of `track`'s candidate, with `stale` also set when `current` (the track's patch now)
    /// differs from the one Detect read.
    pub(crate) fn get(&self, track: TrackId, current: Option<&PatchV1>) -> Option<Candidate> {
        let mut c = lock(&self.shared).get(&track).cloned()?;
        c.stale |= current != Some(&c.patch);
        Some(c)
    }

    /// Whether `id` was ever handed out.
    pub(crate) fn issued(&self, id: u64) -> bool {
        (1..self.next).contains(&id)
    }

    /// Drops `track`'s candidate (only the one with `id`, when given) and cancels its work: the
    /// id dropped.
    pub(crate) fn discard(&mut self, track: TrackId, id: Option<u64>) -> Option<u64> {
        let mut g = lock(&self.shared);
        let c = g.get(&track).filter(|c| id.is_none_or(|i| i == c.id))?;
        c.cancel.store(true, Ordering::Relaxed);
        let id = c.id;
        g.remove(&track);
        Some(id)
    }

    /// After a document change: a candidate whose track's patch differs from the one it was
    /// detected from becomes stale; one whose track lost its Sampler patch is discarded.
    pub(crate) fn observe(&mut self, s: &Session) {
        let mut g = lock(&self.shared);
        if g.is_empty() {
            return;
        }
        g.retain(|t, c| match crate::cmd::ready_sampler_patch(s, *t) {
            None => {
                c.cancel.store(true, Ordering::Relaxed);
                false
            }
            Some(p) => {
                c.stale |= *p != c.patch;
                true
            }
        });
    }

    /// Drops every candidate (the session was replaced).
    pub fn clear(&mut self) {
        let mut g = lock(&self.shared);
        for c in g.values() {
            c.cancel.store(true, Ordering::Relaxed);
        }
        g.clear();
    }

    /// Detection jobs started since the engine was made.
    pub fn workers_started(&self) -> u64 {
        self.started
    }

    /// How later jobs run (crate tests).
    #[cfg(test)]
    pub(crate) fn set_run_for_testing(&mut self, run: Run) {
        self.run = run;
    }

    /// The held jobs, oldest first (crate tests).
    #[cfg(test)]
    pub(crate) fn take_held_for_testing(&mut self) -> Vec<Held> {
        std::mem::take(&mut self.held)
    }

    /// Runs a held job on the calling thread, as its worker would (crate tests).
    #[cfg(test)]
    pub(crate) fn run_held_for_testing(&self, h: &Held) -> Result<Found, Stop> {
        work(&self.shared, h.track, h.id, &h.job, &h.cancel)
    }

    /// Waits for every worker thread started so far (crate tests; joins, never sleeps) and returns
    /// the names of the threads the jobs ran on.
    #[cfg(test)]
    pub(crate) fn join_for_testing(&mut self) -> Vec<Option<String>> {
        for h in std::mem::take(&mut self.handles) {
            let _ = h.join();
        }
        std::mem::take(&mut *self.threads.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
