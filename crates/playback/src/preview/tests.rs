#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};
use transit_rt::testing::measure;

/// A scratch folder of this test's own.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-preview-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ramp(channels: usize, frames: usize, rate: u32) -> AudioBuffer {
    let channels = (0..channels).map(|c| (0..frames).map(|i| ((i % 997) as f32 / 997.0 - 0.5) * if c == 0 { 1.0 } else { -0.5 }).collect()).collect();
    AudioBuffer { sample_rate: rate, channels }
}

fn write(dir: &Path, name: &str, buf: &AudioBuffer, format: FileFormat) -> PathBuf {
    let bit_depth = if format == FileFormat::Flac { BitDepth::Int24 } else { BitDepth::Float32 };
    let bytes = encode(buf, &EncodeOptions { format, bit_depth, dither: false, bwf: None }).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Run blocks of `frames` until `done` says so (at most `blocks`), pumping the feeder before each
/// as the device-free Player does; returns Main's two channels as they came out.
fn run(preview: &Preview, voice: &mut PreviewVoice, frames: usize, blocks: usize, transport: bool) -> [Vec<f32>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let mut main = vec![vec![0.0f32; frames]; 2];
    for _ in 0..blocks {
        preview.pump();
        main.iter_mut().for_each(|c| c.fill(0.0));
        voice.mix(&mut main, 2, frames, transport);
        out[0].extend_from_slice(&main[0]);
        out[1].extend_from_slice(&main[1]);
    }
    out
}

#[test]
fn a_file_at_the_engine_rate_plays_sample_for_sample_then_ends() {
    let dir = scratch("exact");
    let buf = ramp(2, 10_000, 48_000);
    let path = write(&dir, "a.wav", &buf, FileFormat::Wav);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&path);
    assert_eq!(preview.status().state, PreviewState::Loading);
    let out = run(&preview, &mut voice, 512, 30, false);
    for (c, (heard, file)) in out.iter().zip(&buf.channels).enumerate() {
        assert_eq!(&heard[..10_000], &file[..], "channel {c}: exactly the file at unity gain");
        assert!(heard[10_000..].iter().all(|v| *v == 0.0), "silence after the end");
    }
    let status = preview.status();
    assert_eq!(status.state, PreviewState::Ended);
    assert_eq!(status.file, Some(FileFacts { rate: 48_000, channels: 2, frames: 10_000 }));
    assert_eq!((status.played, status.underruns), (10_000, 0));
}

#[test]
fn another_rate_is_converted_and_mono_plays_on_both_sides() {
    let dir = scratch("rate");
    let path = write(&dir, "m.aiff", &ramp(1, 44_100, 44_100), FileFormat::Aiff);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&path);
    let out = run(&preview, &mut voice, 480, 120, false);
    let heard = out[0].iter().rposition(|v| *v != 0.0).unwrap() + 1;
    assert!((47_990..=48_000).contains(&heard), "one second of audio is one second at 48 kHz: {heard}");
    assert_eq!(out[0], out[1], "mono is centered");
    assert_eq!(preview.status().state, PreviewState::Ended);
}

#[test]
fn the_transport_starting_stops_it_and_a_preview_started_while_playing_plays() {
    let dir = scratch("transport");
    let path = write(&dir, "a.wav", &ramp(2, 48_000, 48_000), FileFormat::Wav);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&path);
    let before = run(&preview, &mut voice, 256, 4, false);
    assert!(before[0].iter().any(|v| *v != 0.0));
    assert_eq!(preview.status().state, PreviewState::Playing);
    let after = run(&preview, &mut voice, 256, 4, true);
    assert!(after[0].iter().chain(&after[1]).all(|v| *v == 0.0), "cut at the start of the transport");
    assert_eq!(preview.status().state, PreviewState::Cut);
    // The transport keeps running: a new preview is not a transport start.
    preview.play(&path);
    let again = run(&preview, &mut voice, 256, 4, true);
    assert!(again[0].iter().any(|v| *v != 0.0));
    assert_eq!(preview.status().state, PreviewState::Playing);
}

