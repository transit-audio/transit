//! The Ogg container (RFC 3533) as Ogg Opus uses it (RFC 7845): a page writer for one logical
//! stream and a page reader that reassembles packets. Pure Rust; no libogg.

use crate::{AudioError, Result};
use std::io::Write;

/// The Ogg page CRC: polynomial 0x04C11DB7, initial value 0, no reflection, no final XOR.
fn crc_table() -> &'static [u32; 256] {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            }
            *e = r;
        }
        t
    })
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let t = crc_table();
    data.iter().fold(0u32, |crc, &b| (crc << 8) ^ t[usize::from(((crc >> 24) as u8) ^ b)])
}

/// Page header flags.
const CONTINUED: u8 = 0x01;
const BOS: u8 = 0x02;
const EOS: u8 = 0x04;

/// Writes the pages of one logical stream. Packets are queued with [`PageWriter::packet`] and go
/// out as pages of at most `MAX_SEGMENTS` lacing values when [`PageWriter::flush`] is called or a
/// page would overflow; a packet longer than a page continues on the next.
pub(crate) struct PageWriter<W: Write> {
    out: W,
    serial: u32,
    sequence: u32,
    /// The lacing values and the body of the page being filled.
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// The granule position of the last packet completed on this page (`-1`: none yet).
    granule: i64,
    /// The page starts with the rest of a packet begun on the previous page.
    continued: bool,
    first: bool,
    /// Bytes written so far.
    pub(crate) written: u64,
}

const MAX_SEGMENTS: usize = 255;

impl<W: Write> PageWriter<W> {
    pub(crate) fn new(out: W, serial: u32) -> Self {
        PageWriter { out, serial, sequence: 0, lacing: Vec::new(), body: Vec::new(), granule: -1, continued: false, first: true, written: 0 }
    }

    /// Queues one packet that ends at `granule` (the stream's sample count after it).
    pub(crate) fn packet(&mut self, data: &[u8], granule: u64) -> Result<()> {
        let mut rest = data;
        loop {
            if self.lacing.len() == MAX_SEGMENTS {
                // The packet continues on the next page.
                self.write_page(false)?;
                self.continued = true;
            }
            let take = rest.len().min(255);
            self.lacing.push(take as u8);
            self.body.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            // A segment shorter than 255 (zero included) ends the packet.
            if take < 255 {
                break;
            }
        }
        self.granule = i64::try_from(granule).map_err(|_| AudioError::TooLarge("granule position".into()))?;
        Ok(())
    }

    /// Writes the queued packets as a page (none queued: nothing, unless `eos`). `eos` marks the
    /// last page of the stream.
    pub(crate) fn flush(&mut self, eos: bool) -> Result<()> {
        if self.lacing.is_empty() && !eos {
            return Ok(());
        }
        self.write_page(eos)
    }

    fn write_page(&mut self, eos: bool) -> Result<()> {
        let mut flags = 0u8;
        if self.continued {
            flags |= CONTINUED;
        }
        if self.first {
            flags |= BOS;
        }
        if eos {
            flags |= EOS;
        }
        let mut page = Vec::with_capacity(27 + self.lacing.len() + self.body.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&self.granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(self.lacing.len() as u8);
        page.extend_from_slice(&self.lacing);
        page.extend_from_slice(&self.body);
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.out.write_all(&page)?;
        self.written += page.len() as u64;
        self.sequence = self.sequence.wrapping_add(1);
        self.lacing.clear();
        self.body.clear();
        self.granule = -1;
        self.continued = false;
        self.first = false;
        Ok(())
    }

    /// The queued packets' segment count (a page holds 255).
    pub(crate) fn queued_segments(&self) -> usize {
        self.lacing.len()
    }

    pub(crate) fn into_inner(self) -> W {
        self.out
    }
}

/// One page as read.
#[derive(Debug)]
pub(crate) struct Page<'a> {
    pub(crate) flags: u8,
    pub(crate) granule: i64,
    pub(crate) serial: u32,
    pub(crate) lacing: &'a [u8],
    pub(crate) body: &'a [u8],
}

impl Page<'_> {
    pub(crate) fn eos(&self) -> bool {
        self.flags & EOS != 0
    }
}

