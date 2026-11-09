//! Tests for `wav_cues` and `wav_tempo`. Fixtures are built here: audio from the crate's own WAV
//! encoder, metadata chunks written byte by byte.

use super::*;
use crate::{AudioBuffer, BitDepth, EncodeOptions, encode, encode_wav_rf64, probe};

/// A mono 16-bit WAV of `frames` frames (an even data length, so no pad byte follows it).
fn base_wav(frames: usize, rf64: bool) -> Vec<u8> {
    let mut buf = AudioBuffer::new(44_100, 1, frames);
    for (i, s) in buf.channels[0].iter_mut().enumerate() {
        *s = ((i as f32) * 0.05).sin() * 0.5;
    }
    let opts = EncodeOptions { bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
    let b = if rf64 { encode_wav_rf64(&buf, &opts) } else { encode(&buf, &opts) }.unwrap();
    assert_eq!(&b[0..4], if rf64 { b"RF64" } else { b"RIFF" });
    b
}

/// One chunk with an explicit declared size (pass the body length for a well-formed chunk).
fn chunk_sized(id: &[u8; 4], declared: u32, body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&declared.to_le_bytes());
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    chunk_sized(id, body.len() as u32, body)
}

/// A `cue ` body with `count` as its count field and the given (id, frame) points. `dwPosition`
/// holds a different value than `dwSampleOffset`, so reading the wrong field shows.
fn cue_body(count: u32, points: &[(u32, u32)]) -> Vec<u8> {
    let mut c = count.to_le_bytes().to_vec();
    for &(id, frame) in points {
        c.extend_from_slice(&id.to_le_bytes());
        c.extend_from_slice(&frame.wrapping_add(7777).to_le_bytes()); // dwPosition
        c.extend_from_slice(b"data");
        c.extend_from_slice(&0u32.to_le_bytes()); // dwChunkStart
        c.extend_from_slice(&0u32.to_le_bytes()); // dwBlockStart
        c.extend_from_slice(&frame.to_le_bytes()); // dwSampleOffset
    }
    c
}

fn cue_chunk(points: &[(u32, u32)]) -> Vec<u8> {
    chunk(b"cue ", &cue_body(points.len() as u32, points))
}

fn labl(id: u32, text: &[u8]) -> Vec<u8> {
    let mut body = id.to_le_bytes().to_vec();
    body.extend_from_slice(text);
    chunk(b"labl", &body)
}

/// A `LIST` of type `list_type` holding the given sub-chunks.
fn list(list_type: &[u8; 4], subs: &[Vec<u8>]) -> Vec<u8> {
    let mut body = list_type.to_vec();
    for s in subs {
        body.extend_from_slice(s);
    }
    chunk(b"LIST", &body)
}

fn acid(tempo_bits: u32, len: usize) -> Vec<u8> {
    // The fields before the tempo hold junk: only the tempo is read.
    let mut body = vec![0xA5u8; 20];
    body.extend_from_slice(&tempo_bits.to_le_bytes());
    body.truncate(len);
    chunk(b"acid", &body)
}

/// Offset of the `data` chunk header in an encoder-written WAV.
fn data_at(b: &[u8]) -> usize {
    let mut pos = 12;
    while &b[pos..pos + 4] != b"data" {
        pos += 8 + u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
    }
    pos
}

/// `base` with `before` inserted ahead of its `data` chunk and `after` appended, and the RIFF (or
/// RF64 `ds64`) size updated.
fn place(base: &[u8], before: &[u8], after: &[u8]) -> Vec<u8> {
    let at = data_at(base);
    let mut b = base[..at].to_vec();
    b.extend_from_slice(before);
    b.extend_from_slice(&base[at..]);
    b.extend_from_slice(after);
    let riff = (b.len() - 8) as u64;
    if &b[0..4] == b"RF64" {
        b[20..28].copy_from_slice(&riff.to_le_bytes());
    } else {
        b[4..8].copy_from_slice(&(riff as u32).to_le_bytes());
    }
    b
}

