//! WAV cue points with their labels (`cue ` and `LIST`/`adtl`/`labl`, per the Microsoft
//! multimedia RIFF specification) and the `acid` chunk's tempo, read from the bytes of a RIFF,
//! RF64 or BW64 WAVE file. Only those plain RIFF chunks are read.
//!
//! The input is untrusted: every chunk size and count is bounded by the bytes that hold it before
//! it is used, offsets use checked arithmetic, and the walk stops after
//! [`MAX_CHUNKS`](crate::wav::MAX_CHUNKS) chunks and sub-chunks. Nothing here panics. What is
//! returned is bounded: cue points by the 24 bytes each takes in the file, each `labl` is given
//! to at most one cue, and a label's text is at most three bytes per `labl` byte in the file (an
//! invalid UTF-8 byte reads as the three-byte U+FFFD) and at most [`MAX_CUE_LABEL_BYTES`]. The
//! bad-chunk list holds at most two entries per chunk walked.

use crate::pcm::{le_u16, le_u32, le_u64, tag};
use crate::wav::{self, MAX_CHUNKS, WavParse};
use std::collections::HashMap;

/// The longest label kept, in bytes of UTF-8; a longer one is cut at a char boundary.
pub const MAX_CUE_LABEL_BYTES: usize = 4096;

/// Bytes of one cue point in a `cue ` chunk.
const CUE_POINT_LEN: usize = 24;
/// Offset of the tempo (`f32`) in an `acid` chunk.
const ACID_TEMPO_AT: usize = 20;
/// Length of an `acid` chunk.
const ACID_LEN: usize = 24;

/// One cue point kept by [`wav_cues`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// The cue point's id (`dwName`), which labels refer to.
    pub id: u32,
    /// The source frame the cue marks, at the file's native rate.
    pub frame: u64,
    /// The text of the first `labl` with this id, if any (empty text reads as `None`). Only the
    /// first kept cue (lowest frame) with an id carries its label; later ones with that id carry
    /// `None`, so one `labl` is never copied into many cues.
    pub label: Option<String>,
}

/// Why a cue point was read but not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueDiscardReason {
    /// The frame is at or past the file's frame count.
    OutOfRange,
    /// An earlier cue (in file order) already marks this frame.
    Duplicate,
}

/// A cue point read from the file and discarded, for the caller's load note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CueDiscard {
    pub id: u32,
    pub frame: u64,
    pub reason: CueDiscardReason,
}

/// What is wrong with a malformed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadChunkReason {
    /// The chunk's size reaches past the end of the file (or of its `LIST`); what fits was read.
    SizePastEnd,
    /// A `cue ` chunk's point count disagrees with its size; the points that fit were read.
    CountMismatch,
    /// The chunk is too short to hold its fixed fields (a `cue ` without a count, a `labl`
    /// without a cue id); it was ignored.
    TooShort,
}

/// A malformed `cue `, `LIST` or `labl` chunk met while reading cues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadChunk {
    /// The chunk's four-character id.
    pub chunk: [u8; 4],
    /// The byte offset of the chunk's header in the file.
    pub offset: usize,
    pub reason: BadChunkReason,
}

/// The result of [`wav_cues`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CueRead {
    /// The kept cue points, ascending by frame, one per frame.
    pub cues: Vec<Cue>,
    /// The cue points read and discarded, ascending by frame (file order within a frame).
    pub discarded: Vec<CueDiscard>,
    /// The malformed chunks, in file order.
    pub bad_chunks: Vec<BadChunk>,
}

/// One chunk of the walk: its id, header offset, and the body bytes the file holds (all of it,
/// or only what remains when `truncated`).
struct Chunk<'a> {
    id: [u8; 4],
    offset: usize,
    body: &'a [u8],
    truncated: bool,
}

/// Walks the top-level chunks of a WAVE file the way `wav::parse_sized` does: odd sizes padded to
/// even, RF64/BW64 `data` sized by `ds64`, a RIFF `data` of size 0 running to the end. The walk
/// stops after a chunk that reaches past the end of the file, and after [`MAX_CHUNKS`] chunks.
struct Chunks<'a> {
    b: &'a [u8],
    pos: usize,
    count: usize,
    is_rf64: bool,
    ds64_data: Option<u64>,
    done: bool,
}

