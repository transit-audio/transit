//! The browser preview through the device callback (#211): heard after the mix, never part of
//! the plan, stopped by a transport start, and allocation-free on the audio thread.
use super::*;
use crate::preview::{Preview, PreviewState};
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};

/// A stereo file of `frames` frames at `value`, in a scratch folder of this test's own.
fn constant(name: &str, value: f32, frames: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-audio-preview-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![value; frames]; 2] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    let path = dir.join("take.wav");
    std::fs::write(&path, encode(&buf, &opts).unwrap()).unwrap();
    path
}

fn heard(out: &[f32], value: f32) -> usize {
    out.iter().filter(|v| (**v - value).abs() < 1e-6).count()
}

#[test]
fn a_preview_plays_through_the_callback_without_a_plan_change_a_meter_or_an_allocation() {
    let (control, mut audio, _shared) = fixture(48_000);
    fill(&mut audio, &mut [0.0; 8]);
    let (preview, voice) = Preview::manual(48_000);
    audio.set_preview(voice);
    let plan = *audio.plans.engine().active();
    preview.play(&constant("plays", 0.25, 48_000));
    let mut out = vec![0.0f32; 480 * 2];
    let mut frames = 0;
    for _ in 0..20 {
        preview.pump();
        let ((), heap) = measure(|| fill(&mut audio, &mut out));
        assert!(heap.is_zero(), "{heap:?}");
        frames += heard(&out, 0.25);
    }
    assert_eq!(frames, (20 * 480 - crate::preview::fade_frames(48_000) + 1) * 2, "every sample of both channels is the file's after the fade-in");
    assert_eq!(preview.status().state, PreviewState::Playing);
    assert_eq!(*audio.plans.engine().active(), plan, "the plan is the one adopted before the preview");
    assert_eq!(audio.plans.engine().active_main_meter().peak, [0.0, 0.0], "Main's meter never sees the preview");
    control.retire(audio.plans, audio.actions);
}

#[test]
fn starting_the_transport_stops_the_preview_in_the_callback() {
    let (mut control, mut audio, _shared) = fixture(48_000);
    fill(&mut audio, &mut [0.0; 8]);
    let (preview, voice) = Preview::manual(48_000);
    audio.set_preview(voice);
    preview.play(&constant("transport", 0.5, 48_000));
    let mut out = vec![0.0f32; 480 * 2];
    preview.pump();
    fill(&mut audio, &mut out);
    assert!(heard(&out, 0.5) > 0);
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    for _ in 0..4 {
        preview.pump();
        let ((), heap) = measure(|| fill(&mut audio, &mut out));
        assert!(heap.is_zero(), "{heap:?}");
    }
    assert!(audio.transport.playing);
    assert_eq!(heard(&out, 0.5), 0, "the empty session plays silence once the preview is cut");
    assert_eq!(preview.status().state, PreviewState::Cut);
    control.retire(audio.plans, audio.actions);
}
