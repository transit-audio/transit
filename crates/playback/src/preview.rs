//! The browser's preview player (issue #211): a file from the Files tab plays before it is
//! dragged in, outside the document.
//!
//! Convention: Ableton and Logic agree, so transit follows them. Ableton's browser previews
//! through the Preview/Cue path with its own Preview Volume; Logic's browsers play through a
//! preview channel with its own volume. Neither puts the preview on a track: it does not record,
//! meter or route, and it never enters the document (no undo entry, no source). The preview stops
//! when another file is chosen, when the transport starts, on Escape, and at the end of the file.
//! Ableton's tempo-synced preview of warped clips is not built.
//!
//! The preview is not part of the plan. [`Preview`] (Main) hands each request to a feeder thread
//! that opens the file with [`transit_audio_io::BlockReader`], decodes it a block at a time,
//! converts it to the engine's rate and to stereo, and pushes fixed-size chunks into a lock-free
//! ring. [`PreviewVoice`] (the audio thread) drains the ring after the master section and adds the
//! chunks into Main at the preview volume: plain copies and atomics, no allocation, lock or
//! decode on the audio thread. A long file starts at once: only the ring's worth is decoded
//! ahead. Main has no cue output of its own yet (the I/O map names no cue path), so the preview
//! plays on Main's first two channels.
//!
//! Requests carry a generation: the voice plays only the latest one, drops the chunks of older
//! ones, and publishes what it did (sounding, ended, cut by the transport) for [`Preview::status`].

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use transit_audio_io::BlockReader;

/// Frames per chunk of the ring.
pub const CHUNK_FRAMES: usize = 256;
/// Chunks the ring holds: about 0.7 s at 48 kHz decoded ahead.
pub const RING_CHUNKS: usize = 128;
/// The preview volume's range in dB; at the floor the preview is silent.
pub const MIN_DB: f32 = -60.0;
pub const MAX_DB: f32 = 6.0;
/// The fade at a start, a stop, a switch and a cut: about 5 ms at `rate`.
pub fn fade_frames(rate: u32) -> usize {
    (rate as usize / 200).clamp(16, 1024)
}
/// Frames the feeder reads from the file at a time.
const READ_FRAMES: usize = 4096;

/// A chunk of converted stereo audio of one request.
#[derive(Clone, Copy)]
struct Chunk {
    generation: u64,
    frames: u16,
    /// The file ends with this chunk.
    last: bool,
    data: [f32; 2 * CHUNK_FRAMES],
}

impl Chunk {
    const EMPTY: Chunk = Chunk { generation: 0, frames: 0, last: false, data: [0.0; 2 * CHUNK_FRAMES] };
}

/// What Main, the feeder and the audio thread share: atomics only.
struct Shared {
    /// The latest request (play or stop), written by Main only. 0: none yet.
    wanted: AtomicU64,
    /// The preview volume (linear, `f32` bits), written by Main only.
    gain: AtomicU32,
    /// Written by the audio thread only: the request sounding now (0: none), the latest one it cut
    /// because the transport started, the latest one it played to the end, the frames of the
    /// sounding request played, and the blocks the ring ran dry mid-file.
    sounding: AtomicU64,
    cut: AtomicU64,
    ended: AtomicU64,
    played: AtomicU64,
    underruns: AtomicU64,
}

fn gain_of(db: f32) -> f32 {
    if !db.is_finite() || db <= MIN_DB { 0.0 } else { 10f32.powf(db.min(MAX_DB) / 20.0) }
}

enum Job {
    Play { generation: u64, path: PathBuf },
    Stop,
}

/// What the feeder found out about a request.
enum Report {
    Opened { generation: u64, file: FileFacts },
    Failed { generation: u64, error: String },
}

/// The file a preview plays, as its header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub rate: u32,
    pub channels: u16,
    /// 0 when the header does not say (an MP3 without a length frame).
    pub frames: u64,
}

/// Why a preview stopped by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewState {
    /// Nothing asked for (or stopped from Main).
    Idle,
    /// Opening and decoding the first chunks.
    Loading,
    Playing,
    /// It played to the end of the file.
    Ended,
    /// The transport started.
    Cut,
    /// The file could not be read (see [`PreviewStatus::error`]).
    Failed,
}

/// The preview as Main sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewStatus {
    pub path: Option<PathBuf>,
    pub state: PreviewState,
    pub file: Option<FileFacts>,
    /// Frames played at the engine's rate.
    pub played: u64,
    pub rate: u32,
    pub underruns: u64,
    pub error: Option<String>,
}

