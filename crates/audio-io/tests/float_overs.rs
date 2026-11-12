//! 32-bit float files keep samples beyond full scale (#519): the float writer stores every finite
//! sample bit for bit and writes a NaN or an infinity as 0.0; the integer formats still clip at
//! full scale.

use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileEncoder, FileFormat, decode, encode};

fn float(format: FileFormat) -> EncodeOptions {
    EncodeOptions { format, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: None, title: None }
}

/// The issue's case: four stereo frames of `[1.5, -2.0, 0.25, 1.0e3]` on each channel.
fn overs() -> AudioBuffer {
    let ch = vec![1.5f32, -2.0, 0.25, 1.0e3];
    AudioBuffer { sample_rate: 48_000, channels: vec![ch.clone(), ch] }
}

fn bits(buf: &AudioBuffer) -> Vec<Vec<u32>> {
    buf.channels.iter().map(|ch| ch.iter().map(|s| s.to_bits()).collect()).collect()
}

/// The raw sample words of a float file as the writer stored them, before any decoder's guard:
/// the `data` chunk of a WAV (little endian) or the `SSND` chunk of an AIFF-C (big endian).
fn stored_words(bytes: &[u8], format: FileFormat) -> Vec<u32> {
    let (id, skip, big): (&[u8], usize, bool) = if format == FileFormat::Aiff { (b"SSND", 16, true) } else { (b"data", 8, false) };
    let at = bytes.windows(4).position(|w| w == id).unwrap();
    let size = bytes[at + 4..at + 8].try_into().map(|b: [u8; 4]| if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }).unwrap() as usize;
    let len = if big { size - 8 } else { size };
    bytes[at + skip..at + skip + len]
        .chunks_exact(4)
        .map(|w| {
            let w: [u8; 4] = w.try_into().unwrap();
            if big { u32::from_be_bytes(w) } else { u32::from_le_bytes(w) }
        })
        .collect()
}

#[test]
fn a_float_wav_or_aiff_keeps_samples_beyond_full_scale_bit_for_bit() {
    let src = overs();
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        let bytes = encode(&src, &float(format)).unwrap();
        let (_, out) = decode(&bytes, None).unwrap();
        assert_eq!(bits(&out), bits(&src), "{format:?}: {:?}", out.channels);
        let expect: Vec<u32> = [1.5f32, 1.5, -2.0, -2.0, 0.25, 0.25, 1.0e3, 1.0e3].iter().map(|s| s.to_bits()).collect();
        assert_eq!(stored_words(&bytes, format), expect, "{format:?} as stored");
        // The block-by-block writer stores the same bytes.
        let mut enc = FileEncoder::new(std::io::Cursor::new(Vec::new()), &float(format), 2, 48_000, None).unwrap();
        enc.write(&src.channels, 4).unwrap();
        assert_eq!(enc.finish().unwrap().into_inner(), bytes, "{format:?} streamed");
    }
}

#[test]
fn a_float_file_never_holds_a_non_finite_sample() {
    let ch = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -f32::NAN, 0.5];
    let src = AudioBuffer { sample_rate: 48_000, channels: vec![ch] };
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        let bytes = encode(&src, &float(format)).unwrap();
        let expect: Vec<u32> = [0.0f32, 0.0, 0.0, 0.0, 0.5].iter().map(|s| s.to_bits()).collect();
        assert_eq!(stored_words(&bytes, format), expect, "{format:?}");
    }
}

#[test]
fn subnormals_and_negative_zero_round_trip_bit_for_bit() {
    let ch = vec![-0.0f32, f32::from_bits(1), -f32::from_bits(1), f32::MIN_POSITIVE / 2.0, -f32::MIN_POSITIVE / 2.0, f32::MIN_POSITIVE, 0.0];
    let src = AudioBuffer { sample_rate: 48_000, channels: vec![ch.clone(), ch.iter().rev().copied().collect()] };
    for format in [FileFormat::Wav, FileFormat::Aiff] {
        let (_, out) = decode(&encode(&src, &float(format)).unwrap(), None).unwrap();
        assert_eq!(bits(&out), bits(&src), "{format:?}: {:?}", out.channels);
    }
}

#[test]
fn integer_files_still_clip_at_full_scale() {
    let src = overs();
    for format in [FileFormat::Wav, FileFormat::Aiff, FileFormat::Flac] {
        for (depth, top) in [(BitDepth::Int16, 32_767.0 / 32_768.0), (BitDepth::Int24, 8_388_607.0 / 8_388_608.0), (BitDepth::Int32, 1.0)] {
            let opts = EncodeOptions { bit_depth: depth, ..float(format) };
            let (_, out) = decode(&encode(&src, &opts).unwrap(), None).unwrap();
            for ch in &out.channels {
                assert_eq!(ch, &vec![top, -1.0, 0.25, top], "{format:?} {depth:?}");
            }
        }
    }
}
