//! The streamer: reads disk-backed sources' files into their rings ahead of the playhead (ADR 0035).
//!
//! [`DiskSource::pump`] is one pass over one source: it takes the audio thread's requests, cues a
//! ring for each, and tops every live ring up to a window ahead of where its reader is. It runs on
//! the streaming thread [`register`] starts, on a control thread priming the transport
//! ([`DiskSource::prime`]), and in tests, which call it between blocks (block-driven, no clock).

use super::{CUEING, Cache, DiskSource, FREE, LIVE, StreamMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

/// How long the streaming thread sleeps between passes. A ring holds [`super::WINDOW_SECS`]
/// seconds, so a pass this often keeps it far ahead of any device period.
pub const PUMP_INTERVAL: Duration = Duration::from_millis(5);

/// Frames a pass writes into one ring before it publishes them.
const CHUNK: usize = 1 << 14;

/// What only the streamer knows about each slot.
#[derive(Default)]
pub(crate) struct Pump {
    /// The clip map of each slot's stream (by slot index).
    maps: Vec<Option<StreamMap>>,
    /// The slot's read count when last seen, and the pass it last changed in.
    seen: Vec<(u64, u64)>,
    pass: u64,
    cache: Option<Cache>,
    /// The file could not be opened: the error is logged once, and the source plays silence.
    broken: bool,
}

impl DiskSource {
    /// One streamer pass over this source (see the module documentation). Returns the frames it
    /// wrote. Never runs on the audio thread: it locks, reads the file and may allocate a ring.
    pub fn pump(&self) -> usize {
        let mut guard = self.pump.lock().unwrap_or_else(PoisonError::into_inner);
        let p = &mut *guard;
        p.pass += 1;
        p.maps.resize(self.slots.len(), None);
        p.seen.resize(self.slots.len(), (0, 0));
        for (i, slot) in self.slots.iter().enumerate() {
            let reads = slot.reads.load(Ordering::Relaxed);
            if reads != p.seen[i].0 {
                p.seen[i] = (reads, p.pass);
            }
        }
        for r in self.requests.iter() {
            if let Some((map, k)) = r.take() {
                self.cue_slot(p, &map, k);
            }
        }
        if p.cache.is_none() && !p.broken {
            match (self.opener)() {
                Ok(file) => p.cache = Some(Cache::new(file, self.frames, self.path.clone())),
                Err(e) => {
                    log::warn!("{}: the streamed source could not be opened: {e}; it plays as silence", self.path.display());
                    p.broken = true;
                }
            }
        }
        let Some(cache) = p.cache.as_mut() else { return 0 };
        let mut written = 0;
        for (i, slot) in self.slots.iter().enumerate() {
            let (Some(map), Some(ring)) = (p.maps[i], slot.ring.get()) else { continue };
            if slot.state.load(Ordering::Acquire) != LIVE {
                continue;
            }
            let target = slot.read.load(Ordering::Acquire).saturating_add(self.window as i64).min(map.length);
            let mut head = slot.head.load(Ordering::Relaxed);
            while head < target {
                let n = usize::try_from(target - head).unwrap_or(0).min(CHUNK);
                fill(ring, self.window, self.channels, &map, head, n, cache);
                head += n as i64;
                slot.head.store(head, Ordering::Release);
                written += n;
            }
        }
        written
    }

    /// Give `map`'s clip a stream from clip time `k`: nothing when a ring already holds or is
    /// filling toward it; else a free slot, else the one idle longest (never one the reader is in).
    fn cue_slot(&self, p: &mut Pump, map: &StreamMap, k: i64) {
        if self.slots.iter().any(|s| s.covers(map, k, self.window)) {
            return;
        }
        let mut order: Vec<usize> = (0..self.slots.len()).collect();
        // Free slots first, then the longest idle; a slot read in this pass is never taken.
        order.sort_by_key(|&i| (self.slots[i].state.load(Ordering::Acquire) != FREE, p.seen[i].1));
        for i in order {
            let slot = &self.slots[i];
            let was = slot.state.load(Ordering::Acquire);
            if was != FREE && p.seen[i].1 >= p.pass {
                continue;
            }
            slot.state.store(CUEING, Ordering::SeqCst);
            if slot.busy.load(Ordering::SeqCst) {
                slot.state.store(was, Ordering::SeqCst);
                continue;
            }
            let window = self.window;
            let channels = self.channels;
            slot.ring.get_or_init(|| (0..window.saturating_mul(channels)).map(|_| AtomicU32::new(0)).collect());
            slot.clip.store(map.clip, Ordering::Relaxed);
            slot.map.store(map.fingerprint(), Ordering::Relaxed);
            slot.base.store(k, Ordering::Relaxed);
            slot.head.store(k, Ordering::Relaxed);
            slot.read.store(k, Ordering::Relaxed);
            p.maps[i] = Some(*map);
            p.seen[i] = (slot.reads.load(Ordering::Relaxed), p.pass);
            slot.state.store(LIVE, Ordering::Release);
            return;
        }
    }
}

/// Write clip times `from..from + n` of `map` into `ring`, every channel.
fn fill(ring: &[AtomicU32], window: usize, channels: usize, map: &StreamMap, from: i64, n: usize, cache: &mut Cache) {
    for k in from..from + n as i64 {
        let at = (k.max(0) as usize) % window;
        for ch in 0..channels {
            let v = map.sample(k, |x| cache.get(ch, x));
            if let Some(cell) = ring.get(ch * window + at) {
                cell.store(v.to_bits(), Ordering::Relaxed);
            }
        }
    }
}

/// The sources the streaming thread serves.
static REGISTRY: Mutex<Vec<Weak<DiskSource>>> = Mutex::new(Vec::new());
static THREAD: OnceLock<()> = OnceLock::new();

/// Serve `source` from the streaming thread (started on first use) and build its waveform peaks
/// in the background, from its sidecar when that is current ([`super::load_or_build_peaks`]).
pub fn register(source: &Arc<DiskSource>) {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::downgrade(source));
    THREAD.get_or_init(|| {
        let spawned = std::thread::Builder::new().name("transit-disk-stream".into()).spawn(|| {
            loop {
                let live: Vec<Arc<DiskSource>> = {
                    let mut r = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
                    r.retain(|w| w.strong_count() > 0);
                    r.iter().filter_map(Weak::upgrade).collect()
                };
                for s in &live {
                    s.pump();
                }
                drop(live);
                std::thread::sleep(PUMP_INTERVAL);
            }
        });
        if let Err(e) = spawned {
            log::error!("the disk streaming thread could not start: {e}; disk-backed sources play as silence");
        }
    });
    if source.peaks().is_none() {
        let weak = Arc::downgrade(source);
        let (path, opener, channels, frames) = (source.path.clone(), Arc::clone(&source.opener), source.channels, source.frames);
        let spawned = std::thread::Builder::new().name("transit-disk-peaks".into()).spawn(move || {
            match super::load_or_build_peaks(&path, &opener, channels, frames) {
                Ok((peaks, _)) => {
                    if let Some(s) = weak.upgrade() {
                        s.set_peaks(peaks);
                    }
                }
                Err(e) => log::warn!("{}: waveform peaks could not be built: {e}", path.display()),
            }
        });
        if let Err(e) = spawned {
            log::warn!("the waveform peaks thread could not start: {e}");
        }
    }
}