/// Main's side of the preview. See the module documentation.
pub struct Preview {
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    reports: Receiver<Report>,
    rate: u32,
    generation: Cell<u64>,
    path: RefCell<Option<PathBuf>>,
    file: Cell<Option<FileFacts>>,
    error: RefCell<Option<String>>,
    /// Tests: the feeder runs on the caller's thread, in [`Preview::pump`].
    manual: Option<RefCell<Feeder>>,
}

impl Preview {
    /// A preview for an engine running at `rate`, with its feeder thread; the voice goes to the
    /// audio thread.
    pub fn new(rate: u32) -> std::io::Result<(Preview, PreviewVoice)> {
        let (preview, voice, feeder) = Self::parts(rate);
        std::thread::Builder::new().name("transit-preview".into()).spawn(move || feeder.run())?;
        Ok((preview, voice))
    }

    /// A preview whose feeder runs only in [`Preview::pump`], on the caller's thread (tests).
    pub fn manual(rate: u32) -> (Preview, PreviewVoice) {
        let (mut preview, voice, feeder) = Self::parts(rate);
        preview.manual = Some(RefCell::new(feeder));
        (preview, voice)
    }

    fn parts(rate: u32) -> (Preview, PreviewVoice, Feeder) {
        let shared = Arc::new(Shared {
            wanted: AtomicU64::new(0),
            gain: AtomicU32::new(1.0f32.to_bits()),
            sounding: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            ended: AtomicU64::new(0),
            played: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        });
        let (ring_in, ring_out) = rtrb::RingBuffer::new(RING_CHUNKS);
        let (jobs, job_rx) = channel();
        let (report_tx, reports) = channel();
        let feeder = Feeder { shared: Arc::clone(&shared), jobs: job_rx, reports: report_tx, ring: ring_in, rate, current: None, gone: false };
        let voice = PreviewVoice {
            shared: Arc::clone(&shared),
            ring: ring_out,
            chunk: Box::new(Chunk::EMPTY),
            at: 0,
            held: false,
            generation: 0,
            cut: 0,
            ended: 0,
            started: false,
            played: 0,
            was_playing: false,
            fade_len: fade_frames(rate),
            fade_chunk: Box::new(Chunk::EMPTY),
            fade_at: 0,
            fade_left: 0,
            fade_generation: 0,
        };
        let preview = Preview {
            shared,
            jobs,
            reports,
            rate,
            generation: Cell::new(0),
            path: RefCell::new(None),
            file: Cell::new(None),
            error: RefCell::new(None),
            manual: None,
        };
        (preview, voice, feeder)
    }

    fn next_generation(&self) -> u64 {
        let g = self.generation.get().saturating_add(1);
        self.generation.set(g);
        // Published before the job is sent: the feeder and the voice never see a chunk newer than
        // the request they know of.
        self.shared.wanted.store(g, Ordering::Release);
        g
    }

    /// Play `path` from its start, replacing whatever previews now.
    pub fn play(&self, path: &Path) {
        let generation = self.next_generation();
        *self.path.borrow_mut() = Some(path.to_path_buf());
        self.file.set(None);
        *self.error.borrow_mut() = None;
        if self.jobs.send(Job::Play { generation, path: path.to_path_buf() }).is_err() {
            *self.error.borrow_mut() = Some("the preview thread is not running".into());
        }
    }

    /// Stop the preview (Escape, the stop button, another file chosen).
    pub fn stop(&self) {
        self.next_generation();
        *self.path.borrow_mut() = None;
        self.file.set(None);
        *self.error.borrow_mut() = None;
        let _ = self.jobs.send(Job::Stop);
    }

    /// The preview volume in dB ([`MIN_DB`] and below: silent). One atomic store.
    pub fn set_volume_db(&self, db: f32) {
        self.shared.gain.store(gain_of(db).to_bits(), Ordering::Relaxed);
    }