fn cue(id: u32, frame: u64, label: Option<&str>) -> Cue {
    Cue { id, frame, label: label.map(str::to_string) }
}

fn labelled_file(rf64: bool) -> Vec<u8> {
    let labels = list(b"adtl", &[labl(7, b"Snare\0"), labl(2, b"Kick\0"), labl(42, b"Nobody\0"), labl(5, b"Hat\0")]);
    let cues = cue_chunk(&[(7, 600), (2, 100), (9, 800), (5, 350)]);
    place(&base_wav(1000, rf64), &labels, &cues)
}

#[test]
fn wav_cues_reads_labelled_points() {
    let read = wav_cues(&labelled_file(false));
    assert_eq!(read.cues, vec![cue(2, 100, Some("Kick")), cue(5, 350, Some("Hat")), cue(7, 600, Some("Snare")), cue(9, 800, None)]);
    assert_eq!(read.discarded, vec![]);
    assert_eq!(read.bad_chunks, vec![]);
}

#[test]
fn wav_cues_rf64() {
    let riff = wav_cues(&labelled_file(false));
    let rf64 = wav_cues(&labelled_file(true));
    assert_eq!(rf64, riff);
    assert_eq!(rf64.cues.len(), 4);
}

#[test]
fn wav_cues_bounds_bad_sizes() {
    let base = base_wav(1000, false);
    let bad = |chunk: &[u8; 4], offset: usize, reason| BadChunk { chunk: *chunk, offset, reason };

    // A `cue ` chunk claiming more bytes than the file holds, last in the file.
    let body = cue_body(2, &[(1, 300), (2, 100)]);
    let b = place(&base, &[], &chunk_sized(b"cue ", 1_000_000, &body));
    let read = wav_cues(&b);
    assert_eq!(read.cues, vec![cue(2, 100, None), cue(1, 300, None)]);
    assert_eq!(read.bad_chunks, vec![bad(b"cue ", base.len(), BadChunkReason::SizePastEnd)]);

    // A `LIST` claiming more bytes than the file holds: the sub-chunks it holds are read.
    let cues = cue_chunk(&[(1, 300), (2, 100)]);
    let mut list_body = b"adtl".to_vec();
    list_body.extend_from_slice(&labl(1, b"One\0"));
    list_body.extend_from_slice(&labl(2, b"Two\0"));
    let mut after = cues.clone();
    after.extend_from_slice(&chunk_sized(b"LIST", u32::MAX, &list_body));
    let b = place(&base, &[], &after);
    let read = wav_cues(&b);
    assert_eq!(read.cues, vec![cue(2, 100, Some("Two")), cue(1, 300, Some("One"))]);
    assert_eq!(read.bad_chunks, vec![bad(b"LIST", base.len() + cues.len(), BadChunkReason::SizePastEnd)]);

    // A count larger than the points the chunk's size holds; the walk goes on past it.
    let mut after = chunk(b"cue ", &cue_body(50, &[(1, 300), (2, 100)]));
    after.extend_from_slice(&cue_chunk(&[(3, 500)]));
    let b = place(&base, &[], &after);
    let read = wav_cues(&b);
    assert_eq!(read.cues, vec![cue(2, 100, None), cue(1, 300, None), cue(3, 500, None)]);
    assert_eq!(read.bad_chunks, vec![bad(b"cue ", base.len(), BadChunkReason::CountMismatch)]);

    // A zero-length `labl` (no cue id) among good ones, and a `cue ` too short for its count.
    let labels = list(b"adtl", &[labl(1, b"One\0"), chunk(b"labl", &[]), labl(2, b"Two\0")]);
    let zero_at = base.len() + 12 + labl(1, b"One\0").len();
    let mut after = labels.clone();
    after.extend_from_slice(&chunk(b"cue ", &[1, 0]));
    after.extend_from_slice(&cue_chunk(&[(1, 300), (2, 100)]));
    let b = place(&base, &[], &after);
    let read = wav_cues(&b);
    assert_eq!(read.cues, vec![cue(2, 100, Some("Two")), cue(1, 300, Some("One"))]);
    assert_eq!(
        read.bad_chunks,
        vec![bad(b"labl", zero_at, BadChunkReason::TooShort), bad(b"cue ", base.len() + labels.len(), BadChunkReason::TooShort)]
    );
}

