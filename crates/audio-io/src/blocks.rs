//! Streamed decode: [`BlockReader`] hands out a file's audio a block at a time, so a caller (the
//! browser's preview, issue #211) can start playing a long file at once and never holds the
//! decoded file whole.
//!
//! WAV/BWF/RF64 and AIFF/AIFF-C PCM read only their header first (sized against the file's
//! length, as [`crate::probe_sized`] does), then the sample data block by block through the
//! caller's reader. Everything else (FLAC, MP3, Ogg, ADPCM WAV, compressed AIFF-C, ...) goes through
//! the symphonia packet decoder [`crate::decode`] uses, over the whole file held in memory: the
//! container checks that bound symphonia's allocations walk the whole file. Only the decoded
//! audio is streamed. Either way the samples are exactly [`crate::decode`]'s (the tests compare
//! them).

use crate::pcm::{PcmKind, decode_sample};
use crate::symph::{Packets, guarded};
use crate::{AudioError, AudioInfo, Result, aiff, detect_format, wav};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;

/// A source of a file's bytes the reader may seek in (a `File`, a `Cursor`).
pub trait ReadSeek: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> ReadSeek for T {}

/// The first bytes read for a native header.
const HEADER_BYTES: u64 = 1 << 20;
/// The largest file decoded through symphonia: it is held in memory while it is read.
pub const MAX_HELD_BYTES: u64 = 1 << 30;
/// The most samples (frames times channels) one read of native PCM returns.
const MAX_READ_SAMPLES: usize = 1 << 20;

/// A file's audio, decoded on demand in blocks. See the module documentation.
pub struct BlockReader {
    info: AudioInfo,
    inner: Inner,
}

enum Inner {
    Pcm(Pcm),
    Packets(Box<Held>),
}

/// Native PCM: the data region, read through the caller's source.
struct Pcm {
    src: Box<dyn ReadSeek>,
    kind: PcmKind,
    channels: usize,
    /// Byte offset of the first frame, the next one, and the end of the data.
    start: u64,
    pos: u64,
    end: u64,
    bytes: Vec<u8>,
}

/// A symphonia decode of a file held in memory, and the packet being handed out.
struct Held {
    packets: Packets,
    /// Frames of the current packet already handed out, of `frames`, in `channels` channels.
    at: usize,
    frames: usize,
    channels: usize,
    done: bool,
}

/// Where a native file's samples are.
struct Layout {
    info: AudioInfo,
    kind: PcmKind,
    data_start: u64,
}

/// The native layout of `b`, the first bytes of a file of `total` bytes; `None` when the file is
/// not native PCM (another format, or a WAV/AIFF codec symphonia decodes).
fn native(b: &[u8], total: u64) -> Option<Result<Layout>> {
    let total = usize::try_from(total).unwrap_or(usize::MAX);
    if wav::is_wav(b) {
        return match wav::parse_sized(b, total) {
            Ok(wav::WavParse::Native(l)) => Some(Ok(Layout { info: l.info, kind: l.kind, data_start: l.data_start as u64 })),
            Ok(wav::WavParse::Foreign { .. }) => None,
            Err(e) => Some(Err(e)),
        };
    }
    if aiff::is_aiff(b) {
        return match aiff::parse_sized(b, total) {
            Ok(aiff::AiffParse::Native(l)) => Some(Ok(Layout { info: l.info, kind: l.kind, data_start: l.data_start as u64 })),
            Ok(aiff::AiffParse::Foreign) => None,
            Err(e) => Some(Err(e)),
        };
    }
    None
}

/// Read up to `len` bytes of `src` from `at`.
fn read_at(src: &mut dyn ReadSeek, at: u64, len: u64) -> Result<Vec<u8>> {
    src.seek(SeekFrom::Start(at))?;
    let mut out = Vec::new();
    src.take(len).read_to_end(&mut out)?;
    Ok(out)
}

impl BlockReader {
    /// Open a file of `len` bytes read through `src`. `ext_hint` is its extension, as for
    /// [`crate::decode`]. Native PCM reads only its header here; other formats read the whole
    /// file (at most [`MAX_HELD_BYTES`]) and decode nothing yet.
    pub fn open(mut src: Box<dyn ReadSeek>, len: u64, ext_hint: Option<&str>) -> Result<BlockReader> {
        let head = read_at(src.as_mut(), 0, len.min(HEADER_BYTES))?;
        // A header the first bytes do not hold (a chunk before the data larger than them) is
        // parsed again from the whole file below.
        if let Some(Ok(layout)) = native(&head, len) {
            return Self::pcm(src, layout, len);
        }
        if len > MAX_HELD_BYTES {
            return Err(AudioError::TooLarge(format!("{len} bytes is more than this format is read in")));
        }
        let all: Arc<[u8]> = Arc::from(read_at(src.as_mut(), 0, len)?);
        let total = all.len() as u64;
        match native(&all, total) {
            Some(Ok(layout)) => Self::pcm(Box::new(Cursor::new(Arc::clone(&all))), layout, total),
            Some(Err(e)) => Err(e),
            None => {
                let format = detect_format(&all, ext_hint);
                let packets = guarded(|| Packets::open(&all, Box::new(Cursor::new(Arc::clone(&all))), format))?;
                let info = AudioInfo { frames: packets.declared_frames.unwrap_or(0), ..packets.info.clone() };
                Ok(BlockReader { info, inner: Inner::Packets(Box::new(Held { packets, at: 0, frames: 0, channels: 0, done: false })) })
            }
        }
    }