    /// The preview volume as a linear gain.
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.shared.gain.load(Ordering::Relaxed))
    }

    /// Tests: run the feeder on this thread until the ring is full or the request is done.
    pub fn pump(&self) {
        if let Some(feeder) = &self.manual {
            feeder.borrow_mut().pump();
        }
    }

    /// What the preview is doing now.
    pub fn status(&self) -> PreviewStatus {
        let g = self.generation.get();
        while let Ok(report) = self.reports.try_recv() {
            match report {
                Report::Opened { generation, file } if generation == g => self.file.set(Some(file)),
                Report::Failed { generation, error } if generation == g => *self.error.borrow_mut() = Some(error),
                _ => {}
            }
        }
        let path = self.path.borrow().clone();
        let error = self.error.borrow().clone();
        let load = |a: &AtomicU64| a.load(Ordering::Acquire);
        let state = if path.is_none() || g == 0 {
            PreviewState::Idle
        } else if load(&self.shared.cut) >= g {
            PreviewState::Cut
        } else if error.is_some() {
            PreviewState::Failed
        } else if load(&self.shared.ended) >= g {
            PreviewState::Ended
        } else if load(&self.shared.sounding) == g {
            PreviewState::Playing
        } else {
            PreviewState::Loading
        };
        let played = if state == PreviewState::Idle { 0 } else { load(&self.shared.played) };
        PreviewStatus { path, state, file: self.file.get(), played, rate: self.rate, underruns: load(&self.shared.underruns), error }
    }
}

/// One request being decoded.
struct Stream {
    generation: u64,
    reader: BlockReader,
    convert: Convert,
    block: Vec<Vec<f32>>,
    /// A chunk the ring had no room for yet.
    held: Option<Box<Chunk>>,
}

/// A request is live until Main asks for another one or the transport cut it.
fn live(shared: &Shared, generation: u64) -> bool {
    shared.wanted.load(Ordering::Acquire) == generation && shared.cut.load(Ordering::Acquire) < generation
}

/// The feeder: opens requests, decodes and converts them, fills the ring.
struct Feeder {
    shared: Arc<Shared>,
    jobs: Receiver<Job>,
    reports: Sender<Report>,
    ring: rtrb::Producer<Chunk>,
    rate: u32,
    current: Option<Stream>,
    /// Main dropped the preview.
    gone: bool,
}

impl Feeder {
    fn run(mut self) {
        loop {
            if self.current.is_none() {
                match self.jobs.recv() {
                    Ok(job) => self.start(job),
                    Err(_) => return,
                }
            }
            self.pump();
            if self.gone {
                return;
            }
            if self.current.is_some() {
                // The ring is full: the voice drains a chunk every few milliseconds.
                std::thread::park_timeout(std::time::Duration::from_millis(5));
            }
        }
    }

    fn start(&mut self, job: Job) {
        self.current = None;
        let Job::Play { generation, path } = job else { return };
        if !live(&self.shared, generation) {
            return;
        }
        let opened = std::fs::File::open(&path).and_then(|f| f.metadata().map(|m| (f, m.len()))).map_err(|e| e.to_string()).and_then(|(f, len)| {
            let ext = path.extension().and_then(|e| e.to_str());
            BlockReader::open(Box::new(f), len, ext).map_err(|e| e.to_string())
        });
        match opened {
            Ok(reader) => {
                let i = reader.info();
                let file = FileFacts { rate: i.sample_rate, channels: i.channels, frames: i.frames };
                let _ = self.reports.send(Report::Opened { generation, file });
                let convert = Convert::new(i.sample_rate, self.rate);
                self.current = Some(Stream { generation, reader, convert, block: Vec::new(), held: None });
            }
            Err(error) => {
                let _ = self.reports.send(Report::Failed { generation, error });
            }
        }
    }

    /// Take the newest job, then fill the ring with the current request until it is full or done.
    fn pump(&mut self) {
        loop {
            match self.jobs.try_recv() {
                Ok(job) => self.start(job),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.current = None;
                    self.gone = true;
                    return;
                }
            }
        }
        while let Some(stream) = self.current.as_mut() {
            let generation = stream.generation;
            if !live(&self.shared, generation) {
                self.current = None;
                return;
            }
            let chunk = match stream.held.take() {
                Some(chunk) => chunk,
                None => {
                    let mut chunk = Box::new(Chunk { generation, ..Chunk::EMPTY });
                    match stream.convert.fill(&mut stream.reader, &mut stream.block, &mut chunk.data) {
                        Ok((frames, last)) => {
                            chunk.frames = frames as u16;
                            chunk.last = last;
                        }
                        Err(error) => {
                            // End what plays so far; Main shows why.
                            chunk.frames = 0;
                            chunk.last = true;
                            let _ = self.reports.send(Report::Failed { generation, error });
                        }
                    }
                    chunk
                }
            };
            let last = chunk.last;
            match self.ring.push(*chunk) {
                Ok(()) if last => self.current = None,
                Ok(()) => {}
                Err(rtrb::PushError::Full(chunk)) => {
                    stream.held = Some(Box::new(chunk));
                    return;
                }
            }
        }
    }
}

