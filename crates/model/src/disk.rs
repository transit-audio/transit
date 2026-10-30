//! Disk-backed audio sources (ADR 0035, issue #187).
//!
//! A source is either decoded into memory ([`crate::SourceAudio::buffer`]) or disk-backed: a
//! [`DiskSource`] that reads its file on demand. Live playback never reads the file on the audio
//! thread. A streaming thread ([`DiskSource::pump`], run by [`register`]'s thread) reads each
//! playing clip's content into one of the source's fixed ring slots ahead of the playhead; the
//! audio thread only drains a ring ([`DiskSource::stream`]), allocation-free and lock-free. A ring
//! that does not hold the block plays silence for that clip and counts a disk underrun
//! ([`underruns`]); the callback never waits. Offline renders (bounce, export, consolidate) read
//! the same file synchronously through [`DiskSource::with_sync`], so they produce what an
//! in-memory source of the same file produces, sample for sample.
//!
//! A ring is keyed by clip time (samples from the clip's start), and holds the source channels'
//! samples already mapped through the clip's loop and stretch ([`StreamMap::sample`], the
//! function the in-memory read uses too). Clip time only moves forward while the transport
//! plays, so a ring never needs to wrap content; a locate or a transport loop is a new stream.

mod peaks;
mod stream;

use crate::{Clip, ClipContent, ClipLoop, Session};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use transit_audio_io::{AudioInfo, BlockReader};
use transit_time::{Range, Samples};

pub use peaks::{DiskPeaks, PEAKS_BUCKET, PeaksOrigin, load_or_build_peaks, sidecar_path};
pub use stream::{PUMP_INTERVAL, register};

/// Ring slots per source: streams of its clips that can be held at once (the playing one and the
/// transport loop's start for each clip under the playhead).
pub const SLOTS: usize = 8;
/// Cue and refill requests the audio thread can leave between two streamer passes.
pub const REQUESTS: usize = 16;
/// Seconds of audio a ring holds ahead of the playhead.
pub const WINDOW_SECS: u32 = 2;
/// The smallest ring, frames (a very low sample rate still gets several device periods).
const MIN_WINDOW: usize = 1 << 14;
/// Frames the synchronous reader and the streamer read from the file at a time.
const CACHE_FRAMES: usize = 1 << 16;

/// A take longer than this many minutes is placed disk-backed (the default of the setting).
pub const DEFAULT_STREAM_MINUTES: f64 = 10.0;
/// The session setting (`EditState::values`) with the minutes over which a take is disk-backed.
pub const STREAM_MINUTES_KEY: &str = "disk.stream_minutes";
/// Environment variable overriding [`STREAM_MINUTES_KEY`] for the process (minutes, > 0).
pub const STREAM_MINUTES_ENV: &str = "TRANSIT_DISK_STREAM_MINUTES";

/// Clip renders that found their ring short and played silence, every source in the process
/// ("Disk underruns" in the diagnostics).
static UNDERRUNS: AtomicU64 = AtomicU64::new(0);

/// Disk underruns since the process started: clip blocks the audio thread played as silence
/// because the streamer had not read them yet.
pub fn underruns() -> u64 {
    UNDERRUNS.load(Ordering::Relaxed)
}

/// The length over which a take at `rate` is placed disk-backed: [`STREAM_MINUTES_ENV`] when set
/// to a positive number, else the session's [`STREAM_MINUTES_KEY`], else [`DEFAULT_STREAM_MINUTES`].
pub fn stream_threshold_frames(s: &Session, rate: u32) -> u64 {
    let env = std::env::var(STREAM_MINUTES_ENV).ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|m| m.is_finite() && *m > 0.0);
    let minutes = env.unwrap_or_else(|| s.edit.value(STREAM_MINUTES_KEY, DEFAULT_STREAM_MINUTES));
    let minutes = if minutes.is_finite() && minutes > 0.0 { minutes } else { DEFAULT_STREAM_MINUTES };
    (minutes * 60.0 * f64::from(rate)).min(u64::MAX as f64) as u64
}

