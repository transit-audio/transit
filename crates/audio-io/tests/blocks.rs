//! `BlockReader` (issue #211): a file read block by block gives exactly `decode`'s samples, for
//! every format the importer reads that this crate can write, the recorder's RF64 takes included.

use std::io::Cursor;
use transit_audio_io::{AudioBuffer, BitDepth, BlockReader, BwfInfo, EncodeOptions, FileFormat, decode, encode, encode_wav_rf64, wav_float_header};

fn tone(channels: usize, frames: usize, rate: u32) -> AudioBuffer {
    let channels = (0..channels).map(|c| (0..frames).map(|i| ((i * (c + 3)) as f32 * 0.0113).sin() * 0.9).collect()).collect();
    AudioBuffer { sample_rate: rate, channels }
}

/// Every block of `bytes` read with the sizes in `sizes` (cycled), concatenated per channel.
fn streamed(bytes: &[u8], hint: &str, sizes: &[usize]) -> (u16, u32, Vec<Vec<f32>>) {
    let mut r = BlockReader::open(Box::new(Cursor::new(bytes.to_vec())), bytes.len() as u64, Some(hint)).unwrap();
    let mut all: Vec<Vec<f32>> = Vec::new();
    let mut block = Vec::new();
    for &size in sizes.iter().cycle() {
        let n = r.read(&mut block, size).unwrap();
        if n == 0 {
            break;
        }
        assert!(n <= size, "a read never returns more than asked");
        all.resize_with(block.len(), Vec::new);
        for (a, b) in all.iter_mut().zip(&block) {
            assert_eq!(b.len(), n);
            a.extend_from_slice(b);
        }
    }
    (r.info().channels, r.info().sample_rate, all)
}

fn assert_bit_exact(name: &str, bytes: &[u8], hint: &str) {
    let (info, whole) = decode(bytes, Some(hint)).unwrap();
    for sizes in [&[4096usize][..], &[1, 333, 1000, 4097]] {
        let (channels, rate, got) = streamed(bytes, hint, sizes);
        assert_eq!((channels, rate), (info.channels, info.sample_rate), "{name}");
        assert_eq!(got.len(), whole.channels.len(), "{name}: channels");
        for (c, (g, w)) in got.iter().zip(&whole.channels).enumerate() {
            assert_eq!(g.len(), w.len(), "{name}: channel {c} length");
            let first = g.iter().zip(w).position(|(a, b)| a.to_bits() != b.to_bits());
            assert_eq!(first, None, "{name}: channel {c} differs from the whole-file decode");
        }
    }
}

#[test]
fn every_writable_format_streams_exactly_what_decode_returns() {
    let opts = |format, bit_depth| EncodeOptions { format, bit_depth, dither: false, bwf: None };
    let mono = tone(1, 30_011, 44_100);
    let stereo = tone(2, 30_011, 48_000);
    let six = tone(6, 9_001, 96_000);
    let mut cases: Vec<(String, Vec<u8>, &str)> = Vec::new();
    for (buf, label) in [(&mono, "mono"), (&stereo, "stereo"), (&six, "6 ch")] {
        for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32, BitDepth::Float32] {
            cases.push((format!("WAV {depth:?} {label}"), encode(buf, &opts(FileFormat::Wav, depth)).unwrap(), "wav"));
            cases.push((format!("AIFF {depth:?} {label}"), encode(buf, &opts(FileFormat::Aiff, depth)).unwrap(), "aiff"));
        }
        for depth in [BitDepth::Int16, BitDepth::Int24] {
            cases.push((format!("FLAC {depth:?} {label}"), encode(buf, &opts(FileFormat::Flac, depth)).unwrap(), "flac"));
        }
        cases.push((format!("RF64 {label}"), encode_wav_rf64(buf, &opts(FileFormat::Wav, BitDepth::Int24)).unwrap(), "wav"));
    }
    let bwf = BwfInfo { description: "take".into(), time_reference: 1_234_567, ..Default::default() };
    let with_bext = EncodeOptions { bwf: Some(bwf), ..opts(FileFormat::Wav, BitDepth::Int24) };
    cases.push(("BWF".into(), encode(&stereo, &with_bext).unwrap(), "bwf"));
    for (name, bytes, hint) in &cases {
        assert_bit_exact(name, bytes, hint);
    }
}

/// The recorder's take: a float WAV whose header is written for the length it will have (RF64 past
/// 4 GiB) while the file holds what has been recorded so far.
fn take(channels: usize, header_frames: u64, frames: usize) -> Vec<u8> {
    let mut bytes = wav_float_header(channels, 48_000, header_frames).unwrap();
    for i in 0..frames * channels {
        bytes.extend_from_slice(&((i as f32 * 0.001).sin() * 0.5).to_le_bytes());
    }
    bytes
}

#[test]
fn a_recorder_take_streams_as_decode_reads_it_rf64_and_still_growing_included() {
    let rf64 = take(2, 1 << 31, 20_000);
    assert_eq!(&rf64[..4], b"RF64", "a header for more than 4 GiB is RF64");
    assert_bit_exact("RF64 take, still growing", &rf64, "wav");
    assert_bit_exact("finished take", &take(1, 12_345, 12_345), "wav");
    let (_, _, got) = streamed(&rf64, "wav", &[4096]);
    assert_eq!(got[0].len(), 20_000, "only the frames the file holds");
}

#[test]
fn a_header_past_the_first_megabyte_is_read_from_the_whole_file() {
    // A JUNK chunk larger than the header read pushes `data` out of the first bytes.
    let wav =
        encode(&tone(2, 5_000, 48_000), &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None }).unwrap();
    let junk = 3 << 20;
    let mut bytes = wav[..12].to_vec();
    bytes.extend_from_slice(b"JUNK");
    bytes.extend_from_slice(&(junk as u32).to_le_bytes());
    bytes.resize(bytes.len() + junk, 0);
    bytes.extend_from_slice(&wav[12..]);
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    assert_bit_exact("data after a large chunk", &bytes, "wav");
}

#[test]
fn a_file_that_is_not_audio_is_refused_at_open() {
    let junk = b"this is not an audio file at all".repeat(10);
    assert!(BlockReader::open(Box::new(Cursor::new(junk.clone())), junk.len() as u64, Some("wav")).is_err());
}