/// The file's frames, converted to stereo at the engine's rate (linear interpolation; an exact
/// copy when the rates match). Mono plays on both sides; more channels play their first two.
struct Convert {
    /// The file's rate and the engine's: output frame `k` sits at source frame `k * from / to`,
    /// computed exactly in integers (no drift over a long file).
    from: u64,
    to: u64,
    /// Output frames made so far.
    made: u64,
    /// Source frames dropped from the front of `src`.
    dropped: u64,
    src: Vec<[f32; 2]>,
    eof: bool,
}

impl Convert {
    fn new(from: u32, to: u32) -> Convert {
        let (from, to) = if from == 0 || to == 0 { (1, 1) } else { (u64::from(from), u64::from(to)) };
        Convert { from, to, made: 0, dropped: 0, src: Vec::new(), eof: false }
    }

    fn refill(&mut self, reader: &mut BlockReader, block: &mut Vec<Vec<f32>>) -> Result<(), String> {
        let n = reader.read(block, READ_FRAMES).map_err(|e| e.to_string())?;
        if n == 0 {
            self.eof = true;
            return Ok(());
        }
        let at = |c: usize, i: usize| block.get(c).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0);
        let stereo = block.len() >= 2;
        self.src.extend((0..n).map(|i| if stereo { [at(0, i), at(1, i)] } else { [at(0, i); 2] }));
        Ok(())
    }

    /// The next output frame's source frame (in `src`) and its fraction toward the one after.
    fn position(&self) -> (usize, f32) {
        let num = u128::from(self.made) * u128::from(self.from);
        let whole = num / u128::from(self.to);
        let frac = (num % u128::from(self.to)) as f64 / self.to as f64;
        (usize::try_from(whole.saturating_sub(u128::from(self.dropped))).unwrap_or(usize::MAX), frac as f32)
    }

    /// Fill `out` (interleaved stereo) as far as the file goes: (frames, the file ended).
    fn fill(&mut self, reader: &mut BlockReader, block: &mut Vec<Vec<f32>>, out: &mut [f32]) -> Result<(usize, bool), String> {
        let want = out.len() / 2;
        let mut n = 0;
        while n < want {
            let (i, f) = self.position();
            while self.src.len() < i.saturating_add(2) && !self.eof {
                self.refill(reader, block)?;
            }
            let Some(&a) = self.src.get(i) else { break };
            let b = self.src.get(i + 1).copied().unwrap_or(a);
            out[2 * n] = a[0] + (b[0] - a[0]) * f;
            out[2 * n + 1] = a[1] + (b[1] - a[1]) * f;
            n += 1;
            self.made += 1;
            if i >= READ_FRAMES {
                // Drop what is behind the read position.
                self.src.drain(..i);
                self.dropped += i as u64;
            }
        }
        let ended = self.eof && self.position().0 >= self.src.len();
        Ok((n, ended))
    }
}

/// The audio thread's side of the preview. See the module documentation.
pub struct PreviewVoice {
    shared: Arc<Shared>,
    ring: rtrb::Consumer<Chunk>,
    /// The chunk being played (boxed: the voice moves with the audio state).
    chunk: Box<Chunk>,
    at: usize,
    held: bool,
    /// The request being played, the latest cut and the latest ended (mirrors of the shared ones).
    generation: u64,
    cut: u64,
    ended: u64,
    /// A chunk of the request has arrived (an empty ring before that is not an underrun).
    started: bool,
    played: u64,
    was_playing: bool,
    /// The fade's length in frames (about 5 ms at the engine's rate).
    fade_len: usize,
    /// What was playing when it was stopped, switched or cut: it fades out from `fade_at` in
    /// `fade_chunk` (and the chunks of `fade_generation` behind it) over `fade_left` more frames.
    fade_chunk: Box<Chunk>,
    fade_at: usize,
    fade_left: usize,
    fade_generation: u64,
}