/// A file a disk-backed source reads, frames at a time. [`BlockFile`] is the real one; tests
/// supply their own.
pub trait DiskFile: Send {
    /// Read up to `frames` frames from frame `at` into `out`, one vector per channel, cleared and
    /// refilled. Returns the frames read: fewer at the end of the file, 0 past it.
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize>;
}

/// Opens a new reader of a source's file: the streamer, the synchronous reader and the peaks
/// builder each hold their own.
pub type Opener = Arc<dyn Fn() -> Result<Box<dyn DiskFile>, String> + Send + Sync>;

/// A native PCM file (WAV, BWF, RF64, AIFF) read through [`BlockReader`]. Compressed formats are
/// not streamed (ADR 0035: they stay in memory in slice 1).
pub struct BlockFile {
    reader: BlockReader,
    pos: u64,
    tmp: Vec<Vec<f32>>,
}

impl BlockFile {
    /// Open the file at `path`, `len` bytes long (its real length, or what its header is sized
    /// against), as native PCM. Anything [`BlockReader`] cannot seek in is refused.
    pub fn open(path: &Path, len: u64) -> Result<(BlockFile, AudioInfo), String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let mut reader = BlockReader::open(Box::new(file), len, ext.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
        if !reader.seek(0).map_err(|e| format!("{}: {e}", path.display()))? {
            return Err(format!("{}: only PCM WAV and AIFF files are streamed from disk", path.display()));
        }
        let info = reader.info().clone();
        Ok((BlockFile { reader, pos: 0, tmp: Vec::new() }, info))
    }
}

impl DiskFile for BlockFile {
    fn read_at(&mut self, at: u64, frames: usize, out: &mut Vec<Vec<f32>>) -> std::io::Result<usize> {
        if at != self.pos {
            self.reader.seek(at).map_err(std::io::Error::other)?;
            self.pos = at;
        }
        let channels = usize::from(self.reader.info().channels);
        out.resize_with(channels, Vec::new);
        out.iter_mut().for_each(Vec::clear);
        let mut got = 0;
        while got < frames {
            let n = self.reader.read(&mut self.tmp, frames - got).map_err(std::io::Error::other)?;
            if n == 0 {
                break;
            }
            for (o, t) in out.iter_mut().zip(&self.tmp) {
                o.extend_from_slice(t);
            }
            got += n;
        }
        self.pos += got as u64;
        Ok(got)
    }
}

/// How a clip's time maps to its source: what a stream needs of the clip, `Copy` so the audio
/// thread can hand it to the streamer without allocating.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamMap {
    pub clip: u64,
    pub offset: Samples,
    pub looping: Option<ClipLoop>,
    /// The clip's stretch, 1.0 when it is not a finite positive number (as the mix reads it).
    pub stretch: f64,
    pub length: Samples,
}

impl StreamMap {
    /// The map of an audio clip; `None` for a MIDI clip.
    pub fn of(clip: &Clip) -> Option<StreamMap> {
        let ClipContent::Audio { offset, .. } = clip.content else { return None };
        let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
        Some(StreamMap { clip: clip.id.0, offset, looping: clip.looping, stretch, length: clip.length })
    }

    /// One source channel's sample `k` samples of clip time in, read through `get` (a source frame
    /// index to its sample, `None` past the data): the loop, then the stretch, linear between two
    /// frames. Every reader of a clip, in memory or from disk, samples through this function.
    #[inline]
    pub fn sample(&self, k: Samples, mut get: impl FnMut(usize) -> Option<f32>) -> f32 {
        let rel = crate::clip::content_pos(self.looping, k);
        if (self.stretch - 1.0).abs() < 1e-9 {
            usize::try_from(self.offset.saturating_add(rel)).ok().and_then(&mut get).unwrap_or(0.0)
        } else {
            let f = self.offset as f64 + rel as f64 / self.stretch;
            let x0 = f.floor();
            let fr = (f - x0) as f32;
            let a = if x0 >= 0.0 { get(x0 as usize).unwrap_or(0.0) } else { 0.0 };
            let b = if x0 + 1.0 >= 0.0 { get((x0 as usize).saturating_add(1)).unwrap_or(0.0) } else { 0.0 };
            a + (b - a) * fr
        }
    }
}