impl<'a> Chunks<'a> {
    fn new(b: &'a [u8]) -> Self {
        let ok = wav::is_wav(b);
        Chunks { b, pos: 12, count: 0, is_rf64: tag(b, 0).is_some_and(|t| &t != b"RIFF"), ds64_data: None, done: !ok }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Chunk<'a>> {
        if self.done {
            return None;
        }
        self.count += 1;
        let header = (self.count <= MAX_CHUNKS).then(|| Some((tag(self.b, self.pos)?, le_u32(self.b, self.pos.checked_add(4)?)?))).flatten();
        let (Some((id, size32)), Some(body_at)) = (header, self.pos.checked_add(8)) else {
            self.done = true;
            return None;
        };
        let remaining = self.b.len().saturating_sub(body_at);
        let mut size = u64::from(size32);
        if &id == b"data" && self.is_rf64 && size32 == u32::MAX {
            size = self.ds64_data.unwrap_or(remaining as u64);
        }
        if &id == b"data" && !self.is_rf64 && size32 == 0 {
            size = remaining as u64;
        }
        let declared = usize::try_from(size).unwrap_or(usize::MAX);
        let truncated = declared > remaining;
        let len = declared.min(remaining);
        let body = self.b.get(body_at..body_at.saturating_add(len)).unwrap_or_default();
        if &id == b"ds64" {
            self.ds64_data = le_u64(body, 8);
        }
        let offset = self.pos;
        match body_at.checked_add(declared).and_then(|end| end.checked_add(declared & 1)) {
            Some(next) if !truncated => self.pos = next,
            _ => self.done = true,
        }
        Some(Chunk { id, offset, body, truncated })
    }
}

/// The file's frame count: the native decoder's count for PCM and float files, else (a codec
/// decoded elsewhere) the `data` length over the `fmt ` block align; 0 when unknown.
fn frame_count(b: &[u8]) -> u64 {
    if let Ok(WavParse::Native(layout)) = wav::parse(b) {
        return layout.info.frames;
    }
    let mut block_align = None;
    for c in Chunks::new(b) {
        match &c.id {
            b"fmt " if block_align.is_none() => block_align = le_u16(c.body, 12),
            b"data" => return block_align.filter(|&a| a > 0).map_or(0, |a| (c.body.len() / usize::from(a)) as u64),
            _ => {}
        }
    }
    0
}

/// A cue point as read, before range checks and labels.
struct RawCue {
    id: u32,
    frame: u64,
}

/// Reads the points of a `cue ` chunk into `out`.
///
/// A cue point is `dwName, dwPosition, fccChunk, dwChunkStart, dwBlockStart, dwSampleOffset`.
/// The frame is `dwSampleOffset` (byte 20): the sample-frame offset into the `data` chunk, which
/// is where the cue lies in the decoded audio. `dwPosition` is the position in a playlist order
/// that only `wavl` files define, so it is not read.
fn read_cue_chunk(c: &Chunk<'_>, out: &mut Vec<RawCue>, bad: &mut Vec<BadChunk>) {
    let mut report = |reason| bad.push(BadChunk { chunk: c.id, offset: c.offset, reason });
    if c.truncated {
        report(BadChunkReason::SizePastEnd);
    }
    let Some(count) = le_u32(c.body, 0) else {
        report(BadChunkReason::TooShort);
        return;
    };
    let fit = c.body.len().saturating_sub(4) / CUE_POINT_LEN;
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    let sized = count.checked_mul(CUE_POINT_LEN).and_then(|n| n.checked_add(4));
    if !c.truncated && sized != Some(c.body.len()) {
        report(BadChunkReason::CountMismatch);
    }
    let n = count.min(fit);
    out.reserve(n);
    for i in 0..n {
        let at = 4 + i * CUE_POINT_LEN;
        if let (Some(id), Some(frame)) = (le_u32(c.body, at), le_u32(c.body, at + 20)) {
            out.push(RawCue { id, frame: u64::from(frame) });
        }
    }
}

/// Collects the `labl` sub-chunks of a `LIST` of type `adtl` as (cue id, text bytes); `chunks`
/// is the shared chunk budget. Other list types and other sub-chunks are skipped.
fn read_adtl<'a>(c: &Chunk<'a>, labels: &mut Vec<(u32, &'a [u8])>, bad: &mut Vec<BadChunk>, chunks: &mut usize) {
    if c.truncated {
        bad.push(BadChunk { chunk: c.id, offset: c.offset, reason: BadChunkReason::SizePastEnd });
    }
    if tag(c.body, 0) != Some(*b"adtl") {
        return;
    }
    let mut pos = 4usize;
    while let (Some(id), Some(size)) = (tag(c.body, pos), pos.checked_add(4).and_then(|p| le_u32(c.body, p))) {
        *chunks += 1;
        if *chunks > MAX_CHUNKS {
            return;
        }
        let offset = c.offset.saturating_add(8).saturating_add(pos);
        let body_at = pos + 8;
        let remaining = c.body.len().saturating_sub(body_at);
        let declared = usize::try_from(size).unwrap_or(usize::MAX);
        let body = c.body.get(body_at..body_at.saturating_add(declared.min(remaining))).unwrap_or_default();
        if declared > remaining {
            bad.push(BadChunk { chunk: id, offset, reason: BadChunkReason::SizePastEnd });
        }
        if &id == b"labl" {
            match le_u32(body, 0) {
                Some(cue) => labels.push((cue, body.get(4..).unwrap_or_default())),
                None => bad.push(BadChunk { chunk: id, offset, reason: BadChunkReason::TooShort }),
            }
        }
        if declared > remaining {
            return;
        }
        match body_at.checked_add(declared).and_then(|end| end.checked_add(declared & 1)) {
            Some(next) => pos = next,
            None => return,
        }
    }
}