/// Reads the page at the start of `b`; returns it and its length. The CRC is checked.
pub(crate) fn read_page(b: &[u8]) -> Result<(Page<'_>, usize)> {
    let bad = |what: &str| AudioError::Malformed(format!("Ogg page: {what}"));
    if b.get(..4) != Some(b"OggS") {
        return Err(bad("no capture pattern"));
    }
    let header = b.get(..27).ok_or_else(|| bad("truncated header"))?;
    if header[4] != 0 {
        return Err(bad("unknown stream structure version"));
    }
    let nseg = usize::from(header[26]);
    let lacing = b.get(27..27 + nseg).ok_or_else(|| bad("truncated lacing"))?;
    let body_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
    let len = 27 + nseg + body_len;
    let all = b.get(..len).ok_or_else(|| bad("truncated body"))?;
    let mut check = all.to_vec();
    check[22..26].fill(0);
    let want = u32::from_le_bytes([header[22], header[23], header[24], header[25]]);
    if crc32(&check) != want {
        return Err(bad("CRC mismatch"));
    }
    let granule = i64::from_le_bytes([header[6], header[7], header[8], header[9], header[10], header[11], header[12], header[13]]);
    let serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
    Ok((Page { flags: header[5], granule, serial, lacing, body: &all[27 + nseg..] }, len))
}

/// A packet reassembled from the pages, with the granule position of the page it ends on when it
/// is the last packet to end there.
#[derive(Debug)]
pub(crate) struct Packet {
    pub(crate) data: Vec<u8>,
    pub(crate) granule: Option<i64>,
}

/// Every packet of the first logical stream in `b`, in order, up to its EOS page. Pages of other
/// streams (a grouped file) are skipped.
pub(crate) fn packets(b: &[u8]) -> Result<Vec<Packet>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut serial = None;
    let mut partial: Option<Vec<u8>> = None;
    while pos < b.len() {
        let (page, len) = read_page(&b[pos..])?;
        pos += len;
        match serial {
            None => serial = Some(page.serial),
            // Another stream's page (a grouped file): the first stream is the one read.
            Some(s) if s != page.serial => continue,
            _ => {}
        }
        if page.flags & CONTINUED == 0 && partial.is_some() {
            return Err(AudioError::Malformed("Ogg page: a packet was cut off".into()));
        }
        let mut at = 0usize;
        let ends: Vec<usize> = page.lacing.iter().enumerate().filter(|(_, l)| **l < 255).map(|(i, _)| i).collect();
        let last_end = ends.last().copied();
        let mut cur = if page.flags & CONTINUED != 0 { partial.take().unwrap_or_default() } else { Vec::new() };
        for (i, &l) in page.lacing.iter().enumerate() {
            let l = usize::from(l);
            cur.extend_from_slice(&page.body[at..at + l]);
            at += l;
            if l < 255 {
                let granule = if Some(i) == last_end { Some(page.granule) } else { None };
                out.push(Packet { data: std::mem::take(&mut cur), granule });
            }
        }
        if page.lacing.last().is_some_and(|&l| l == 255) {
            partial = Some(cur);
        }
        if page.eos() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_matches_the_ogg_reference_value() {
        // One set bit runs the register through the polynomial once.
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(&[0x01]), 0x04C1_1DB7);
    }

    #[test]
    fn packets_round_trip_including_one_longer_than_a_page() {
        let mut w = PageWriter::new(Vec::new(), 7);
        let small: Vec<u8> = (0..100u8).collect();
        let exact: Vec<u8> = vec![9; 510];
        let huge: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        w.packet(&small, 0).unwrap();
        w.flush(false).unwrap();
        w.packet(&exact, 960).unwrap();
        w.packet(&huge, 1920).unwrap();
        w.packet(&[], 2880).unwrap();
        w.flush(true).unwrap();
        let bytes = w.into_inner();
        let p = packets(&bytes).unwrap();
        assert_eq!(p.len(), 4);
        assert_eq!(p[0].data, small);
        assert_eq!(p[0].granule, Some(0));
        assert_eq!(p[1].data, exact);
        assert_eq!(p[2].data, huge);
        assert!(p[3].data.is_empty());
        assert_eq!(p[3].granule, Some(2880));
        // The first page is BOS, the last EOS, and a page is never over 255 segments.
        let (first, _) = read_page(&bytes).unwrap();
        assert_eq!(first.flags & BOS, BOS);
    }

    #[test]
    fn a_corrupt_byte_fails_the_crc() {
        let mut w = PageWriter::new(Vec::new(), 1);
        w.packet(b"hello", 0).unwrap();
        w.flush(true).unwrap();
        let mut bytes = w.into_inner();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(read_page(&bytes), Err(AudioError::Malformed(_))));
    }
}