/// A slot's states. Only the streamer moves a slot; the audio thread reads `LIVE` ones.
const FREE: u8 = 0;
const CUEING: u8 = 1;
const LIVE: u8 = 2;

/// One ring: a stream of one clip's content from clip time `base`.
///
/// The streamer writes clip times `head..` and publishes `head` (Release); it never writes more
/// than a ring ahead of `read`, so it only overwrites what the reader has passed. The reader takes
/// `busy` around a read; the streamer re-cues a slot only when it is not busy (both SeqCst, so one
/// of them sees the other).
struct Slot {
    state: AtomicU8,
    busy: AtomicBool,
    clip: AtomicU64,
    base: AtomicI64,
    head: AtomicI64,
    read: AtomicI64,
    /// Reads so far, so the streamer can tell a stream still being played from an idle one.
    reads: AtomicU64,
    /// `channels × window` samples (f32 bits), channel-major; made the first time the slot is used.
    ring: OnceLock<Box<[AtomicU32]>>,
}

impl Slot {
    fn new() -> Slot {
        Slot {
            state: AtomicU8::new(FREE),
            busy: AtomicBool::new(false),
            clip: AtomicU64::new(0),
            base: AtomicI64::new(0),
            head: AtomicI64::new(0),
            read: AtomicI64::new(0),
            reads: AtomicU64::new(0),
            ring: OnceLock::new(),
        }
    }

    /// Whether this slot streams `clip` and holds, or will hold, clip time `k`.
    fn covers(&self, clip: u64, k: Samples, window: usize) -> bool {
        self.state.load(Ordering::Acquire) == LIVE && self.clip.load(Ordering::Acquire) == clip && {
            let (base, read) = (self.base.load(Ordering::Acquire), self.read.load(Ordering::Acquire));
            k >= base && k >= read && k < read.saturating_add(window as i64)
        }
    }
}

/// A cue or refill request the audio thread leaves for the streamer.
struct Request {
    state: AtomicU8,
    clip: AtomicU64,
    k: AtomicI64,
    offset: AtomicI64,
    loop_start: AtomicI64,
    loop_end: AtomicI64,
    looped: AtomicBool,
    stretch: AtomicU64,
    length: AtomicI64,
}

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

impl Request {
    fn new() -> Request {
        Request {
            state: AtomicU8::new(EMPTY),
            clip: AtomicU64::new(0),
            k: AtomicI64::new(0),
            offset: AtomicI64::new(0),
            loop_start: AtomicI64::new(0),
            loop_end: AtomicI64::new(0),
            looped: AtomicBool::new(false),
            stretch: AtomicU64::new(0),
            length: AtomicI64::new(0),
        }
    }

