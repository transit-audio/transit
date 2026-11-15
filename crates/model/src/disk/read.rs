//! How an audio clip reads its source (ADR 0056 decisions 5.3, 8 and 8.6; issue #604).
//!
//! Every reader of a clip, in memory or from disk, live or offline, reads through
//! [`StreamMap::read`]: the loop, then the clip's mapping `x = offset + rel·q` with `q = 1 / stretch`.
//! A stretch of exactly 1 is the direct index (the source frame itself, bit for bit); any other
//! stretch goes through the band-limited reader of decision 8 (`transit_resample`), whose taps reach
//! `16·max(1, q)` source frames on both sides of `x`. A disk-backed source makes those frames resident
//! before the read ([`ClipSource::need`]), so a read never sees a half-filled window.

use super::{Cache, StreamMap};
use std::ops::Range;
use transit_resample::{SincTable, read_frame, taps};
use transit_time::Samples;

pub use transit_resample::Frames;

/// The highest rate `q = 1 / stretch` the reader takes (decision 5.2's bound): a clip sped up more
/// than this (a stretch below `1 / MAX_RATE`, which no command sets: they keep the stretch within
/// 0.05..20) reads silence, and `session.audio_health` names it.
pub const MAX_RATE: f64 = transit_resample::MAX_RATE;

/// Build the reader's table (64 KiB) on this thread. The mix calls it before it compiles a plan, so
/// the audio thread only ever loads it.
pub fn prepare_reader() {
    let _ = SincTable::shared();
}

/// A clip's source as [`StreamMap::read`] takes it: frames by index, 0 outside the source.
pub trait ClipSource: Frames {
    /// Make source frames `frames` readable before a read (a disk reader loads them from its file);
    /// an in-memory source holds every frame already.
    fn need(&mut self, frames: Range<i64>) {
        let _ = frames;
    }
}

/// An in-memory source's channels (a channel shorter than the others reads 0 past its end).
#[derive(Debug, Clone, Copy)]
pub struct Memory<'a>(pub &'a [Vec<f32>]);

impl Frames for Memory<'_> {
    fn channels(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        let Ok(i) = usize::try_from(frame) else { return 0.0 };
        self.0.get(channel).and_then(|c| c.get(i)).copied().unwrap_or(0.0)
    }
}

impl ClipSource for Memory<'_> {}

/// Channels `first..` of a source.
struct Skip<'a, S: ?Sized> {
    src: &'a S,
    first: usize,
}

impl<S: Frames + ?Sized> Frames for Skip<'_, S> {
    fn channels(&self) -> usize {
        self.src.channels().saturating_sub(self.first)
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        self.src.sample(self.first.saturating_add(channel), frame)
    }
}

impl StreamMap {
    /// Source frames per sample of clip time, `q = 1 / stretch` (decision 7.6 with no transpose).
    #[inline]
    pub fn rate(&self) -> f64 {
        1.0 / self.stretch
    }

    /// Source channels `first..first + out.len()` at clip time `k` into `out` (channels past the
    /// source's read 0): the loop (ADR 0019), then the mapping. At `q = 1` exactly the source frame
    /// `offset + rel` itself; otherwise the reader at `x = offset + rel·q`, formed as decision 5.3
    /// states (`rel·q` rounded once, then added to the offset), so a position never depends on the
    /// block it falls in. A rate above [`MAX_RATE`] reads silence.
    ///
    /// Real time: no allocation, no lock; at most `2·16·max(1, q) + 1` taps a channel, and an
    /// in-memory source needs nothing more. A disk source's [`ClipSource::need`] reads its file and is
    /// for the streamer and offline renders only.
    #[inline]
    pub fn read<S: ClipSource + ?Sized>(&self, k: Samples, src: &mut S, first: usize, out: &mut [f32]) {
        let rel = crate::clip::content_pos(self.looping, k);
        let q = self.rate();
        if q == 1.0 {
            let at = self.offset.saturating_add(rel);
            src.need(at..at.saturating_add(1));
            for (i, o) in out.iter_mut().enumerate() {
                *o = src.sample(first.saturating_add(i), at);
            }
            return;
        }
        let x = self.offset as f64 + rel as f64 * q;
        src.need(taps(x, q));
        read_frame(SincTable::shared(), &Skip { src: &*src, first }, x, q, out);
    }
}

impl Frames for Cache {
    fn channels(&self) -> usize {
        self.channels
    }

    /// The frame from the window [`ClipSource::need`] loaded; 0 past the file, after a read error,
    /// and outside the window (which a read never asks for after its `need`).
    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        let Ok(x) = u64::try_from(frame) else { return 0.0 };
        if x >= self.frames || self.failed || x < self.start {
            return 0.0;
        }
        let Ok(i) = usize::try_from(x - self.start) else { return 0.0 };
        if i >= self.len {
            return 0.0;
        }
        self.data.get(channel).and_then(|c| c.get(i)).copied().unwrap_or(0.0)
    }
}

impl ClipSource for Cache {
    /// Load the window holding every frame of `frames` that lies in the file, unless it is resident:
    /// one read from the first of them, a cache window long (more when the taps span more). The
    /// clip's position only moves forward between two loop seams, so the window then serves the
    /// reads that follow.
    fn need(&mut self, frames: Range<i64>) {
        let lo = u64::try_from(frames.start.max(0)).unwrap_or(0);
        let hi = u64::try_from(frames.end.max(0)).unwrap_or(0).min(self.frames);
        if lo >= hi || self.failed || (lo >= self.start && hi <= self.start.saturating_add(self.len as u64)) {
            return;
        }
        let want = self.window.max(usize::try_from(hi - lo).unwrap_or(usize::MAX));
        self.load(lo, want);
    }
}

/// The synchronous reader of a disk-backed source as a [`ClipSource`]
/// ([`super::DiskSource::with_sync_source`]): silence when the file could not be opened.
pub struct SyncSource<'a> {
    pub(super) cache: Option<&'a mut Cache>,
    pub(super) channels: usize,
}

impl Frames for SyncSource<'_> {
    fn channels(&self) -> usize {
        self.channels
    }

    #[inline]
    fn sample(&self, channel: usize, frame: i64) -> f32 {
        self.cache.as_deref().map_or(0.0, |c| c.sample(channel, frame))
    }
}

impl ClipSource for SyncSource<'_> {
    fn need(&mut self, frames: Range<i64>) {
        if let Some(c) = self.cache.as_deref_mut() {
            c.need(frames);
        }
    }
}
