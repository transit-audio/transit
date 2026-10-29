//! `probe_sized`: a file's frame count from its header and size alone (#192).

use transit_audio_io::{AudioError, MAX_DECODE_SAMPLES, probe_sized, wav_float_header};

#[test]
fn an_rf64_header_alone_gives_the_frame_count_of_a_file_over_the_limit() {
    let frames = MAX_DECODE_SAMPLES + 1_000;
    let header = wav_float_header(1, 48_000, frames).unwrap();
    let file_len = header.len() as u64 + frames * 4;
    let info = probe_sized(&header, file_len).unwrap();
    assert_eq!((info.frames, info.channels, info.sample_rate), (frames, 1, 48_000));
    assert!(info.frames * u64::from(info.channels) > MAX_DECODE_SAMPLES);
}

#[test]
fn a_file_shorter_than_its_header_says_counts_the_frames_it_holds() {
    let header = wav_float_header(2, 44_100, 10_000_000).unwrap();
    let info = probe_sized(&header, header.len() as u64 + 8 * 100).unwrap();
    assert_eq!(info.frames, 100);
}

#[test]
fn other_files_are_not_sized() {
    assert!(matches!(probe_sized(b"fLaC....", 8), Err(AudioError::Unsupported(_))));
    assert!(probe_sized(&[], 0).is_err());
}