    /// Fill and publish an empty request; `false` when it was taken meanwhile.
    fn post(&self, map: &StreamMap, k: Samples) -> bool {
        if self.state.compare_exchange(EMPTY, WRITING, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        self.clip.store(map.clip, Ordering::Relaxed);
        self.k.store(k, Ordering::Relaxed);
        self.offset.store(map.offset, Ordering::Relaxed);
        self.loop_start.store(map.looping.map_or(0, |l| l.start), Ordering::Relaxed);
        self.loop_end.store(map.looping.map_or(0, |l| l.end), Ordering::Relaxed);
        self.looped.store(map.looping.is_some(), Ordering::Relaxed);
        self.stretch.store(map.stretch.to_bits(), Ordering::Relaxed);
        self.length.store(map.length, Ordering::Relaxed);
        self.state.store(READY, Ordering::Release);
        true
    }

    /// Take a published request, leaving the entry empty.
    fn take(&self) -> Option<(StreamMap, Samples)> {
        if self.state.load(Ordering::Acquire) != READY {
            return None;
        }
        let looping = self
            .looped
            .load(Ordering::Relaxed)
            .then(|| ClipLoop { start: self.loop_start.load(Ordering::Relaxed), end: self.loop_end.load(Ordering::Relaxed) });
        let map = StreamMap {
            clip: self.clip.load(Ordering::Relaxed),
            offset: self.offset.load(Ordering::Relaxed),
            looping,
            stretch: f64::from_bits(self.stretch.load(Ordering::Relaxed)),
            length: self.length.load(Ordering::Relaxed),
        };
        let k = self.k.load(Ordering::Relaxed);
        self.state.store(EMPTY, Ordering::Release);
        Some((map, k))
    }

    fn is(&self, clip: u64, k: Samples) -> bool {
        self.state.load(Ordering::Acquire) == READY && self.clip.load(Ordering::Relaxed) == clip && self.k.load(Ordering::Relaxed) == k
    }
}

/// A source file's frames, read a window at a time: the streamer's and the synchronous reader's.
pub(crate) struct Cache {
    file: Box<dyn DiskFile>,
    frames: u64,
    start: u64,
    len: usize,
    data: Vec<Vec<f32>>,
    failed: bool,
    path: PathBuf,
}

impl Cache {
    fn new(file: Box<dyn DiskFile>, frames: u64, path: PathBuf) -> Cache {
        Cache { file, frames, start: 0, len: 0, data: Vec::new(), failed: false, path }
    }

    /// Channel `ch`'s sample at frame `x`; `None` past the file (as an in-memory source reads).
    /// A read error is logged once and the file then reads as silence.
    pub(crate) fn get(&mut self, ch: usize, x: usize) -> Option<f32> {
        let x = x as u64;
        if x >= self.frames || self.failed {
            return None;
        }
        if x < self.start || x >= self.start + self.len as u64 {
            // A little behind as well: a stretched read takes the frame after, then the one before.
            let at = x.saturating_sub(4);
            match self.file.read_at(at, CACHE_FRAMES, &mut self.data) {
                Ok(n) => (self.start, self.len) = (at, n),
                Err(e) => {
                    log::warn!("{}: reading the streamed source failed: {e}; it plays as silence", self.path.display());
                    self.failed = true;
                    return None;
                }
            }
        }
        let i = usize::try_from(x.checked_sub(self.start)?).ok()?;
        self.data.get(ch)?.get(i).copied()
    }
}

/// A disk-backed source: its file, the rings the streamer fills and the audio thread drains, and
/// the waveform peaks once they are built. See the module documentation.
pub struct DiskSource {
    path: PathBuf,
    channels: usize,
    frames: u64,
    rate: u32,
    window: usize,
    opener: Opener,
    slots: Box<[Slot]>,
    requests: Box<[Request]>,
    pump: Mutex<stream::Pump>,
    sync: Mutex<Option<Cache>>,
    peaks: OnceLock<DiskPeaks>,
    underruns: AtomicU64,
}

impl std::fmt::Debug for DiskSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskSource")
            .field("path", &self.path)
            .field("channels", &self.channels)
            .field("frames", &self.frames)
            .field("rate", &self.rate)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

impl DiskSource {
    /// A source of `frames` frames of `channels` channels at `rate`, read through `opener`. Not
    /// streamed until [`register`]ed (tests drive [`DiskSource::pump`] themselves).
    pub fn new(path: PathBuf, channels: usize, frames: u64, rate: u32, opener: Opener) -> Arc<DiskSource> {
        let window = (rate as usize).saturating_mul(WINDOW_SECS as usize).max(MIN_WINDOW);
        Self::with_window(path, channels, frames, rate, window, opener)
    }