/// A label's text: NUL bytes removed, read as UTF-8 (invalid bytes become U+FFFD), cut to at most
/// [`MAX_CUE_LABEL_BYTES`] at a char boundary; `None` when nothing remains.
fn label_text(raw: &[u8]) -> Option<String> {
    // Every byte read yields at least one byte of text and a char is at most four bytes, so the
    // first MAX + 4 bytes decode to the same first MAX bytes as the whole label would.
    let bytes: Vec<u8> = raw.iter().copied().filter(|&c| c != 0).take(MAX_CUE_LABEL_BYTES + 4).collect();
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_CUE_LABEL_BYTES {
        let mut cut = MAX_CUE_LABEL_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    (!text.is_empty()).then_some(text)
}

/// The cue points of a WAV file with their labels.
///
/// Frames are source-frame indices at the file's native rate. Sources are resampled to the
/// session rate on load, and the caller scales cue frames once with the same rounding; nothing
/// is scaled here. The kept cues are sorted ascending by frame with one cue per frame (the first
/// in file order wins); a cue at or past the frame count (the `data` length over the block size)
/// is discarded as out of range. Labels come from `labl` sub-chunks of `LIST`/`adtl` matched by
/// cue id (the first `labl` for an id wins; one for an unknown id is ignored). The region's start
/// and end are not added; the caller adds them. Malformed chunks are reported in
/// [`CueRead::bad_chunks`] after reading what fits. A file that is not a WAVE file has no cues.
pub fn wav_cues(bytes: &[u8]) -> CueRead {
    let mut raw = Vec::new();
    let mut labels = Vec::new();
    let mut bad_chunks = Vec::new();
    let mut chunks = 0usize;
    for c in Chunks::new(bytes) {
        chunks += 1;
        if chunks > MAX_CHUNKS {
            break;
        }
        match &c.id {
            b"cue " => read_cue_chunk(&c, &mut raw, &mut bad_chunks),
            b"LIST" => read_adtl(&c, &mut labels, &mut bad_chunks, &mut chunks),
            _ => {}
        }
    }
    let frames = if raw.is_empty() { 0 } else { frame_count(bytes) };
    // The first `labl` for an id wins; at most one entry per sub-chunk walked.
    let mut by_id: HashMap<u32, &[u8]> = HashMap::with_capacity(labels.len());
    for (id, text) in labels {
        by_id.entry(id).or_insert(text);
    }
    // Stable: cues on the same frame stay in file order, so the first one is kept.
    raw.sort_by_key(|c| c.frame);
    let mut cues: Vec<Cue> = Vec::with_capacity(raw.len());
    let mut discarded = Vec::new();
    for c in raw {
        let reason = if c.frame >= frames {
            Some(CueDiscardReason::OutOfRange)
        } else if cues.last().is_some_and(|k| k.frame == c.frame) {
            Some(CueDiscardReason::Duplicate)
        } else {
            None
        };
        match reason {
            Some(reason) => discarded.push(CueDiscard { id: c.id, frame: c.frame, reason }),
            None => {
                // Taken, not copied: the label goes to the first kept cue with this id only.
                let label = by_id.remove(&c.id).and_then(label_text);
                cues.push(Cue { id: c.id, frame: c.frame, label });
            }
        }
    }
    CueRead { cues, discarded, bad_chunks }
}

/// The tempo in BPM of the first `acid` chunk of a WAV file: the `f32` at byte 20 of the 24-byte
/// chunk (no other field is read). `None` without an `acid` chunk, when that chunk is shorter
/// than 24 bytes, or when the tempo is not finite and positive.
pub fn wav_tempo(bytes: &[u8]) -> Option<f64> {
    let acid = Chunks::new(bytes).find(|c| &c.id == b"acid")?;
    if acid.body.len() < ACID_LEN {
        return None;
    }
    let tempo = f64::from(f32::from_bits(le_u32(acid.body, ACID_TEMPO_AT)?));
    (tempo.is_finite() && tempo > 0.0).then_some(tempo)
}

#[cfg(test)]
#[path = "wav_cues_tests.rs"]
mod tests;