#[test]
fn stop_and_another_file_replace_what_plays_and_no_stale_chunk_is_heard() {
    let dir = scratch("replace");
    let a = write(&dir, "a.wav", &AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 48_000]; 2] }, FileFormat::Wav);
    let b = write(&dir, "b.wav", &AudioBuffer { sample_rate: 48_000, channels: vec![vec![-0.5; 48_000]; 2] }, FileFormat::Wav);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&a);
    let first = run(&preview, &mut voice, 256, 4, false);
    assert!(first[0].iter().all(|v| *v == 0.25));
    // The ring is full of `a` when `b` is chosen: none of it is heard.
    preview.play(&b);
    let second = run(&preview, &mut voice, 256, 8, false);
    assert!(second[0].iter().all(|v| *v == -0.5), "only the new file");
    assert_eq!(preview.status().path.as_deref(), Some(b.as_path()));
    preview.stop();
    let stopped = run(&preview, &mut voice, 256, 4, false);
    assert!(stopped[0].iter().all(|v| *v == 0.0));
    assert_eq!(preview.status().state, PreviewState::Idle);
    assert_eq!(preview.status().path, None);
}

#[test]
fn the_preview_volume_scales_it_and_the_floor_is_silence() {
    let dir = scratch("volume");
    let path = write(&dir, "a.wav", &AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 9_000]; 2] }, FileFormat::Wav);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.set_volume_db(-6.0);
    preview.play(&path);
    let out = run(&preview, &mut voice, 256, 4, false);
    assert!((out[0][0] - 0.5 * 10f32.powf(-6.0 / 20.0)).abs() < 1e-6, "{}", out[0][0]);
    preview.set_volume_db(MIN_DB);
    let out = run(&preview, &mut voice, 256, 4, false);
    assert!(out[0].iter().all(|v| *v == 0.0));
}

#[test]
fn a_file_that_cannot_be_read_fails_with_a_reason_and_plays_nothing() {
    let dir = scratch("fail");
    let path = dir.join("broken.wav");
    std::fs::write(&path, b"RIFF....WAVEnothing here").unwrap();
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&path);
    let out = run(&preview, &mut voice, 256, 4, false);
    assert!(out[0].iter().all(|v| *v == 0.0));
    let status = preview.status();
    assert_eq!(status.state, PreviewState::Failed);
    assert!(status.error.is_some());
    preview.play(&dir.join("missing.wav"));
    run(&preview, &mut voice, 256, 1, false);
    assert_eq!(preview.status().state, PreviewState::Failed);
}

#[test]
fn the_voice_allocates_nothing_while_it_plays_switches_ends_and_is_cut() {
    let dir = scratch("alloc");
    let a = write(&dir, "a.wav", &ramp(2, 20_000, 48_000), FileFormat::Wav);
    let b = write(&dir, "b.flac", &ramp(1, 3_000, 44_100), FileFormat::Flac);
    let (preview, mut voice) = Preview::manual(48_000);
    let mut main = vec![vec![0.0f32; 512]; 2];
    let mut block = |voice: &mut PreviewVoice, transport: bool| {
        preview.pump();
        let ((), heap) = measure(|| voice.mix(&mut main, 2, 512, transport));
        assert!(heap.is_zero(), "{heap:?}");
    };
    preview.play(&a);
    for _ in 0..8 {
        block(&mut voice, false);
    }
    preview.play(&b);
    for _ in 0..20 {
        block(&mut voice, false);
    }
    assert_eq!(preview.status().state, PreviewState::Ended);
    preview.play(&a);
    block(&mut voice, false);
    block(&mut voice, true);
    assert_eq!(preview.status().state, PreviewState::Cut);
}

#[test]
fn a_mono_main_hears_both_sides_at_half() {
    let dir = scratch("mono-main");
    let path = write(&dir, "a.wav", &AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4_000], vec![0.25; 4_000]] }, FileFormat::Wav);
    let (preview, mut voice) = Preview::manual(48_000);
    preview.play(&path);
    preview.pump();
    let mut main = vec![vec![0.0f32; 256]];
    voice.mix(&mut main, 1, 256, false);
    assert!(main[0].iter().all(|v| (*v - 0.375).abs() < 1e-7));
}