#[test]
fn wav_cues_discards_out_of_range() {
    let cues = cue_chunk(&[(1, 999), (2, 1000), (3, 1001), (4, 500), (5, 500), (6, 0)]);
    let read = wav_cues(&place(&base_wav(1000, false), &[], &cues));
    assert_eq!(read.cues, vec![cue(6, 0, None), cue(4, 500, None), cue(1, 999, None)]);
    let discard = |id, frame, reason| CueDiscard { id, frame, reason };
    assert_eq!(
        read.discarded,
        vec![
            discard(5, 500, CueDiscardReason::Duplicate),
            discard(2, 1000, CueDiscardReason::OutOfRange),
            discard(3, 1001, CueDiscardReason::OutOfRange),
        ]
    );
    assert_eq!(read.bad_chunks, vec![]);
}

#[test]
fn wav_cues_truncation_never_panics() {
    // Metadata both ahead of the audio and after it, so every chunk kind is cut somewhere.
    let mut before = list(b"adtl", &[labl(1, b"One\0"), labl(2, b"Two\0")]);
    before.extend_from_slice(&acid(96f32.to_bits(), 24));
    let mut after = cue_chunk(&[(1, 30), (2, 10)]);
    after.extend_from_slice(&list(b"adtl", &[labl(3, b"Three\0")]));
    after.extend_from_slice(&cue_chunk(&[(3, 50)]));
    for rf64 in [false, true] {
        let b = place(&base_wav(64, rf64), &before, &after);
        for n in 0..=b.len() {
            let _ = wav_cues(&b[..n]);
            let _ = wav_tempo(&b[..n]);
        }
        assert_eq!(wav_cues(&b).cues, vec![cue(2, 10, Some("Two")), cue(1, 30, Some("One")), cue(3, 50, Some("Three"))]);
        assert_eq!(wav_tempo(&b), Some(96.0));
    }
}

#[test]
fn wav_cues_strip_nul_and_cap_label() {
    // 'a' then 2500 two-byte chars: byte 4096 falls inside a char, so the cut is at 4095.
    let long = format!("a{}", "\u{e9}".repeat(2500));
    assert_eq!(long.len(), 5001);
    let labels = list(b"adtl", &[labl(1, b"Ki\0ck\0\0"), labl(2, long.as_bytes()), labl(3, b"\xffAB\0"), labl(4, b"\0\0")]);
    let cues = cue_chunk(&[(1, 10), (2, 20), (3, 30), (4, 40)]);
    let read = wav_cues(&place(&base_wav(100, false), &labels, &cues));
    let cut = &long[..4095];
    assert_eq!(read.cues, vec![cue(1, 10, Some("Kick")), cue(2, 20, Some(cut)), cue(3, 30, Some("\u{fffd}AB")), cue(4, 40, None)]);
    assert!(read.cues.iter().all(|c| c.label.as_ref().is_none_or(|l| l.len() <= MAX_CUE_LABEL_BYTES)));
}

#[test]
fn wav_cues_empty_without_cue_chunk() {
    let labels = list(b"adtl", &[labl(1, b"One\0")]);
    assert_eq!(wav_cues(&place(&base_wav(100, false), &labels, &[])), CueRead::default());
    assert_eq!(wav_cues(&base_wav(100, false)), CueRead::default());
    assert_eq!(wav_cues(b"not a wave file"), CueRead::default());
}

#[test]
fn wav_cues_labels_none_without_labl() {
    // An INFO list holding a `labl`-shaped entry is not an `adtl` list and is skipped.
    let info = list(b"INFO", &[labl(1, b"Wrong\0")]);
    let cues = cue_chunk(&[(1, 10), (2, 20)]);
    let read = wav_cues(&place(&base_wav(100, false), &info, &cues));
    assert_eq!(read.cues, vec![cue(1, 10, None), cue(2, 20, None)]);
    assert_eq!(read.bad_chunks, vec![]);
}