    /// [`DiskSource::new`] with rings of `window` frames (tests use short ones).
    pub fn with_window(path: PathBuf, channels: usize, frames: u64, rate: u32, window: usize, opener: Opener) -> Arc<DiskSource> {
        Arc::new(DiskSource {
            path,
            channels: channels.max(1),
            frames,
            rate,
            window: window.max(1),
            opener,
            slots: (0..SLOTS).map(|_| Slot::new()).collect(),
            requests: (0..REQUESTS).map(|_| Request::new()).collect(),
            pump: Mutex::new(stream::Pump::default()),
            sync: Mutex::new(None),
            peaks: OnceLock::new(),
            underruns: AtomicU64::new(0),
        })
    }

    /// The native PCM file at `path`, `len` bytes long, through [`BlockFile`].
    pub fn open(path: &Path, len: u64) -> Result<Arc<DiskSource>, String> {
        let (_, info) = BlockFile::open(path, len)?;
        let owned = path.to_path_buf();
        let opener: Opener = Arc::new(move || BlockFile::open(&owned, len).map(|(f, _)| Box::new(f) as Box<dyn DiskFile>));
        Ok(Self::new(path.to_path_buf(), usize::from(info.channels), info.frames, info.sample_rate, opener))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn rate(&self) -> u32 {
        self.rate
    }
    /// Frames a ring holds ahead of the playhead.
    pub fn window(&self) -> usize {
        self.window
    }
    pub fn opener(&self) -> &Opener {
        &self.opener
    }
    /// This source's disk underruns (see [`underruns`] for the process).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    /// The waveform overview, once the background build or the sidecar has delivered it.
    pub fn peaks(&self) -> Option<&DiskPeaks> {
        self.peaks.get()
    }
    pub fn set_peaks(&self, peaks: DiskPeaks) {
        let _ = self.peaks.set(peaks);
    }

    /// Audio thread: the ring holding clip time `k0..k0 + n` of `clip`, or `None` when no ring
    /// holds all of it yet. Allocation-free and lock-free; never touches the file.
    pub fn stream(&self, clip: u64, k0: Samples, n: usize) -> Option<StreamView<'_>> {
        let k1 = k0.saturating_add(n as i64);
        for slot in self.slots.iter() {
            if slot.state.load(Ordering::Acquire) != LIVE || slot.clip.load(Ordering::Acquire) != clip {
                continue;
            }
            slot.busy.store(true, Ordering::SeqCst);
            let ok = slot.state.load(Ordering::SeqCst) == LIVE && slot.clip.load(Ordering::SeqCst) == clip && {
                let (base, read, head) = (slot.base.load(Ordering::Acquire), slot.read.load(Ordering::Acquire), slot.head.load(Ordering::Acquire));
                k0 >= base && k0 >= read && k1 <= head
            };
            match (ok, slot.ring.get()) {
                (true, Some(ring)) => return Some(StreamView { slot, ring, window: self.window, end: k1 }),
                _ => slot.busy.store(false, Ordering::Release),
            }
        }
        None
    }

    /// Audio thread: a clip block found no ring ([`DiskSource::stream`] gave `None`). Counts a
    /// disk underrun and asks the streamer for clip time `k` on.
    pub fn underrun(&self, map: &StreamMap, k: Samples) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
        UNDERRUNS.fetch_add(1, Ordering::Relaxed);
        self.cue(map, k);
    }

    /// Ask the streamer to have clip time `k` of `map`'s clip ready, unless a ring holds or is
    /// filling toward it, or the same request is waiting. Lock-free; a full request table drops
    /// it (the next block asks again).
    pub fn cue(&self, map: &StreamMap, k: Samples) {
        let k = k.clamp(0, map.length.max(0));
        if self.slots.iter().any(|s| s.covers(map.clip, k, self.window)) || self.requests.iter().any(|r| r.is(map.clip, k)) {
            return;
        }
        let _ = self.requests.iter().any(|r| r.post(map, k));
    }