    fn pcm(mut src: Box<dyn ReadSeek>, layout: Layout, len: u64) -> Result<BlockReader> {
        let channels = usize::from(layout.info.channels);
        let block = (layout.kind.bytes() * channels) as u64;
        let end = layout.data_start.saturating_add(layout.info.frames.saturating_mul(block)).min(len);
        src.seek(SeekFrom::Start(layout.data_start))?;
        let pcm = Pcm { src, kind: layout.kind, channels, start: layout.data_start, pos: layout.data_start, end, bytes: Vec::new() };
        Ok(BlockReader { info: layout.info, inner: Inner::Pcm(pcm) })
    }

    /// What the header says. `frames` is 0 when the container does not say (an MP3 without a
    /// length frame); `channels` follows the decoded stream once a block has been read.
    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    /// Move to `frame` (clamped to the end) when the file can seek: native PCM can, a symphonia
    /// decode cannot (`Ok(false)`, nothing moves). The next [`Self::read`] starts there.
    pub fn seek(&mut self, frame: u64) -> Result<bool> {
        let Inner::Pcm(p) = &mut self.inner else { return Ok(false) };
        let block = (p.kind.bytes() * p.channels) as u64;
        let at = p.start.saturating_add(frame.saturating_mul(block)).min(p.end);
        p.src.seek(SeekFrom::Start(at))?;
        p.pos = at;
        Ok(true)
    }

    /// Decode the next frames, at most `max` (fewer for a file of very many channels), into `out`:
    /// one vector per channel of the file, cleared and refilled (their capacity is reused).
    /// Returns the frames read; 0 at the end.
    pub fn read(&mut self, out: &mut Vec<Vec<f32>>, max: usize) -> Result<usize> {
        let n = match &mut self.inner {
            Inner::Pcm(p) => p.read(out, max)?,
            Inner::Packets(h) => {
                let n = h.read(out, max)?;
                self.info.channels = h.packets.info.channels;
                n
            }
        };
        Ok(n)
    }
}

impl Pcm {
    fn read(&mut self, out: &mut Vec<Vec<f32>>, max: usize) -> Result<usize> {
        let size = self.kind.bytes();
        let block = size * self.channels;
        out.resize_with(self.channels, Vec::new);
        out.iter_mut().for_each(Vec::clear);
        if block == 0 {
            return Ok(0);
        }
        let left = usize::try_from(self.end.saturating_sub(self.pos) / block as u64).unwrap_or(usize::MAX);
        // A header may claim thousands of channels: a read never holds more than MAX_READ_SAMPLES.
        let max = max.min((MAX_READ_SAMPLES / self.channels.max(1)).max(1));
        let want = left.min(max).saturating_mul(block);
        if want == 0 {
            return Ok(0);
        }
        self.bytes.resize(want, 0);
        let mut got = 0;
        while got < want {
            match self.src.read(&mut self.bytes[got..want]) {
                Ok(0) => break,
                Ok(k) => got += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let frames = got / block;
        if frames * block < want {
            // The file is shorter than its header said (it was cut while recording): stop here.
            self.end = self.pos;
        }
        self.pos = self.pos.saturating_add((frames * block) as u64);
        for frame in self.bytes.chunks_exact(block).take(frames) {
            for (ch, s) in out.iter_mut().zip(frame.chunks_exact(size)) {
                ch.push(decode_sample(s, self.kind));
            }
        }
        Ok(frames)
    }
}

impl Held {
    fn read(&mut self, out: &mut Vec<Vec<f32>>, max: usize) -> Result<usize> {
        out.iter_mut().for_each(Vec::clear);
        let mut n = 0;
        while n < max && !self.done {
            if self.at >= self.frames {
                let packets = &mut self.packets;
                match guarded(|| packets.next(true)) {
                    Ok(Some((frames, channels))) => (self.at, self.frames, self.channels) = (0, frames, channels),
                    Ok(None) => self.done = true,
                    Err(e) => {
                        self.done = true;
                        return Err(e);
                    }
                }
                continue;
            }
            if out.len() < self.packets.channels {
                out.resize_with(self.packets.channels, Vec::new);
            }
            let take = (self.frames - self.at).min(max - n);
            let samples = self.packets.samples();
            let from = self.at * self.channels;
            let to = from + take * self.channels;
            for frame in samples.get(from..to).unwrap_or_default().chunks_exact(self.channels.max(1)) {
                for (dst, &s) in out.iter_mut().zip(frame) {
                    dst.push(if s.is_finite() { s } else { 0.0 });
                }
            }
            self.at += take;
            n += take;
        }
        Ok(n)
    }
}