/// Add `src` (interleaved stereo) into Main's first channels from frame `at`, each frame `k` at
/// `gain(k)`.
fn add(main: &mut [Vec<f32>], channels: usize, at: usize, src: &[f32], gain: impl Fn(usize) -> f32) {
    let frames = src.as_chunks::<2>().0;
    match main {
        [l, r, ..] if channels >= 2 => {
            for (k, (s, (dl, dr))) in frames.iter().zip(l.iter_mut().skip(at).zip(r.iter_mut().skip(at))).enumerate() {
                let g = gain(k);
                *dl += s[0] * g;
                *dr += s[1] * g;
            }
        }
        [m, ..] if channels == 1 => {
            for (k, (s, d)) in frames.iter().zip(m.iter_mut().skip(at)).enumerate() {
                *d += (s[0] + s[1]) * 0.5 * gain(k);
            }
        }
        _ => {}
    }
}

impl PreviewVoice {
    /// Audio thread, after the master section: add the preview into the first `channels`
    /// channels of `main` (Main's), `frames` of them. `transport` is whether the transport runs
    /// (a count-in included): when it starts, the preview stops. A stop, a new request or a cut
    /// fades what played out over about 5 ms, and a new request fades in, so neither clicks.
    /// Allocation-free and lock-free.
    pub fn mix(&mut self, main: &mut [Vec<f32>], channels: usize, frames: usize, transport: bool) {
        let wanted = self.shared.wanted.load(Ordering::Acquire);
        let mut cut_now = false;
        if transport && !self.was_playing && wanted > self.cut {
            self.cut = wanted;
            self.shared.cut.store(wanted, Ordering::Release);
            cut_now = true;
        }
        self.was_playing = transport;
        let switching = wanted != self.generation;
        let gain = f32::from_bits(self.shared.gain.load(Ordering::Relaxed));
        let gain = if gain.is_finite() { gain.clamp(0.0, 2.0) } else { 0.0 };
        if (switching || cut_now) && self.held && self.ended < self.generation {
            // Keep what was playing to fade it out; the new request gets the other buffer.
            std::mem::swap(&mut self.chunk, &mut self.fade_chunk);
            (self.fade_at, self.fade_left, self.fade_generation) = (self.at, self.fade_len, self.generation);
            self.held = false;
        }
        if self.fade_left > 0 {
            self.fade_out(main, channels, frames, gain);
        }
        if switching {
            self.generation = wanted;
            self.held = false;
            self.started = false;
            self.played = 0;
        }
        if wanted <= self.cut || wanted <= self.ended {
            // Nothing to play: free the ring of what is left of older requests (but what fades).
            if self.fade_left == 0 {
                self.skip(u64::MAX);
            }
            self.held = false;
            self.shared.sounding.store(0, Ordering::Release);
            return;
        }
        if self.fade_left == 0 {
            self.skip(wanted);
        }
        let mut i = 0;
        let mut pops = 0;
        while i < frames {
            if !self.held || self.at >= usize::from(self.chunk.frames) {
                if self.held && self.chunk.last {
                    self.end(wanted);
                    break;
                }
                pops += 1;
                if pops > 2 * RING_CHUNKS {
                    break;
                }
                if self.fade_left == 0 {
                    self.skip(wanted);
                }
                match self.ring.peek() {
                    // Behind the fading request's chunks: wait for its fade to take them.
                    Ok(c) if c.generation < wanted => break,
                    Ok(_) => {}
                    Err(_) => {
                        if self.started {
                            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                        }
                        break;
                    }
                }
                if let Ok(c) = self.ring.pop() {
                    *self.chunk = c;
                    (self.at, self.held) = (0, true);
                    if !self.started {
                        self.started = true;
                    }
                }
                continue;
            }
            let n = (frames - i).min(usize::from(self.chunk.frames) - self.at);
            let src = self.chunk.data.get(2 * self.at..2 * (self.at + n)).unwrap_or_default();
            // The request's first frames fade in.
            let (played, len) = (self.played as usize + i, self.fade_len.max(1));
            add(main, channels, i, src, |k| gain * ((played + k + 1) as f32 / len as f32).min(1.0));
            self.at += n;
            i += n;
        }
        if self.held && self.chunk.last && self.at >= usize::from(self.chunk.frames) {
            self.end(wanted);
        }
        self.played = self.played.saturating_add(i as u64);
        self.shared.played.store(self.played, Ordering::Relaxed);
        let sounding = if self.started && self.ended < wanted { wanted } else { 0 };
        self.shared.sounding.store(sounding, Ordering::Release);
    }

