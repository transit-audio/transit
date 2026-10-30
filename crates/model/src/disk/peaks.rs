//! Waveform peaks of a disk-backed source (ADR 0035): built once in the background and cached
//! beside the file in a `.peaks` sidecar, which records the file's length, modification time and
//! a hash of its first bytes; a sidecar that no longer matches the file is rebuilt.
//!
//! Sidecar layout (little-endian): `TPKS`, version (u32), file length (u64), modification time
//! (ns since the Unix epoch, u64, 0 when unknown), source hash (u64), frames (u64), frames per
//! bucket (u32), channels (u32), buckets (u64), then per channel each bucket's min and max (f32).

use super::{DiskFile, Opener};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Frames summed up in one peaks bucket: 1.4 M buckets (11 MB a channel) for 8 h at 48 kHz.
pub const PEAKS_BUCKET: usize = 1024;
const MAGIC: &[u8; 4] = b"TPKS";
const VERSION: u32 = 1;
const HEADER: usize = 4 + 4 + 8 + 8 + 8 + 8 + 4 + 4 + 8;
/// Bytes of the file's start the source hash covers.
const HASHED: u64 = 64 * 1024;
/// Frames read at a time while building.
const READ_FRAMES: usize = 1 << 16;

/// Each channel's min and max per [`PEAKS_BUCKET`] frames: what a waveform of a disk-backed source
/// draws from (an in-memory source keeps exact `Peaks` instead).
#[derive(Debug, Clone, PartialEq)]
pub struct DiskPeaks {
    pub frames: u64,
    pub bucket: usize,
    pub channels: Vec<Vec<(f32, f32)>>,
}

impl DiskPeaks {
    /// (min, max) of channel `ch` over frames `start..end`, by whole buckets; `None` outside the file.
    pub fn minmax(&self, ch: usize, start: f64, end: f64) -> Option<(f32, f32)> {
        let buckets = self.channels.get(ch)?;
        if !(start.is_finite() && end.is_finite()) || end < 0.0 || start >= self.frames as f64 {
            return None;
        }
        let b = self.bucket.max(1) as f64;
        let lo = (start.max(0.0) / b).floor() as usize;
        let hi = ((end.max(start) / b).floor() as usize).min(buckets.len().saturating_sub(1));
        let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
        for &(a, z) in buckets.get(lo..=hi)? {
            mn = mn.min(a);
            mx = mx.max(z);
        }
        (mn <= mx).then_some((mn, mx))
    }
}

/// Where the sidecar of the file at `path` lives: `<file>.peaks` beside it.
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".peaks");
    PathBuf::from(p)
}

/// Where a source's peaks came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeaksOrigin {
    Sidecar,
    Built,
}

/// What identifies the file the peaks were built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime: u64,
    hash: u64,
}

fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(HASHED).read_to_end(&mut head)?;
    // FNV-1a over the length and the first bytes.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in meta.len().to_le_bytes().iter().chain(&head) {
        hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    Ok(Stamp { len: meta.len(), mtime, hash })
}

/// The peaks of the source file at `path` (`channels` × `frames`, read through `opener`): from its
/// sidecar when that matches the file, else built by reading the whole file once and written to
/// the sidecar (a folder that cannot be written keeps them in memory only).
pub fn load_or_build_peaks(path: &Path, opener: &Opener, channels: usize, frames: u64) -> Result<(DiskPeaks, PeaksOrigin), String> {
    let side = sidecar_path(path);
    let now = stamp(path).ok();
    if let Some(peaks) = now.and_then(|s| read_sidecar(&side, s, channels, frames)) {
        return Ok((peaks, PeaksOrigin::Sidecar));
    }
    let mut file = opener()?;
    let peaks = build(file.as_mut(), channels, frames).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(s) = now
        && let Err(e) = write_sidecar(&side, s, &peaks)
    {
        log::info!("{}: the peaks sidecar was not written: {e}", side.display());
    }
    Ok((peaks, PeaksOrigin::Built))
}