    /// Whether a ring holds clip time `k..k + frames` of `clip`, clipped to the clip's `length` and
    /// to what the ring can hold ahead of its reader.
    pub fn ready(&self, clip: u64, k: Samples, frames: usize, length: Samples) -> bool {
        self.slots.iter().any(|s| {
            let held = s.read.load(Ordering::Acquire).saturating_add(self.window as i64);
            s.covers(clip, k, self.window) && s.head.load(Ordering::Acquire) >= k.saturating_add(frames as i64).min(length).min(held)
        })
    }

    /// Control side: have `map`'s clip ready from clip time `k`, a ring's worth (the transport
    /// waits for this before it plays, as it waits for a plan). Reads on the calling thread.
    pub fn prime(&self, map: &StreamMap, k: Samples) -> bool {
        self.cue(map, k);
        self.pump();
        self.ready(map.clip, k.clamp(0, map.length.max(0)), self.window, map.length)
    }

    /// Off the audio thread: `f` with this file's frames read synchronously (bounce, export,
    /// consolidate). `get(ch, frame)` is `None` past the file.
    pub fn with_sync<R>(&self, f: impl FnOnce(&mut dyn FnMut(usize, usize) -> Option<f32>) -> R) -> R {
        let mut guard = self.sync.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.is_none() {
            match (self.opener)() {
                Ok(file) => *guard = Some(Cache::new(file, self.frames, self.path.clone())),
                Err(e) => log::warn!("{}: the streamed source could not be opened: {e}; it renders as silence", self.path.display()),
            }
        }
        match guard.as_mut() {
            Some(cache) => f(&mut |ch, x| cache.get(ch, x)),
            None => f(&mut |_, _| None),
        }
    }
}

/// A ring's view of clip time `..end` for one audio-thread read; dropping it marks the clip read
/// up to `end` and releases the slot.
pub struct StreamView<'a> {
    slot: &'a Slot,
    ring: &'a [AtomicU32],
    window: usize,
    end: Samples,
}

impl StreamView<'_> {
    /// Source channel `ch`'s mapped sample at clip time `k` (within the range asked for).
    #[inline]
    pub fn get(&self, ch: usize, k: Samples) -> f32 {
        let i = ch * self.window + (k.max(0) as usize) % self.window;
        self.ring.get(i).map_or(0.0, |x| f32::from_bits(x.load(Ordering::Relaxed)))
    }
}

impl Drop for StreamView<'_> {
    fn drop(&mut self) {
        self.slot.read.store(self.end, Ordering::Release);
        self.slot.reads.fetch_add(1, Ordering::Relaxed);
        self.slot.busy.store(false, Ordering::Release);
    }
}

/// Control side, before the transport plays or locates to `pos`: prime every disk-backed clip
/// the playhead reaches within a ring's length of `pos`, and of the cycle's start when Cycle is
/// on. Returns how many clips are not ready (0 when all are, or there are none).
pub fn prime_session(s: &Session, pos: Samples) -> usize {
    if !s.pool.has_disk() {
        return 0;
    }
    let mut late = 0;
    let cycle = s.edit.cycle_range();
    let starts = [Some(pos), (s.edit.loop_playback && !cycle.is_empty()).then_some(cycle.start)];
    for t in s.tracks.iter().filter(|t| !t.inactive) {
        for clip in t.clips().iter().filter(|c| !c.muted) {
            let Some(d) = clip.source().and_then(|id| s.pool.get(id)).and_then(|a| a.disk.as_deref()) else { continue };
            let Some(map) = StreamMap::of(clip) else { continue };
            for at in starts.into_iter().flatten() {
                let ahead = Range::new(at, at.saturating_add(d.window() as i64));
                if clip.range().overlaps(&ahead) && !d.prime(&map, at.saturating_sub(clip.start).max(0)) {
                    late += 1;
                }
            }
        }
    }
    late
}

#[cfg(test)]
mod tests;