    /// Fade the stopped request out over the rest of its fade, taking its next chunk from the
    /// ring when the held one runs out.
    fn fade_out(&mut self, main: &mut [Vec<f32>], channels: usize, frames: usize, gain: f32) {
        let mut i = 0;
        while i < frames && self.fade_left > 0 {
            if self.fade_at >= usize::from(self.fade_chunk.frames) {
                let next = matches!(self.ring.peek(), Ok(c) if c.generation == self.fade_generation);
                match self.ring.pop() {
                    Ok(c) if next => (*self.fade_chunk, self.fade_at) = (c, 0),
                    _ => {
                        // The request ended (or nothing more came): the fade is over.
                        self.fade_left = 0;
                        break;
                    }
                }
                continue;
            }
            let n = (frames - i).min(usize::from(self.fade_chunk.frames) - self.fade_at).min(self.fade_left);
            let src = self.fade_chunk.data.get(2 * self.fade_at..2 * (self.fade_at + n)).unwrap_or_default();
            let (left, len) = (self.fade_left, self.fade_len.max(1));
            add(main, channels, i, src, |k| gain * (left - k - 1) as f32 / len as f32);
            self.fade_at += n;
            self.fade_left -= n;
            i += n;
        }
    }

    /// Drop the chunks at the front of the ring older than `generation`, without copying them.
    fn skip(&mut self, generation: u64) {
        let slots = self.ring.slots();
        if let Ok(chunk) = self.ring.read_chunk(slots) {
            let (a, b) = chunk.as_slices();
            let stale = a.iter().chain(b).take_while(|c| c.generation < generation).count();
            chunk.commit(stale);
        }
    }

    fn end(&mut self, generation: u64) {
        self.ended = generation;
        self.held = false;
        self.shared.ended.store(generation, Ordering::Release);
    }
}

/// A file longer than this many frames that can seek is overviewed from a short read per column.
const SPARSE_FROM: u64 = 2_000_000;
/// Frames read per column of a sparse overview.
const SPARSE_READ: usize = 2048;
/// The most frames read for a full overview (a long compressed file shows its start).
const MAX_OVERVIEW_FRAMES: u64 = 60_000_000;

/// A file's waveform in `bins` columns, (min, max) over every channel, for the browser's preview
/// footer: every frame of a short file, a short read per column of a long one that can seek.
/// Reads the file: call it on a background thread. `None` when the file cannot be read.
pub fn overview(path: &Path, bins: usize) -> Option<Vec<(f32, f32)>> {
    let bins_n = bins.max(1);
    let f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let ext = path.extension().and_then(|e| e.to_str());
    let mut r = BlockReader::open(Box::new(f), len, ext).ok()?;
    let frames = r.info().frames;
    let mut block = Vec::new();
    let mut out = vec![(0.0f32, 0.0f32); bins_n];
    let fold = |bin: &mut (f32, f32), block: &[Vec<f32>], n: usize| {
        for ch in block {
            for &v in ch.iter().take(n) {
                bin.0 = bin.0.min(v);
                bin.1 = bin.1.max(v);
            }
        }
    };
    if frames > SPARSE_FROM && r.seek(0).ok()? {
        for (i, bin) in out.iter_mut().enumerate() {
            r.seek(frames * i as u64 / bins_n as u64).ok()?;
            let n = r.read(&mut block, SPARSE_READ).ok()?;
            fold(bin, &block, n);
        }
        return Some(out);
    }
    // Every frame, in column-sized pieces; a file of unknown length is spread over what was read.
    let mut cols: Vec<(f32, f32)> = Vec::new();
    let per = if frames > 0 { frames.div_ceil(bins_n as u64).max(1) as usize } else { 4096 };
    let mut read = 0u64;
    while read < MAX_OVERVIEW_FRAMES {
        let n = r.read(&mut block, per).ok()?;
        if n == 0 {
            break;
        }
        read += n as u64;
        let mut bin = (0.0, 0.0);
        fold(&mut bin, &block, n);
        cols.push(bin);
    }
    if cols.is_empty() {
        return Some(out);
    }
    for (i, bin) in out.iter_mut().enumerate() {
        let a = i * cols.len() / bins_n;
        let b = ((i + 1) * cols.len() / bins_n).max(a + 1).min(cols.len());
        for c in cols.get(a..b).unwrap_or_default() {
            bin.0 = bin.0.min(c.0);
            bin.1 = bin.1.max(c.1);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