#[test]
fn wav_tempo_reads_valid_acid() {
    for rf64 in [false, true] {
        let b = place(&base_wav(100, rf64), &acid(96.5f32.to_bits(), 24), &[]);
        assert_eq!(wav_tempo(&b), Some(96.5));
        let b = place(&base_wav(100, rf64), &[], &acid(140f32.to_bits(), 24));
        assert_eq!(wav_tempo(&b), Some(140.0));
    }
}

#[test]
fn wav_tempo_rejects_nan_zero_negative_and_short_chunk() {
    let base = base_wav(100, false);
    for bits in [f32::NAN.to_bits(), 0f32.to_bits(), (-0f32).to_bits(), (-120f32).to_bits(), f32::INFINITY.to_bits()] {
        assert_eq!(wav_tempo(&place(&base, &acid(bits, 24), &[])), None, "tempo bits {bits:#x}");
    }
    assert_eq!(wav_tempo(&place(&base, &acid(120f32.to_bits(), 23), &[])), None);
    assert_eq!(wav_tempo(&base), None);
    // The first `acid` decides: a valid second one is not read.
    let mut two = acid(0f32.to_bits(), 24);
    two.extend_from_slice(&acid(120f32.to_bits(), 24));
    assert_eq!(wav_tempo(&place(&base, &two, &[])), None);
}

#[test]
fn probe_ignores_cue_list_and_acid_chunks() {
    for rf64 in [false, true] {
        let base = base_wav(1000, rf64);
        let mut before = list(b"adtl", &[labl(1, b"One\0")]);
        before.extend_from_slice(&acid(96f32.to_bits(), 24));
        let with = place(&base, &before, &cue_chunk(&[(1, 10)]));
        assert_eq!(probe(&with, Some("wav")).unwrap(), probe(&base, Some("wav")).unwrap());
    }
}

#[test]
fn wav_cues_label_goes_to_first_cue_with_its_id() {
    // Many cues on one id and one large label: the label is given once, never copied per cue.
    let frames = 20_000u32;
    let text = vec![b'x'; MAX_CUE_LABEL_BYTES];
    let labels = list(b"adtl", &[labl(1, &text)]);
    let points: Vec<(u32, u32)> = (0..frames).map(|f| (1, f)).collect();
    let read = wav_cues(&place(&base_wav(frames as usize, false), &labels, &cue_chunk(&points)));
    assert_eq!(read.cues.len(), frames as usize);
    let labelled: Vec<&Cue> = read.cues.iter().filter(|c| c.label.is_some()).collect();
    let label_bytes: usize = read.cues.iter().filter_map(|c| c.label.as_ref()).map(String::len).sum();
    assert!(label_bytes <= text.len(), "{label_bytes} label bytes returned from {} labl bytes", text.len());
    assert_eq!(labelled.len(), 1);
    assert_eq!((labelled[0].id, labelled[0].frame), (1, 0));
    assert_eq!(read.discarded, vec![]);
}

#[test]
fn wav_cues_caps_points_read() {
    let over = MAX_CUE_POINTS + 10;
    let base = base_wav(over + 100, false);
    let points: Vec<(u32, u32)> = (0..over as u32).map(|f| (f, f)).collect();
    let first = cue_chunk(&points);
    let mut after = first.clone();
    after.extend_from_slice(&cue_chunk(&[(9_999_999, 5)]));
    let read = wav_cues(&place(&base, &[], &after));
    assert_eq!(read.cues.len(), MAX_CUE_POINTS);
    assert_eq!(read.cues.last().map(|c| c.frame), Some(MAX_CUE_POINTS as u64 - 1));
    assert_eq!(read.discarded, vec![]);
    let too_many = |offset| BadChunk { chunk: *b"cue ", offset, reason: BadChunkReason::TooMany };
    assert_eq!(read.bad_chunks, vec![too_many(base.len()), too_many(base.len() + first.len())]);
}