fn build(file: &mut dyn DiskFile, channels: usize, frames: u64) -> std::io::Result<DiskPeaks> {
    let buckets = usize::try_from(frames.div_ceil(PEAKS_BUCKET as u64)).unwrap_or(usize::MAX);
    let mut out = vec![Vec::with_capacity(buckets); channels];
    let mut acc = vec![(f32::INFINITY, f32::NEG_INFINITY); channels];
    let mut in_bucket = 0;
    let mut data = Vec::new();
    let mut at = 0u64;
    while at < frames {
        let n = file.read_at(at, READ_FRAMES, &mut data)?;
        if n == 0 {
            break;
        }
        for i in 0..n {
            for (ch, a) in acc.iter_mut().enumerate() {
                let v = data.get(ch).and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                if v.is_finite() {
                    *a = (a.0.min(v), a.1.max(v));
                }
            }
            in_bucket += 1;
            if in_bucket == PEAKS_BUCKET {
                for (o, a) in out.iter_mut().zip(acc.iter_mut()) {
                    o.push(std::mem::replace(a, (f32::INFINITY, f32::NEG_INFINITY)));
                }
                in_bucket = 0;
            }
        }
        at += n as u64;
    }
    if in_bucket > 0 {
        for (o, a) in out.iter_mut().zip(acc) {
            o.push(a);
        }
    }
    // A bucket of silence-only NaNs reads as zero.
    for o in &mut out {
        for b in o.iter_mut().filter(|b| b.0 > b.1) {
            *b = (0.0, 0.0);
        }
    }
    Ok(DiskPeaks { frames, bucket: PEAKS_BUCKET, channels: out })
}

fn write_sidecar(side: &Path, s: Stamp, p: &DiskPeaks) -> std::io::Result<()> {
    let count = p.channels.first().map_or(0, Vec::len);
    let mut bytes = Vec::with_capacity(HEADER + p.channels.len() * count * 8);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    for v in [s.len, s.mtime, s.hash, p.frames] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(&(p.bucket as u32).to_le_bytes());
    bytes.extend_from_slice(&(p.channels.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(count as u64).to_le_bytes());
    for ch in &p.channels {
        for &(a, z) in ch.iter().take(count) {
            bytes.extend_from_slice(&a.to_le_bytes());
            bytes.extend_from_slice(&z.to_le_bytes());
        }
    }
    let tmp = sidecar_tmp(side);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(&bytes)?;
        f.sync_all()
    });
    let done = written.and_then(|()| std::fs::rename(&tmp, side));
    if done.is_err() {
        // Nothing half-written is left beside the user's audio.
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

fn sidecar_tmp(side: &Path) -> PathBuf {
    let mut p = side.as_os_str().to_owned();
    p.push(".tmp");
    PathBuf::from(p)
}

fn read_sidecar(side: &Path, now: Stamp, channels: usize, frames: u64) -> Option<DiskPeaks> {
    let bytes = std::fs::read(side).ok()?;
    let u32_at = |i: usize| bytes.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let u64_at = |i: usize| bytes.get(i..i + 8).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
    if bytes.get(..4)? != MAGIC || u32_at(4)? != VERSION {
        return None;
    }
    let was = Stamp { len: u64_at(8)?, mtime: u64_at(16)?, hash: u64_at(24)? };
    let (f, bucket, ch, count) = (u64_at(32)?, u32_at(40)? as usize, u32_at(44)? as usize, usize::try_from(u64_at(48)?).ok()?);
    if was != now || f != frames || bucket != PEAKS_BUCKET || ch != channels || count != usize::try_from(frames.div_ceil(PEAKS_BUCKET as u64)).ok()? {
        return None;
    }
    if bytes.len() != HEADER.checked_add(ch.checked_mul(count)?.checked_mul(8)?)? {
        return None;
    }
    let f32_at = |i: usize| bytes.get(i..i + 4).and_then(|b| b.try_into().ok()).map(f32::from_le_bytes).unwrap_or(0.0);
    let channels = (0..ch).map(|c| (0..count).map(|b| HEADER + (c * count + b) * 8).map(|i| (f32_at(i), f32_at(i + 4))).collect()).collect();
    Some(DiskPeaks { frames, bucket, channels })
}
