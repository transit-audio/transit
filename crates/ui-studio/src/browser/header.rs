use super::{FileInfo, Path};
use std::io::Read;

/// How much of an audio file is read for its header.
const HEADER_BYTES: u64 = 64 * 1024;

/// The most RIFF chunk headers walked, and the furthest file offset looked at, for one WAV.
const MAX_CHUNKS: usize = 32;
const MAX_WALK_BYTES: u64 = 4 * 1024 * 1024;

/// A file's header, parsed. A WAV is walked chunk by chunk (headers read, bodies skipped by
/// seeking, bounded in count and offset) so a large bext, iXML or artwork chunk before `fmt `
/// does not hide the format; anything else is parsed from its first [`HEADER_BYTES`].
pub(super) fn read_info(p: &Path) -> Option<FileInfo> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if matches!(&head[..4], b"RIFF" | b"RF64") && &head[8..12] == b"WAVE" {
        return read_wav_info(&mut f, head);
    }
    let mut b = head.to_vec();
    f.take(HEADER_BYTES - 12).read_to_end(&mut b).ok()?;
    parse_header(&b)
}

/// Collect `fmt `, `ds64` and the `data` header of a WAV into a small synthetic header and parse
/// that, so the format rules live in [`parse_header`] only.
fn read_wav_info<F: Read + std::io::Seek>(f: &mut F, head: [u8; 12]) -> Option<FileInfo> {
    let mut b = head.to_vec();
    let mut off = 12u64;
    for _ in 0..MAX_CHUNKS {
        if off > MAX_WALK_BYTES {
            return None;
        }
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let size = u64::from(u32::from_le_bytes([h[4], h[5], h[6], h[7]]));
        let keep = match &h[..4] {
            b"fmt " | b"ds64" => size.min(64) as usize,
            b"data" => {
                b.extend_from_slice(&h);
                return parse_header(&b);
            }
            _ => 0,
        };
        if keep > 0 {
            b.extend_from_slice(&h[..4]);
            b.extend_from_slice(&(keep as u32).to_le_bytes());
            let at = b.len();
            b.resize(at + keep, 0);
            f.read_exact(&mut b[at..]).ok()?;
        }
        let next = off.checked_add(8)?.checked_add(size)?.checked_add(size & 1)?;
        f.seek(std::io::SeekFrom::Start(next)).ok()?;
        // The synthetic chunk is `keep` bytes long (padded to even), whatever the real one was.
        if keep & 1 == 1 {
            b.push(0);
        }
        off = next;
    }
    None
}

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
fn u32be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Length, channels and rate from the start of a WAV (RIFF, RF64), AIFF/AIFF-C or FLAC file;
/// `None` for anything else (MP3 and Ogg have no length in their header).
pub fn parse_header(b: &[u8]) -> Option<FileInfo> {
    match b.get(..4)? {
        b"RIFF" | b"RF64" if b.get(8..12)? == b"WAVE" => {
            let (mut at, mut fmt, mut ds64, mut data) = (12usize, None, None, None);
            while at + 8 <= b.len() {
                let id = b.get(at..at + 4)?;
                let size = u32le(b, at + 4)? as usize;
                let body = at + 8;
                match id {
                    b"fmt " => fmt = Some((u16le(b, body + 2)?, u32le(b, body + 4)?, u16le(b, body + 12)?)),
                    b"ds64" => ds64 = u64le(b, body + 8),
                    b"data" => {
                        data = Some(if size == u32::MAX as usize { ds64? } else { size as u64 });
                        break;
                    }
                    _ => {}
                }
                at = body.checked_add(size)?.checked_add(size & 1)?;
            }
            let ((channels, rate, align), data) = (fmt?, data?);
            (channels > 0 && rate > 0 && align > 0).then(|| FileInfo { secs: (data / u64::from(align)) as f64 / f64::from(rate), channels, rate })
        }
        b"FORM" if matches!(b.get(8..12)?, b"AIFF" | b"AIFC") => {
            let mut at = 12usize;
            while at + 8 <= b.len() {
                let id = b.get(at..at + 4)?;
                let size = u32be(b, at + 4)? as usize;
                let body = at + 8;
                if id == b"COMM" {
                    let channels = u16::from_be_bytes(b.get(body..body + 2)?.try_into().ok()?);
                    let frames = u32be(b, body + 2)?;
                    let rate = extended(b.get(body + 8..body + 18)?)?;
                    return (channels > 0 && rate > 0.0).then(|| FileInfo { secs: f64::from(frames) / rate, channels, rate: rate.round() as u32 });
                }
                at = body.checked_add(size)?.checked_add(size & 1)?;
            }
            None
        }
        b"fLaC" => {
            // The first metadata block is STREAMINFO: rate (20 bits), channels-1 (3), bits-1 (5),
            // total samples (36), from byte 10 of its body.
            let s = b.get(8 + 10..8 + 18)?;
            let rate = (u32::from(s[0]) << 12) | (u32::from(s[1]) << 4) | (u32::from(s[2]) >> 4);
            let channels = u16::from((s[2] >> 1) & 0x7) + 1;
            let total = (u64::from(s[3] & 0x0F) << 32) | u64::from(u32::from_be_bytes([s[4], s[5], s[6], s[7]]));
            (rate > 0).then(|| FileInfo { secs: total as f64 / f64::from(rate), channels, rate })
        }
        _ => None,
    }
}

/// An 80-bit IEEE extended float (AIFF's sample rate).
fn extended(b: &[u8]) -> Option<f64> {
    let exp = i32::from(u16::from_be_bytes([b[0], b[1]]) & 0x7FFF);
    let mant = u64::from_be_bytes(b.get(2..10)?.try_into().ok()?);
    if exp == 0 && mant == 0 {
        return Some(0.0);
    }
    Some(mant as f64 * 2f64.powi(exp - 16383 - 63))
}
